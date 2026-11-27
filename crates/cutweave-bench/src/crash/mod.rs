// SPDX-License-Identifier: GPL-3.0-or-later
//! The crash-injection harness (WP-2J.05): `cutweave-bench crash --count 20 --seed N`.
//!
//! A real engine process (the launch protocol, `--deterministic-ids`) opens a seeded project
//! and is put to work the way the app and an agent work at once: the editor's commands over
//! `/rpc`, pipelined, and an agent's tool calls over `/mcp` ([`script`]). At a seeded point the
//! engine is killed (`TerminateProcess` / `SIGKILL`; one point uses the engine's own
//! `CUTWEAVE_TEST_CRASH_AFTER`), restarted on the project, and its recovery is checked
//! ([`Point`] lists the crash points and what each one aims at).
//!
//! # What is checked after every crash
//!
//! A second engine, the **reference**, is never killed. It is given exactly the commands the
//! engine under test acknowledged, in the order of their revisions; a command that was in
//! flight when the engine died is sent again with its original request id and counts from
//! then on. After the recovery:
//!
//! - the project opens, writable (the dead engine's lock is taken over), and offers its
//!   journal for recovery; `project.recover` succeeds;
//! - the recovered document is at least at the last acknowledged revision;
//! - once the in-flight commands are settled, the document equals the reference's, compared
//!   by structure ([`compare`]): **no acknowledged command is lost, and a command that was in
//!   flight is there whole or not at all** (a half-applied one would differ from both);
//! - the checkpoints are the reference's, the export queue's pause is what was acknowledged,
//!   the document on disk is a readable project file, and a save after the recovery leaves no
//!   temporary file ([`disk::check`]).
//!
//! A run ends with a save, one more kill and a start that must find nothing to recover.
//!
//! # Reproducing
//!
//! The work, the crash points, their order and the delays before each kill follow from the
//! seed; a failing run prints it. The instant a kill lands inside the engine is the operating
//! system's, so a rerun gets the same crash plan, not the same interleaving.

pub mod clients;
pub mod compare;
pub mod disk;
pub mod script;
mod session;

use std::fmt::Write as _;
use std::path::PathBuf;

use crate::BenchError;
use crate::fixture::Rng;

/// Where the engine is killed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Point {
    /// With editor commands pipelined and an agent call in flight.
    Edit,
    /// By the engine itself right after its Nth commit, before the reply leaves
    /// (`CUTWEAVE_TEST_CRASH_AFTER`, debug builds): the journal has a command nobody was told
    /// about.
    CommitAbort,
    /// While `project.save` runs (journal rotation, the atomic document write), with edits
    /// around it.
    Save,
    /// When autosave is due (2 s after the last edit), with an agent call in flight.
    Autosave,
    /// In the middle of a drag: the gesture is open and has live updates, nothing committed.
    Drag,
    /// With the drag's commit in flight.
    DragCommit,
    /// After an acknowledged edit; then the journal's last record is cut short, as a torn
    /// write: recovery must stop there, keep everything before it and go on.
    TornTail,
    /// Twice: with edits in flight, and again while `project.recover` replays the journal.
    Recover,
    /// While `media.import` registers a file.
    Import,
    /// After the export queue's pause changed (`.cutweave/exports.sqlite`), with an edit in
    /// flight.
    Queue,
    /// `--with-media` only: while the first of two queued exports is being encoded. After the
    /// restart both must still be queued in order and finish, with no partial file left.
    Export,
    /// `--with-media` only: while a proxy is being made. After the restart its state must be
    /// sane (a proxy called ready has its file) and making it again must finish.
    Proxy,
}

impl Point {
    /// Every crash point.
    pub const ALL: [Point; 10] = [
        Point::Edit,
        Point::CommitAbort,
        Point::Save,
        Point::Autosave,
        Point::Drag,
        Point::DragCommit,
        Point::TornTail,
        Point::Recover,
        Point::Import,
        Point::Queue,
    ];

    /// The crash points that need real media: an engine built with FFmpeg, a GPU and the
    /// project's source files (`--with-media`).
    pub const REAL: [Point; 2] = [Point::Export, Point::Proxy];

    /// Its name in reports and `--points`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Point::Edit => "edit",
            Point::CommitAbort => "commit-abort",
            Point::Save => "save",
            Point::Autosave => "autosave",
            Point::Drag => "drag",
            Point::DragCommit => "drag-commit",
            Point::TornTail => "torn-tail",
            Point::Recover => "recover",
            Point::Import => "import",
            Point::Queue => "queue",
            Point::Export => "export",
            Point::Proxy => "proxy",
        }
    }

    /// The point called `name`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Point> {
        Point::ALL
            .into_iter()
            .chain(Point::REAL)
            .find(|p| p.name() == name)
    }
}

/// A fault the harness injects to test itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// At every [`Point::TornTail`] the journal loses its last record but the harness keeps
    /// counting the command as acknowledged, as if the engine had acknowledged an edit it
    /// never wrote: the run must fail.
    LoseAcknowledged,
}

/// How to run the harness.
#[derive(Clone, Debug)]
pub struct CrashOptions {
    /// The `cutweave-bench` executable (its `engine` command is the engine process).
    pub exe: PathBuf,
    /// A folder for the projects and the engines' files; emptied first.
    pub scratch: PathBuf,
    /// How many crashes.
    pub count: u32,
    /// Seeds the project, the work, the crash points and the delays.
    pub seed: u64,
    /// Crash only at these points (in seeded order); `None`: all of them.
    pub points: Option<Vec<Point>>,
    /// The engine under test is the full engine (FFmpeg, the GPU) on a project whose source
    /// files exist, so exports and proxies really run: adds [`Point::REAL`] and leaves out
    /// [`Point::Import`] (the reference engine keeps the stub prober). `exe` must be built
    /// with the `ffmpeg` feature.
    pub media: bool,
    /// Self-test.
    pub fault: Option<Fault>,
    /// Print each crash as it is checked.
    pub progress: bool,
}

/// One crash and what its recovery showed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// 1-based.
    pub n: u32,
    /// Where the engine was killed.
    pub point: Point,
    /// Commands acknowledged in this life of the engine.
    pub acknowledged: usize,
    /// Commands in flight at the kill.
    pub in_flight: usize,
    /// Of those, how many the journal already had (their original result came back).
    pub journaled: usize,
    /// Transactions `project.recover` replayed.
    pub replayed: u64,
    /// The revision of the recovered document.
    pub recovered_rev: u64,
    /// What else happened (a torn tail, a second kill).
    pub note: String,
    /// `Err`: what was wrong after this crash.
    pub result: Result<(), String>,
    /// From the kill to the end of the checks, in milliseconds.
    pub ms: u128,
}

/// A whole run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrashReport {
    /// The seed.
    pub seed: u64,
    /// One row per crash, in order; a failed crash is the last row.
    pub rows: Vec<Row>,
    /// Commands the engine under test acknowledged in the whole run.
    pub acknowledged: usize,
    /// Commands it refused (they are part of the work, see [`script`]).
    pub refused: usize,
    /// The closing check (save, kill, clean start): `None` when a crash failed before it,
    /// `Some(Err)` when it failed.
    pub closing: Option<Result<(), String>>,
}

impl CrashReport {
    /// Whether every crash was recovered from and the closing check passed.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.failure().is_none()
    }

    /// What failed first.
    #[must_use]
    pub fn failure(&self) -> Option<String> {
        self.rows
            .iter()
            .find_map(|row| {
                let why = row.result.as_ref().err()?;
                Some(format!("crash {} ({}): {why}", row.n, row.point.name()))
            })
            .or_else(|| {
                let failed = self.closing.as_ref()?.as_ref().err()?;
                Some(format!("closing check: {failed}"))
            })
    }

    /// The pass/fail matrix: crash points against crashes, then one line per crash.
    #[must_use]
    pub fn table(&self) -> String {
        let mut out = format!("crash harness, seed {}\n\n", self.seed);
        let _ = writeln!(out, "{:<14} {:>7} {:>7}", "point", "crashes", "passed");
        for point in Point::ALL.into_iter().chain(Point::REAL) {
            let rows: Vec<&Row> = self.rows.iter().filter(|r| r.point == point).collect();
            if rows.is_empty() {
                continue;
            }
            let passed = rows.iter().filter(|r| r.result.is_ok()).count();
            let _ = writeln!(out, "{:<14} {:>7} {:>7}", point.name(), rows.len(), passed);
        }
        let _ = writeln!(
            out,
            "\n{:>3}  {:<13} {:>5} {:>9} {:>9} {:>8} {:>6} {:>6}  result",
            "#", "point", "acked", "in flight", "journaled", "replayed", "rev", "ms"
        );
        for row in &self.rows {
            let result = match &row.result {
                Ok(()) => "PASS".to_owned(),
                Err(why) => format!("FAIL: {why}"),
            };
            let note = if row.note.is_empty() {
                String::new()
            } else {
                format!(" ({})", row.note)
            };
            let _ = writeln!(
                out,
                "{:>3}  {:<13} {:>5} {:>9} {:>9} {:>8} {:>6} {:>6}  {result}{note}",
                row.n,
                row.point.name(),
                row.acknowledged,
                row.in_flight,
                row.journaled,
                row.replayed,
                row.recovered_rev,
                row.ms
            );
        }
        let passed = self.rows.iter().filter(|r| r.result.is_ok()).count();
        let closing = match &self.closing {
            Some(Ok(())) => "clean start after the last save: PASS".to_owned(),
            Some(Err(why)) => format!("clean start after the last save: FAIL: {why}"),
            None => "clean start after the last save: not run".to_owned(),
        };
        let _ = writeln!(
            out,
            "\n{passed}/{} crashes recovered with nothing lost; {} commands acknowledged, {} refused\n{closing}",
            self.rows.len(),
            self.acknowledged,
            self.refused
        );
        if !self.passed() {
            let _ = writeln!(
                out,
                "FAILED; reproduce with: cutweave-bench crash --count {} --seed {}",
                self.rows.len(),
                self.seed
            );
        }
        out
    }
}

/// The crash points of a run, in order: every point once in a seeded order, then again, until
/// there are `count`. `media`: the points of a run with real media (see
/// [`CrashOptions::media`]).
#[must_use]
pub fn plan(count: u32, seed: u64, only: Option<&[Point]>, media: bool) -> Vec<Point> {
    let all = || {
        let stub = Point::ALL
            .into_iter()
            .filter(|p| !(media && *p == Point::Import));
        let real = Point::REAL.into_iter().filter(|_| media);
        stub.chain(real).collect()
    };
    let pool: Vec<Point> = only.map_or_else(all, <[Point]>::to_vec);
    let mut rng = Rng::new(seed ^ 0xC2A5);
    let mut out = Vec::new();
    while out.len() < count as usize && !pool.is_empty() {
        let mut round = pool.clone();
        // Fisher-Yates.
        for i in (1..round.len()).rev() {
            let j = usize::try_from(rng.below(i64::try_from(i + 1).unwrap_or(1))).unwrap_or(0);
            round.swap(i, j);
        }
        out.extend(round);
    }
    out.truncate(count as usize);
    out
}

/// Runs the harness. `Err`: the harness itself could not run (no engine, no project); a
/// recovery that fails is in the report.
pub fn run(options: &CrashOptions) -> Result<CrashReport, BenchError> {
    let points = plan(
        options.count,
        options.seed,
        options.points.as_deref(),
        options.media,
    );
    let misplaced = points.iter().find(|p| {
        if options.media {
            **p == Point::Import
        } else {
            Point::REAL.contains(p)
        }
    });
    if let Some(point) = misplaced {
        return Err(BenchError::Spec(format!(
            "the crash point `{}` cannot be used {} --with-media",
            point.name(),
            if options.media { "with" } else { "without" }
        )));
    }
    let mut session = session::Session::start(options, points.first().copied())?;
    let mut rows = Vec::new();
    for (i, point) in points.iter().enumerate() {
        let n = u32::try_from(i + 1).unwrap_or(u32::MAX);
        let row = session.crash(n, *point, points.get(i + 1).copied())?;
        if options.progress {
            let result = row.result.as_ref().map_or_else(String::as_str, |()| "ok");
            eprintln!("crash {n}/{} at {}: {result}", points.len(), point.name());
        }
        let failed = row.result.is_err();
        rows.push(row);
        if failed {
            break;
        }
    }
    let closing = rows
        .iter()
        .all(|r| r.result.is_ok())
        .then(|| session.closing_check());
    let (acknowledged, refused) = session.totals();
    Ok(CrashReport {
        seed: options.seed,
        rows,
        acknowledged,
        refused,
        closing,
    })
}

/// Usage of `cutweave-bench crash`.
pub const USAGE: &str = "usage: cutweave-bench crash [--count N] [--seed S] [--points a,b,...]
                            [--scratch DIR] [--with-media] [--self-test] [--quiet]
Kills a real engine N times (default 20) during seeded editing work, restarts it and checks
that nothing it acknowledged is lost (WP-2J.05). --seed (default: from the clock, printed)
repeats the work and the crash plan of a run. --points limits the crash points:
edit, commit-abort, save, autosave, drag, drag-commit, torn-tail, recover, import, queue.
--with-media runs the full engine on real source files and adds the points export and proxy
(needs a build with --features ffmpeg, the FFmpeg bundle and the GPU; no import point).
--self-test makes the journal lose an acknowledged edit and passes only if the run notices.
--scratch (default target/tmp/crash-harness) is emptied first.";

/// Parsed arguments of `cutweave-bench crash`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrashArgs {
    /// `--count`.
    pub count: u32,
    /// `--seed`.
    pub seed: Option<u64>,
    /// `--points`.
    pub points: Option<Vec<Point>>,
    /// `--scratch`.
    pub scratch: Option<PathBuf>,
    /// `--self-test`.
    pub self_test: bool,
    /// `--with-media`.
    pub with_media: bool,
    /// `--quiet`: no line per crash while it runs.
    pub quiet: bool,
}

impl CrashArgs {
    /// Parses the arguments after `crash`.
    pub fn parse(args: &[String]) -> Result<CrashArgs, BenchError> {
        let mut out = CrashArgs {
            count: 20,
            seed: None,
            points: None,
            scratch: None,
            self_test: false,
            with_media: false,
            quiet: false,
        };
        let mut it = args.iter();
        while let Some(flag) = it.next() {
            let mut value = || {
                it.next()
                    .ok_or_else(|| BenchError::Spec(format!("{flag} needs a value")))
            };
            let number = |v: &String| {
                v.parse::<u64>()
                    .map_err(|_| BenchError::Spec(format!("{flag}: `{v}` is not a number")))
            };
            match flag.as_str() {
                "--count" => {
                    out.count = u32::try_from(number(value()?)?)
                        .map_err(|_| BenchError::Spec("--count is too large".into()))?;
                }
                "--seed" => out.seed = Some(number(value()?)?),
                "--scratch" => out.scratch = Some(PathBuf::from(value()?)),
                "--points" => {
                    let points = value()?
                        .split(',')
                        .map(|name| {
                            Point::parse(name.trim()).ok_or_else(|| {
                                BenchError::Spec(format!("unknown crash point `{name}`"))
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    out.points = Some(points);
                }
                "--self-test" => out.self_test = true,
                "--with-media" => out.with_media = true,
                "--quiet" => out.quiet = true,
                other => return Err(BenchError::Spec(format!("unknown option `{other}`"))),
            }
        }
        Ok(out)
    }
}

/// `cutweave-bench crash`: runs the harness in the checkout at `root` with the engine of
/// `exe` and returns what to print and whether the run passed.
pub fn command(
    root: &std::path::Path,
    exe: &std::path::Path,
    args: &[String],
) -> Result<(String, bool), BenchError> {
    let args = CrashArgs::parse(args)?;
    let seed = args.seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_secs())
    });
    let mut options = CrashOptions {
        exe: exe.to_path_buf(),
        scratch: args
            .scratch
            .unwrap_or_else(|| root.join("target").join("tmp").join("crash-harness")),
        count: args.count,
        seed,
        points: args.points,
        media: args.with_media,
        fault: None,
        progress: !args.quiet,
    };
    if !args.self_test {
        let report = run(&options)?;
        return Ok((report.table(), report.passed()));
    }
    options.fault = Some(Fault::LoseAcknowledged);
    options
        .points
        .get_or_insert_with(|| vec![Point::TornTail, Point::Edit]);
    let report = run(&options)?;
    let caught = report
        .rows
        .iter()
        .any(|row| row.point == Point::TornTail && row.result.is_err());
    let verdict = if caught {
        "self-test PASS: the lost acknowledged edit was caught"
    } else {
        "self-test FAIL: an acknowledged edit was lost and the run did not notice"
    };
    Ok((format!("{}\n{verdict}\n", report.table()), caught))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arguments_of_the_command() {
        let parse = |s: &str| {
            let args: Vec<String> = s.split_whitespace().map(str::to_owned).collect();
            CrashArgs::parse(&args)
        };
        let plain = parse("").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            (plain.count, plain.seed, plain.self_test),
            (20, None, false)
        );
        let full = parse("--count 5 --seed 42 --points save,torn-tail --scratch x --quiet")
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((full.count, full.seed, full.quiet), (5, Some(42), true));
        assert_eq!(full.points, Some(vec![Point::Save, Point::TornTail]));
        assert_eq!(full.scratch, Some(PathBuf::from("x")));
        assert!(parse("--self-test").is_ok_and(|a| a.self_test));
        assert!(parse("--with-media").is_ok_and(|a| a.with_media));
        for bad in ["--count", "--count x", "--points nope", "--frobnicate"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_plan_covers_every_point_before_repeating_and_follows_the_seed() {
        let twenty = plan(20, 7, None, false);
        assert_eq!(twenty.len(), 20);
        for round in twenty.chunks(Point::ALL.len()) {
            let mut sorted = round.to_vec();
            sorted.sort();
            assert_eq!(sorted, Point::ALL);
        }
        assert_eq!(twenty, plan(20, 7, None, false));
        assert!((8..40).any(|seed| plan(20, seed, None, false) != twenty));
        assert_eq!(plan(3, 1, Some(&[Point::Save]), false), [Point::Save; 3]);
        assert_eq!(plan(4, 1, Some(&[]), false), Vec::<Point>::new());
        // With real media: the export and proxy points, and no import.
        let real = plan(11, 7, None, true);
        assert!(Point::REAL.iter().all(|p| real.contains(p)), "{real:?}");
        assert!(
            !real.contains(&Point::Import) && real.len() == 11,
            "{real:?}"
        );
        assert!(!twenty.iter().any(|p| Point::REAL.contains(p)));
    }

    #[test]
    fn points_have_names_and_the_table_names_a_failure_with_its_seed() {
        for point in Point::ALL.into_iter().chain(Point::REAL) {
            assert_eq!(Point::parse(point.name()), Some(point));
        }
        assert_eq!(Point::parse("nope"), None);
        let row = |n, point, result: Result<(), String>| Row {
            n,
            point,
            acknowledged: 12,
            in_flight: 2,
            journaled: 1,
            replayed: 9,
            recovered_rev: 40,
            note: String::new(),
            result,
            ms: 800,
        };
        let mut report = CrashReport {
            seed: 99,
            rows: vec![row(1, Point::Edit, Ok(())), row(2, Point::Save, Ok(()))],
            acknowledged: 24,
            refused: 3,
            closing: Some(Ok(())),
        };
        assert!(report.passed());
        assert!(
            report
                .table()
                .contains("2/2 crashes recovered with nothing lost")
        );
        assert!(!report.table().contains("FAILED"));
        report.closing = Some(Err("recovery offered".into()));
        assert_eq!(
            report.failure().as_deref(),
            Some("closing check: recovery offered")
        );
        report.closing = None;
        report
            .rows
            .push(row(3, Point::TornTail, Err("an edit is gone".into())));
        assert_eq!(
            report.failure().as_deref(),
            Some("crash 3 (torn-tail): an edit is gone")
        );
        let table = report.table();
        assert!(table.contains("FAIL: an edit is gone"), "{table}");
        assert!(table.contains("clean start after the last save: not run"));
        assert!(
            table.contains("cutweave-bench crash --count 3 --seed 99"),
            "{table}"
        );
    }
}
