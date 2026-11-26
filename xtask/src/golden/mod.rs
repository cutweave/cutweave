// SPDX-License-Identifier: GPL-3.0-or-later
//! The golden-frame suite (WP-1O.01; docs/01-architecture.md §8, §20; testdata/golden/README.md).
//!
//! ```text
//! cargo xtask golden-projects                    build the CLI (FFmpeg), the sources and the projects
//! cargo xtask golden [--adapter CHOICE]...       render and compare (default: the software adapter)
//!                    [--all-adapters]            every local adapter that has a reference set
//!                    [--update --i-know [--reason TEXT]]   rewrite this adapter's references
//! cargo xtask golden --check-refs [--base REV] [--pr-labels A,B]   the update guard alone
//! ```
//!
//! A run: the update guard ([`refs::guard`]); `cargo build -p cutweave-cli --features ffmpeg`;
//! sources and projects ([`projects`]); for each adapter, every project through
//! `cutweave render --frames ... --check-determinism` with `CUTWEAVE_GPU` set (so each frame is
//! also rendered twice and through the export readback, bit-identically); then every frame
//! against the reference of **the adapter it rendered on** ([`compare`]). A failure writes
//! `<frame>.actual.png`, `.expected.png` and `.diff.png` under `target/golden/failures/`. The
//! negative test ([`projects::NEGATIVE`]) must fail and writes its diffs under
//! `target/golden/negative/`. `target/golden/report.json` holds the results.
//!
//! Needs the FFmpeg bundle in the environment (`. scripts/ffmpeg/env.sh`; `just golden` does it).

mod compare;
mod projects;
mod refs;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use serde_json::{Value, json};

use compare::{Judgement, Verdict, diff_image, judge, read_png, write_png};
use projects::{Built, NEGATIVE, NEGATIVE_AGAINST, PROJECTS};

/// The default adapter choice: the software rasteriser (WARP on Windows, lavapipe on Linux), the
/// CI class.
const DEFAULT_ADAPTER: &str = "software";

/// Options of `cargo xtask golden`.
#[derive(Debug, Default)]
#[allow(clippy::struct_excessive_bools)] // independent command-line switches
struct Opts {
    adapters: Vec<String>,
    all_adapters: bool,
    update: bool,
    i_know: bool,
    reason: Option<String>,
    check_refs: bool,
    base: Option<String>,
    pr_labels: Option<String>,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut it = args.iter();
    let value = |it: &mut std::slice::Iter<'_, String>, flag: &str| {
        it.next()
            .cloned()
            .ok_or_else(|| format!("{flag} needs a value"))
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--adapter" => o.adapters.push(value(&mut it, a)?),
            "--all-adapters" => o.all_adapters = true,
            "--update" => o.update = true,
            "--i-know" => o.i_know = true,
            "--reason" => o.reason = Some(value(&mut it, a)?),
            "--check-refs" => o.check_refs = true,
            "--base" => o.base = Some(value(&mut it, a)?),
            "--pr-labels" => o.pr_labels = Some(value(&mut it, a)?),
            other => return Err(format!("golden: unknown argument `{other}`")),
        }
    }
    if o.update && !o.i_know {
        return Err(
            "golden --update rewrites the reference frames of the adapter it runs on. \
                    Pass --i-know to confirm (and give the pull request the `golden-update` \
                    label); the run records the change in testdata/golden/UPDATES.md"
                .into(),
        );
    }
    if o.update && o.all_adapters {
        return Err("golden --update rewrites one adapter's set at a time (--adapter)".into());
    }
    if o.adapters.is_empty() && !o.all_adapters {
        o.adapters.push(DEFAULT_ADAPTER.into());
    }
    Ok(o)
}

/// The frames every reference set holds: `<project>/f<frame>.png`.
fn known_frames() -> BTreeSet<String> {
    PROJECTS
        .iter()
        .flat_map(|p| {
            p.frames
                .iter()
                .map(move |f| format!("{}/{}", p.name, refs::frame_file(*f)))
        })
        .collect()
}

/// `target/golden` (or `$CARGO_TARGET_DIR/golden`).
fn work_dir(root: &Path) -> PathBuf {
    target_dir(root).join("golden")
}

fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from)
}

/// Builds the `cutweave` binary with FFmpeg and returns its path.
fn build_cli(root: &Path) -> Result<PathBuf, String> {
    if std::env::var_os("FFMPEG_DIR").is_none() {
        return Err(
            "golden needs the FFmpeg bundle: `. scripts/ffmpeg/env.sh` first (`just \
                    golden` does), or `just ffmpeg-fetch` if there is none"
                .into(),
        );
    }
    let mut cmd = super::cargo();
    cmd.args([
        "build",
        "--quiet",
        "-p",
        "cutweave-cli",
        "--features",
        "ffmpeg",
        "--bin",
        "cutweave",
    ]);
    if !super::run(cmd) {
        return Err("cargo build -p cutweave-cli --features ffmpeg failed".into());
    }
    let exe = target_dir(root)
        .join("debug")
        .join(format!("cutweave{}", std::env::consts::EXE_SUFFIX));
    if exe.is_file() {
        Ok(exe)
    } else {
        Err(format!("{} was not built", exe.display()))
    }
}

/// Sources and projects (`cargo xtask golden-projects` and the first half of a run).
fn prepare(root: &Path) -> Result<(PathBuf, Vec<Built>), String> {
    let cutweave = build_cli(root)?;
    let work = work_dir(root);
    let started = Instant::now();
    projects::generate_sources(&work.join("sources"))?;
    let built = projects::build_projects(&cutweave, &work)?;
    println!(
        "golden: {} sources and {} projects in {} ({:.1} s)",
        projects::SOURCES.len(),
        built.len(),
        work.display(),
        started.elapsed().as_secs_f64()
    );
    Ok((cutweave, built))
}

/// `cargo xtask golden-projects`.
pub fn golden_projects(root: &Path, args: &[String]) -> bool {
    if let Some(a) = args.first() {
        eprintln!("golden-projects: unknown argument `{a}`");
        return false;
    }
    match prepare(root) {
        Ok((_, built)) => {
            for b in &built {
                println!(
                    "  {:<22} {}  ({})",
                    b.golden.name,
                    b.document.display(),
                    b.golden.about
                );
            }
            true
        }
        Err(e) => {
            eprintln!("golden-projects: {e}");
            false
        }
    }
}

/// One frame of one `cutweave render` run.
struct Rendered {
    project: &'static str,
    frame: u32,
    file: PathBuf,
}

/// One adapter's renders.
struct Run {
    adapter: Value,
    key: String,
    frames: Vec<Rendered>,
    negative: Vec<Rendered>,
}

/// `cutweave render` processes running at once (each opens its own engine and devices).
const RENDER_JOBS: usize = 3;

/// A render's JSON result and its seconds, once it ran.
type RenderSlot = std::sync::Mutex<Option<Result<(Value, f64), String>>>;

/// One `cutweave render --frames ... --check-determinism` of `b` on `choice`: its JSON result
/// and the seconds it took.
fn render_one(
    cutweave: &Path,
    b: &Built,
    work: &Path,
    choice: &str,
) -> Result<(Value, f64), String> {
    let frames: Vec<String> = b.golden.frames.iter().map(|f| format!("f:{f}")).collect();
    let started = Instant::now();
    let result = Command::new(cutweave)
        .arg("render")
        .arg("--project")
        .arg(&b.document)
        .arg("--frames")
        .arg(frames.join(","))
        .arg("--out-dir")
        .arg(work.join("actual").join(b.golden.name))
        // Caches of its own: never the person's, and no two processes on one SQLite file.
        .arg("--cache-dir")
        .arg(work.join("cache").join(b.golden.name))
        .arg("--check-determinism")
        .env("CUTWEAVE_GPU", choice)
        .output()
        .map_err(|e| format!("cannot run {}: {e}", cutweave.display()))?;
    if !result.status.success() {
        return Err(format!(
            "cutweave render of `{}` on `{choice}` failed ({}): {}",
            b.golden.name,
            result.status,
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    let report = serde_json::from_slice(&result.stdout)
        .map_err(|e| format!("cutweave render printed no JSON: {e}"))?;
    Ok((report, started.elapsed().as_secs_f64()))
}

/// The frames of one render result, checked against the recipe: the frames asked for, each
/// showing a clip, none with unavailable media.
fn rendered_frames(b: &Built, report: &Value) -> Result<Vec<Rendered>, String> {
    let name = b.golden.name;
    let frames = report["frames"].as_array().cloned().unwrap_or_default();
    if frames.len() != b.golden.frames.len() {
        return Err(format!("`{name}`: cutweave render returned {report}"));
    }
    let mut rendered = Vec::new();
    for (f, frame) in frames.iter().zip(b.golden.frames) {
        if f["frame"].as_u64() != Some(u64::from(frame)) {
            return Err(format!("`{name}`: expected frame {frame}, got {f}"));
        }
        if f["unavailable"].as_u64().unwrap_or(0) > 0 {
            return Err(format!(
                "`{name}` frame {frame}: media unavailable ({})",
                f["notes"]
            ));
        }
        // Every golden frame shows a clip: a background-only frame means a broken recipe.
        if f["visible"].as_u64().unwrap_or(0) == 0 {
            return Err(format!(
                "`{name}` frame {frame}: no item is visible ({})",
                f["notes"]
            ));
        }
        rendered.push(Rendered {
            project: name,
            frame,
            file: PathBuf::from(f["file"].as_str().unwrap_or_default()),
        });
    }
    Ok(rendered)
}

/// Renders every project on `choice` (`CUTWEAVE_GPU`) with the determinism check, up to
/// [`RENDER_JOBS`] at a time, and checks that they all ran on one adapter.
fn render_all(cutweave: &Path, work: &Path, built: &[Built], choice: &str) -> Result<Run, String> {
    let out_root = work.join("actual");
    if out_root.exists() {
        std::fs::remove_dir_all(&out_root).map_err(|e| format!("{}: {e}", out_root.display()))?;
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<RenderSlot> = built.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..RENDER_JOBS.min(built.len()) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(b) = built.get(i) else {
                        break;
                    };
                    let result = render_one(cutweave, b, work, choice);
                    if let Ok(mut slot) = results[i].lock() {
                        *slot = Some(result);
                    }
                }
            });
        }
    });
    let mut run = Run {
        adapter: Value::Null,
        key: String::new(),
        frames: Vec::new(),
        negative: Vec::new(),
    };
    for (b, slot) in built.iter().zip(results) {
        let (report, seconds) = slot
            .into_inner()
            .ok()
            .flatten()
            .unwrap_or_else(|| Err(format!("`{}` was not rendered", b.golden.name)))?;
        let key = report["adapter"]["key"].as_str().unwrap_or("").to_owned();
        if !refs::is_adapter_key(&key) {
            return Err(format!("cutweave render named no adapter: {report}"));
        }
        if run.key.is_empty() {
            run.key = key;
            run.adapter = report["adapter"].clone();
        } else if run.key != key {
            return Err(format!(
                "`{}` rendered on `{key}`, the other projects on `{}`",
                b.golden.name, run.key
            ));
        }
        let rendered = rendered_frames(b, &report)?;
        println!(
            "  rendered {:<22} {} frames, each bit-identical twice and through the export \
             readback ({seconds:.1} s)",
            b.golden.name,
            rendered.len(),
        );
        if b.golden.name == NEGATIVE.name {
            run.negative = rendered;
        } else {
            run.frames.extend(rendered);
        }
    }
    Ok(run)
}

/// Writes `<dir>/<stem>.actual.png`, `.expected.png` and `.diff.png`.
fn write_evidence(
    dir: &Path,
    stem: &str,
    actual: &cutweave_render::Image,
    expected: &cutweave_render::Image,
) -> Result<(), String> {
    write_png(&dir.join(format!("{stem}.actual.png")), actual, false)?;
    write_png(&dir.join(format!("{stem}.expected.png")), expected, false)?;
    write_png(
        &dir.join(format!("{stem}.diff.png")),
        &diff_image(actual, expected),
        false,
    )
}

/// `f00045`.
fn stem(frame: u32) -> String {
    refs::frame_file(frame).replace(".png", "")
}

/// Compares (or, with `update`, rewrites) one adapter's frames, then runs the negative test;
/// returns the report rows and whether everything passed.
fn check_run(
    root: &Path,
    work: &Path,
    run: &Run,
    update: bool,
    reason: Option<&str>,
) -> Result<(Vec<Value>, bool), String> {
    let key = &run.key;
    for d in [work.join("failures"), work.join("negative")] {
        let d = d.join(key);
        if d.exists() {
            std::fs::remove_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
    }
    let had_set = root.join(refs::EXPECTED).join(key).is_dir();
    if had_set {
        refs::materialize(root, key)?;
    }
    let (mut rows, mut ok) = if update {
        (update_frames(root, run, had_set, reason)?, true)
    } else {
        compare_frames(root, work, run)?
    };
    let (negative_rows, negative_ok) = negative_test(root, work, run)?;
    rows.extend(negative_rows);
    ok &= negative_ok;
    Ok((rows, ok))
}

/// Every frame against its reference on the same adapter; failures write their evidence.
fn compare_frames(root: &Path, work: &Path, run: &Run) -> Result<(Vec<Value>, bool), String> {
    let key = &run.key;
    let mut rows = Vec::new();
    let mut ok = true;
    for r in &run.frames {
        let name = format!("{}/{}", r.project, refs::frame_file(r.frame));
        let reference = refs::reference_path(root, key, r.project, r.frame);
        if !reference.is_file() {
            ok = false;
            println!(
                "  {name:<32} MISSING: no reference in {}/{key} (a new adapter or frame: \
                 `cargo xtask golden --update --i-know --adapter key:{key}`)",
                refs::EXPECTED
            );
            rows.push(json!({"frame": name, "verdict": "missing"}));
            continue;
        }
        let actual = read_png(&r.file)?;
        let expected = read_png(&reference)?;
        let j = judge(&actual, &expected);
        if !j.verdict.passed() {
            ok = false;
            let dir = work.join("failures").join(key).join(r.project);
            write_evidence(&dir, &stem(r.frame), &actual, &expected)?;
        }
        println!("  {name:<32} {j}");
        rows.push(row_json(&name, &j));
    }
    Ok((rows, ok))
}

/// `--update`: writes every frame whose pixels differ from its reference (or that has none),
/// removes stale files of the set, and records the change in the update log.
fn update_frames(
    root: &Path,
    run: &Run,
    had_set: bool,
    reason: Option<&str>,
) -> Result<Vec<Value>, String> {
    let key = &run.key;
    let mut rows = Vec::new();
    let mut written = Vec::new();
    for r in &run.frames {
        let name = format!("{}/{}", r.project, refs::frame_file(r.frame));
        let reference = refs::reference_path(root, key, r.project, r.frame);
        let actual = read_png(&r.file)?;
        let before = if reference.is_file() {
            Some(judge(&actual, &read_png(&reference)?))
        } else {
            None
        };
        if before.is_some_and(|j| j.verdict == Verdict::BitIdentical) {
            println!("  {name:<32} unchanged");
        } else {
            write_png(&reference, &actual, true)?;
            let size = std::fs::metadata(&reference).map_or(0, |m| m.len());
            if size > refs::MAX_REF_BYTES {
                return Err(format!(
                    "{name}: the reference is {size} bytes, over the {} limit",
                    refs::MAX_REF_BYTES
                ));
            }
            println!(
                "  {name:<32} written ({size} bytes){}",
                before.map_or_else(String::new, |j| format!("; was {j}"))
            );
            written.push(name.clone());
        }
        rows.push(json!({"frame": name, "verdict": "updated"}));
    }
    // Files of this set that are no longer frames of a golden project.
    let known = known_frames();
    let set_dir = root.join(refs::EXPECTED).join(key);
    for f in refs::files_under(&set_dir) {
        let rel = refs::relative(&f, &set_dir);
        if !known.contains(&rel) {
            std::fs::remove_file(&f).map_err(|e| format!("{}: {e}", f.display()))?;
            println!("  {rel:<32} removed (stale)");
            written.push(format!("{rel} (removed)"));
        }
    }
    // An entry is due when files changed, or when the log has none for the set as it is.
    if written.is_empty() && refs::is_recorded(root, key)? {
        return Ok(rows);
    }
    let total = known.len();
    let what = if written.is_empty() {
        format!("{total} of {total} references unchanged, recorded again")
    } else if written.len() == total && !had_set {
        format!("{total} of {total} references written (new set)")
    } else {
        format!(
            "{} of {total} references written ({})",
            written.len(),
            written.join(", ")
        )
    };
    let line = format!(
        "{what} · {} ({}) · FFmpeg {} · reason: {}",
        run.adapter["name"].as_str().unwrap_or("?"),
        run.adapter["driver"].as_str().unwrap_or("?"),
        projects::ffmpeg_version(),
        reason.unwrap_or("(none given)")
    );
    refs::record_update(root, key, &line)?;
    println!("  recorded in {}", refs::UPDATES);
    Ok(rows)
}

/// The negative test: [`NEGATIVE`] (one pixel of offset) must fail against every reference of
/// [`NEGATIVE_AGAINST`]; its evidence goes to `target/golden/negative/<adapter>/`.
fn negative_test(root: &Path, work: &Path, run: &Run) -> Result<(Vec<Value>, bool), String> {
    let key = &run.key;
    let mut rows = Vec::new();
    let mut ok = true;
    for r in &run.negative {
        let name = format!(
            "{} vs {NEGATIVE_AGAINST}/{}",
            NEGATIVE.name,
            refs::frame_file(r.frame)
        );
        let reference = refs::reference_path(root, key, NEGATIVE_AGAINST, r.frame);
        if !reference.is_file() {
            ok = false;
            println!("  {name:<32} no reference to compare with");
            continue;
        }
        let actual = read_png(&r.file)?;
        let expected = read_png(&reference)?;
        let j = judge(&actual, &expected);
        let dir = work.join("negative").join(key);
        write_evidence(&dir, &stem(r.frame), &actual, &expected)?;
        if j.verdict.passed() {
            ok = false;
            println!("  {name:<32} PASSED, but a 1-pixel offset must fail: {j}");
        } else {
            println!("  {name:<32} fails as it must: {j}");
        }
        let mut row = row_json(&name, &j);
        row["negative"] = json!(true);
        rows.push(row);
    }
    Ok((rows, ok))
}

fn row_json(name: &str, j: &Judgement) -> Value {
    let mut row = json!({
        "frame": name,
        "verdict": match j.verdict {
            Verdict::BitIdentical => "bit-identical",
            Verdict::WithinTolerance => "within-tolerance",
            Verdict::Failed => "failed",
        },
    });
    if let Some(c) = &j.comparison {
        row["psnr_db"] = json!(if c.psnr_db.is_finite() {
            c.psnr_db
        } else {
            f64::MAX
        });
        row["ssim"] = json!(c.ssim);
        row["max_abs_diff"] = json!(c.max_abs_diff);
    }
    row
}

/// The adapter choices of `--all-adapters`: the software adapter plus every local adapter that
/// has a reference set.
fn all_adapter_choices(root: &Path) -> Vec<String> {
    let stored: BTreeSet<String> = refs::stored_sets(root).into_iter().collect();
    let mut choices = vec![DEFAULT_ADAPTER.to_owned()];
    for a in cutweave_render::gpu::list_adapters() {
        if !a.software && stored.contains(&a.key) && !choices.contains(&format!("key:{}", a.key)) {
            choices.push(format!("key:{}", a.key));
        }
    }
    choices
}

/// The update guard and the store check; prints what it found.
fn run_guard(root: &Path, opts: &Opts) -> bool {
    let mut ok = true;
    match refs::guard(root, opts.base.as_deref(), opts.pr_labels.as_deref()) {
        Ok(report) if report.changed.is_empty() => {
            println!("golden: references unchanged since {}", short(&report.base));
        }
        Ok(report) => {
            for (set, files) in &report.changed {
                println!(
                    "golden: {} reference(s) of `{set}` changed since {}, recorded in {}",
                    files.len(),
                    short(&report.base),
                    refs::UPDATES
                );
            }
        }
        Err(e) => {
            eprintln!("golden: reference-update guard: {e}");
            ok = false;
        }
    }
    let (problems, files, total) = refs::check_store(root, &known_frames());
    for p in &problems {
        eprintln!("golden: {p}");
    }
    if problems.is_empty() {
        println!(
            "golden: store ok: {files} reference(s), {} KiB of {} KiB, sets: {}",
            total / 1024,
            refs::MAX_TOTAL_BYTES / 1024,
            refs::stored_sets(root).join(", ")
        );
    }
    ok && problems.is_empty()
}

fn short(rev: &str) -> &str {
    rev.get(..10).unwrap_or(rev)
}

/// `cargo xtask golden`.
pub fn golden(root: &Path, args: &[String]) -> bool {
    let opts = match parse(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            return false;
        }
    };
    if opts.check_refs {
        return run_guard(root, &opts);
    }
    let started = Instant::now();
    if !opts.update && !run_guard(root, &opts) {
        return false;
    }
    let (cutweave, built) = match prepare(root) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("golden: {e}");
            return false;
        }
    };
    let work = work_dir(root);
    let choices = if opts.all_adapters {
        all_adapter_choices(root)
    } else {
        opts.adapters.clone()
    };
    let mut ok = true;
    let mut report = Vec::new();
    for choice in &choices {
        println!("golden: rendering on `{choice}`");
        let run = match render_all(&cutweave, &work, &built, choice) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("golden: {e}");
                ok = false;
                continue;
            }
        };
        println!(
            "golden: {} — {} ({}), compared with {}/{}",
            run.key,
            run.adapter["name"].as_str().unwrap_or("?"),
            run.adapter["driver"].as_str().unwrap_or("?"),
            refs::EXPECTED,
            run.key
        );
        match check_run(root, &work, &run, opts.update, opts.reason.as_deref()) {
            Ok((rows, passed)) => {
                let count = |verdict: &str| {
                    rows.iter()
                        .filter(|r| r["verdict"] == verdict && r.get("negative").is_none())
                        .count()
                };
                let frames = rows.iter().filter(|r| r.get("negative").is_none()).count();
                let what = if opts.update {
                    format!("{frames} frames rendered into the reference set")
                } else {
                    format!(
                        "{frames} frames: {} bit-identical, {} within tolerance, {} failed, {} \
                         missing",
                        count("bit-identical"),
                        count("within-tolerance"),
                        count("failed"),
                        count("missing")
                    )
                };
                println!(
                    "golden: {}: {what}; {}",
                    run.key,
                    if passed {
                        "pass"
                    } else {
                        "FAILED (evidence in target/golden/failures)"
                    }
                );
                ok &= passed;
                report.push(json!({"choice": choice, "adapter": run.adapter, "frames": rows, "passed": passed}));
            }
            Err(e) => {
                eprintln!("golden: {e}");
                ok = false;
            }
        }
    }
    if opts.update {
        ok &= run_guard(root, &opts);
    }
    let elapsed = started.elapsed().as_secs_f64();
    let _ = std::fs::write(
        work.join("report.json"),
        serde_json::to_string_pretty(&json!({"passed": ok, "seconds": elapsed, "runs": report}))
            .unwrap_or_default(),
    );
    println!(
        "golden: {} in {elapsed:.1} s (report: {})",
        if ok { "ok" } else { "FAILED" },
        work.join("report.json").display()
    );
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn the_software_adapter_is_the_default() {
        assert_eq!(parse(&[]).unwrap().adapters, ["software"]);
        let o = parse(&args(&[
            "--adapter",
            "key:dx12-warp",
            "--adapter",
            "integrated",
        ]))
        .unwrap();
        assert_eq!(o.adapters, ["key:dx12-warp", "integrated"]);
    }

    #[test]
    fn updating_needs_i_know() {
        let e = parse(&args(&["--update"])).unwrap_err();
        assert!(e.contains("--i-know"), "{e}");
        let o = parse(&args(&["--update", "--i-know", "--reason", "new shader"])).unwrap();
        assert!(o.update && o.reason.as_deref() == Some("new shader"));
        assert!(parse(&args(&["--update", "--i-know", "--all-adapters"])).is_err());
        assert!(parse(&args(&["--bogus"])).is_err());
    }

    #[test]
    fn twenty_seven_known_frames() {
        let known = known_frames();
        assert_eq!(known.len(), 27);
        assert!(known.contains("cut_at_45/f00045.png"));
        assert!(known.contains("flipped_phone/f00110.png"));
        assert!(known.contains("side_by_side/f00119.png"));
    }
}
