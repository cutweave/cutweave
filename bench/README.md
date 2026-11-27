# bench/

Performance budgets (docs/01-architecture.md §19) and the benchmark harness (WP-0I.03).

- `budgets.toml`: one `[[budget]]` row per §19 line (`id`, `metric`, `op`, `value`, `unit`, `machine_scope`); exactly 10, asserted by a test. `[[wp]]` rows are limits a work package sets for a metric of its own, and `[[synthetic]]` holds the budget of the smoke metric; neither is a §19 row. `machine_scope` is `["any"]` or reference-machine ids (WP-0I.02); results from other machines skip that row.
- `result.schema.json`: the result format `{machine, commit, date, toolchain{rustc,ffmpeg,wgpu,electron}, metrics:[{id,value,unit,samples}]}`. Metric ids are budget ids. Rust suites and `scripts/bench/ui-bench.ts` (P2) emit the same shape. The types and the comparison are `cutweave_bench::result`.

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

The rest (`command_rtt_in_process_*`, `doc_patched_fanout_*`, `placement_ripple_p5k_*`, …) is informational. The result goes to `bench-results/{machine}/{date}.json` (git-ignored; `cargo xtask bench-check <file>` judges it again, with `--baseline` against an earlier run) and the round-trip histogram to `{date}.command-rtt.json` next to it. The run itself always exits 0: the gate is `bench-check` (WP-2J.04).

`CUTWEAVE_BENCH_PROBES=0` runs criterion only; `CUTWEAVE_BENCH_SMOKE=1` runs everything on a tiny project. `cargo run --release -p cutweave-bench -- run [--smoke] [--with-media]` is the suite without criterion. The engine process opens the GPU, so on a shared machine run it through `python scripts/gpu-lock.py -- …`.

The same probes run at smoke scale as an ordinary test (`crates/cutweave-bench/tests/smoke.rs`, a few seconds, no GPU), so a change that breaks the suite fails `just check-affected`.

## Adding a metric

1. Add the budget row (or reuse the §19 id) in `budgets.toml`; keep §19 in sync if a §19 row changes.
2. Emit `{id, value, unit, samples}` from a probe in `crates/cutweave-bench/src/probes.rs` (`run_all`); for p95 budgets report the p95. The unit must match the budget's.
3. Add a comparator test if it uses a new operator or scope rule.
