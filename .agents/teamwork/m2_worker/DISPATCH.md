## 2026-11-17T18:34:40Z

You are m2_worker, the implementation and verification worker for Milestone 2 of the Sycomopse defect remediation project.

Your assigned working directory is:
/home/shadymui/sycomopse/.agents/teamwork/m2_worker

## MANDATORY INTEGRITY WARNING
DO NOT CHEAT. All implementations must be genuine. DO NOT hardcode test results, create dummy/facade implementations, or circumvent the intended task. A teamwork_auditor will independently verify your work. Integrity violations WILL be detected and your work WILL be rejected.

## MANDATORY INSTRUCTIONS:
1. First, read `/home/shadymui/sycomopse/.agents/teamwork/ORIGINAL_REQUEST.md`.
2. Read the technical remediation designs produced by the explorers:
   - `/home/shadymui/sycomopse/.agents/teamwork/m2_explorer_1/handoff.md` (Backend Architecture: DEF-14, DEF-15, DEF-16, DEF-17, DEF-AUDIO)
   - `/home/shadymui/sycomopse/.agents/teamwork/m2_explorer_2/handoff.md` (Frontend Architecture & UX: DEF-13, DEF-20, DEF-25, DEF-26, DEF-27, DEF-28, DEF-30)
   - `/home/shadymui/sycomopse/.agents/teamwork/m2_explorer_3/handoff.md` (Tooling, Dependencies & Infrastructure: DEF-18, DEF-19, DEF-21, DEF-22, DEF-23, DEF-24, DEF-29)
   - `/home/shadymui/sycomopse/.agents/teamwork/m1_worker/handoff.md` (Milestone 1 Baseline)
   - `/home/shadymui/sycomopse/ANALISI_CODICE.md` (§DEF-13 through §DEF-30)
   - `/home/shadymui/sycomopse/.agents/teamwork/orchestrator/PROJECT.md`

## SCOPE OF IMPLEMENTATION:
Implement all 18 cataloged defects for Milestone 2 plus DEF-AUDIO:
1. DEF-13: Isolation of demo mock engine into `src/demoApi.ts`, while maintaining synchronous `createHubApi()` in `src/api.ts` via dynamic lazy import gated on `import.meta.env.VITE_DEMO === "true"` so production bundle leaves no demo mock residue.
2. DEF-14: Backend modularization of `src-tauri/src/remapping.rs` and `src-tauri/src/audio.rs` into submodules (`src-tauri/src/remapping/` and `src-tauri/src/audio/`) while keeping root files re-exporting public symbols for 100% backward compatibility.
3. DEF-15: Quiesce agent systemctl timeout reduction to 5 seconds (`src-tauri/src/remapping.rs:1680-1686`).
4. DEF-16: Fragile FHS path replacement with environment overrides and system PATH lookup (`src-tauri/src/surround.rs`, `src-tauri/src/audio_worker.rs`, `src-tauri/src/process.rs`, `src-tauri/src/remapping.rs`).
5. DEF-17: Process command execution safety with system directory whitelist (`src-tauri/src/process.rs`).
6. DEF-18: Standard CI workflow `.github/workflows/ci.yml` running fast multi-job checks on push/PR to main.
7. DEF-19: Linter & formatter configs (`package.json` scripts/devDependencies, root `eslint.config.js`, `.prettierrc`, `.prettierignore`, `src-tauri/clippy.toml`, `src-tauri/rustfmt.toml`).
8. DEF-20: `App.test.tsx` DPI error alert timeout stabilization (`timeout: 3000` on waitFor).
9. DEF-21: Removal of dead `quick-xml` and unmaintained `fs2` in `src-tauri/Cargo.toml`, replaced with clean `FileLockExt` wrapping `libc::flock` in `src-tauri/src/remapping.rs`.
10. DEF-22: Removal of redundant spatializer template from `src-tauri/tauri.conf.json:38` (`"resources": []`).
11. DEF-23: Audio worker semaphore queuing with 500ms timeout in `src-tauri/src/audio_worker.rs:356-361`.
12. DEF-24: Redundant snapshot readback suppression on connection error in `src/App.tsx:118-126`.
13. DEF-25: Imperative DOM querying replacement with React `useRef` (`panelRef`, `tabRefs`) in `src/App.tsx`.
14. DEF-26: Stable reconciliation keys on device and headset limitations (`src/views/HeadsetPanel.tsx`, `src/views/MousePanel.tsx`).
15. DEF-27: Preservation of `AudioPanel` draft form state across focus navigation (`src/views/AudioPanel.tsx`, `src/App.tsx`).
16. DEF-28: RFC 4122 v4 UUID fallback for `crypto.randomUUID` in legacy WebKitGTK (`src/api.ts`).
17. DEF-29: Robust tray click state machine handling Linux StatusNotifierItem events in `src-tauri/src/lib.rs:866-902`.
18. DEF-30: WCAG 2.1 AA color contrast and font size accessibility in `src/styles.css`.
19. DEF-AUDIO: Fix PipeWire port fixture direction property unwrap and assertion node 41 in `src-tauri/src/audio.rs:2010-2033`.
20. Note on test cleanup: Check `src/tests/m1_challenger_empirical.test.tsx` — if present and causing `tsc --noEmit` errors, clean up or align types so `tsc --noEmit` passes 100% cleanly.
