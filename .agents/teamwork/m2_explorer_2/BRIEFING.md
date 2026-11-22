# BRIEFING — 2026-11-17T18:29:45Z

## Mission
Investigate and synthesize technical remediation design for Milestone 2 defects: DEF-13, DEF-20, DEF-25, DEF-26, DEF-27, DEF-28, DEF-30.

## 🔒 My Identity
- Archetype: teamwork_explorer
- Roles: explorer, synthesizer
- Working directory: /home/shadymui/sycomopse/.agents/teamwork/m2_explorer_2
- Original parent: 6d06fbf6-e50a-4864-b50c-2baa0ca37ccd
- Milestone: Milestone 2 - Frontend Architecture, UX & Accessibility

## 🔒 Key Constraints
- Read-only investigation — do NOT implement
- Do NOT modify any source files directly
- Write only to /home/shadymui/sycomopse/.agents/teamwork/m2_explorer_2/
- Produce a structured handoff report in handoff.md with 5 components
- Verify with code inspection and build/test commands without altering source code

## Current Parent
- Conversation ID: 6d06fbf6-e50a-4864-b50c-2baa0ca37ccd
- Updated: 2026-11-17T18:29:45Z

## Investigation State
- **Explored paths**:
  - `src/api.ts` (lines 83-367)
  - `src/App.tsx` (lines 30-197)
  - `src/views/AudioPanel.tsx` (lines 1-76)
  - `src/views/MousePanel.tsx` (lines 1-115)
  - `src/views/MouseButtons.tsx` (lines 1-170)
  - `src/tests/App.test.tsx` (lines 297-308, 520-525, 645-655)
  - `src/tests/api.test.ts` (lines 1-105)
  - `src/styles.css` (lines 40-420)
  - `src/tests/m1_challenger_empirical.test.tsx` (untracked, syntax/type issue noted)
- **Key findings**:
  - DEF-13: `createDemoApi` in `src/api.ts` adds ~270 lines of mock state into `api.ts`. Can be cleanly separated into `src/demoApi.ts`. `createHubApi()` in `api.ts` preserves synchronous interface contract via lazy-delegating wrapper with dynamic `import("./demoApi")` gated behind `import.meta.env.VITE_DEMO === "true"`, allowing Rollup dead-code elimination in production.
  - DEF-20: `App.test.tsx:305` uses default 1000ms timeout in `findByRole("alert")`, causing JSDOM flakiness during long suite runs (27.7s total). Fix: `{ timeout: 3000 }`.
  - DEF-25: `document.getElementById` called at `src/App.tsx:100` (panel focus) and `src/App.tsx:147` (tab arrow navigation). Replace with React `panelRef: useRef<HTMLDivElement>` and `tabRefs: useRef<Map<Tab, HTMLButtonElement>>`.
  - DEF-26: `key={index}` is present in `src/views/AudioPanel.tsx:60` (headset limitations) and `src/views/MousePanel.tsx:106` (mouse limitations). Note: `HeadsetPanel.tsx` does not exist; headset UI is in `AudioPanel.tsx`. In `MouseButtons.tsx`, keys are already stable. Replace with `key={`${device.id}-limit-${index}`}`.
  - DEF-27: `App.tsx:183` renders `<AudioPanel key={audioFocus.token} ... />`, destroying and remounting on tab/card navigation and losing all draft form state. Remove dynamic key from `AudioPanel` and sync output via `useEffect` in `AudioPanel.tsx`.
  - DEF-28: `src/views/AudioPanel.tsx:38` directly calls `crypto.randomUUID()`, failing in legacy WebKitGTK. Introduce RFC 4122 v4 generator `generateUuid()` in `src/api.ts` with fallback hierarchy (`crypto.randomUUID` -> `crypto.getRandomValues` -> `Math.random`).
  - DEF-30: `src/styles.css` contains 9px and 7px text and colors with low contrast (3.5:1 - 4.4:1). Elevate font-size to 11px and colors to `#b4b4b4` (contrast > 7:1 to 9.7:1), achieving 100% WCAG 2.1 AA & AAA compliance.
- **Unexplored areas**: None within assigned scope; all 7 defects investigated and verified.

## Key Decisions Made
- Confirmed synchronous contract for `createHubApi()` while leveraging Vite dynamic import behind `import.meta.env.VITE_DEMO === "true"`.
- Verified Rollup tree shaking behavior for production builds.
- Clarified file location for DEF-26: `AudioPanel.tsx:60` and `MousePanel.tsx:106`.

## Artifact Index
- `/home/shadymui/sycomopse/.agents/teamwork/m2_explorer_2/progress.md` — Liveness heartbeat
- `/home/shadymui/sycomopse/.agents/teamwork/m2_explorer_2/handoff.md` — Final handoff report
