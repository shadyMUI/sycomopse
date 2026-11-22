# BRIEFING — 2026-11-17T18:33:00Z

## Mission
Investigate and design technical remediations for Milestone 2 defects: §DEF-14, §DEF-15, §DEF-16, §DEF-17, and §DEF-AUDIO.

## 🔒 My Identity
- Archetype: explorer
- Roles: read-only investigation, code analysis, proposal design
- Working directory: /home/shadymui/sycomopse/.agents/teamwork/m2_explorer_1
- Original parent: 6d06fbf6-e50a-4864-b50c-2baa0ca37ccd
- Milestone: Milestone 2

## 🔒 Key Constraints
- Read-only investigation — do NOT implement
- Do NOT modify any source files directly
- Output structured findings and proposal to handoff.md
- All findings must have evidence chains citing exact paths and line numbers

## Current Parent
- Conversation ID: 6d06fbf6-e50a-4864-b50c-2baa0ca37ccd
- Updated: not yet

## Investigation State
- **Explored paths**:
  - `src-tauri/src/remapping.rs:1-2345`
  - `src-tauri/src/audio.rs:1-2264`
  - `src-tauri/src/process.rs:1-204`
  - `src-tauri/src/surround.rs:1-45, 130-160`
  - `src-tauri/src/audio_worker.rs:715-740`
  - `src-tauri/src/audio_lifecycle_tests.rs:1-30`
  - `src-tauri/tests/audio_registry.json`
  - `src-tauri/tests/audio_surround_graph.json`
  - `src-tauri/tests/remapping/recovery.rs`
  - `src-tauri/tests/check_audio.py`
  - `ANALISI_CODICE.md` (§DEF-14, §DEF-15, §DEF-16, §DEF-17)
  - `survey_explorer_3/handoff.md`
- **Key findings**:
  - §DEF-AUDIO: Test in `audio.rs:2014` panicked on `Option::unwrap()` because `props` lacks `port.direction`; in addition line 2025 asserted on node 42 instead of node 41 (which holds port 52). Remediated by extracting `raw` from `info.direction` or `props`, populating `props["port.direction"]` for property fallback testing, mapping to `"input"`/`"output"` for `info.direction`, and asserting against node 41.
  - §DEF-15: `quiesce_agent` in `remapping.rs:1683` used 100-second timeout. Reduced to 5 seconds, aligning with start/restart timeout in `remapping.rs:1191`.
  - §DEF-16: Hardcoded FHS paths `/usr/share/libmysofa/default.sofa` (`surround.rs`), `/usr/bin/wpexec` (`audio_worker.rs`), `/usr/bin/peripheral-hub-remapping-agent` (`remapping.rs`), and `/usr/lib/peripheral-hub/headsetcontrol` (`process.rs`) break on NixOS/Flatpak. Remediated via environment variable overrides (`PERIPHERAL_HUB_*`), current_exe relative resolution, and PATH lookups.
  - §DEF-17: `process.rs` invoked bare command names through `$PATH`. Remediated by introducing a trusted system directory whitelist (`/usr/bin`, `/bin`, `/usr/sbin`, `/sbin`, `/usr/local/bin`), canonical path resolution, and environment overrides.
  - §DEF-14: Monolithic `remapping.rs` (2344 LOC) and `audio.rs` (2263 LOC) modularized into submodules (`remapping/types.rs`, `store.rs`, `bridge.rs`, `controller.rs`, `agent.rs`; and `audio/parser.rs`, `registry.rs`, `graph.rs`, `controller.rs`, `monitor.rs`) while preserving module root re-exports to guarantee zero breakage for callers and tests.
- **Unexplored areas**: none within assigned scope.

## Key Decisions Made
- Confirmed reproducing test failure in Docker environment (`exit code 101` at `audio.rs:2014`).
- Designed exact zero-breakage modularization retaining module roots `audio.rs` and `remapping.rs` with re-exports.
- Synthesized exact before/after implementations for worker agent.

## Artifact Index
- /home/shadymui/sycomopse/.agents/teamwork/m2_explorer_1/DISPATCH.md — Dispatch log
- /home/shadymui/sycomopse/.agents/teamwork/m2_explorer_1/progress.md — Liveness heartbeat
- /home/shadymui/sycomopse/.agents/teamwork/m2_explorer_1/handoff.md — Final investigation report
