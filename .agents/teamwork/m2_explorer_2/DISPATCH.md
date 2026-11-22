# Dispatch: m2_explorer_2

**Archetype**: teamwork_explorer
**Working Directory**: /home/shadymui/sycomopse/.agents/teamwork/m2_explorer_2
**Scope**: Milestone 2 - Frontend Architecture, UX & Accessibility (DEF-13, DEF-20, DEF-25, DEF-26, DEF-27, DEF-28, DEF-30)

## Key References
- `/home/shadymui/sycomopse/.agents/teamwork/ORIGINAL_REQUEST.md` (MANDATORY TO READ FIRST)
- `/home/shadymui/sycomopse/ANALISI_CODICE.md` (§DEF-13, §DEF-20, §DEF-25, §DEF-26, §DEF-27, §DEF-28, §DEF-30)
- `/home/shadymui/sycomopse/.agents/teamwork/orchestrator/PROJECT.md`
- `/home/shadymui/sycomopse/.agents/teamwork/m1_worker/handoff.md`

## 2026-11-17T18:23:21Z
You are m2_explorer_2, a read-only exploration agent for Milestone 2 of the Sycomopse defect remediation project.

Your assigned working directory is:
/home/shadymui/sycomopse/.agents/teamwork/m2_explorer_2

## MANDATORY INSTRUCTIONS:
1. First, read `/home/shadymui/sycomopse/.agents/teamwork/ORIGINAL_REQUEST.md`.
2. Read the defect specifications in `/home/shadymui/sycomopse/ANALISI_CODICE.md` for your assigned scope:
   - §DEF-13: Isolation of demo mock engine (`src/demoApi.ts`) from production bundle in `src/api.ts` (ensure dynamic/lazy loading or tree-shaking pattern so `npm run build` leaves production bundle clean of demo mocks unless `VITE_DEMO` is active)
   - §DEF-20: `App.test.tsx` DPI error alert timeout stabilization (`timeout: 3000` on waitFor)
   - §DEF-25: Imperative DOM querying replacement with React `useRef` in `src/App.tsx`
   - §DEF-26: Stable reconciliation keys on device and headset limitations (`src/views/HeadsetPanel.tsx`, `src/views/MouseButtons.tsx`)
   - §DEF-27: Preservation of `AudioPanel` form state across focus navigation (`src/views/AudioPanel.tsx`)
   - §DEF-28: RFC 4122 v4 UUID fallback for `crypto.randomUUID` in legacy WebKitGTK (`src/api.ts`)
   - §DEF-30: WCAG 2.1 AA color contrast and font size accessibility in `src/styles.css`
3. Also read:
   - `/home/shadymui/sycomopse/.agents/teamwork/orchestrator/PROJECT.md`
   - `/home/shadymui/sycomopse/.agents/teamwork/m1_worker/handoff.md`
4. Inspect the target source files in `src/` to verify current implementations and craft exact, non-breaking remediation designs.
5. DO NOT MODIFY ANY SOURCE FILES DIRECTLY. You are read-only.
6. Write your progress heartbeat to `/home/shadymui/sycomopse/.agents/teamwork/m2_explorer_2/progress.md`.
7. Write your detailed technical proposal and findings to `/home/shadymui/sycomopse/.agents/teamwork/m2_explorer_2/handoff.md` (Observation, Logic Chain, Proposed Code Changes with line numbers, Verification Plan, Caveats).
8. Send a message to orchestrator (`6d06fbf6-e50a-4864-b50c-2baa0ca37ccd`) when your handoff report is ready.
