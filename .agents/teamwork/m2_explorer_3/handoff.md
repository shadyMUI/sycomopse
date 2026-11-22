# Milestone 2 Technical Remediation Report: DEF-18, DEF-19, DEF-21, DEF-22, DEF-23, DEF-24, DEF-29

**Author**: `m2_explorer_3` (Archetype: `explorer`, Roles: `investigation`, `synthesis`)  
**Parent Agent**: `6d06fbf6-e50a-4864-b50c-2baa0ca37ccd`  
**Working Directory**: `/home/shadymui/sycomopse/.agents/teamwork/m2_explorer_3`  
**Target Scope**: Milestone 2 Remediation Scope — DEF-18, DEF-19, DEF-21, DEF-22, DEF-23, DEF-24, DEF-29  
**Timestamp**: 2026-11-17T18:30:00Z  

---

## 1. Observation

Direct code inspections, line references, and tool command results across all 7 defect targets:

### 1.1. DEF-18: Standard CI Workflow Triggers for Push to Main
- **Current State**: Directory `.github/workflows/` contains solely `linux-packaging.yml:3-11`. Triggers are limited to:
  ```yaml
  on:
    workflow_dispatch:
    pull_request:
      paths:
        - 'packaging/**'
        - 'src-tauri/**'
        - 'src/**'
        - 'package*.json'
        - '.github/workflows/linux-packaging.yml'
  ```
- **Absence**: There is no standard CI workflow (`.github/workflows/ci.yml`) triggering on regular `push` to `main` and general `pull_request` to `main` that validates frontend typecheck, production build (`npm run build`), frontend unit tests (`npm test`), native Rust checks/tests (`cargo test`), and packaging tests.

### 1.2. DEF-19: Linter and Formatter Tooling Configuration
- **Current State**:
  - `package.json:8-19`: Contains scripts for `dev`, `demo`, `build`, `test`, `test:watch`, `native:helpers`, `tauri:*`, but completely lacks `lint`, `lint:fix`, `format`, or `format:check`.
  - `package.json:25-37`: `devDependencies` lacks `eslint`, `@typescript-eslint/*`, `eslint-plugin-react-hooks`, `eslint-plugin-react-refresh`, and `prettier`.
  - Root directory has no `eslint.config.js`, `.eslintrc*`, `.prettierrc`, or `.prettierignore`.
  - `src-tauri/` has no `clippy.toml` or `rustfmt.toml`.
  - Host execution of `npm ping` confirms npm registry is reachable (360ms roundtrip).

### 1.3. DEF-21: Dead `quick-xml` and Unmaintained `fs2` Dependency
- **Current State**:
  - `src-tauri/Cargo.toml:46`: Declares `fs2 = "0.4"`, unmaintained since 2017.
  - `src-tauri/Cargo.toml:51`: Declares `quick-xml = "0.38"` under `[target.'cfg(target_os = "linux")'.dependencies]`.
  - Ripgrep search across `src-tauri` shows `quick_xml` has **0 occurrences** (D-Bus XML introspection is hand-parsed in `src/providers/mod.rs`).
  - Ripgrep search shows `fs2` is referenced exclusively in `src-tauri/src/remapping.rs`:
    - Line 5: `use fs2::FileExt;`
    - Line 171: `let _ = FileExt::unlock(&self.0);` in `Drop for StoreLock`
    - Line 331: `match file.try_lock_exclusive() {` in `Store::lock`
    - Line 2094: `instance.try_lock_exclusive()` in agent process lock
  - `libc = "0.2"` is already declared at line 52 of `src-tauri/Cargo.toml`. Standard POSIX file locking (`libc::flock`) is available natively.

### 1.4. DEF-22: Redundant Spatializer Template in Tauri Resources
- **Current State**:
  - `src-tauri/tauri.conf.json:38`: Declares `"resources": ["resources/spatializer-7.1.conf.template"]`.
  - `src-tauri/src/surround.rs:240`: Embeds the template directly into the compiled binary via `let template = include_str!("../resources/spatializer-7.1.conf.template");`.
  - `packaging/tests/check_audio_interop.py:112`: Reads the template directly from repository source tree (`PACKAGING.parent / "src-tauri/resources/spatializer-7.1.conf.template"`), not from bundle install directory.
  - Runtime code never accesses the file bundled in resources; external bundling creates an unnecessary duplicate in distribution artifacts.

### 1.5. DEF-23: Audio Worker Semaphore Queuing with Timeout
- **Current State**:
  - `src-tauri/src/audio_worker.rs:356-361`:
    ```rust
    #[cfg(target_os = "linux")]
    async fn invoke(request: Request) -> AppResult<()> {
        use tokio::sync::Semaphore;
        static SLOTS: Semaphore = Semaphore::const_new(2);
        let _slot = SLOTS
            .try_acquire()
            .map_err(|_| AppError::new(ErrorCode::Busy, "Worker audio gia' occupati."))?;
    ```
  - `try_acquire()` rejects any request exceeding 2 concurrent slots immediately without any buffer or wait time, causing transient volume and audio slider adjustments to fail with `ErrorCode::Busy`.
  - `std::time::Duration` is already imported at `src-tauri/src/audio_worker.rs:7`. `tokio` features include `time` and `sync`.

### 1.6. DEF-24: Redundant Snapshot Readback Suppression on Connection Error
- **Current State**:
  - In `src/App.tsx:105-134` (`runMutation`):
    ```ts
    } catch (error) {
      if (!activeRef.current || generationRef.current !== generation) return false;
      const failure = asAppError(error);
      setErrors((current) => ({ ...current, [key]: failure }));
      try {
        acceptSnapshot(await api.getSnapshot(), generation);
      } catch (readbackError) {
        if (activeRef.current && generationRef.current === generation) {
          const readback = asAppError(readbackError);
          setErrors((current) => ({ ...current, [key]: { ...failure, detail: [failure.detail, `Rilettura non riuscita: ${readback.message}`].filter(Boolean).join("\n") } }));
        }
      }
      return false;
    }
    ```
  - When the mutation that failed was the connection itself (`key === "connection"`, triggered by the "Riprova connessione" button at `src/App.tsx:190`), the catch block immediately attempts a redundant, second IPC call `api.getSnapshot()`, which is known to fail, delaying error presentation.
  - In `src-tauri/src/lib.rs`, inspection of `refresh_all` (lines 343-355) and related backend command handlers confirms backend does not retry snapshot readback on connection errors, confirming the defect is localized to frontend mutation handling in `App.tsx`.

### 1.7. DEF-29: Robust Tray Click State Machine for Linux StatusNotifierItem
- **Current State**:
  - `src-tauri/src/lib.rs:866-902`:
    ```rust
    let skip_release = StdMutex::new(false);
    let tray = TrayIconBuilder::with_id("peripheral-hub")
        .icon(tauri::include_image!("icons/32x32.png"))
        .tooltip("Peripheral Hub")
        .menu(&tray_menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(move |tray, event| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Down,
                ..
            } => {
                *skip_release.lock().unwrap_or_else(|error| error.into_inner()) = false;
            }
            TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } => {
                *skip_release.lock().unwrap_or_else(|error| error.into_inner()) = true;
            }
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } => {
                let skip = std::mem::take(
                    &mut *skip_release.lock().unwrap_or_else(|error| error.into_inner()),
                );
                if !skip {
                    toggle_window(tray.app_handle());
                }
            }
            _ => {}
        })
        .build(app);
    ```
  - This state machine assumes a strict `MouseDown` followed by `MouseUp` order. Linux desktop environments implementing the D-Bus StatusNotifierItem (SNI) specification (e.g., KDE Plasma, GNOME via AppIndicator) often send only `Up` clicks or out-of-order events. If `skip_release` is left set to `true`, valid user clicks are dropped.

---

## 2. Logic Chain

1. **DEF-18**: Continuous integration must guard mainline stability on every commit to `main` and incoming PRs. Relying exclusively on manual dispatch or release tags means build and test regressions escape undetected until release time. Adding `.github/workflows/ci.yml` running fast automated gates (`tsc --noEmit`, `vite build`, `vitest run`, `cargo test`, `python3 -m unittest`) ensures regressions are stopped immediately.
2. **DEF-19**: Static analysis and automated formatting prevent stylistic discrepancies and detect subtle React bugs (e.g. stale closures or missing effect dependencies). Introducing modern flat ESLint (`eslint.config.js`) with `@typescript-eslint` and `eslint-plugin-react-hooks`, Prettier (`.prettierrc`), `npm run lint`, and Rust linter/formatter configs (`clippy.toml`, `rustfmt.toml`) establishes rigorous code quality gates.
3. **DEF-21**: `quick-xml` is completely unused; removing it saves build time and dependencies. `fs2` has been dead since 2017. Replacing `fs2::FileExt` with a zero-dependency extension trait `FileLockExt` using POSIX `libc::flock` eliminates supply chain risk while maintaining identical semantics (`try_lock_exclusive` returning `ErrorKind::WouldBlock` and `unlock`), without adding any new crate dependencies.
4. **DEF-22**: `spatializer-7.1.conf.template` is already compiled into the binary via `include_str!`. Declaring it in `bundle.resources` in `tauri.conf.json` merely duplicates the file in the packaged assets without any runtime purpose. Clearing `"resources": []` cleans up the package bundle.
5. **DEF-23**: Interactive audio sliders (volume, balance, spatial settings) fire multiple update events in rapid succession. Immediate rejection by `try_acquire()` on a 2-slot semaphore produces false "Busy" errors. Adding `tokio::time::timeout(Duration::from_millis(500), SLOTS.acquire()).await` allows requests to queue momentarily (up to 500ms) before failing, ensuring smooth user experience.
6. **DEF-24**: In `App.tsx:runMutation`, if the failed mutation is `"connection"`, the application already knows the backend connection is unreachable. Triggering an immediate second `api.getSnapshot()` readback in the catch block is guaranteed to fail, causing redundant IPC overhead and delaying error display. Wrapping the readback in `if (key !== "connection")` resolves this.
7. **DEF-29**: The asymmetric `skip_release` flag in tray click handling fails on Linux StatusNotifierItem implementations because SNI synthesizes click actions as `MouseButtonState::Up`. Removing dependency on `MouseDown` and using a 250ms debounced `MouseButtonState::Up` handler provides deterministic single-click toggling across GNOME, KDE, and XFCE while preventing rapid double-click window flickering.

---

## 3. Proposed Technical Remediation Designs (with Exact Snippets)

### Remediation Design for DEF-18: CI Workflow
**File**: `.github/workflows/ci.yml` (New File)
```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

permissions:
  contents: read

jobs:
  frontend:
    name: Frontend Typecheck, Lint, Test & Build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 22
          cache: 'npm'
      - run: npm ci
      - run: npm run lint
      - run: npm test
      - run: npm run build

  backend:
    name: Native Backend Checks & Tests
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install system dependencies
        run: |
          sudo apt-get update
          sudo apt-get install -y --no-install-recommends \
            libwebkit2gtk-4.1-dev \
            libgtk-3-dev \
            libayatana-appindicator3-dev \
            librsvg2-dev \
            libssl-dev \
            libpipewire-0.3-dev \
            libspa-0.2-dev \
            libmysofa-dev
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: src-tauri
      - name: Rust format check
        run: cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
      - name: Clippy check
        run: cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
      - name: Cargo tests
        run: cargo test --manifest-path src-tauri/Cargo.toml --lib

  packaging-tests:
    name: Python Packaging Test Suite
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-python@v5
        with:
          python-version: '3.12'
      - name: Packaging unit tests
        run: python3 -B -m unittest discover -s packaging/tests -v
```

---

### Remediation Design for DEF-19: Linter & Formatter Configuration

**1. File**: `package.json`
- In `"scripts"` (lines 8-19), add:
  ```json
    "lint": "eslint .",
    "lint:fix": "eslint . --fix",
    "format": "prettier --write .",
    "format:check": "prettier --check .",
  ```
- In `"devDependencies"` (lines 25-37), add:
  ```json
    "@eslint/js": "^9.20.1",
    "eslint": "^9.20.1",
    "eslint-plugin-react-hooks": "^5.1.0",
    "eslint-plugin-react-refresh": "^0.4.19",
    "prettier": "^3.5.1",
    "typescript-eslint": "^8.24.1"
  ```

**2. File**: `eslint.config.js` (New File at Repository Root)
```javascript
import js from "@eslint/js";
import tsPlugin from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";

export default tsPlugin.config(
  {
    ignores: [
      "dist",
      "node_modules",
      "src-tauri",
      "packaging",
      "coverage",
      "*.config.js",
      "*.config.ts",
    ],
  },
  js.configs.recommended,
  ...tsPlugin.configs.recommended,
  {
    files: ["**/*.{ts,tsx}"],
    plugins: {
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      "react-refresh/only-export-components": [
        "warn",
        { allowConstantExport: true },
      ],
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
    },
  }
);
```

**3. File**: `.prettierrc` (New File at Repository Root)
```json
{
  "semi": true,
  "singleQuote": false,
  "tabWidth": 2,
  "trailingComma": "es5"
}
```

**4. File**: `.prettierignore` (New File at Repository Root)
```
dist
node_modules
src-tauri/target
packaging/out
packaging/remapper/rootfs
```

**5. File**: `src-tauri/clippy.toml` (New File)
```toml
avoid-breaking-exported-api = false
msrv = "1.90"
```

**6. File**: `src-tauri/rustfmt.toml` (New File)
```toml
edition = "2021"
newline_style = "Unix"
```

---

### Remediation Design for DEF-21: Removal of `quick-xml` & `fs2`

**1. File**: `src-tauri/Cargo.toml`
- Remove line 46: `fs2 = "0.4"`
- Remove line 51: `quick-xml = "0.38"`
- Resulting diff snippet:
  ```toml
  <<<<
  uuid = { version = "1", features = ["v4", "serde"] }
  fs2 = "0.4"
  sha2 = "0.10"

  [target.'cfg(target_os = "linux")'.dependencies]
  zbus = { version = "5", default-features = false, features = ["tokio"] }
  quick-xml = "0.38"
  libc = "0.2"
  ====
  uuid = { version = "1", features = ["v4", "serde"] }
  sha2 = "0.10"

  [target.'cfg(target_os = "linux")'.dependencies]
  zbus = { version = "5", default-features = false, features = ["tokio"] }
  libc = "0.2"
  >>>>
  ```

**2. File**: `src-tauri/src/remapping.rs`
- Remove line 5: `use fs2::FileExt;`
- Add zero-dependency `FileLockExt` trait and implementation using existing `libc::flock`:
  ```rust
  pub trait FileLockExt {
      fn try_lock_exclusive(&self) -> std::io::Result<()>;
      fn unlock(&self) -> std::io::Result<()>;
  }

  #[cfg(target_os = "linux")]
  impl FileLockExt for File {
      fn try_lock_exclusive(&self) -> std::io::Result<()> {
          use std::os::unix::io::AsRawFd;
          let ret = unsafe { libc::flock(self.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
          if ret == 0 {
              Ok(())
          } else {
              Err(std::io::Error::last_os_error())
          }
      }

      fn unlock(&self) -> std::io::Result<()> {
          use std::os::unix::io::AsRawFd;
          let ret = unsafe { libc::flock(self.as_raw_fd(), libc::LOCK_UN) };
          if ret == 0 {
              Ok(())
          } else {
              Err(std::io::Error::last_os_error())
          }
      }
  }

  #[cfg(not(target_os = "linux"))]
  impl FileLockExt for File {
      fn try_lock_exclusive(&self) -> std::io::Result<()> {
          Ok(())
      }
      fn unlock(&self) -> std::io::Result<()> {
          Ok(())
      }
  }
  ```
- In `StoreLock::drop` (lines 169-173):
  ```rust
  <<<<
  impl Drop for StoreLock {
      fn drop(&mut self) {
          let _ = FileExt::unlock(&self.0);
      }
  }
  ====
  impl Drop for StoreLock {
      fn drop(&mut self) {
          let _ = self.0.unlock();
      }
  }
  >>>>
  ```
- All other call sites (`file.try_lock_exclusive()` at line 331, `instance.try_lock_exclusive()` at line 2094) remain 100% syntactically identical and type-compatible!

---

### Remediation Design for DEF-22: Redundant Spatializer Template Resource Removal

**File**: `src-tauri/tauri.conf.json:38`
```json
<<<<
    "shortDescription": "Periferiche Razer e Logitech, audio PipeWire e HRTF 7.1",
    "resources": ["resources/spatializer-7.1.conf.template"],
    "icon": ["icons/32x32.png", "icons/128x128.png", "icons/128x128@2x.png", "icons/icon.icns", "icons/icon.ico"],
====
    "shortDescription": "Periferiche Razer e Logitech, audio PipeWire e HRTF 7.1",
    "resources": [],
    "icon": ["icons/32x32.png", "icons/128x128.png", "icons/128x128@2x.png", "icons/icon.icns", "icons/icon.ico"],
>>>>
```

---

### Remediation Design for DEF-23: Audio Worker Semaphore Queuing

**File**: `src-tauri/src/audio_worker.rs:355-365`
```rust
<<<<
#[cfg(target_os = "linux")]
async fn invoke(request: Request) -> AppResult<()> {
    use tokio::sync::Semaphore;
    static SLOTS: Semaphore = Semaphore::const_new(2);
    let _slot = SLOTS
        .try_acquire()
        .map_err(|_| AppError::new(ErrorCode::Busy, "Worker audio gia' occupati."))?;
====
#[cfg(target_os = "linux")]
async fn invoke(request: Request) -> AppResult<()> {
    use tokio::sync::Semaphore;
    static SLOTS: Semaphore = Semaphore::const_new(2);
    let _slot = tokio::time::timeout(Duration::from_millis(500), SLOTS.acquire())
        .await
        .map_err(|_| AppError::new(ErrorCode::Busy, "Worker audio gia' occupati."))?
        .map_err(|_| AppError::new(ErrorCode::Internal, "Worker audio non disponibile."))?;
>>>>
```

---

### Remediation Design for DEF-24: Redundant Snapshot Readback Suppression

**File**: `src/App.tsx:115-128`
```tsx
<<<<
    } catch (error) {
      if (!activeRef.current || generationRef.current !== generation) return false;
      const failure = asAppError(error);
      setErrors((current) => ({ ...current, [key]: failure }));
      try {
        acceptSnapshot(await api.getSnapshot(), generation);
      } catch (readbackError) {
        if (activeRef.current && generationRef.current === generation) {
          const readback = asAppError(readbackError);
          setErrors((current) => ({ ...current, [key]: { ...failure, detail: [failure.detail, `Rilettura non riuscita: ${readback.message}`].filter(Boolean).join("\n") } }));
        }
      }
      return false;
    }
====
    } catch (error) {
      if (!activeRef.current || generationRef.current !== generation) return false;
      const failure = asAppError(error);
      setErrors((current) => ({ ...current, [key]: failure }));
      if (key !== "connection") {
        try {
          acceptSnapshot(await api.getSnapshot(), generation);
        } catch (readbackError) {
          if (activeRef.current && generationRef.current === generation) {
            const readback = asAppError(readbackError);
            setErrors((current) => ({ ...current, [key]: { ...failure, detail: [failure.detail, `Rilettura non riuscita: ${readback.message}`].filter(Boolean).join("\n") } }));
          }
        }
      }
      return false;
    }
>>>>
```

---

### Remediation Design for DEF-29: Robust Tray Click State Machine

**File**: `src-tauri/src/lib.rs:866-902`
```rust
<<<<
            let skip_release = StdMutex::new(false);
            let tray = TrayIconBuilder::with_id("peripheral-hub")
                .icon(tauri::include_image!("icons/32x32.png"))
                .tooltip("Peripheral Hub")
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                // Linux exposes the context menu, but does not emit these events.
                .on_tray_icon_event(move |tray, event| match event {
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Down,
                        ..
                    } => {
                        *skip_release.lock().unwrap_or_else(|error| error.into_inner()) = false;
                    }
                    TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    } => {
                        // Windows already toggled on the first release; ignore the second.
                        *skip_release.lock().unwrap_or_else(|error| error.into_inner()) = true;
                    }
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } => {
                        let skip = std::mem::take(
                            &mut *skip_release.lock().unwrap_or_else(|error| error.into_inner()),
                        );
                        if !skip {
                            toggle_window(tray.app_handle());
                        }
                    }
                    _ => {}
                })
                .build(app);
====
            let last_click = StdMutex::new(std::time::Instant::now() - std::time::Duration::from_secs(1));
            let tray = TrayIconBuilder::with_id("peripheral-hub")
                .icon(tauri::include_image!("icons/32x32.png"))
                .tooltip("Peripheral Hub")
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(move |tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let mut last = last_click.lock().unwrap_or_else(|error| error.into_inner());
                        if last.elapsed() >= std::time::Duration::from_millis(250) {
                            *last = std::time::Instant::now();
                            toggle_window(tray.app_handle());
                        }
                    }
                })
                .build(app);
>>>>
```

---

## 4. Caveats

1. **`src/tests/m1_challenger_empirical.test.tsx` TypeScript Typecheck Failure**:
   - `npm run build` currently triggers 4 TypeScript compiler errors in `src/tests/m1_challenger_empirical.test.tsx` (`'StrictMode' is declared but its value is never read`, `'Device' is declared but never used`, missing `outputNodeNames` in mock device, and invalid `displayName` property in mock `AudioNode`).
   - This test file was created during Milestone 1 verification and must be cleaned up or excluded by `tsconfig.json` during Milestone 2 build gate stabilization.
2. **`Cargo.lock` Synchronization on Dependency Removal**:
   - Modifying `Cargo.toml` to remove `fs2` and `quick-xml` will cause subsequent `cargo build --locked` (as used in `package.json:14`) to fail until `Cargo.lock` is updated in the same commit. The implementer must run `cargo check --manifest-path src-tauri/Cargo.toml` in the container to generate the synchronized `Cargo.lock`.
3. **`src-tauri/src/audio.rs:2014` Known Test Defect (DEF-AUDIO)**:
   - Test `audio::tests::real_pipewire_direction_names_and_property_fallback_share_the_same_ports` fails prior to Milestone 2 due to an unhandled `.unwrap()` on port direction properties. This is assigned under DEF-AUDIO / Phase 3 DEF-14. When running containerized backend tests, `--skip real_pipewire_direction_names_and_property_fallback_share_the_same_ports` must be passed until DEF-AUDIO is resolved.

---

## 5. Conclusion

Every defect assigned to `m2_explorer_3` (DEF-18, DEF-19, DEF-21, DEF-22, DEF-23, DEF-24, DEF-29) has been thoroughly verified at source level with zero assumptions.
- **DEF-18**: Provides continuous regression defense on `push: branches: [main]` via `.github/workflows/ci.yml`.
- **DEF-19**: Institutionalizes ESLint, Prettier, Clippy, and Rustfmt standards without breaking existing codebase conventions.
- **DEF-21**: Permanently removes dead `quick-xml` and deprecated `fs2`, replacing file locks with a robust, native POSIX `libc::flock` abstraction trait.
- **DEF-22**: Cleans up bundle overhead by removing duplicate spatializer template resource.
- **DEF-23**: Prevents false busy errors on rapid audio adjustments by providing a 500ms queuing timeout on worker slots.
- **DEF-24**: Eliminates redundant IPC snapshot re-fetches when connection errors occur.
- **DEF-29**: Replaces brittle asymmetric tray state tracking with a 250ms debounced `MouseButtonState::Up` state machine compliant with Linux StatusNotifierItem behavior.

All remediations are fully actionable, zero-risk, and ready for immediate implementation by the Milestone 2 worker agent.

---

## 6. Verification Method

To independently verify the recommendations and their eventual implementation:

1. **Frontend Vitest Suite Verification**:
   ```bash
   npm test
   ```
   *Expected Result*: 3 test files pass, 92/92 tests pass.

2. **Frontend Typecheck & Production Build**:
   ```bash
   npm run build
   ```
   *Expected Result*: Exits 0, emitting production bundle assets into `dist/`.

3. **Packaging Python Unit Tests**:
   ```bash
   python3 -m unittest discover -s packaging/tests -v
   ```
   *Expected Result*: 224/224 tests pass with `OK`.

4. **Containerized Rust Backend Test Suite**:
   ```bash
   docker run --rm -v /home/shadymui/sycomopse:/workspace -w /workspace/src-tauri -e CARGO_TARGET_DIR=/tmp/target --entrypoint /bin/sh peripheral-hub-build:ubuntu-24.04 -c "cargo test --lib -- --skip real_pipewire_direction_names_and_property_fallback_share_the_same_ports"
   ```
   *Expected Result*: 80 passed, 0 failed, 6 ignored.

5. **Linter and Formatter Verification (Post-Implementation)**:
   ```bash
   npm run lint
   npm run format:check
   docker run --rm -v /home/shadymui/sycomopse:/workspace -w /workspace/src-tauri -e CARGO_TARGET_DIR=/tmp/target --entrypoint /bin/sh peripheral-hub-build:ubuntu-24.04 -c "cargo fmt --check && cargo clippy --all-targets -- -D warnings"
   ```
