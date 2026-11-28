# bench/

Performance budgets (docs/01-architecture.md §19) and the benchmark harness (WP-0I.03).

- `budgets.toml`: one `[[budget]]` row per §19 line (`id`, `metric`, `op`, `value`, `unit`, `machine_scope`); exactly 10, asserted by a test. `[[wp]]` rows are limits a work package sets for a metric of its own, and `[[synthetic]]` holds the budget of the smoke metric; neither is a §19 row. `machine_scope` is `["any"]` or reference-machine ids (WP-0I.02); results from other machines skip that row.
- `result.schema.json`: the result format `{machine, commit, date, toolchain{rustc,ffmpeg,wgpu,electron}, metrics:[{id,value,unit,samples}]}`. Metric ids are budget ids. The Rust suites and the UI suite (below) emit the same shape; `details` is optional and says where the numbers come from (CPU, GPU, memory, how the run was set up). The types and the comparison are `cutweave_bench::result`.

```
cargo xtask bench --suite smoke --out results.json [--synthetic-value N]
cargo xtask bench-check results.json [--baseline previous.json]
just bench            # both, writing bench/results.json (git-ignored)
```

`bench-check` exits non-zero when a metric violates its budget or is more than 10% worse than the `--baseline` run. Set `CUTWEAVE_BENCH_MACHINE` to the reference-machine id when running on one (default `local`).

## Benchmark projects (WP-2J.01)

Generated on each machine from a seed, never committed:

```
cargo xtask gen-bench-project                 # P-400, P-5K and P-short into testdata/bench-projects/
cargo xtask gen-bench-project --check         # generate each twice, compare the hashes, write nothing
cargo xtask gen-bench-project --items 50 --tracks 4 --keyframes 100 --name P-x [--seed 7]
cargo xtask gen-bench-project --with-media    # also make the source files (needs FFmpeg)
```

| Project | What it is |
|---|---|
| `P-400` | 1 hour, 400 items on 8 tracks (100 back to back on the magnetic main track), 800 keyframes, 1080p30 |
| `P-5K` | 5,000 items and 50,000 keyframes on 10 tracks over 2 hours; 2,000 items on the main track |
| `P-short` | 30 s, 1080×1920, 3 video layers, 12 items, 96 keyframes |

Each is built by running the registered commands with seeded ids, so the same spec gives the same document bytes (`sha256` in the output) and the projects stay valid as the model changes. A folder holds the project, `media/` (six sources: four 120 s 1080p test patterns and two tones; empty until `--with-media`) and `bench-fixture.json`: the spec, the hash, the sequence, track, item and media ids, and each item's place. The UI benchmarks (WP-2J.02) and the playback benchmarks (WP-2J.03) open these folders and read that file. `cargo run --release -p cutweave-bench -- gen …` is the same command in a release build (P-5K takes about 10 s instead of a minute).

## Engine suite (WP-2J.01)

```
cargo bench -p cutweave-bench -- --save-baseline p2
. scripts/ffmpeg/env.sh && cargo bench -p cutweave-bench --features ffmpeg -- --save-baseline p2   # idle memory with real media
```

It runs the criterion benches (project open, command round trip in process and over the WebSocket, `doc.patched` fan-out to 1, 8 and 32 subscribers, a ripple at the start of P-5K's main track, `timeline.get`), then measures each budget once more the way the budget is worded and prints every metric with its budget and PASS or FAIL:

| Metric | How it is measured |
|---|---|
| `project_open_1h_ms` | `project.open` of P-400 plus reading its whole timeline, median of 10 |
| `command_roundtrip_p95_ms` | 10,000 mixed commands (property changes, keyframes, moves, mutes, markers) on P-5K over the WebSocket: request, response and the `doc.patched` it causes. They take turns with 10,000 in-process calls (`command_rtt_in_process_*`), so both see the same growing document |
| `timeline_get_p400_ms` | `timeline.get` at depth 1 on P-400, p95 of 200 |
| `app_start_engine_ms` | a real engine process (`cutweave-bench engine`, the launch protocol, started in `Gui` mode with the handshake as the app does: GPU and default audio output) from spawn to `project.new` answered, median of 3 (the slowest, usually the first, is `app_start_engine_max_ms`); Electron's share of `app_start_ms` is WP-2J.02's |
| `idle_memory_1h_1080p_mb` | resident memory of such an engine process with P-400 open after 60 s idle. Reported under this id only with `--features ffmpeg` and the source files made; otherwise as `idle_memory_no_media_mb`, without a budget |

The rest (`command_rtt_in_process_*`, `doc_patched_fanout_*`, `placement_ripple_p5k_*`, …) is informational. The result goes to `bench-results/{machine}/{date}.json` (git-ignored; `cargo xtask bench-check <file>` judges it again, with `--baseline` against an earlier run) and the round-trip histogram to `{date}.command-rtt.json` next to it. `cargo bench` exits 0 whatever the verdicts: the gate is `bench-check` (WP-2J.04).

`CUTWEAVE_BENCH_PROBES=0` runs criterion only; `CUTWEAVE_BENCH_SMOKE=1` runs everything on a tiny project. `cargo run --release -p cutweave-bench -- run [--smoke] [--with-media]` is the suite without criterion; it exits 1 when a metric is over budget or a probe could not run. The engine process opens the GPU, so on a shared machine run it through `python scripts/gpu-lock.py -- …`.

The same probes run at smoke scale as an ordinary test (`crates/cutweave-bench/tests/smoke.rs`, a few seconds, no GPU), so a change that breaks the suite fails `just check-affected`.

## UI suite (WP-2J.02)

```
. scripts/ffmpeg/env.sh                       # optional: real thumbnails and waveforms on the timeline
pnpm --filter @cutweave/desktop perf:ui       # build, generate, measure, print the table, exit 1 over budget
pnpm --filter @cutweave/desktop perf:ui --no-build --slow 20  # must FAIL: the budgets catch a slow editor
```

It drives the built app with the release engine on copies of P-400 and P-5K, the way the user flows do (real clicks, the wheel, a drag; only the OS file picker is stubbed), and measures from outside: `performance` in the page, the DevTools protocol and the diagnostics the app already has. No product code knows about it. The app runs on the hidden desktop (`scripts/hidden-desktop.py`) and under the machine's GPU lock (`scripts/gpu-lock.py`, which also lowers the run's priority; `--no-lock` on a machine that runs nothing else). The code is `apps/desktop/e2e/perf/` (`suite.ts` the probes, `result.ts` the numbers), started by `apps/desktop/scripts/perf-ui.ts`.

| Metric | How it is measured |
|---|---|
| `app_start_ms` | the app process's creation to the frame in which the editor shows "Engine connected" on an untitled project, on a fresh profile. `app_start_warm_ms` is the median of 3 more starts on that profile; `app_first_paint_ms` the window's first contentful paint, when the page reports one |
| `project_open_1h_ui_ms` | P-400: the click on File > Open project… to the second animation frame after the editor's document holds every item (the frame after the timeline drew them), median of 3 opens with P-short opened in between (`…_first_ms`, `…_max_ms`). `project_open_p5k_ui_ms` is one open of P-5K |
| `ui_command_rtt_p95_ms` | P-5K: a clip on a free video track is dragged along its own track (inside the stretch it has to itself, clear of the lanes' edges so the timeline does not scroll under the pointer; the run fails if the clip ends on another track) at a hand's pace (the flows' `handPacedDrag`: a move every 16 ms whether or not the app keeps up) in drags of 5 s until 500 updates came back (six drags at most, so a slow app yields fewer samples; `samples` says how many); each is the app's own round trip (User Timing `cutweave.timeline.rtt`: UI, engine, patch, UI). With it: `ui_command_answer_p50_ms` and `…_p95_ms` (the engine's and the socket's share: from the WebSocket frame that carries an update to the frame that carries its response, as the DevTools protocol reports them; the rest of a round trip is the patch arriving and the editor applying it), `ui_drag_received_kb_per_update` (what the socket received per update: the responses and the events, `doc.patched` above all), `ui_drag_fps` and `ui_drag_frame_p95_ms` (the page's frame intervals during the drags) and `ui_drag_script_ms_per_update`, `ui_drag_layout_ms_per_update` (main-thread script, and style plus layout, per update, from Chromium's counters: the whole window's cost of following a drag, Inspector and bin included) |
| `timeline_panzoom_5000_fps` | P-5K: 20 s of wheel pan (out and back at up to 3,000 px a second, half of it zoomed out to a few hundred clips on screen) and 10 s of Ctrl+wheel zoom in and out, the events sent at a steady pace; frames divided by time, from an animation-frame loop in the page. `timeline_panzoom_5000_frame_p95_ms` is the p95 frame time; `timeline_pan_*` and `timeline_zoom_*` are the two parts; `ui_idle_fps` is the page's rate with nothing happening, the ceiling of the others |
| `app_memory_1h_mb`, `app_memory_p5k_mb` | working sets of every app process plus the engine, right after the project opened (`engine_memory_*_mb` is the engine's share, `editor_js_heap_p5k_mb` the editor's JavaScript heap) |
| `timeline.*` | the timeline renderer alone on P-5K's model with every keyframe lane open: `packages/timeline/bench` in headless Chromium, no app and no engine. A slow frame in the app with a fast one here is the app's, not the renderer's |

`app_start_ms` and `timeline_panzoom_5000_fps` are §19 rows; `project_open_1h_ui_ms`, `ui_command_rtt_p95_ms` and `timeline_panzoom_5000_frame_p95_ms` have `[[wp]]` rows; the rest is informational. The result goes to `bench-results/{machine}/{date}.ui.json` (next to the engine suite's `{date}.json`; `cargo xtask bench-check <file> [--baseline <file>]` judges it again), with the CPU, GPU, memory, window size, engine binary and media mode in `details`. `--slow N` slows the editor's main thread N times while the timeline is panned and zoomed (Chromium's CPU throttling) and writes `{date}.ui-slowed.json`: the stand-in for a slowed build, which must fail the frame budgets.

Not measured, because nothing outside the app can see it: what one panel (the Inspector, the bin) costs during a drag by itself, and the moment the canvas has drawn (the open time ends on the frame after the document arrived).

Every probe runs once on the tiny P-smoke project, with no thresholds, as `apps/desktop/e2e/flows/perf-smoke.spec.ts` in the user-flow suite (`just e2e-flows`), and the arithmetic is unit-tested (`apps/desktop/e2e/perf/result.test.ts`), so a change that breaks the suite fails the gate.

## Crash harness (WP-2J.05)

```
cargo run -p cutweave-bench -- crash --count 20 --seed 7      # 20 forced crashes, about 20 s
cargo run -p cutweave-bench -- crash --self-test              # the harness must catch a lost edit
cargo run -p cutweave-bench -- crash --points save,torn-tail --count 6 --seed 7
```

It starts a real engine process on a seeded project (a debug build, so that the engine's `CUTWEAVE_TEST_CRASH_AFTER` works) and makes it work as the app and an agent do at once: the editor's commands pipelined over `/rpc` (trims, ripple trims, splits, speed, audio, moves, keyframes, markers, sequences, imports, checkpoints, undo and redo, drags, saves) and an agent's tool calls over `/mcp`. Then it kills the engine, starts it again on the project, lets it recover and checks the result. A second engine, the reference, is never killed and gets exactly the commands the first one acknowledged, in the order of their revisions.

| Point | The engine dies |
|---|---|
| `edit` | with pipelined editor commands and an agent call in flight |
| `commit-abort` | by itself right after its Nth commit, before the reply leaves (`CUTWEAVE_TEST_CRASH_AFTER`) |
| `save` | while `project.save` runs, with edits around it |
| `autosave` | when autosave is due, 2 s after the last edit |
| `drag` | with a gesture open and live updates in it |
| `drag-commit` | with the drag's commit in flight |
| `torn-tail` | after an acknowledged edit; the journal's last record is then cut short, as a torn write |
| `recover` | with edits in flight, and again while `project.recover` replays the journal |
| `import` | while `media.import` registers a file |
| `queue` | after the export queue's pause changed |
| `export`, `proxy` | `--with-media` only: while a queued export is encoded, while a proxy is made |

A run takes every point once in a seeded order, then again. After each crash: the project opens writable (the dead engine's lock is taken over) and recovers; the recovered document is at least at the last acknowledged revision; the commands that were in flight are sent again under their request ids, and the document then equals the reference's (compared by structure, ids aside), so nothing acknowledged is lost and nothing is half applied; the checkpoints and the export queue's pause are the acknowledged ones; the document on disk is a readable project file and a save leaves no temporary file. A run ends with a save, a kill and a start that must find nothing to recover. It prints a table per point and per crash and exits 1 on the first failure, with the command to repeat it. The seed fixes the work, the crash points and the delay before each kill; where a kill lands inside the engine is up to the operating system.

`--with-media` runs the full engine (build with `--features ffmpeg`, FFmpeg bundle, GPU: use `python scripts/gpu-lock.py -- …`) on a project whose source files are made first, and adds the two points that need real jobs. After `export` both queued exports must still be queued in order and finish with their files and no partial file; after `proxy` the proxy's state must be sane and making it again must finish.

One crash at every point, and the self-test, run as ordinary tests (`crates/cutweave-bench/tests/crash.rs`, about 10 s, no GPU), so a change that breaks recovery or the harness fails `just check-affected`. Not covered here: GPU device loss (the engine has no way to inject it), recording (not built yet), power loss, and the Electron side (`apps/desktop/e2e/m1/m1-crash.spec.ts` kills the engine and the app under the UI).

## Agent evaluation (WP-2I.09)

```
ANTHROPIC_API_KEY=... cargo run -p cutweave-bench -- agent-eval     # 57 sessions with a model; costs money
cargo run -p cutweave-bench -- agent-eval --dry-run                  # the first request, nothing sent
cargo run -p cutweave-bench -- agent-eval --replay crates/cutweave-bench/tests/fixtures/agent-eval/split-and-trim.json
```

It gives a model ten editing tasks over MCP on fresh headless engines (stub media, no GPU), records every tool call, checks the final document with a deterministic checker per task and writes a dated result and report to `docs/reference/agent-eval/`. One run compares the standard tool list with the described one, a core tool with the same command through `cutweave_execute`, and one batch with several calls. It needs a key and a model, so it is never part of the gate; without `ANTHROPIC_API_KEY` it prints how to run it and exits 0. Everything else is in [docs/reference/agent-eval.md](../docs/reference/agent-eval.md).

Ten sessions written by hand, one per task, are replayed on real engines with no model and no network as an ordinary test (`crates/cutweave-bench/tests/agent_eval.rs`, about 10 s), so a change that breaks the runner, a checker or a tool the tasks use fails `just check-affected`.

## Adding a metric

1. Add the budget row (or reuse the §19 id) in `budgets.toml`; keep §19 in sync if a §19 row changes.
2. Emit `{id, value, unit, samples}` from a probe in `crates/cutweave-bench/src/probes.rs` (`run_all`); for p95 budgets report the p95. The unit must match the budget's.
3. Add a comparator test if it uses a new operator or scope rule.
