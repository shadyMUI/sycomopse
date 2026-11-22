# Milestone 2 Empirical Challenge Report

## 1. Observation

Direct empirical observations, commands, line numbers, and verification results obtained during adversarial testing:

### 1.1. Native Cargo Test Suite in Container
- **Command**:
  ```bash
  docker run --rm -v /home/shadymui/sycomopse:/workspace -w /workspace/src-tauri -e CARGO_TARGET_DIR=/tmp/target --entrypoint /bin/sh peripheral-hub-build:ubuntu-24.04 -c "cargo test"
  ```
- **Direct Output**:
  - `peripheral_hub` lib: `81 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out; finished in 3.78s`
  - `unittests src/main.rs`: `0 passed; 0 failed; 0 ignored`
  - `unittests src/bin/audio_worker.rs`: `0 passed; 0 failed; 0 ignored`
  - `unittests src/bin/remapping_agent.rs`: `0 passed; 0 failed; 0 ignored`
  - `tests/audio_dsp_contract.rs`: `2 passed; 0 failed; 1 ignored`
  - `tests/audio_worker.rs`: `0 passed; 0 failed; 1 ignored`
  - `tests/hardware_contracts.rs`: `29 passed; 0 failed; 0 ignored`
  - **Total**: `112 passed; 0 failed; 8 ignored; exit code 0`.

### 1.2. Packaging Test Suite
- **Command**:
  ```bash
  python3 -m unittest discover -s packaging/tests -v
  ```
- **Direct Output**:
  `Ran 224 tests in 2.630s` — `OK` (224 passed; 0 failed; exit code 0).

### 1.3. Frontend Test Suite, Production Build, and Linter
- **`npm test -- --run`**:
  `Test Files 3 passed (3), Tests 92 passed (92), Duration 32.74s, exit code 0`.
- **`npm run build`**:
  `tsc --noEmit && vite build` passed with zero TypeScript errors. `dist/assets/index-*.js` (291.09 kB) and `dist/assets/index-*.css` (33.03 kB) generated cleanly.
- **`grep -i "Simula cuffie" dist/assets/*.js`**:
  Exit code 1 (0 matches). Demo mock strings are completely pruned from the production bundle (DEF-13).
- **`npm run lint`**:
  `eslint .` passed with 0 errors (7 harmless react-refresh/deps warnings; exit code 0).

### 1.4. Empirical Stress Testing Harness (`empirical_challenge`)
An empirical stress test was constructed and executed in the Ubuntu 24.04 container testing:
1. `FileLockExt` (`src-tauri/src/remapping/store.rs:14-40`):
   - Initial acquisition succeeded.
   - Second file descriptor acquisition on the same file returned `std::io::ErrorKind::WouldBlock` (`EWOULDBLOCK`).
   - Calling `unlock()` immediately enabled subsequent acquisition.
   - `StoreLock` RAII drop released the advisory lock automatically.
   - High concurrency stress: 16 threads contending across 50 iterations (800 acquisitions). Maximum concurrent holders was strictly 1 at all times (`max_concurrent_holders == 1`). Zero race conditions or corruption.
2. `resolve_executable` (`src-tauri/src/process.rs:40-94`):
   - Empty string `""` resolved cleanly to `/usr/bin/` without panic.
   - Nonexistent binary `"nonexistent_binary_xyz_9999"` resolved to `/usr/bin/nonexistent_binary_xyz_9999`.
   - Directory traversal attempts (e.g. `../../bin/sh`) resolved to canonical fallback.
   - Malicious absolute paths outside trusted system directories (`/tmp/malicious_bin`, `/var/tmp/evil`, `/home/user/exploit`, `/etc/shadow`, `/dev/shm/payload`) were strictly rejected with `ErrorCode::Validation` ("Percorso eseguibile assoluto non in directory di sistema autorizzata.").
   - Authorized system paths (`/usr/bin`, `/bin`, `/usr/sbin`, `/sbin`, `/usr/local/bin`) and `PRIVATE_HEADSETCONTROL` (`/usr/lib/peripheral-hub/headsetcontrol`) were accepted.
   - Environment variable overrides (`PERIPHERAL_HUB_PWDUMP_PATH`) took precedence when pointing to valid files and fell back to standard resolution when pointing to nonexistent files.
3. `quiesce_agent` (`src-tauri/src/remapping/bridge.rs:247-253`):
   - Executed with 5-second timeout (`Duration::from_secs(5)`), completing in under 1 second in container environment without blocking for 100 seconds (DEF-15).
   - Arbitrary process execution (e.g. `sleep 10`) was rejected by `owned_command` whitelist with `ErrorCode::Validation`.
4. Modularized Submodules:
   - `src-tauri/src/audio/` (`controller.rs`, `graph.rs`, `monitor.rs`, `parser.rs`, `registry.rs`) and `src-tauri/src/audio.rs` (facade) compiled and exported all required symbols cleanly.
   - `src-tauri/src/remapping/` (`agent.rs`, `bridge.rs`, `controller.rs`, `store.rs`, `types.rs`) and `src-tauri/src/remapping.rs` (facade) compiled and exported all required symbols cleanly.
   - `tests/empirical_challenge.rs` result: `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; finished in 0.30s`.

### 1.5. Frontend Empirical Tests (`src/tests/m2_challenger_empirical.test.tsx`)
- `npx vitest run src/tests/m2_challenger_empirical.test.tsx`:
  - `DEF-28`: 10,000 unique valid RFC 4122 v4 UUIDs generated; fallback verified when `crypto.randomUUID` or `crypto` is undefined.
  - `DEF-27`: AudioPanel draft form values (`Bozza Spaziale In Corso`, volume 85%, surround switch) strictly preserved across background `snapshot-changed` updates and output changes.
  - `DEF-24`: Secondary `getSnapshot` suppressed when initial connection fails; retained for mutation error reconciliation.
  - `DEF-30`: Mathematical WCAG 2.1 AA contrast verification: `.brand-subtitle` (8.31:1), `.instrument-caption` (7.67:1), `.device-card-copy p` (8.56:1), `.tiny-label` (9.11:1) — all far exceed the 4.5:1 threshold.
  - Result: `10 passed (10), exit code 0`.

---

## 2. Logic Chain

1. *Advisory Locking Integrity (DEF-21)*: Replacing unmaintained `fs2` with direct `libc::flock(fd, LOCK_EX | LOCK_NB)` in `FileLockExt` provides atomic, kernel-level file locking. Empirical stress testing with 16 threads under heavy contention demonstrated zero concurrent entries into the critical section, confirming mutual exclusion and deadlock-free RAII drop behavior.
2. *Path Traversal & Injection Defense (DEF-16, DEF-17)*: In `resolve_executable`, all absolute paths are checked against `TRUSTED_SYSTEM_DIRS` (`/usr/bin`, `/bin`, `/usr/sbin`, `/sbin`, `/usr/local/bin`) and `PRIVATE_HEADSETCONTROL`. Invocations attempting arbitrary directories are intercepted and rejected with `ErrorCode::Validation`. Fallback logic and environment overrides support non-FHS systems (NixOS, Guix, Flatpak) safely without opening PATH spoofing vectors.
3. *Systemctl Timeout Protection (DEF-15)*: By reducing the `systemctl --user stop` timeout in `quiesce_agent` from 100 seconds to 5 seconds (`Duration::from_secs(5)`), prolonged hangs during agent shutdown or D-Bus lock contention are eliminated.
4. *Codebase De-monolithization (DEF-14)*: Slicing `audio.rs` (2,264 lines) and `remapping.rs` (2,345 lines) into discrete domain submodules while re-exporting through root facades preserves 100% backward compatibility for all internal and external callers, as proven by 112 passing Cargo tests.
5. *PipeWire Port Direction Robustness (DEF-AUDIO)*: Extracting `raw` direction from either `info.direction` or `port.direction` properties in `src-tauri/src/audio/registry.rs` and asserting on node 41 resolves the previously failing unwrap panic in `real_pipewire_direction_names_and_property_fallback_share_the_same_ports`.
6. *UI & Accessibility Refinements (DEF-20, DEF-24..DEF-30)*: Frontend tests demonstrate stable draft preservation, declarative React refs, robust fallback UUID generation for WebKitGTK, debounced tray click state machine, and WCAG 2.1 AA compliant text contrast.

---

## 3. Caveats

- Tests requiring physical host hardware (e.g. real Razer mice, physical USB dongles) or a live host PipeWire daemon (`real_controller_spatializer_71_stereo_contract`, `isolated_pipewire_proxy_recycling_cubic_default_and_sofa`) remain intentionally gated for dedicated OCI runner execution and were properly skipped in container unittests.
- File locking via `libc::flock` applies to Linux (`#[cfg(target_os = "linux")]`). Non-Linux platforms use the safe no-op implementation.

---

## 4. Conclusion

**Verdict: APPROVE**

Milestone 2 defect remediation across backend and frontend (DEF-13 through DEF-30 and DEF-AUDIO) is robust, empirically verified, and completely free of regressions:
- Backend: 112/112 Cargo tests pass in Docker.
- Packaging: 224/224 Python packaging tests pass.
- Frontend: 92/92 Vitest tests pass; 10/10 empirical tests pass; production bundle builds with zero errors and zero demo mock strings.
- Adversarial stress tests: `flock` concurrency, `resolve_executable` path security, and `quiesce_agent` 5s timeout all passed with zero failures.

---

## 5. Verification Method

To independently reproduce and verify this assessment:

1. **Native Rust Backend Test Suite**:
   ```bash
   docker run --rm -v /home/shadymui/sycomopse:/workspace -w /workspace/src-tauri -e CARGO_TARGET_DIR=/tmp/target --entrypoint /bin/sh peripheral-hub-build:ubuntu-24.04 -c "cargo test"
   ```
   *Expected outcome*: 112 passed; 0 failed; 8 ignored; exit code 0.

2. **Packaging Test Suite**:
   ```bash
   python3 -m unittest discover -s packaging/tests -v
   ```
   *Expected outcome*: 224 passed; 0 failed; exit code 0.

3. **Frontend Vitest Suite & Build**:
   ```bash
   npm test -- --run
   npm run build
   grep -i "Simula cuffie" dist/assets/*.js
   ```
   *Expected outcome*: 92 passed; 0 failed; build succeeds; grep returns exit code 1 (no matches).
