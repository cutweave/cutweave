---
name: brief-agent
description: Write the brief for a Cutweave sub-agent that implements a work package or fix (WP id, Area, docs, rules, done criteria), and resume or redirect running agents. Use before every Agent launch for repo work.
---

# Brief a work-package agent

The brief is all the agent knows. Use this skeleton and fill every `<…>`.

```
Implement **WP-<id>: <title>** (GitHub issue #<n>) for Cutweave, an open-source desktop video
editor (Rust engine, Electron/React UI). <One line: why the owner wants it.>
You work in an isolated git worktree. Create branch `<wp/ID-slug | fix/N-slug>` from **<sha>**
(<"main" | "branch X, not yet on main, because it needs Y">).

**Read first:** `AGENTS.md`; `docs/wbs/P<n>-*.md` section WP-<id> (scope + acceptance
criteria — tests first); <arch §§, ADRs, spec files, research docs that matter>.

**Hand-off / what exists:** <APIs, files and notes from the WPs it builds on — paste the
previous agent's "notes for WP-x" here>.

**Area:** <paths>. Outside it only <explicitly allowed exceptions>. Larger than ~4 agent-days → stop
and propose a split.

**Rules:**
- Every mutation is a registered `#[command]` via `Tx`, with UI/MCP/CLI parity. Time is flicks.
  Address entities by ID.
- UI: `packages/ui` components and tokens only (`just lint-literals`); strings through i18n.
- Contracts change only via an ADR (draft it as **Proposed** and stop at its spike).
- `git commit -s`, conventional commits, NO AI attribution trailer; commit early (the machine has restarted before).
- No GitHub Actions (never trigger them, never edit workflows unless that is the task).
- Gate through the GPU lock: `python scripts/gpu-lock.py -- just check-affected --base <sha>`
  (only GPU steps wait). Run `pnpm install --offline --frozen-lockfile` in the worktree first.
  Add `just ui-snapshots` if `packages/ui` changes.
- User flows (ADR-0018): drive them like a person and wait for visible state with **bounded**
  waits (`toPass({ timeout })`, no endless retries). Never sleep. Quit with `quitApp`. When
  debugging, rerun the single test (`just e2e-flows -g "<name>"`), not the suite.
- Keep LF line endings. Timing flakes: rerun alone and report them; never weaken assertions.
- If a cached build artifact fails to load (after a crash), delete that crate's `target/` folder.
- Don't push.

**Done means:** the gate exits 0 <+ FFmpeg suite / spec-check / M1 trials if relevant>.
Report **briefly**: branch; a one-line-per-criterion checklist; deviations; notes for <next WP>.
```

## Choosing
- **Model:** `sonnet` by default. Use `opus` for `exec:opus` WPs and for design-heavy, concurrency, GPU or tricky-integration work.
- **Settings:** always `isolation: "worktree"` and `run_in_background: true`.
- **Capacity:** at most **3 GPU-heavy build agents at once**: work whose own testing is mostly app flows, rendering, media or the GPU. The owner allows more for unblocked work that is mostly CPU-only (model/ops logic, data, docs, pure TS), since its GPU steps queue on the lock anyway. Docs-only research agents don't count.
- **Base:** state the exact sha. If the work depends on an unmerged branch, cut from that branch and say so.

## Resuming and redirecting
- `SendMessage` to the agent id resumes it with its context. It's cheaper than a new agent for "merge main and regate" or "fix what your branch broke on main".
- When main moves under a running agent whose files overlap, tell it the new sha and what changed, and have it merge before its final gate.
- If an agent asks you to do something its sandbox denied, don't. Queue it for the owner.
