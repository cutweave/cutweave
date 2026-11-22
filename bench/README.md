# bench/

Performance budgets (docs/01-architecture.md §19) and the benchmark harness (WP-0I.03).

- `budgets.toml`: one `[[budget]]` row per §19 line (`id`, `metric`, `op`, `value`, `unit`, `machine_scope`); exactly 10, asserted by a test. `[[synthetic]]` holds the budget of the smoke metric and is not a §19 row. `machine_scope` is `["any"]` or reference-machine ids (WP-0I.02); results from other machines skip that row.
- `result.schema.json`: the result format `{machine, commit, date, toolchain{rustc,ffmpeg,wgpu,electron}, metrics:[{id,value,unit,samples}]}`. Metric ids are budget ids. Rust suites and `scripts/bench/ui-bench.ts` (P2) emit the same shape.

```
cargo xtask bench --suite smoke --out results.json [--synthetic-value N]
cargo xtask bench-check results.json [--baseline previous.json]
just bench            # both, writing bench/results.json (git-ignored)
```

`bench-check` exits non-zero when a metric violates its budget or is more than 10% worse than the `--baseline` run. Set `CUTWEAVE_BENCH_MACHINE` to the reference-machine id when running on one (default `local`).

## Adding a metric

1. Add the budget row (or reuse the §19 id) in `budgets.toml`; keep §19 in sync if a §19 row changes.
2. Emit `{id, value, unit, samples}` from the suite in `xtask/src/bench.rs` (`run_suite`); for p95 budgets report the p95. The unit must match the budget's.
3. Add a comparator test if it uses a new operator or scope rule.
