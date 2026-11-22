# Milestone 2 Forensic Integrity Audit Report

**Work Product**: Milestone 2 Defect Remediation (Phase 3 DEF-13..17 and Phase 4 DEF-18..30, DEF-AUDIO)  
**Profile**: General Project  
**Integrity Mode**: Development (per `ORIGINAL_REQUEST.md:8`)  
**Verdict**: **CLEAN**

---

## 1. Observation

All 18 cataloged defects from `ANALISI_CODICE.md` (§DEF-13 through §DEF-30) and auxiliary defect `DEF-AUDIO` were forensically inspected and empirically verified against the codebase:

### 1.1 Demo Engine Isolation (`DEF-13`)
- **Inspection**: `src/api.ts:106-140` implements `createHubApi()` with conditional dynamic import `import("./demoApi")` gated strictly on `import.meta.env.VITE_DEMO === "true"`.
- **Bundle Verification**: Executed `npm run build`. In production mode, Vite dead-code eliminates the `VITE_DEMO === "true"` block.
  - Search command:
    ```bash
    grep -E "Razer Viper|BlackShark|demo-viper|Simula cuffie" dist/assets/*.js
    ```
  - Result: 0 matches (exit code 1).
  - Built files:
    - `dist/index.html` (0.51 kB)
    - `dist/assets/index-Ws5j8gHV.css` (33.03 kB)
    - `dist/assets/index-Do9Z57AO.js` (291.09 kB)
  - No demo mock strings or virtual device objects exist in the production bundle.

### 1.2 Backend Modularization (`DEF-14`)
- Monolithic modules were dismantled and decomposed into clean single-responsibility submodules:
  - `src-tauri/src/audio.rs` reduced from 2,264 lines to 343 lines (mostly re-exports and integration tests), backed by:
    - `src-tauri/src/audio/controller.rs` (42.6 KB)
    - `src-tauri/src/audio/graph.rs` (7.4 KB)
    - `src-tauri/src/audio/monitor.rs` (2.6 KB)
    - `src-tauri/src/audio/parser.rs` (2.7 KB)
    - `src-tauri/src/audio/registry.rs` (20.1 KB)
  - `src-tauri/src/remapping.rs` reduced from 2,345 lines to 218 lines, backed by:
    - `src-tauri/src/remapping/agent.rs` (16.7 KB)
    - `src-tauri/src/remapping/bridge.rs` (11.3 KB)
    - `src-tauri/src/remapping/controller.rs` (25.6 KB)
    - `src-tauri/src/remapping/store.rs` (18.8 KB)
    - `src-tauri/src/remapping/types.rs` (10.0 KB)

### 1.3 Systemctl Quiesce Timeout Reduction (`DEF-15`)
- In `src-tauri/src/remapping/bridge.rs:250`:
  ```rust
  pub async fn quiesce_agent() -> AppResult<()> {
      crate::process::run("systemctl", STOP_AGENT_ARGUMENTS, Duration::from_secs(5))
          .await
          .map(|_| ())
  }
  ```
  Verified: Timeout lowered from 100 seconds to 5 seconds (`Duration::from_secs(5)`).
- Stress-tested in `tests/empirical_challenge.rs:test_quiesce_agent_timeout_is_5_seconds`: execution duration bounded < 6s.

### 1.4 Dynamic FHS Resolution & System Directory Whitelist (`DEF-16` & `DEF-17`)
- In `src-tauri/src/process.rs:13-19`, `TRUSTED_SYSTEM_DIRS` defines `["/usr/bin", "/bin", "/usr/sbin", "/sbin", "/usr/local/bin"]`.
- `resolve_executable` (`src-tauri/src/process.rs:40-94`):
  - Checks environment overrides: `PERIPHERAL_HUB_HEADSETCONTROL_PATH`, `PERIPHERAL_HUB_PWDUMP_PATH`, `PERIPHERAL_HUB_WPCTL_PATH`, `PERIPHERAL_HUB_SYSTEMCTL_PATH`.
  - Enforces that absolute paths must reside in `TRUSTED_SYSTEM_DIRS` or match `PRIVATE_HEADSETCONTROL`.
  - Resolves relative binary names only across trusted directories and valid system `PATH`.
  - `owned_command` (`src-tauri/src/process.rs:96-134`): enforces whitelist (`pw-dump`, `wpctl`, `pipewire`, `headsetcontrol`, `ratbagd`, `systemctl`, `PRIVATE_HEADSETCONTROL`) and injects `libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM)`.
- In `src-tauri/src/audio_worker.rs:705-730`: `resolve_wpexec_path()` dynamically checks `PERIPHERAL_HUB_WPEXEC_PATH`, candidate paths, and system `PATH`.
- In `src-tauri/src/surround.rs:17-33`: `resolve_system_sofa_path()` dynamically checks `PERIPHERAL_HUB_SOFA_PATH` and candidate paths.

### 1.5 CI Workflow & Tooling Configuration (`DEF-18` & `DEF-19`)
- `.github/workflows/ci.yml` triggers on `push: branches: [main]` and `pull_request: branches: [main]`, executing frontend typecheck/lint/test/build, Rust formatting/clippy/tests, and packaging unit tests.
- `package.json` contains scripts `lint`, `lint:fix`, `format`, `format:check`. Configurations `eslint.config.js`, `.prettierrc`, `.prettierignore`, `src-tauri/clippy.toml`, and `src-tauri/rustfmt.toml` present and operational.
- Verified: `npm run lint` executed cleanly with 0 errors.

### 1.6 App.test.tsx Alert Timeout (`DEF-20`)
- `src/tests/App.test.tsx:305`:
  ```ts
  await screen.findByRole("alert", {}, { timeout: 3000 });
  expect(screen.getByText("800 DPI", { selector: "strong" })).toBeVisible();
  expect(getSnapshot).toHaveBeenCalledTimes(2);
  ```
  Timeout stabilized to 3000ms. All assertions retained without softening or mock evasion.

### 1.7 Dead Dependency Cleanup & POSIX `libc::flock` Locking (`DEF-21`)
- `src-tauri/Cargo.toml`: `quick-xml` and `fs2` dependencies completely removed.
- `src-tauri/src/remapping/store.rs:14-58`: `FileLockExt` implemented directly using `libc::flock(self.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB)` and `libc::LOCK_UN`. `StoreLock` provides safe RAII unlocking on drop.
- Verified via empirical tests `test_flock_basic_mutual_exclusion`, `test_store_lock_raii_drop`, and `test_flock_high_concurrency_stress`: all passed under high contention.

### 1.8 Bundle Resource Deduplication (`DEF-22`)
- `src-tauri/tauri.conf.json:38`: `"resources": []`. Redundant `spatializer-7.1.conf.template` bundling eliminated.

### 1.9 Audio Worker Semaphore Queuing (`DEF-23`)
- `src-tauri/src/audio_worker.rs:358-362`:
  ```rust
  static SLOTS: Semaphore = Semaphore::const_new(2);
  let _slot = tokio::time::timeout(Duration::from_millis(500), SLOTS.acquire())
      .await
      .map_err(|_| AppError::new(ErrorCode::Busy, "Worker audio gia' occupati."))?
      .map_err(|_| AppError::new(ErrorCode::Io, "Worker audio non disponibile."))?;
  ```
  Replaced instant failure with 500ms bounded queuing.

### 1.10 UI React Reconciliation, Focus, and Forms (`DEF-24`, `DEF-25`, `DEF-26`, `DEF-27`)
- **DEF-24**: `src/App.tsx:121` wraps snapshot readback in `if (key !== "connection")`.
- **DEF-25**: `src/App.tsx:102, 151` uses `panelRef` and `tabRefs` (`useRef<Map<string, HTMLButtonElement>>`) instead of `document.getElementById`.
- **DEF-26**: `src/views/AudioPanel.tsx:66` and `src/views/MousePanel.tsx:106` use stable keys `${headset.id}-limit-${index}` and `${device.id}-limit-${index}`.
- **DEF-27**: `src/App.tsx:187` removed dynamic recreation key `key={audioFocus.token}`; `AudioPanel.tsx:20-23` preserves draft state (`profileName`, `profileVolume`, `profileSurround`) across focus switches.

### 1.11 RFC 4122 v4 UUID Fallback (`DEF-28`)
- `src/api.ts:83-100`: `generateUuid()` implements a 3-tier fallback hierarchy (`crypto.randomUUID` -> `crypto.getRandomValues` CSPRNG -> RFC 4122 v4 pseudorandom generator).
- `src/views/AudioPanel.tsx:44`: profile creation invokes `generateUuid()`.

### 1.12 StatusNotifierItem Tray Debouncer (`DEF-29`)
- `src-tauri/src/lib.rs:866-886`: Replaced `MouseButtonState::Down` and atomic flag with `MouseButtonState::Up` and 250ms debouncer mutex, avoiding race conditions in Linux desktop shells.

### 1.13 WCAG 2.1 AA Contrast Accessibility (`DEF-30`)
- `src/styles.css:43, 64, 81, 120`: Bumped micro-labels and captions to 11px and colors to `#b4b4b4` (> 4.5:1 contrast against dark background `#181818` / `#222222`).

### 1.14 PipeWire Fixture Direction Unwrap (`DEF-AUDIO`)
- `src-tauri/src/audio/registry.rs:489-499`: Direction parsing normalizes both "input"/"output" and "in"/"out", falling back to `port.direction` property.

---

## 2. Logic Chain

1. **Absence of Facades or Stubs**: Detailed source inspection of all touched modules confirmed that all changes introduce genuine production logic rather than mock returns, pre-baked strings, or fake tests.
2. **Empirical Independent Verification**: All test suites across the frontend, Rust backend, and packaging pipelines were executed independently and passed with 100% success rate:
   - Vitest: 4 test files, 102 tests passed, 0 failures.
   - Vite build & TypeScript: 0 type errors, clean production bundle.
   - Docker container cargo test: 120 tests passed, 0 failures.
   - Packaging unittest: 224 tests passed, 0 failures.
3. **No Test Masking or Integrity Circumvention**: The slight timeout enhancement in `App.test.tsx` (DEF-20) addresses JSDOM rendering latency and preserves all underlying behavioral assertions. The empirical challenge test suite (`empirical_challenge.rs`) further validates system invariants under stress without failure.
4. **Conclusion Follows from Observations**: Because all 18 defects and DEF-AUDIO are resolved with genuine logic, no hardcoded results exist, tests pass without evasion, and all acceptance criteria for Milestone 2 are met, the work product is authentic and clean.

---

## 3. Caveats

- Tests requiring physical host audio hardware (`real_controller_spatializer_71_stereo_contract`, `isolated_pipewire_proxy_recycling_cubic_default_and_sofa`) are appropriately ignored (`#[ignore]`) during containerized execution, consistent with the project architecture.
- Full multi-distribution package generation (`packaging/build.py`) belongs to Milestone 3 scope (`ORIGINAL_REQUEST.md §R2`).

---

## 4. Conclusion

**Final Verdict**: **CLEAN**

All Milestone 2 defects (DEF-13 through DEF-30 and DEF-AUDIO) have been completely and authentically remediated. There are zero integrity violations, zero facades, and zero regression failures. The work product is ready for Milestone 3 (Verification & Distribution Packaging).

---

## 5. Verification Method

To independently reproduce the forensic verification:

```bash
# 1. Frontend tests
npm test

# 2. Frontend production build and treeshaking verification
npm run build
grep -E "Razer Viper|BlackShark|demo-viper|Simula cuffie" dist/assets/*.js # Must return 0 matches

# 3. Frontend linting
npm run lint

# 4. Native backend tests in Docker
docker run --rm -v /home/shadymui/sycomopse:/workspace -w /workspace/src-tauri \
  -e CARGO_TARGET_DIR=/tmp/target --entrypoint /bin/sh \
  peripheral-hub-build:ubuntu-24.04 -c "cargo test"

# 5. Packaging tests
python3 -m unittest discover -s packaging/tests -v
```
