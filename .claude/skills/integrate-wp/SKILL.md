---
name: integrate-wp
description: Integrate a finished agent branch (work package or fix) into main as the Cutweave integrator — review, merge in a worktree, gate, push, close the issue, clean up. Use whenever an agent reports a WP or fix done.
---

# Integrate a finished work package

You are the integrator (see `CLAUDE.md`). An agent reported its branch done. Take it to `main`.

## 1. Review before merging
```bash
B=<agent branch>; BASE=<the base it was cut from>
git log --oneline $BASE..$B
git diff --stat $BASE...$B | tail -40
git diff --name-only $BASE...$B | grep -vE "<the WP's Area prefixes>"  # outside its Area?
git log --format=%B $BASE..$B | grep -ci co-authored                   # must be 0
git diff $BASE...$B | grep -c $'\r'                                    # must be 0 (LF only)
git diff --name-only $BASE...$B | grep -iE '\.(mp4|mov|mkv|wav|webm)$'  # no media
git diff --name-only $BASE...$B -- .github                             # workflows only if intended
```
Read the report's **deviations**. Decide each one: accept, send back, or split into a follow-up (file it with `file-issue`). Contract changes (command schemas, the project format, the MCP surface, WIT, the plugin manifest) merge only after their ADR is **Accepted**.

## 2. Merge in a worktree
Never use the main checkout while the owner's app runs from it, or while another gate runs there.
```bash
git worktree add -q .claude/worktrees/integrate-<x> -b integrate/<x> main
cd .claude/worktrees/integrate-<x>
git merge --no-ff --signoff -m "Merge <WP-id>: <what a user gets, one line>" $B
pnpm install --offline --frozen-lockfile   # a new worktree has no node_modules
```
- **`wip:` commits:** use `git merge --squash $B`, then one conventional `git commit -s`.
- **Conflicts:** use the `merge-conflicts` skill.
- **A branch that drags in unrelated commits:** cherry-pick (`-x`) the commits you need instead.

## 3. Decide what to gate
- **Branch base == current main, and the merge tree == the branch tree** (`git rev-parse HEAD^{tree}` vs `$B^{tree}`): the agent's gate already covers it. Push.
- **Main moved since the base:** compare the changed files.
  ```bash
  comm -12 <(git diff --name-only $BASE $B | sort) <(git diff --name-only $BASE main | sort)
  ```
  - **No overlap:** the agent's gate plus main's gate cover it. Run cheap checks (`just wbs-lint`, links), then push.
  - **Overlap:** gate the merged tree with the `gate-and-triage` skill (base = main).
- **Changes to `justfile`, `Cargo.toml`/`Cargo.lock` or `scripts/check-affected.py`:** these force the full gate.

## 4. Push and close
```bash
cd /e/projects/Vidai && git merge --ff-only -q integrate/<x> && git push origin main
gh issue close <n> --comment "<1–2 plain sentences: what users get> (main <sha>)."
```
- Push only after a green gate. Never force-push, never trigger CI.
- Keep comments short; the owner wants plain words, not test counts.

## 5. Clean up (Windows)
`git worktree remove` can hang for hours, so delete the folder directly:
```powershell
git worktree unlock <path> 2>$null; cmd /c "rd /s /q <path>"; git worktree prune
git branch -D integrate/<x> <agent branch> worktree-agent-<id>
```
Keep a branch that another running agent is based on. Delete it after that agent lands.

## 6. Record
Append one line to the integrator memory (`integrator-state.md`): what landed, the new main sha, anything left for the owner. Then tell the owner in 3–6 plain lines (Done / Now / Running).
