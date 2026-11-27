// SPDX-License-Identifier: GPL-3.0-or-later
//! Benchmark harness (WP-0I.03): `bench` and `bench-check`.
//!
//! Budgets live in `bench/budgets.toml`, results follow `bench/result.schema.json`; the result
//! types and the budget comparison are `cutweave_bench::result`, shared with the engine suite
//! (`cargo bench -p cutweave-bench`, WP-2J.01). The only suite here is `smoke` (a synthetic
//! metric that exercises the harness end to end).

use cutweave_bench::result::{BenchResult, Budgets, Metric, check};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn read_json<T: for<'de> Deserialize<'de>>(p: &Path) -> Result<T, String> {
    let text = fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))
}

fn load_budgets(root: &Path) -> Result<Budgets, String> {
    Budgets::load(root)
}

/// Fixed hashing workload; its wall time in ms is the synthetic smoke metric.
fn smoke_workload_ms() -> f64 {
    let start = Instant::now();
    let mut h = Sha256::new();
    for i in 0..20_000u32 {
        h.update(i.to_le_bytes());
    }
    std::hint::black_box(h.finalize());
    start.elapsed().as_secs_f64() * 1000.0
}

fn run_suite(suite: &str, synthetic: Option<f64>) -> Result<Vec<Metric>, String> {
    match suite {
        "smoke" => Ok(vec![Metric {
            id: "smoke_synthetic_ms".into(),
            value: synthetic.unwrap_or_else(smoke_workload_ms),
            unit: "ms".into(),
            samples: 1,
        }]),
        other => Err(format!("unknown suite `{other}` (available: smoke)")),
    }
}

/// `cargo xtask bench --suite <name> --out <file> [--synthetic-value <n>]`.
pub fn bench(root: &Path, args: &[String]) -> bool {
    let run = || -> Result<(), String> {
        let (mut suite, mut out, mut synthetic) =
            ("smoke".to_owned(), PathBuf::from("results.json"), None);
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let mut val = || it.next().cloned().ok_or(format!("{a} needs a value"));
            match a.as_str() {
                "--suite" => suite = val()?,
                "--out" => out = PathBuf::from(val()?),
                "--synthetic-value" => {
                    synthetic = Some(val()?.parse::<f64>().map_err(|e| e.to_string())?);
                }
                other => return Err(format!("unknown option `{other}`")),
            }
        }
        let result = BenchResult::now(root, &suite, run_suite(&suite, synthetic)?);
        let json = serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?;
        fs::write(&out, json + "\n").map_err(|e| format!("{}: {e}", out.display()))?;
        println!(
            "bench: wrote {} metric(s) to {}",
            result.metrics.len(),
            out.display()
        );
        Ok(())
    };
    run().map_err(|e| eprintln!("bench: {e}")).is_ok()
}

/// `cargo xtask bench-check <results.json> [--baseline <results.json>]`: non-zero exit on any failure.
pub fn bench_check(root: &Path, args: &[String]) -> bool {
    let run = || -> Result<bool, String> {
        let (mut file, mut baseline) = (None, None);
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--baseline" => {
                    baseline = Some(PathBuf::from(it.next().ok_or("--baseline needs a value")?));
                }
                other if other.starts_with("--") => {
                    return Err(format!("unknown option `{other}`"));
                }
                other => file = Some(PathBuf::from(other)),
            }
        }
        let file = file.ok_or("usage: bench-check <results.json> [--baseline <results.json>]")?;
        let result: BenchResult = read_json(&file)?;
        let base: Option<BenchResult> = baseline.as_deref().map(read_json).transpose()?;
        let report = check(&load_budgets(root)?, &result, base.as_ref());
        for n in &report.notes {
            println!("{n}");
        }
        for f in &report.failures {
            eprintln!("FAIL {f}");
        }
        Ok(report.ok())
    };
    run().unwrap_or_else(|e| {
        eprintln!("bench-check: {e}");
        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutweave_bench::result::{Op, Toolchain, iso_date};
    use serde_json::Value;

    fn budgets() -> Budgets {
        load_budgets(&crate::workspace_root()).unwrap()
    }

    fn result(machine: &str, metrics: &[(&str, f64, &str)]) -> BenchResult {
        BenchResult {
            machine: machine.into(),
            commit: "abc".into(),
            date: "2026-01-01".into(),
            suite: None,
            toolchain: Toolchain::default(),
            metrics: metrics
                .iter()
                .map(|(id, v, u)| Metric {
                    id: (*id).into(),
                    value: *v,
                    unit: (*u).into(),
                    samples: 3,
                })
                .collect(),
        }
    }

    #[test]
    fn budgets_file_has_exactly_the_ten_section_19_rows() {
        let ids: Vec<String> = budgets().budget.into_iter().map(|b| b.id).collect();
        assert_eq!(
            ids,
            [
                "app_start_ms",
                "project_open_1h_ms",
                "command_roundtrip_p95_ms",
                "scrub_proxy_p95_ms",
                "scrub_4k_longgop_p95_ms",
                "playback_1080p60_dropped_frames",
                "timeline_panzoom_5000_fps",
                "av_sync_offset_ms",
                "export_1080p_h264_realtime_x",
                "idle_memory_1h_1080p_mb",
            ]
        );
    }

    #[test]
    fn operators() {
        assert!(Op::Lt.holds(1.0, 2.0) && !Op::Lt.holds(2.0, 2.0));
        assert!(Op::Le.holds(2.0, 2.0) && !Op::Le.holds(3.0, 2.0));
        assert!(Op::Ge.holds(2.0, 2.0) && !Op::Ge.holds(1.0, 2.0));
        assert!(Op::Gt.holds(3.0, 2.0) && !Op::Gt.holds(2.0, 2.0));
    }

    #[test]
    fn within_budget_passes_and_violations_fail() {
        let b = budgets();
        assert!(
            check(
                &b,
                &result("local", &[("scrub_proxy_p95_ms", 79.0, "ms")]),
                None
            )
            .ok()
        );
        assert_eq!(
            check(
                &b,
                &result("local", &[("scrub_proxy_p95_ms", 80.0, "ms")]),
                None
            )
            .failures
            .len(),
            1
        );
        // `>=` budget: lower is a violation.
        assert_eq!(
            check(
                &b,
                &result("local", &[("timeline_panzoom_5000_fps", 54.0, "fps")]),
                None
            )
            .failures
            .len(),
            1
        );
        assert!(
            check(
                &b,
                &result("local", &[("timeline_panzoom_5000_fps", 55.0, "fps")]),
                None
            )
            .ok()
        );
        // Wrong unit is a failure, unknown metric is informational.
        assert_eq!(
            check(
                &b,
                &result("local", &[("scrub_proxy_p95_ms", 1.0, "s")]),
                None
            )
            .failures
            .len(),
            1
        );
        assert!(check(&b, &result("local", &[("mystery", 1e9, "x")]), None).ok());
    }

    #[test]
    fn machine_scope_is_honoured() {
        let b = budgets();
        let drops = [("playback_1080p60_dropped_frames", 5.0, "frames")];
        assert!(check(&b, &result("ref-win-dgpu", &drops), None).ok());
        assert_eq!(
            check(&b, &result("ref-mac-m1", &drops), None)
                .failures
                .len(),
            1
        );
    }

    #[test]
    fn baseline_regression_over_ten_percent_fails() {
        let b = budgets();
        let base = result(
            "local",
            &[
                ("scrub_proxy_p95_ms", 50.0, "ms"),
                ("timeline_panzoom_5000_fps", 100.0, "fps"),
            ],
        );
        let ok = result(
            "local",
            &[
                ("scrub_proxy_p95_ms", 55.0, "ms"),
                ("timeline_panzoom_5000_fps", 91.0, "fps"),
            ],
        );
        assert!(check(&b, &ok, Some(&base)).ok());
        let bad = result(
            "local",
            &[
                ("scrub_proxy_p95_ms", 56.0, "ms"),
                ("timeline_panzoom_5000_fps", 89.0, "fps"),
            ],
        );
        assert_eq!(check(&b, &bad, Some(&base)).failures.len(), 2);
    }

    #[test]
    fn synthetic_row_gates_the_smoke_metric() {
        let b = budgets();
        assert!(
            check(
                &b,
                &result("local", &[("smoke_synthetic_ms", 10.0, "ms")]),
                None
            )
            .ok()
        );
        assert!(
            !check(
                &b,
                &result("local", &[("smoke_synthetic_ms", 9000.0, "ms")]),
                None
            )
            .ok()
        );
        assert!(run_suite("nope", None).is_err());
    }

    #[test]
    fn dates() {
        assert_eq!(iso_date(0), "1970-01-01");
        assert_eq!(iso_date(1_709_164_800), "2024-02-29");
    }

    /// Minimal JSON-schema subset (type, required, properties, additionalProperties, items, minimum):
    /// enough to validate harness output without a JSON-schema dependency.
    fn validate(schema: &Value, v: &Value, at: &str, errs: &mut Vec<String>) {
        if let Some(t) = schema.get("type") {
            let allowed: Vec<&str> = t.as_array().map_or_else(
                || vec![t.as_str().unwrap_or("")],
                |a| a.iter().filter_map(Value::as_str).collect(),
            );
            let ok = allowed.iter().any(|t| match *t {
                "object" => v.is_object(),
                "array" => v.is_array(),
                "string" => v.is_string(),
                "number" => v.is_number(),
                "integer" => v.is_i64() || v.is_u64(),
                "null" => v.is_null(),
                _ => false,
            });
            if !ok {
                errs.push(format!("{at}: expected {allowed:?}"));
                return;
            }
        }
        if let (Some(min), Some(n)) = (schema.get("minimum").and_then(Value::as_f64), v.as_f64())
            && n < min
        {
            errs.push(format!("{at}: below minimum"));
        }
        if let Some(obj) = v.as_object() {
            for r in schema
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !obj.contains_key(r) {
                    errs.push(format!("{at}: missing `{r}`"));
                }
            }
            let props = schema.get("properties").and_then(Value::as_object);
            for (k, child) in obj {
                match props.and_then(|p| p.get(k)) {
                    Some(s) => validate(s, child, &format!("{at}.{k}"), errs),
                    None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                        errs.push(format!("{at}: unexpected `{k}`"));
                    }
                    None => {}
                }
            }
        }
        if let (Some(items), Some(arr)) = (schema.get("items"), v.as_array()) {
            for (i, e) in arr.iter().enumerate() {
                validate(items, e, &format!("{at}[{i}]"), errs);
            }
        }
    }

    #[test]
    fn schema_validates_harness_output_and_rejects_bad_output() {
        let root = crate::workspace_root();
        let schema: Value = read_json(&root.join("bench/result.schema.json")).unwrap();
        let mut r = result("local", &[("smoke_synthetic_ms", 1.5, "ms")]);
        r.suite = Some("smoke".into());
        r.toolchain.rustc = Some("rustc 1.100.0".into());
        let mut errs = vec![];
        validate(&schema, &serde_json::to_value(&r).unwrap(), "$", &mut errs);
        assert!(errs.is_empty(), "{errs:?}");

        let mut bad = serde_json::to_value(&r).unwrap();
        bad.as_object_mut().unwrap().remove("commit");
        bad["metrics"][0]["samples"] = Value::from(0);
        bad["extra"] = Value::from(1);
        let mut errs = vec![];
        validate(&schema, &bad, "$", &mut errs);
        assert_eq!(errs.len(), 3, "{errs:?}");
    }

    #[test]
    fn smoke_end_to_end_pass_then_fail() {
        let root = crate::workspace_root();
        let out = std::env::temp_dir().join(format!("cutweave-bench-{}.json", std::process::id()));
        let o = |extra: &[&str]| -> Vec<String> {
            let mut v = vec![
                "--suite".to_owned(),
                "smoke".into(),
                "--out".into(),
                out.display().to_string(),
            ];
            v.extend(extra.iter().map(|s| (*s).to_owned()));
            v
        };
        let chk = vec![out.display().to_string()];
        assert!(bench(&root, &o(&["--synthetic-value", "10"])));
        assert!(bench_check(&root, &chk));
        assert!(bench(&root, &o(&["--synthetic-value", "6000"])));
        assert!(!bench_check(&root, &chk));
        let _ = fs::remove_file(out);
    }
}
