// SPDX-License-Identifier: GPL-3.0-or-later
//! Benchmark results and budgets: the result format (`bench/result.schema.json`), the budget
//! rows (`bench/budgets.toml`) and the comparison that decides PASS or FAIL. One definition
//! for `cargo bench -p cutweave-bench`, `cargo xtask bench-check` (WP-0I.03) and the CI gate
//! (WP-2J.04).

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A baseline comparison fails when a metric is worse by more than this fraction.
pub const BASELINE_TOLERANCE: f64 = 0.10;

/// Comparison operator of a budget: the *measured value* must satisfy `value <op> budget`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Op {
    /// Less than.
    #[serde(rename = "<")]
    Lt,
    /// At most.
    #[serde(rename = "<=")]
    Le,
    /// At least.
    #[serde(rename = ">=")]
    Ge,
    /// More than.
    #[serde(rename = ">")]
    Gt,
}

impl Op {
    /// Whether `value <op> budget`.
    #[must_use]
    pub fn holds(self, value: f64, budget: f64) -> bool {
        match self {
            Op::Lt => value < budget,
            Op::Le => value <= budget,
            Op::Ge => value >= budget,
            Op::Gt => value > budget,
        }
    }

    /// Whether a smaller measured value is better.
    #[must_use]
    pub fn lower_is_better(self) -> bool {
        matches!(self, Op::Lt | Op::Le)
    }

    /// The operator as written in `budgets.toml`.
    #[must_use]
    pub fn symbol(self) -> &'static str {
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
pub struct Budget {
    /// The metric id results use.
    pub id: String,
    /// What is measured, for people.
    pub metric: String,
    /// The comparison.
    pub op: Op,
    /// The limit.
    pub value: f64,
    /// Unit of the limit; a result in another unit fails.
    pub unit: String,
    /// `["any"]` or the reference machines the row applies to.
    pub machine_scope: Vec<String>,
}

impl Budget {
    /// Whether the row applies to results from `machine`.
    #[must_use]
    pub fn applies_to(&self, machine: &str) -> bool {
        self.machine_scope
            .iter()
            .any(|s| s == "any" || s == machine)
    }
}

/// `bench/budgets.toml`.
#[derive(Debug, Deserialize)]
pub struct Budgets {
    /// The arch §19 rows.
    pub budget: Vec<Budget>,
    /// Limits a work package sets for a metric that is not a §19 row.
    #[serde(default)]
    pub wp: Vec<Budget>,
    /// The budget of the harness's own smoke metric.
    #[serde(default)]
    pub synthetic: Vec<Budget>,
}

impl Budgets {
    /// Every row.
    pub fn all(&self) -> impl Iterator<Item = &Budget> {
        self.budget.iter().chain(&self.wp).chain(&self.synthetic)
    }

    /// The row for metric `id`.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Budget> {
        self.all().find(|b| b.id == id)
    }

    /// Reads `<root>/bench/budgets.toml`.
    pub fn load(root: &Path) -> Result<Budgets, String> {
        let p = root.join("bench").join("budgets.toml");
        let text = fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        toml::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))
    }
}

/// Toolchain versions recorded with a result.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Toolchain {
    /// `rustc --version`.
    pub rustc: Option<String>,
    /// First line of `ffmpeg -version`, when FFmpeg is on the path.
    pub ffmpeg: Option<String>,
    /// The wgpu version, for GPU suites.
    pub wgpu: Option<String>,
    /// The Electron version, for UI suites.
    pub electron: Option<String>,
}

/// One measured metric.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metric {
    /// A budget id, or a name of its own for an informational metric.
    pub id: String,
    /// The value (for p95 budgets, the p95).
    pub value: f64,
    /// Its unit.
    pub unit: String,
    /// How many samples it is from.
    pub samples: u32,
}

impl Metric {
    /// A metric.
    #[must_use]
    pub fn new(id: &str, value: f64, unit: &str, samples: usize) -> Metric {
        Metric {
            id: id.to_owned(),
            value,
            unit: unit.to_owned(),
            samples: u32::try_from(samples).unwrap_or(u32::MAX).max(1),
        }
    }
}

/// A benchmark run (`bench/result.schema.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchResult {
    /// Reference machine id, or `local`.
    pub machine: String,
    /// The commit measured.
    pub commit: String,
    /// UTC date, `YYYY-MM-DD`.
    pub date: String,
    /// The suite that ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suite: Option<String>,
    /// Tool versions.
    pub toolchain: Toolchain,
    /// The metrics.
    pub metrics: Vec<Metric>,
}

/// Outcome of comparing a result with the budgets (and optionally a baseline).
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    /// One line per violated budget or regression.
    pub failures: Vec<String>,
    /// One line per metric that passed, was skipped or has no budget.
    pub notes: Vec<String>,
}

impl Report {
    /// True when nothing failed.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Compare `result` with `budgets`; metrics with no budget row are informational.
#[must_use]
pub fn check(budgets: &Budgets, result: &BenchResult, baseline: Option<&BenchResult>) -> Report {
    let mut r = Report::default();
    for m in &result.metrics {
        let Some(b) = budgets.get(&m.id) else {
            r.notes.push(format!("{}: no budget (informational)", m.id));
            continue;
        };
        if !b.applies_to(&result.machine) {
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

/// How one metric stands against its budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Inside the budget.
    Pass,
    /// Outside it (or in the wrong unit).
    Fail,
    /// The budget is for other machines.
    Skipped,
    /// No budget row.
    Info,
}

/// The verdict for `metric` measured on `machine`.
#[must_use]
pub fn verdict(budgets: &Budgets, machine: &str, metric: &Metric) -> Verdict {
    match budgets.get(&metric.id) {
        None => Verdict::Info,
        Some(b) if !b.applies_to(machine) => Verdict::Skipped,
        Some(b) if b.unit == metric.unit && b.op.holds(metric.value, b.value) => Verdict::Pass,
        Some(_) => Verdict::Fail,
    }
}

/// The table `cargo bench -p cutweave-bench` prints: every metric with its budget and PASS or
/// FAIL.
#[must_use]
pub fn table(budgets: &Budgets, result: &BenchResult) -> String {
    let mut out = format!(
        "{:<34} {:>12} {:<7} {:<16} {}\n",
        "metric", "value", "unit", "budget", "result"
    );
    for m in &result.metrics {
        let budget = budgets.get(&m.id).map_or_else(
            || "-".to_owned(),
            |b| format!("{} {} {}", b.op.symbol(), b.value, b.unit),
        );
        let word = match verdict(budgets, &result.machine, m) {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL",
            Verdict::Skipped => "skipped (other machines)",
            Verdict::Info => "info",
        };
        let _ = writeln!(
            out,
            "{:<34} {:>12.3} {:<7} {:<16} {}",
            m.id, m.value, m.unit, budget, word
        );
    }
    out
}

/// UTC `YYYY-MM-DD` for a unix timestamp (civil-from-days, no calendar dependency).
#[must_use]
pub fn iso_date(secs: u64) -> String {
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

/// First output line of `cmd args`, when it runs.
#[must_use]
pub fn tool_version(cmd: &str, args: &[&str]) -> Option<String> {
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

impl BenchResult {
    /// A result for this machine (`CUTWEAVE_BENCH_MACHINE`, default `local`), the checkout at
    /// `root` and today.
    #[must_use]
    pub fn now(root: &Path, suite: &str, metrics: Vec<Metric>) -> BenchResult {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        BenchResult {
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
            suite: Some(suite.to_owned()),
            toolchain: Toolchain {
                rustc: tool_version("rustc", &["--version"]),
                ffmpeg: tool_version("ffmpeg", &["-version"]),
                wgpu: None,
                electron: None,
            },
            metrics,
        }
    }

    /// Writes the result as pretty JSON to `path`.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(path, json + "\n").map_err(|e| format!("{}: {e}", path.display()))
    }

    /// `<root>/bench-results/{machine}/{date}.json`: where a run is kept for the CI gate
    /// (WP-2J.04). A later run on the same day replaces it.
    #[must_use]
    pub fn default_path(&self, root: &Path) -> PathBuf {
        root.join("bench-results")
            .join(&self.machine)
            .join(format!("{}.json", self.date))
    }
}
