# ADR-0018: User-flow end-to-end suite as the regression gate

- **Status:** Accepted
- **Date:** 2026-11-24
- **Deciders:** product owner (decision), integrator (recorded)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §10, §19; `docs/06-dev-process.md` §6, §8; `docs/user-flows.md`

## Context

Cutweave is written almost entirely by AI agents, each working on one work package. Unit, property, golden and component-level e2e tests prove each piece in isolation, but nothing checked that the things a person does end to end still work after a merge. Examples: import clips, arrange and trim them, undo, save, quit, reopen, recover after a crash. Two problems showed this on 2026-11-24:

- Test teardown quit an app with unsaved changes and hung on the Save / Discard / Cancel dialog, leaving a window for the owner to click.
- A first fix bypassed that dialog with a test-only switch. The owner rejected it: tests must go through the dialog a user sees.

Manual checking of each part does not scale to an app written by agents in parallel.

## Decision drivers

- Catch broken user journeys right after the change that broke them, not at the milestone.
- Test what users see and do, not internal shortcuts that can drift from it.
- Never leave windows or prompts behind that need a person.
- Keep it runnable locally (the Actions-minutes policy keeps CI manual).

## Considered options

1. **A living user-flow suite, run on every merge that touches app code** (chosen).
2. **Component e2e per WP only** (status quo). Rejected: it misses breakage between components, which is where agent-built changes collide.
3. **Manual smoke test by the owner after merges.** Rejected: slow, inconsistent, and takes the owner's time.
4. **Only the M1 exit suite (WP-1O.04).** Rejected as the sole gate: it runs once per milestone. It becomes a subset of this suite instead.

## Decision

- **A user-flow suite** in `apps/desktop/e2e/flows/` drives the built app the way a person does:
  - the real engine binary and real files on disk;
  - real media (the corpus and the demo media);
  - real input: clicks, pointer drags, keyboard shortcuts and menus;
  - the app's own dialogs, answered through the UI.
- **Assertions** cover what the user sees (accessible roles and text, plus screenshots at key steps) and what the app leaves behind (the saved project on disk, and exported files checked with `ffprobe`).
- **Stubs only at the OS boundary the app does not own.** Native file pickers (`dialog.showOpenDialog` / `showSaveDialog`) return the path a user would pick, and `shell.openExternal` is recorded. Nothing inside the app is bypassed. Read-only test hooks that observe state (for example `windowDirty()`) are allowed; hooks that change app behaviour are not.
- **The flow catalog** lives in `docs/user-flows.md`. Each flow lists its steps, expected outcomes and coverage status (covered, or blocked by a named WP).
  - Every WP that adds or changes user-visible behaviour adds or extends a flow. This is a Definition of Done item.
  - A flow blocked by a missing capability names the WP that unblocks it, and that WP's acceptance includes un-blocking it.
- **Gate.** `just e2e-flows` runs the suite.
  - The integrator runs it after every merge that touches `apps/`, `packages/` or `crates/`.
  - `check-affected` runs it whenever the desktop app, the editor, the UI kit or the engine are affected.
  - The cloud `e2e` job runs it on manual dispatch.
- **Determinism and flakes.**
  - Flows wait for visible state, never for fixed sleeps.
  - A flaky flow is a bug to fix promptly, not to retry away.
- **Teardown quits like a user** (`quitApp` in `e2e/fixtures.ts`): it answers the unsaved-changes dialog, and treats an app still running after 20 s as a failure. It kills that app so nothing stays on screen. An auto fixture closes every app a test left open.
- **WP-1O.04** (M1 exit acceptance) becomes the milestone subset of this suite, plus its CI aggregation.

## Consequences

- **Positive:** regressions in real journeys surface at the merge that causes them. Agents get a concrete definition of "works for a user". The owner stops being the manual smoke test.
- **Negative:** the suite is slower than unit tests (minutes per run) and needs the built app, the engine binary and media. It must be kept deterministic, and flows grow with every user-visible WP.
- **Follow-ups:**
  - WP-1O.05 builds the suite and the catalog, and wires `just e2e-flows` into the gate.
  - Flows blocked today (real playback, export, real decode) are un-blocked by WP-1I.05, WP-1E.03, WP-1M.08 and WP-1D.03.
