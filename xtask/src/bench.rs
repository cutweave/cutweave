// SPDX-License-Identifier: GPL-3.0-or-later
//! Benchmark harness skeleton (WP-0I.03): result schema types, budget comparator, `bench` and `bench-check`.
//!
//! Budgets live in `bench/budgets.toml`, results follow `bench/result.schema.json`. The only suite today
//! is `smoke` (a synthetic metric); real suites arrive in P2 and add their metrics under budget ids.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// A baseline comparison fails when a metric is worse by more than this fraction.
const BASELINE_TOLERANCE: f64 = 0.10;

/// Comparison operator of a budget: the *measured value* must satisfy `value <op> budget`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Op {
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = "<=")]
    Le,
    #[serde(rename = ">=")]
    Ge,
    #[serde(rename = ">")]
    Gt,
}

impl Op {
    fn holds(self, value: f64, budget: f64) -> bool {
        match self {
            Op::Lt => value < budget,
            Op::Le => value <= budget,
            Op::Ge => value >= budget,
            Op::Gt => value > budget,
        }
    }
    fn lower_is_better(self) -> bool {
        matches!(self, Op::Lt | Op::Le)
    }
    fn symbol(self) -> &'static str {
        match self {
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Ge => ">=",
            Op::Gt => ">",
        }
    }
}

/// One budget row.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // `metric` is documentation for humans and the dashboards in 2J
pub struct Budget {
    pub id: String,
    pub metric: String,
    pub op: Op,
    pub value: f64,
    pub unit: String,
    pub machine_scope: Vec<String>,
}

/// `bench/budgets.toml`.
#[derive(Debug, Deserialize)]
pub struct Budgets {
    pub budget: Vec<Budget>,
    #[serde(default)]
    pub synthetic: Vec<Budget>,
}

impl Budgets {
    fn all(&self) -> impl Iterator<Item = &Budget> {
        self.budget.iter().chain(&self.synthetic)
    }
}

/// Toolchain versions recorded with a result.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Toolchain {
    pub rustc: Option<String>,
    pub ffmpeg: Option<String>,
    pub wgpu: Option<String>,
    pub electron: Option<String>,
}

/// One measured metric.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metric {
    pub id: String,
    pub value: f64,
    pub unit: String,
    pub samples: u32,
}

/// A benchmark run (`bench/result.schema.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchResult {
    pub machine: String,
    pub commit: String,
    pub date: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suite: Option<String>,
    pub toolchain: Toolchain,
    pub metrics: Vec<Metric>,
}

/// Outcome of comparing a result with the budgets (and optionally a baseline).
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub failures: Vec<String>,
    pub notes: Vec<String>,
}

impl Report {
    /// True when nothing failed.
    pub fn ok(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Compare `result` with `budgets`; metrics with no budget row are informational.
pub fn check(budgets: &Budgets, result: &BenchResult, baseline: Option<&BenchResult>) -> Report {
    let mut r = Report::default();
    for m in &result.metrics {
        let Some(b) = budgets.all().find(|b| b.id == m.id) else {
            r.notes.push(format!("{}: no budget (informational)", m.id));
            continue;
        };
        if b.machine_scope
            .iter()
            .all(|s| s != "any" && *s != result.machine)
        {
            r.notes.push(format!(
                "{}: skipped (budget scoped to {:?}, machine is {})",
                m.id, b.machine_scope, result.machine
            ));
            continue;
        }
        if m.unit != b.unit {
            r.failures.push(format!(
                "{}: unit `{}` but the budget is in `{}`",
                m.id, m.unit, b.unit
            ));
            continue;
        }
        if b.op.holds(m.value, b.value) {
            r.notes.push(format!(
                "{}: ok ({} {} {} {})",
                m.id,
                m.value,
                b.op.symbol(),
                b.value,
                b.unit
            ));
        } else {
            r.failures.push(format!(
                "{}: {} {} violates budget (need {} {} {})",
                m.id,
                m.value,
                b.unit,
                b.op.symbol(),
                b.value,
                b.unit
            ));
        }
        let base = baseline.and_then(|bl| bl.metrics.iter().find(|x| x.id == m.id));
        if let Some(base) = base {
            let worse = if b.op.lower_is_better() {
                m.value > base.value * (1.0 + BASELINE_TOLERANCE)
            } else {
                m.value < base.value * (1.0 - BASELINE_TOLERANCE)
            };
            if worse {
                r.failures.push(format!(
                    "{}: regressed more than 10% vs baseline ({} -> {} {})",
                    m.id, base.value, m.value, m.unit
                ));
            }
        }
    }
    r
}

fn read_json<T: for<'de> Deserialize<'de>>(p: &Path) -> Result<T, String> {
    let text = fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))
}

fn load_budgets(root: &Path) -> Result<Budgets, String> {
    let p = root.join("bench").join("budgets.toml");
    let text = fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    toml::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))
}

/// UTC `YYYY-MM-DD` for a unix timestamp (civil-from-days, no calendar dependency).
fn iso_date(secs: u64) -> String {
    let z = i64::try_from(secs / 86_400).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn tool_version(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_owned()
    })
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
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let result = BenchResult {
            machine: std::env::var("CUTWEAVE_BENCH_MACHINE").unwrap_or_else(|_| "local".into()),
            commit: Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(root)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map_or_else(
                    || "unknown".into(),
                    |o| String::from_utf8_lossy(&o.stdout).trim().to_owned(),
                ),
            date: iso_date(secs),
            toolchain: Toolchain {
                rustc: tool_version("rustc", &["--version"]),
                ffmpeg: tool_version("ffmpeg", &["-version"]),
                wgpu: None,
                electron: None,
            },
            metrics: run_suite(&suite, synthetic)?,
            suite: Some(suite),
        };
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
