---
name: gate-and-triage
description: Run Cutweave's local gates through the GPU lock (agents and integrator in parallel) and triage failures — load flakes vs real bugs — before any push. Use whenever you gate a branch or a merge, or a gate fails.
---

# Gate locally and triage failures

CI does not run on pushes (Actions-minutes policy). The local gate is the merge gate.

## Run
| What | Command |
|---|---|
| A branch (agent) | `python scripts/gpu-lock.py -- just check-affected --base <sha>` |
| A merge (integrator, keeps going past failures, plus the FFmpeg suites) | `python scripts/gpu-lock.py -- bash scripts/gate-integrate.sh <base> <checkout dir>` |
| The FFmpeg engine suite | `python scripts/gpu-lock.py -- bash -c ". scripts/ffmpeg/env.sh && cargo nextest run -p cutweave-engine --features ffmpeg"` |
| UI kit changes (`check-affected` doesn't pick this up) | `python scripts/gpu-lock.py -- just ui-snapshots` |
| Crash or recovery changes | `python scripts/gpu-lock.py -- just m1-gate --trials 4 --criteria 3` |

- **The lock:** only GPU steps (flows, golden, `just check`, tests of the render/engine/media/playback/bridge/cli crates) wait, and they are built outside the lock first. `<lock>.holder` names the holder.
- **New worktree:** run `pnpm install --offline --frozen-lockfile` first.
- **Long runs:** use `run_in_background` and wait for the notification. After `TaskStop`, also kill the python waiter it started (it survives the bash parent).
- **`just check` stops at its first failing line:** after fixing that line, run the remaining lines yourself instead of the whole gate again.

## Read the result
- **`check-affected`:** `step failed, continuing: …`, `FAILED steps`, `check-affected: ok`, and `GATE FAILED: none` (from gate-integrate).
- **Playwright:** lines starting `  x ` and numbered `N) …` with `Error:`. `test-results/**/error-context.md` holds a page snapshot when one exists; a rerun overwrites it.
- **nextest:** `FAIL [`, `TRY n FAIL`, `FLAKY`, and the panic line (`threads leaked`, `Timeout`, …).

## Triage
1. **Rerun the failing test alone, twice, through the lock.**
   - Playwright: `just e2e-flows -g "<test title words>"`
   - nextest: `-E "test(<name>)"`
2. **It passes alone:** it's a load flake. Note it in memory. If it recurs across gates, harden it (best-of-N timing, bounded waits, or fixing the real race) in a follow-up. **Never** loosen an assertion to make a gate pass.
3. **It fails alone, or always fails after a merge:** it's a real bug. Send it back to the agent that owns the code (`SendMessage`, with the failing step and its context), or fix it in the integration worktree if it's small.
4. **A test that hangs for minutes:** an unbounded wait. Tell the owner, stop it, and have the test bounded.

## Known load-sensitive tests (2026-11-25)
- `full_engine_ffmpeg … shut_down_cleanly` (thread count): it now excludes the Windows thread pool.
- `find.perf.test.ts` (8 ms p95), `menus/host.test.ts` (50 ms p95), `workspace.test.ts` timeouts.
- `full_engine an_audio_only_media_item_plays_its_sound` (60 s), launch `…exits_11/12`, playback `shm` timing tests.
- Flows F21, F29, F31, F41 under heavy load.

## Machine notes (this Windows laptop)
- **Cargo:** `~/.cargo/config.toml` caps builds at `jobs = 4`, links with `rust-lld`, and uses `sccache`. Dev builds keep line tables only.
- **Temp files:** on Windows `gpu-lock.py` puts the temp dir of everything it runs at `<checkout>/.tmp`, inside the Defender exclusion (`scripts/repo_temp.py`). Running `cargo`, `pnpm` or `just` directly still uses the system temp dir.
- **Priority:** after a session restart, set the session's `claude.exe` to BelowNormal so builds inherit it.
- **Unit tests:** use `python scripts/run-vitest.py`, never a bare `pnpm exec vitest` in `packages/ui` (it picks up the Playwright specs).
