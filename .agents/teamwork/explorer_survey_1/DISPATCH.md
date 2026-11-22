# Dispatch Assignment — explorer_survey_1

## 2026-11-17T13:05:59Z
You are teamwork_explorer assigned to survey and analyze the Frontend React/TypeScript codebase of sycomopse.
Your working directory is: /home/shadymui/sycomopse/.agents/teamwork/explorer_survey_1
Read the task assignment at: /home/shadymui/sycomopse/.agents/teamwork/explorer_survey_1/DISPATCH.md
Read the authoritative user request at: /home/shadymui/sycomopse/.agents/teamwork/ORIGINAL_REQUEST.md
Reference skill: /home/shadymui/.gemini/config/skills/code-review-and-quality/SKILL.md

HARD CONSTRAINTS:
- Do NOT modify any existing source code or configuration files in the repository.
- Do NOT rely solely on linter or automated output without direct manual inspection of the code.
- You are read-only. Inspect the actual code files directly.

TASKS:
1. Examine all files in `src/` (components, pages, hooks, state, types, styles, assets).
2. Identify specific defects, bugs, anti-patterns, typing issues (e.g. `any`, unhandled null/undefined), memory leaks, missing cleanup in `useEffect`, rendering performance issues, missing error boundaries, accessibility or UI issues.
3. For EVERY defect or improvement area identified, document:
   - Precise description of the issue
   - Severity level (Critica, Alta, Media, Bassa)
   - Exact code reference (file path and line number or line range)
   - Explanation of impact and recommended remediation
4. Evaluate the frontend across the 6 standard categories:
   - Architettura (Architecture)
   - Qualità del codice (Code Quality)
   - Sicurezza (Security)
   - Prestazioni (Performance)
   - Gestione errori (Error Handling)
   - Test (Testing)
   Assign a score from 1 to 10 for each category with detailed analytical justification.
5. Provide an overall summary and calculate the average score.
6. Write your comprehensive report to `/home/shadymui/sycomopse/.agents/teamwork/explorer_survey_1/handoff.md` and send a completion message with the path when done.
