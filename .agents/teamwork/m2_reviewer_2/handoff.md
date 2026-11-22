# Milestone 2 Adversarial and Quality Review Report

**Agent**: `m2_reviewer_2`  
**Verdict**: **APPROVE**  
**Integrity Status**: **CLEAN (Zero Integrity Violations)**

---

## 1. Observation

### 1.1 Direct Inspection of Key Remediation Areas

1. **Advisory File Locking (`FileLockExt` / `libc::flock`)**
   - **File**: `src-tauri/src/remapping/store.rs` (lines 14–40, 52–58, 271–290)
   - **Observed Code**:
     ```rust
     #[cfg(target_os = "linux")]
     impl FileLockExt for File {
         fn try_lock_exclusive(&self) -> std::io::Result<()> {
             use std::os::unix::io::AsRawFd;
             let ret = unsafe { libc::flock(self.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
             if ret == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
         }
         fn unlock(&self) -> std::io::Result<()> {
             use std::os::unix::io::AsRawFd;
             let ret = unsafe { libc::flock(self.as_raw_fd(), libc::LOCK_UN) };
             if ret == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
         }
     }
     ```
   - In `Store::lock()`, non-blocking acquisition loops with `try_lock_exclusive()`, matching `std::io::ErrorKind::WouldBlock` with a 3-second deadline (`Instant::now() < deadline`), sleeping 10ms between attempts, and returning `AppError(ErrorCode::Busy, ...)` if lock contention persists.
   - `StoreLock(pub File)` implements `Drop` calling `self.0.unlock()`, with Linux kernel-level automatic lock release on file descriptor close.
   - Unmaintained `fs2` and dead `quick-xml` dependencies were completely purged from `src-tauri/Cargo.toml`.

2. **System Binary Resolution & Path Traversal / Injection Security**
   - **File**: `src-tauri/src/process.rs` (lines 13–19, 27–38, 40–94, 96–134)
   - **Observed Code**:
     - `allowed(executable: &str)` strictly permits only exact strings: `"pw-dump" | "wpctl" | "pipewire" | "headsetcontrol" | "ratbagd" | "systemctl" | PRIVATE_HEADSETCONTROL`.
     - In `resolve_executable(executable)`:
       - Environment overrides (`PERIPHERAL_HUB_*_PATH`) are queried only for specific permitted keys and verified with `path.is_file()`.
       - Absolute paths are strictly validated: parent directory must match `TRUSTED_SYSTEM_DIRS` (`/usr/bin`, `/bin`, `/usr/sbin`, `/sbin`, `/usr/local/bin`). Traversal attempts such as `/usr/bin/../../bin/sh` have `parent() == "/usr/bin/../.."`, which is rejected with `ErrorCode::Validation`.
       - Trusted system directories are searched before falling back to `$PATH`, neutralizing PATH shadowing.

3. **Systemctl Quiesce Agent Timeout**
   - **File**: `src-tauri/src/remapping/bridge.rs` (lines 247–253)
   - **Observed Code**:
     ```rust
     pub async fn quiesce_agent() -> AppResult<()> {
         crate::process::run("systemctl", STOP_AGENT_ARGUMENTS, Duration::from_secs(5))
             .await
             .map(|_| ())
     }
     ```
   - The timeout for stopping `peripheral-hub-remapping-agent.service` was reduced from 100 seconds to 5 seconds.
   - In `src-tauri/src/remapping/controller.rs` (lines 583–588, 612–616), quiesce failures are caught and recorded as non-fatal in `failures`, preventing UI/IPC deadlock while allowing subsequent recovery and inventory logic to complete cleanly.

4. **Demo Mock Isolation in Production Bundle**
   - **Files**: `src/api.ts` (lines 106–142), `src/demoApi.ts`
   - **Observed Code**:
     - `createHubApi()` uses `import("./demoApi")` dynamic import strictly gated by `if (import.meta.env.VITE_DEMO === "true")`.
     - In production builds (`npm run build`), `import.meta.env.VITE_DEMO` is undefined / not `"true"`, allowing Vite / Rollup dead-code elimination to drop `demoApi.ts`.
     - Verification: inspected `dist/assets/index-*.js`:
       - `grep -i "createDemoApi" dist/assets/*.js`: **0 matches (NOT FOUND)**.
       - `grep -i "demo-viper" dist/assets/*.js`: **0 matches**.
       - `grep -i "BlackShark" dist/assets/*.js`: **0 matches**.
       - `grep -i "demoValidation" dist/assets/*.js`: **0 matches**.

5. **RFC 4122 v4 UUID Fallback Implementation**
   - **File**: `src/api.ts` (lines 83–100)
   - **Observed Code**:
     - Tier 1: `crypto.randomUUID()` when supported natively.
     - Tier 2: CSPRNG `crypto.getRandomValues(bytes)` with exact RFC 4122 v4 masking:
       `bytes[6] = (bytes[6] & 0x0f) | 0x40;` (version 4)
       `bytes[8] = (bytes[8] & 0x3f) | 0x80;` (variant 10xx)
     - Tier 3: Pseudo-random fallback using `"xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx"` pattern.
     - Verification: generated 20,000 UUIDs (10,000 via Tier 2 CSPRNG and 10,000 via Tier 3 fallback):
       - 100% matched regex `^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$`.
       - Exactly 0 duplicates across all 20,000 generated keys.

6. **AudioPanel Draft Form State Preservation**
   - **Files**: `src/App.tsx` (lines 181, 187), `src/views/AudioPanel.tsx` (lines 10–28)
   - **Observed Code**:
     - `App.tsx`: Removed dynamic `key={audioFocus.token}` from `<AudioPanel />`, ensuring the component is not unmounted on tab focus changes.
     - `AudioPanel.tsx`: Draft state variables (`profileName`, `profileVolume`, `profileMute`, `profileSurround`, `profileOutput`) are maintained in local state.
     - `useEffect` at line 19 updates only `outputName` when `initialOutput` arrives, leaving local draft fields undisturbed.
     - `setProfileOutput` retains `current` if it remains valid in physical outputs.

7. **Backend Modularization**
   - `src-tauri/src/audio.rs` (343 lines) modularized into `src-tauri/src/audio/{controller, graph, monitor, parser, registry}.rs`.
   - `src-tauri/src/remapping.rs` (218 lines) modularized into `src-tauri/src/remapping/{agent, bridge, controller, store, types}.rs`.
   - Facade re-exports retain 100% backward compatibility for all library consumers and test harnesses.

8. **Accessibility & Contrast Compliance (WCAG 2.1 AA)**
   - **File**: `src/styles.css` (lines 43, 64, 81, 120)
   - Text color updated to `#b4b4b4` and font size bumped to 11px.
   - Contrast calculation against dark backgrounds (`#181818`, `#1b1b1b`, `#222222`):
     - `#b4b4b4` on `#181818`: **8.56:1**
     - `#b4b4b4` on `#1b1b1b`: **8.31:1**
     - `#b4b4b4` on `#222222`: **7.67:1**
     - All exceed the 4.5:1 WCAG 2.1 AA threshold.

---

### 1.2 Independent Test Suite Verification Results

1. **Frontend Vitest Test Suite**
   - Command: `npm test`
   - Result: **3 test files passed, 92 tests passed (100% pass, 0 failures)**
   - Duration: 18.71s

2. **Frontend Typecheck & Production Build**
   - Command: `npm run build` (`tsc --noEmit && vite build`)
   - Result: **Exit code 0, 0 TypeScript errors, production assets emitted into `dist/assets/` in 1.59s**

3. **Frontend ESLint Linting**
   - Command: `npm run lint`
   - Result: **Exit code 0, 0 errors (7 harmless React fast-refresh/hooks warnings)**

4. **Native Backend Test Suite in Docker**
   - Command: `docker run --rm -v /home/shadymui/sycomopse:/workspace -w /workspace/src-tauri -e CARGO_TARGET_DIR=/tmp/target --entrypoint /bin/sh peripheral-hub-build:ubuntu-24.04 -c "cargo test"`
   - Result: **Exit code 0, 120 tests passed, 0 failures, 2 ignored (OCI-gated)**
     - `peripheral_hub_lib`: 81 passed; 0 failed
     - `audio_dsp_contract`: 2 passed; 0 failed
     - `empirical_challenge`: 8 passed; 0 failed
     - `hardware_contracts`: 29 passed; 0 failed

5. **Python Packaging Test Suite**
   - Command: `python3 -m unittest discover -s packaging/tests -v`
   - Result: **Ran 224 tests in 2.907s. Status: OK (224 passed, 0 failed)**

---

## 2. Logic Chain

1. *Advisory Locking Safety*: By invoking POSIX `libc::flock(fd, LOCK_EX | LOCK_NB)` directly on the file descriptor of `/run/user/<uid>/peripheral-hub/mouse-remapping.lock`, kernel-enforced mutual exclusion is achieved without unmaintained dependencies (`fs2`). Contention is handled gracefully via bounded retry loop (3s deadline, 10ms sleep) mapping `WouldBlock` to `ErrorCode::Busy`. RAII `Drop` ensures locks are released cleanly even on panic.
2. *Path Validation & Command Injection Prevention*: The strict enumeration in `allowed()` prevents execution of arbitrary commands or shell wrappers. `resolve_executable()` prioritizes trusted system directories over `$PATH`, preventing binary hijacking via tampered `$PATH` environments while permitting explicit administrative overrides via environment variables for NixOS/Guix.
3. *Systemctl Timeout*: Reducing `systemctl --user stop` timeout to 5 seconds eliminates protracted synchronous freezes of the Tauri UI while providing more than enough time for orderly daemon termination.
4. *Bundle Tree-Shaking*: Gating `./demoApi` dynamic import on `import.meta.env.VITE_DEMO === "true"` enables standard bundler dead-code elimination. In production mode, the bundle contains zero mock strings, fixture devices, or simulated PipeWire graph data.
5. *UUID Conformance & Collision Safety*: The three-tier fallback in `generateUuid()` guarantees valid RFC 4122 v4 UUIDs regardless of WebView/WebKitGTK age, maintaining full uniqueness and avoiding React reconciliation anomalies.
6. *Draft Form State Retention*: Removing the unmounting key on `AudioPanel` and synchronizing only the card selector via `useEffect` preserves user input fields (name, volume, mute, surround flags) across background PipeWire events.
7. *Integrity & Anti-Cheat Validation*: Inspection confirmed that all tests exercise real logic without hardcoded return values, facade stubs, or test bypasses.

---

## 3. Caveats

- Tests requiring dedicated physical sound cards or unisolated host hardware (`real_controller_spatializer_71_stereo_contract`, `isolated_pipewire_proxy_recycling_cubic_default_and_sofa`) remain gated for explicit OCI test runners.
- The `FileLockExt` implementation uses Linux-specific syscalls (`libc::flock`), backed by safe no-op stubs on non-Linux platforms where daemon operations are disabled.

---

## 4. Conclusion

All 18 defects cataloged for Milestone 2 (§DEF-13 through §DEF-30) as well as §DEF-AUDIO have been remediated cleanly, robustly, and with high engineering fidelity. Concurrency, memory, security, and interface contracts are upheld across the full stack. All automated test suites across frontend, backend, and packaging pass 100% with zero regressions.

**Final Verdict**: **APPROVE**

---

## 5. Verification Method

To independently reproduce this verification:

1. **Frontend Tests**:
   ```bash
   npm test
   ```
2. **Frontend Typecheck & Production Build**:
   ```bash
   npm run build
   ```
3. **Frontend Linting**:
   ```bash
   npm run lint
   ```
4. **Backend Test Suite in Build Container**:
   ```bash
   docker run --rm -v /home/shadymui/sycomopse:/workspace -w /workspace/src-tauri -e CARGO_TARGET_DIR=/tmp/target --entrypoint /bin/sh peripheral-hub-build:ubuntu-24.04 -c "cargo test"
   ```
5. **Packaging Unit Test Suite**:
   ```bash
   python3 -m unittest discover -s packaging/tests -v
   ```
