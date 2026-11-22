# Project: Sycomopse Defect Remediation & Distribution Packaging

## Architecture
- Presentation Layer: React 19, TypeScript, Vite (`src/`)
- Native Backend: Rust 2021, Tauri v2 (`src-tauri/`)
- Auxiliary Daemons/Workers: `peripheral-hub-remapping-agent`, `peripheral-hub-audio-worker`
- Packaging Pipeline: `packaging/build.py`, `packaging/ci/matrix.py`, `packaging/targets.json`
- Verification Gates: Vitest, TypeScript typecheck (`tsc --noEmit`), Vite production build, Cargo backend test suite in Docker container, Python packaging test suite (`packaging/tests/`).

## Feature Inventory
| # | Feature | Description | Milestone | Source |
|---|---------|-------------|-----------|--------|
| 1 | DEF-01 | Linux exit freeze deadlock (`request_exit` omission) | M1 | ANALISI_CODICE.md §DEF-01 |
| 2 | DEF-02 | `OwnedChild` drop panic vector (`audio_worker.rs`) | M1 | ANALISI_CODICE.md §DEF-02 |
| 3 | DEF-03 | Auxiliary binaries bundling (`externalBin` in `tauri.conf.json`) | M1 | ANALISI_CODICE.md §DEF-03 |
| 4 | DEF-04 | HRTF non-null runtime crash on unmapped profile ID (`api.ts`) | M1 | ANALISI_CODICE.md §DEF-04 |
| 5 | DEF-05 | Conditional `snapshot-changed` emission on true state changes | M1 | ANALISI_CODICE.md §DEF-05 |
| 6 | DEF-06 | Granular hardware mutex locking separating remapping from hardware | M1 | ANALISI_CODICE.md §DEF-06 |
| 7 | DEF-07 | OpenRazer daemon active check & sysfs readback optimization | M1 | ANALISI_CODICE.md §DEF-07 |
| 8 | DEF-08 | React `ErrorBoundary` in `main.tsx` and `App.tsx` | M1 | ANALISI_CODICE.md §DEF-08 |
| 9 | DEF-09 | W3C HTML5 compliance: phrasing content only in `<button>` (`DeviceCard.tsx`) | M1 | ANALISI_CODICE.md §DEF-09 |
| 10 | DEF-10 | User draft preservation during live events (`Controls.tsx`, `MousePanel.tsx`) | M1 | ANALISI_CODICE.md §DEF-10 |
| 11 | DEF-11 | `JSON.stringify` memoization via `useMemo` in `MouseButtons.tsx` | M1 | ANALISI_CODICE.md §DEF-11 |
| 12 | DEF-12 | `React.memo` memoization on `DeviceCard`, `DeviceIllustration`, `Dashboard` | M1 | ANALISI_CODICE.md §DEF-12 |
| 13 | DEF-13 | Isolation of demo mock engine from production bundle (`demoApi.ts`) | M2 | ANALISI_CODICE.md §DEF-13 |
| 14 | DEF-14 | Backend modularization of `remapping.rs` and `audio.rs` | M2 | ANALISI_CODICE.md §DEF-14 |
| 15 | DEF-15 | `quiesce_agent` systemctl timeout reduction to 5 seconds | M2 | ANALISI_CODICE.md §DEF-15 |
| 16 | DEF-16 | Fragile FHS path replacement with environment overrides & PATH lookups | M2 | ANALISI_CODICE.md §DEF-16 |
| 17 | DEF-17 | Process command execution safety with system directory whitelist | M2 | ANALISI_CODICE.md §DEF-17 |
| 18 | DEF-18 | Standard CI workflow triggers for push to main (`.github/workflows/ci.yml`) | M2 | ANALISI_CODICE.md §DEF-18 |
| 19 | DEF-19 | Linter and formatter tooling configuration (ESLint, Prettier, package scripts) | M2 | ANALISI_CODICE.md §DEF-19 |
| 20 | DEF-20 | `App.test.tsx` DPI error alert timeout stabilization (`timeout: 3000`) | M2 | ANALISI_CODICE.md §DEF-20 |
| 21 | DEF-21 | Removal of dead `quick-xml` and unmaintained `fs2` dependency | M2 | ANALISI_CODICE.md §DEF-21 |
| 22 | DEF-22 | Removal of redundant spatializer template from `tauri.conf.json` resources | M2 | ANALISI_CODICE.md §DEF-22 |
| 23 | DEF-23 | Audio worker semaphore queuing with timeout instead of immediate drop | M2 | ANALISI_CODICE.md §DEF-23 |
| 24 | DEF-24 | Redundant snapshot readback suppression on connection error | M2 | ANALISI_CODICE.md §DEF-24 |
| 25 | DEF-25 | Imperative DOM querying replacement with React `useRef` | M2 | ANALISI_CODICE.md §DEF-25 |
| 26 | DEF-26 | Stable reconciliation keys on device and headset limitations | M2 | ANALISI_CODICE.md §DEF-26 |
| 27 | DEF-27 | Preservation of `AudioPanel` form state across focus navigation | M2 | ANALISI_CODICE.md §DEF-27 |
| 28 | DEF-28 | RFC 4122 v4 UUID fallback for `crypto.randomUUID` in legacy WebKitGTK | M2 | ANALISI_CODICE.md §DEF-28 |
| 29 | DEF-29 | Robust tray click state machine handling Linux StatusNotifierItem events | M2 | ANALISI_CODICE.md §DEF-29 |
| 30 | DEF-30 | WCAG 2.1 AA color contrast and font size accessibility in `styles.css` | M2 | ANALISI_CODICE.md §DEF-30 |
| 31 | DEF-AUDIO | Fix PipeWire port fixture direction property unwrap in `audio.rs:2014` | M2 | survey_explorer_3 report |
| 32 | F-VERIFY | Test suite verification: Vitest, TypeScript, Cargo test, Packaging test | M3 | ORIGINAL_REQUEST.md §R2 |
| 33 | F-PKG | Linux distribution package generation: deb, rpm, arch | M3 | ORIGINAL_REQUEST.md §R3 |

## Milestones
| # | Name | Scope | Dependencies | Status |
|---|------|-------|-------------|--------|
| M1 | Phase 1 & 2 Remediation | DEF-01 to DEF-12 (Critical & Performance/Concurrency) | none | DONE |
| M2 | Phase 3 & 4 Remediation | DEF-13 to DEF-30, DEF-AUDIO (Architecture, Tooling, UX) | M1 | DONE |
| M3 | Verification & Packaging | 100% test pass verification and deb, rpm, arch package builds | M2 | IN_PROGRESS |

## Interface Contracts
### Frontend ↔ Backend IPC
- `snapshot-changed`: Event emitted only on actual snapshot revision update or payload delta.
- `bundle.externalBin`: Helper binaries `peripheral-hub-remapping-agent` and `peripheral-hub-audio-worker` registered under Tauri `bundle.externalBin`.
- `api.ts`: Exposes synchronous `createHubApi()`, delegating to lazy-loaded `demoApi.ts` when `VITE_DEMO === "true"`, and `createTauriApi()` in production.

## Code Layout
- Frontend Source: `src/` (components, views, styles, tests)
- Backend Source: `src-tauri/` (lib.rs, main.rs, audio/, remapping/, providers/, tests/)
- Packaging Pipeline: `packaging/` (build.py, ci/matrix.py, tests/)
- Distribution Output: `packaging/out/quarantine/`
