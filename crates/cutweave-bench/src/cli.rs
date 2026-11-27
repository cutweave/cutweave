// SPDX-License-Identifier: GPL-3.0-or-later
//! The commands behind `cargo xtask gen-bench-project` / `cutweave-bench gen` (write fixture
//! projects) and the end of a suite run (the PASS/FAIL table and the result files).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::BenchError;
use crate::fixture::{FixtureSpec, generate};
use crate::probes::{Outcome, Scale};
use crate::result::{BenchResult, Budgets, Verdict, table, verdict};

/// Usage of the generator.
pub const GEN_USAGE: &str = "usage: gen-bench-project [--preset p-400|p-5k|p-short|all]
       gen-bench-project --items N --tracks T --keyframes K [--name NAME] [--seed S]
                         [--duration SECS] [--audio-tracks A] [--main-items M]
                         [--size WxH] [--fps F]
  common: [--out DIR] [--with-media] [--check]

Writes seeded benchmark projects (default: all three presets) to DIR (default
testdata/bench-projects/, git-ignored), one folder each with the project and a
bench-fixture.json manifest. --with-media also makes the source files with FFmpeg (skipped
when there is none). --check generates each one twice, compares the document hashes and
writes nothing.";

/// Parsed generator arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenArgs {
    /// What to generate.
    pub specs: Vec<FixtureSpec>,
    /// The folder the project folders go in; `None`: the default.
    pub out: Option<PathBuf>,
    /// Make the source files too.
    pub with_media: bool,
    /// Check determinism instead of writing.
    pub check: bool,
}

fn number<T: std::str::FromStr>(flag: &str, value: &str) -> Result<T, BenchError> {
    value
        .parse()
        .map_err(|_| BenchError::Spec(format!("{flag}: `{value}` is not a number")))
}

impl GenArgs {
    /// Parses the arguments after the command name.
    pub fn parse(args: &[String]) -> Result<GenArgs, BenchError> {
        let mut out = GenArgs {
            specs: Vec::new(),
            out: None,
            with_media: false,
            check: false,
        };
        let mut custom = FixtureSpec::new("P-custom", 1, 0, 0, 0);
        let mut is_custom = false;
        let mut it = args.iter();
        while let Some(flag) = it.next() {
            let mut value = || {
                it.next()
                    .ok_or_else(|| BenchError::Spec(format!("{flag} needs a value")))
            };
            match flag.as_str() {
                "--with-media" => out.with_media = true,
                "--check" => out.check = true,
                "--out" => out.out = Some(PathBuf::from(value()?)),
                "--preset" => match value()?.as_str() {
                    "all" => out.specs.extend(presets()),
                    name => out.specs.push(
                        FixtureSpec::preset(name)
                            .ok_or_else(|| BenchError::Spec(format!("unknown preset `{name}`")))?,
                    ),
                },
                "--name" => custom.name.clone_from(value()?),
                "--size" => {
                    let v = value()?;
                    let (w, h) = v
                        .split_once('x')
                        .ok_or_else(|| BenchError::Spec("--size wants WxH".into()))?;
                    custom.width = number(flag, w)?;
                    custom.height = number(flag, h)?;
                }
                "--items" | "--tracks" | "--keyframes" | "--seed" | "--duration"
                | "--audio-tracks" | "--main-items" | "--fps" => {
                    let v = value()?;
                    is_custom |= matches!(flag.as_str(), "--items" | "--tracks" | "--keyframes");
                    match flag.as_str() {
                        "--items" => custom.items = number(flag, v)?,
                        "--tracks" => custom.tracks = number(flag, v)?,
                        "--keyframes" => custom.keyframes = number(flag, v)?,
                        "--seed" => custom.seed = number(flag, v)?,
                        "--duration" => custom.duration_secs = number(flag, v)?,
                        "--audio-tracks" => custom.audio_tracks = Some(number(flag, v)?),
                        "--main-items" => custom.main_items = Some(number(flag, v)?),
                        _ => custom.fps = number(flag, v)?,
                    }
                }
                other => return Err(BenchError::Spec(format!("unknown option `{other}`"))),
            }
        }
        if is_custom {
            out.specs.push(custom);
        }
        if out.specs.is_empty() {
            out.specs = presets();
        }
        Ok(out)
    }
}

/// The three presets of the WBS.
#[must_use]
pub fn presets() -> Vec<FixtureSpec> {
    vec![
        FixtureSpec::p_400(),
        FixtureSpec::p_5k(),
        FixtureSpec::p_short(),
    ]
}

/// Runs the generator in the checkout at `root` and returns what to print.
pub fn gen_bench_project(root: &Path, args: &[String]) -> Result<String, BenchError> {
    let args = GenArgs::parse(args)?;
    let out_dir = args
        .out
        .clone()
        .unwrap_or_else(|| root.join("testdata").join("bench-projects"));
    let mut report = String::new();
    for spec in &args.specs {
        let fixture = generate(spec)?;
        let summary = format!(
            "{}: {} items, {} tracks, {} keyframes, sha256 {}",
            spec.name,
            fixture.item_count(),
            fixture.manifest.tracks.len(),
            fixture.manifest.keyframes,
            fixture.sha256()
        );
        if args.check {
            let again = generate(spec)?;
            if again.sha256() != fixture.sha256() {
                return Err(BenchError::Other(format!(
                    "{}: not deterministic ({} then {})",
                    spec.name,
                    fixture.sha256(),
                    again.sha256()
                )));
            }
            let _ = writeln!(report, "{summary} (deterministic)");
            continue;
        }
        std::fs::create_dir_all(&out_dir).map_err(|e| BenchError::io("create", &out_dir, e))?;
        let folder = fixture.write(&out_dir)?;
        let _ = writeln!(report, "{summary}\n  -> {}", folder.display());
        if args.with_media {
            let made = crate::media::synthesize(&folder, &fixture.manifest)?;
            let _ = writeln!(
                report,
                "  media: {}",
                if made { "made" } else { "skipped (no ffmpeg)" }
            );
        }
    }
    Ok(report)
}

/// What the end of a suite run produced.
#[derive(Clone, Debug)]
pub struct Finished {
    /// The table: every metric with its budget and PASS or FAIL.
    pub table: String,
    /// The result file.
    pub result_path: PathBuf,
    /// The round-trip histogram file, next to it.
    pub histogram_path: PathBuf,
    /// Metrics outside their budget.
    pub failed: Vec<String>,
    /// Probes that could not run, with the reason.
    pub not_run: Vec<String>,
}

impl Finished {
    /// The table, where the files went and what is over budget or missing: what a run prints.
    #[must_use]
    pub fn report(&self) -> String {
        let mut out = format!(
            "{}
result:    {}
histogram: {}
",
            self.table,
            self.result_path.display(),
            self.histogram_path.display()
        );
        if !self.failed.is_empty() {
            let _ = writeln!(out, "over budget: {}", self.failed.join(", "));
        }
        for probe in &self.not_run {
            let _ = writeln!(out, "not measured: {probe}");
        }
        out
    }
}

/// Writes the result of a run (`out`, default `bench-results/{machine}/{date}.json`) and the
/// command round-trip histogram next to it, and renders the table.
pub fn finish(
    root: &Path,
    scale: &Scale,
    outcome: &Outcome,
    out: Option<&Path>,
) -> Result<Finished, BenchError> {
    let budgets = Budgets::load(root).map_err(BenchError::Other)?;
    let result = BenchResult::now(root, scale.suite, outcome.metrics.clone());
    let result_path = out.map_or_else(|| result.default_path(root), Path::to_path_buf);
    result.write(&result_path).map_err(BenchError::Other)?;
    let histogram_path = result_path.with_extension("command-rtt.json");
    let mut histogram =
        serde_json::to_string_pretty(&outcome.rtt).map_err(|e| BenchError::Other(e.to_string()))?;
    histogram.push('\n');
    std::fs::write(&histogram_path, histogram)
        .map_err(|e| BenchError::io("write", &histogram_path, e))?;
    let failed = result
        .metrics
        .iter()
        .filter(|m| verdict(&budgets, &result.machine, m) == Verdict::Fail)
        .map(|m| m.id.clone())
        .collect();
    Ok(Finished {
        table: table(&budgets, &result),
        result_path,
        histogram_path,
        failed,
        not_run: outcome.failed.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<GenArgs, BenchError> {
        let args: Vec<String> = s.split_whitespace().map(str::to_owned).collect();
        GenArgs::parse(&args)
    }

    #[test]
    fn no_arguments_mean_the_three_presets() {
        let args = parse("").unwrap();
        let names: Vec<&str> = args.specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["P-400", "P-5K", "P-short"]);
        assert!(!args.check && !args.with_media && args.out.is_none());
    }

    #[test]
    fn the_flags_of_the_wbs_make_a_custom_spec() {
        let args = parse(
            "--items 50 --tracks 4 --keyframes 100 --seed 7 --name P-x --size 1080x1920 \
             --duration 90 --audio-tracks 1 --main-items 10 --fps 60 --out o --check",
        )
        .unwrap();
        let spec = &args.specs[0];
        assert_eq!((spec.items, spec.tracks, spec.keyframes), (50, 4, 100));
        assert_eq!((spec.seed, spec.name.as_str()), (7, "P-x"));
        assert_eq!((spec.width, spec.height, spec.fps), (1080, 1920, 60));
        assert_eq!((spec.audio_tracks, spec.main_items), (Some(1), Some(10)));
        assert_eq!(spec.duration_secs, 90);
        assert!(args.check);
        assert_eq!(args.out, Some(PathBuf::from("o")));
    }

    #[test]
    fn presets_and_mistakes() {
        assert_eq!(parse("--preset p-5k").unwrap().specs, [FixtureSpec::p_5k()]);
        assert_eq!(parse("--preset all").unwrap().specs.len(), 3);
        assert!(parse("--preset nope").is_err());
        assert!(parse("--items").is_err());
        assert!(parse("--items x").is_err());
        assert!(parse("--frobnicate").is_err());
    }

    #[test]
    fn check_reports_the_hash_and_writes_nothing() {
        let args: Vec<String> = "--preset smoke --check --out /nonexistent/never"
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let report = gen_bench_project(Path::new("/nonexistent"), &args).unwrap();
        assert!(report.contains("P-smoke: 32 items, 4 tracks, 64 keyframes, sha256 "));
        assert!(report.contains("(deterministic)"));
    }
}
