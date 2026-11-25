---
name: file-issue
description: File or close Cutweave GitHub issues with the board's labels and a short plain-language body (work packages, bugs, follow-ups, decisions). Use every time you create or close an issue.
---

# File issues the board's way

Every issue gets labels **at creation**; the owner browses the board by label.

| Kind | Labels |
|---|---|
| Work package | `type:work-package,phase:P<n>,epic:<id>,size:<S\|M>,exec:<sonnet\|opus\|human>` |
| Bug | `bug,phase:P<n>,epic:<id>` |
| Feature or follow-up | `enhancement,phase:P<n>,epic:<id>` |
| Decision or question | `question` (+ phase and epic when they fit) |
| Owner-only tracking | `exec:human` |

**Epics.**
- P1:
  - 1A model, 1B command bus, 1C persistence/recovery, 1D media;
  - 1E export, 1F render, 1G audio, 1H playback/SHM;
  - 1I API server, 1J MCP, 1K CLI, 1L Electron shell/supervision;
  - 1M editor UI, 1N codegen, 1O quality gates.
- P2: see `docs/wbs/P2-core-editor.md`. 2A is the timeline, 2C keyframes/properties, 2E audio, 2F media/proxies/folders, 2G export, 2H the UI shell, and so on.

## Creating
```bash
gh issue create --title "WP-2H.11: History panel: every undo step, who made it, go back to any step" \
  --label "type:work-package,phase:P2,epic:2H,size:M,exec:sonnet" \
  --body "<2–4 plain sentences: what the user gets and why.>

WBS: docs/wbs/P2-core-editor.md (WP-2H.11). Blocked by <WP/issue> (if any)."
```
- **Bugs:** what happens, what should happen, where it was found, and the likely area. Keep it short.
- **Splits:** "Split from WP-x (#n)" plus the moved scope.
- **Never** put credentials, personal data or long logs in an issue.

## Closing
```bash
gh issue close <n> --comment "<what a user gets now, 1–2 sentences> (main <sha>)."
```
- A merge can auto-close an issue first ("already closed"). That's fine.
- Bugs fixed by a WP: close them with one line pointing at the fix.

## Checking
```bash
gh issue list --state open --limit 200 --json number,labels -q '.[] | select((.labels|length)==0) | .number'
```
It must print nothing. GitHub Projects (the board's status column) is not used. Don't propose board updates.
