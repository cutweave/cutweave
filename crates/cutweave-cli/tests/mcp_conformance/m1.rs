// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1O.04, M1 exit criterion 2: "through MCP headless, a scripted client does the same [as the
//! UI] and exports an identical-timeline file".
//!
//! A deterministic client (no LLM) drives `cutweave mcp --headless --deterministic-ids 42` on the
//! full engine with MCP tools only: it imports the three corpus clips of the M1 suite (H.264
//! 1080p60, HEVC 10-bit, VFR phone rotated 90°), waits for their analysis, places them back to
//! back on V1 and trims each, splits the first and moves its right half to the end, renders a
//! frame, saves, and exports a 1080p MP4 with `export_start` + `job_wait` (software encoder,
//! the 1080p preset). Every result is checked against its command's output schema.
//!
//! The UI does the same cut through the same commands, typed into the app
//! (`apps/desktop/e2e/m1/m1-exit.spec.ts`). `scripts/m1-gate.py` compares the two saved projects
//! by normalized structure and the two files by streams, length and decoded frames.
//!
//! Needs `cutweave` built with `--features ffmpeg`, the FFmpeg bundle on the path and the corpus
//! (`CUTWEAVE_CORPUS_DIR`, else `testdata/corpus` of this checkout or of the main checkout).
//! Without them the test says why and passes, unless `CUTWEAVE_M1_OUT` is set: then they are
//! required, and the project folder, the export and the session's results are copied to
//! `$CUTWEAVE_M1_OUT/mcp/`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{
    Checker, ContentBlock, Media, Revision, SEC, Session, Transport, fixture_path, normalize_ids,
};

/// The project name (the UI path uses the same one).
const PROJECT: &str = "M1 cut";

/// The M1 clips, in the order they are placed (corpus file names).
pub(crate) const CLIPS: [&str; 3] = [
    "h264_1080p60.mp4",
    "hevc_10bit_sdr.mp4",
    "vfr_phone_rotated90.mp4",
];

/// The cut: each clip's start and end on V1, in seconds, before the split.
const PLACED: [(i64, i64); 3] = [(0, 4), (4, 7), (7, 10)];
/// The first clip is split here; its right half moves to [`MOVED_TO`].
const SPLIT_AT: i64 = 2;
const MOVED_TO: i64 = 10;
/// The timeline's length afterwards.
const LENGTH: i64 = 12;
const LENGTH_SECS: f64 = 12.0;

/// `$CUTWEAVE_M1_OUT`: the gate's artefact folder.
fn out_dir() -> Option<PathBuf> {
    std::env::var_os("CUTWEAVE_M1_OUT").map(PathBuf::from)
}

/// Says why the test cannot run. Under the gate (`CUTWEAVE_M1_OUT`) that is a failure.
fn skip(why: &str) {
    assert!(out_dir().is_none(), "M1 exit 2 cannot run: {why}");
    eprintln!("skipping M1 exit 2: {why}");
}

/// The corpus: `CUTWEAVE_CORPUS_DIR`, else `testdata/corpus` of this checkout, else that of the
/// main checkout when this is a git worktree.
fn corpus_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CUTWEAVE_CORPUS_DIR") {
        let dir = PathBuf::from(dir);
        return dir.is_dir().then_some(dir);
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2)?;
    let local = root.join("testdata").join("corpus");
    if local.is_dir() {
        return Some(local);
    }
    // A worktree's `.git` is a file: `gitdir: <main>/.git/worktrees/<name>`.
    let gitdir = std::fs::read_to_string(root.join(".git")).ok()?;
    let gitdir = PathBuf::from(gitdir.strip_prefix("gitdir:")?.trim());
    let dir = gitdir
        .parent()?
        .parent()?
        .parent()?
        .join("testdata")
        .join("corpus");
    dir.is_dir().then_some(dir)
}

/// `ffprobe` of the FFmpeg bundle (`FFMPEG_DIR/bin`) or the path, if it runs.
fn ffprobe() -> Option<PathBuf> {
    let exe = format!("ffprobe{}", std::env::consts::EXE_SUFFIX);
    let candidate = std::env::var_os("FFMPEG_DIR")
        .map(|d| PathBuf::from(d).join("bin").join(&exe))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(&exe));
    std::process::Command::new(&candidate)
        .arg("-version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| candidate)
}

/// Waits for a job with `job_wait` (at most 60 s a call) until it ends or `limit` passes.
async fn wait_job(c: &Checker<'_>, job: &Value, limit: Duration) -> Value {
    let deadline = Instant::now() + limit;
    loop {
        let state = c
            .tool("job_wait", json!({ "job_id": job, "timeout_ms": 60_000 }))
            .await;
        let running = matches!(state["state"].as_str(), Some("queued" | "running"));
        if !running || Instant::now() >= deadline {
            return state;
        }
    }
}

/// Places `media` on V1 at `at` seconds and trims its end to `end` seconds (the UI: Enter in the
/// media bin at the playhead, then the inspector's Duration). Returns the video clip.
async fn place(
    c: &Checker<'_>,
    sequence: &Value,
    video: &Value,
    media: &Value,
    (at, end): (i64, i64),
) -> Value {
    let inserted = c
        .tool(
            "timeline_insert",
            json!({ "sequence": sequence, "track": video, "media": media, "at": at * SEC }),
        )
        .await;
    assert_eq!(inserted["at"], at * SEC, "{inserted}");
    let clip = inserted["clips"][0].clone();
    let trimmed = c
        .tool(
            "clip_trim",
            json!({ "item": clip, "edge": "end", "to": end * SEC }),
        )
        .await;
    assert_eq!(
        trimmed["items"][0]["duration"],
        (end - at) * SEC,
        "{trimmed}"
    );
    clip
}

/// Start and length of each item per track in flicks, top to bottom (the layout the UI test
/// reads in seconds).
fn layout(timeline: &Value) -> Vec<(String, Vec<(i64, i64)>)> {
    timeline["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            let mut items: Vec<(i64, i64)> = t["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| {
                    (
                        i["start"].as_i64().unwrap(),
                        i["duration"].as_i64().unwrap(),
                    )
                })
                .collect();
            items.sort_unstable();
            (t["name"].as_str().unwrap().to_owned(), items)
        })
        .collect()
}

/// `[(start, length)]` in whole seconds, as flicks.
fn secs(spans: &[(i64, i64)]) -> Vec<(i64, i64)> {
    spans.iter().map(|&(s, d)| (s * SEC, d * SEC)).collect()
}

/// What `ffprobe` says about the export: format and streams, as JSON.
fn probe(ffprobe: &Path, file: &Path) -> Value {
    let out = std::process::Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_format",
            "-show_streams",
            "-of",
            "json",
        ])
        .arg(file)
        .output()
        .expect("run ffprobe");
    assert!(
        out.status.success(),
        "ffprobe refused {}: {}",
        file.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("ffprobe JSON")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

#[tokio::test]
async fn m1_exit_2_an_agent_cuts_and_exports_three_corpus_clips() {
    if !cfg!(feature = "ffmpeg") {
        return skip(
            "cutweave is built without `--features ffmpeg`, so its engine cannot decode or export",
        );
    }
    let Some(corpus) = corpus_dir() else {
        return skip("no corpus (cargo xtask fetch-corpus --tier small)");
    };
    if let Some(missing) = CLIPS.iter().find(|c| !corpus.join(c).is_file()) {
        return skip(&format!("{missing} is not in {}", corpus.display()));
    }
    let Some(ffprobe) = ffprobe() else {
        return skip("no ffprobe (FFMPEG_DIR or the path)");
    };

    // The clips, the project and the export live in `work`, which outlives the session (its
    // own folder goes when it closes); the caches go to a folder of their own.
    let work = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let (work_dir, cache_dir) = (
        work.path().to_str().unwrap().to_owned(),
        cache.path().to_str().unwrap().to_owned(),
    );
    let session = Session::start_with(
        Transport::Stdio,
        Revision::Stateful,
        &[
            "--deterministic-ids",
            "42",
            "--allow-root",
            &work_dir,
            "--cache-dir",
            &cache_dir,
        ],
        Media::Real,
    )
    .await;
    let c = Checker::new(&session, None);
    let dir = work.path().to_path_buf();

    // The clips, copied into the engine's allowed folder (the UI test copies them too).
    let media_dir = dir.join("media");
    std::fs::create_dir_all(&media_dir).unwrap();
    let paths: Vec<String> = CLIPS
        .iter()
        .map(|name| {
            let target = media_dir.join(name);
            std::fs::copy(corpus.join(name), &target).unwrap();
            target.to_string_lossy().into_owned()
        })
        .collect();

    // A project; import the three clips and wait for their analysis.
    let created = c
        .execute(json!([{ "cmd": "project.new", "args": { "name": PROJECT } }]))
        .await;
    let project = created["steps"][0]["output"].clone();
    let (sequence, video) = (project["sequence"].clone(), project["video_track"].clone());
    let imported = c.tool("media_import", json!({ "paths": paths })).await;
    let media = imported["media_ids"].as_array().unwrap().clone();
    assert_eq!(media.len(), 3, "{imported}");
    for job in imported["job_ids"].as_array().unwrap() {
        let done = wait_job(&c, job, Duration::from_mins(5)).await;
        assert_eq!(done["state"], "succeeded", "analysis: {done}");
    }

    // Arrange and trim: A at 0-4 s, B at 4-7 s, C at 7-10 s, back to back on V1.
    let mut placed = Vec::new();
    for (m, span) in media.iter().zip(PLACED) {
        placed.push(place(&c, &sequence, &video, m, span).await);
    }
    // Split A at 2 s and move its right half to the end (10 s).
    let split = c
        .tool(
            "clip_split",
            json!({ "item": placed[0], "at": SPLIT_AT * SEC }),
        )
        .await;
    let right = split["right"].clone();
    c.tool(
        "item_move",
        json!({ "item": right, "start": MOVED_TO * SEC }),
    )
    .await;

    // The timeline: the layout the UI test expects, and the full structure (ids normalized).
    let timeline = c
        .tool("timeline_get", json!({ "sequence": sequence, "depth": 2 }))
        .await;
    assert_eq!(timeline["duration"], LENGTH * SEC, "{timeline}");
    assert_eq!(
        layout(&timeline),
        vec![
            ("V1".to_owned(), secs(&[(0, 2), (4, 3), (7, 3), (10, 2)])),
            // The HEVC clip has no sound.
            ("A1".to_owned(), secs(&[(0, 2), (7, 3), (10, 2)])),
        ],
        "{timeline}"
    );
    let normalized = normalize_ids(&timeline, &mut std::collections::HashMap::new());
    let fixture = fixture_path("m1_timeline.json");
    if std::env::var_os("CUTWEAVE_UPDATE_FIXTURES").is_some() {
        let pretty = serde_json::to_string_pretty(&normalized).unwrap();
        std::fs::write(&fixture, format!("{pretty}\n")).unwrap();
    }
    let expected: Value = serde_json::from_slice(
        &std::fs::read(&fixture).expect("fixture (CUTWEAVE_UPDATE_FIXTURES=1 writes it)"),
    )
    .unwrap();
    assert_eq!(
        normalized,
        expected,
        "the M1 timeline differs from {}",
        fixture.display()
    );

    // A frame of the HEVC clip decodes and renders: one image block.
    let frame = session
        .try_call("render_frame", json!({ "t": "5s", "max_size": 256 }))
        .await
        .expect("tools/call");
    assert_ne!(frame.is_error, Some(true), "render_frame: {frame:?}");
    let images = frame
        .content
        .iter()
        .filter(|b| matches!(b, ContentBlock::Image(_)))
        .count();
    assert_eq!(images, 1, "render_frame: {frame:?}");
    // The picture is the image block; the structured content describes it (no `data`).
    let frame_meta = frame.structured_content.clone().unwrap_or(Value::Null);
    assert_eq!(frame_meta["timecode"], "00:00:05:00", "{frame_meta}");
    assert_eq!(
        frame_meta["items_visible"][0]["item"], placed[1],
        "the HEVC clip is what shows at 5 s: {frame_meta}"
    );

    // Save, then export: the 1080p preset, software encoder, into the project folder (where the
    // UI's export dialog proposes it).
    let project_dir = dir.join(PROJECT);
    c.execute(json!([{ "cmd": "project.save_as", "args": { "path": project_dir } }]))
        .await;
    let file = project_dir.join(format!("{PROJECT}.mp4"));
    let started = c
        .tool("export_start", json!({ "path": file, "hw": "off" }))
        .await;
    assert_eq!(
        (&started["width"], &started["height"]),
        (&json!(1920), &json!(1080)),
        "{started}"
    );
    assert_eq!(
        started["frame_rate"],
        json!({ "num": 30, "den": 1 }),
        "{started}"
    );
    assert_eq!(started["frames"], LENGTH * 30, "{started}");
    let exported = wait_job(&c, &started["job_id"], Duration::from_mins(15)).await;
    assert_eq!(exported["state"], "succeeded", "export: {exported}");

    // The file: H.264 1920x1080 at 30 fps with AAC sound, the timeline's length within a frame.
    let size = std::fs::metadata(&file).expect("the export exists").len();
    assert!(size > 10_000, "{} is {size} bytes", file.display());
    let probed = probe(&ffprobe, &file);
    let streams = probed["streams"].as_array().unwrap();
    let video_stream = streams
        .iter()
        .find(|s| s["codec_type"] == "video")
        .unwrap_or_else(|| panic!("no video stream: {probed}"));
    assert_eq!(video_stream["codec_name"], "h264", "{probed}");
    assert_eq!(
        (&video_stream["width"], &video_stream["height"]),
        (&json!(1920), &json!(1080)),
        "{probed}"
    );
    assert_eq!(video_stream["r_frame_rate"], "30/1", "{probed}");
    let audio_stream = streams
        .iter()
        .find(|s| s["codec_type"] == "audio")
        .unwrap_or_else(|| panic!("no audio stream: {probed}"));
    assert_eq!(audio_stream["codec_name"], "aac", "{probed}");
    let duration: f64 = probed["format"]["duration"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        (duration - LENGTH_SECS).abs() <= 1.0 / 30.0,
        "the file lasts {duration} s, the timeline {LENGTH} s"
    );

    let (code, stderr) = session.close().await.unwrap();
    assert_eq!(code, Some(0), "{stderr}");

    // Under the gate: keep the project folder (with the export) and what the session saw.
    if let Some(out) = out_dir() {
        let mcp = out.join("mcp");
        if mcp.exists() {
            std::fs::remove_dir_all(&mcp).unwrap();
        }
        copy_dir(&project_dir, &mcp.join(PROJECT));
        let report = json!({
            "project": format!("{PROJECT}/{PROJECT}.cutweave"),
            "export": format!("{PROJECT}/{PROJECT}.mp4"),
            "timeline": timeline,
            "export_start": started,
            "job": exported,
            "ffprobe": probed,
        });
        std::fs::write(
            mcp.join("session.json"),
            serde_json::to_string_pretty(&report).unwrap(),
        )
        .unwrap();
        eprintln!(
            "M1 exit 2: kept the MCP project and export in {}",
            mcp.display()
        );
    }
}
