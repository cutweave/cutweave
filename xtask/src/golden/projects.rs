// SPDX-License-Identifier: GPL-3.0-or-later
//! The golden projects (WP-1O.01, WP-2C.04): synthetic sources and the nine `.cutweave` projects
//! built from them with real commands (`cargo xtask golden-projects`).
//!
//! **Sources** are FFmpeg `lavfi` test patterns (project-generated, no licensing question),
//! encoded losslessly (H.264 `-qp 0`, 4:2:0, so the decoded pictures are exactly the
//! generator's) at 640×360, 30 fps, 4 s, with explicit colour tags. They are generated into
//! `target/golden/sources/` (again whenever their recipe or the FFmpeg version changes), never
//! into `testdata/corpus`, and never committed.
//!
//! **Projects** are built by a `cutweave mcp --headless` engine (one process, the real FFmpeg
//! probe for `media.import`) through `cutweave_execute`: `project.new`, `media.import`,
//! `timeline.insert`, `track.add`, `item.set_props`, `keyframe.set`, `clip.split`, `item.delete`,
//! `project.save_as`. So they are schema-valid by construction and follow every change to the
//! commands. They land in `target/golden/projects/<name>/<name>.cutweave` with seeded ids
//! (`--deterministic-ids`) and are rebuilt on every run, because a saved project records the
//! absolute path and content hash of its media, which differ between checkouts.
//!
//! Each project is rendered at the three frames of [`Golden::frames`]. [`NEGATIVE`] is
//! `scaled_offset` moved one pixel to the right: its frames must fail against `scaled_offset`'s
//! references, or the comparator is blind (the negative test, run on every `cargo xtask golden`).

use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

/// Sequence and source width.
pub const WIDTH: u32 = 640;
/// Sequence and source height.
pub const HEIGHT: u32 = 360;
/// Sequence and source frame rate.
pub const FPS: u32 = 30;

/// A synthetic source: a lavfi graph, optionally with a display-matrix rotation.
pub struct Source {
    /// File stem (`<name>.mp4`).
    pub name: &'static str,
    /// The lavfi graph (ends with the colour tags, applied without converting pixels).
    pub graph: &'static str,
    /// Counter-clockwise display rotation in degrees (phone footage), stored as metadata.
    pub rotation: Option<i32>,
}

/// BT.709 limited range, the tags of most sources.
macro_rules! bt709_tv {
    ($g:literal) => {
        concat!(
            $g,
            ",format=yuv420p,setparams=range=tv:colorspace=bt709:color_primaries=bt709:color_trc=bt709"
        )
    };
}

/// The sources (640×360, 30 fps, 4 s, H.264 lossless 4:2:0).
pub const SOURCES: &[Source] = &[
    Source {
        name: "bars",
        graph: bt709_tv!("smptehdbars=size=640x360:rate=30:duration=4"),
        rotation: None,
    },
    Source {
        name: "testsrc",
        graph: bt709_tv!("testsrc2=size=640x360:rate=30:duration=4"),
        rotation: None,
    },
    Source {
        // Landscape-coded, shown portrait: a display matrix of 90°, like phone footage.
        name: "phone",
        graph: bt709_tv!("testsrc2=size=640x360:rate=30:duration=4"),
        rotation: Some(90),
    },
    Source {
        // The same bars tagged BT.601 (SMPTE 170M) limited range.
        name: "bars_bt601",
        graph: "smptebars=size=640x360:rate=30:duration=4,format=yuv420p,\
                setparams=range=tv:colorspace=smpte170m:color_primaries=smpte170m:color_trc=smpte170m",
        rotation: None,
    },
    Source {
        // The same samples tagged BT.709 full range.
        name: "bars_full",
        graph: "smptebars=size=640x360:rate=30:duration=4,format=yuv420p,\
                setparams=range=pc:colorspace=bt709:color_primaries=bt709:color_trc=bt709",
        rotation: None,
    },
];

/// A golden project.
#[derive(Clone, Copy, Debug)]
pub struct Golden {
    /// Folder and project name.
    pub name: &'static str,
    /// What it covers.
    pub about: &'static str,
    /// The frames rendered (at [`FPS`]).
    pub frames: [u32; 3],
    /// The sources it imports, in order.
    pub sources: &'static [&'static str],
}

/// The golden projects: the six of WP-1O.01, two of WP-2C.04 (flip with a display rotation;
/// a keyframed screen-mode layer) and one of WP-2C.08 (a layout), 27 frames.
pub const PROJECTS: [Golden; 9] = [
    Golden {
        name: "bars_100",
        about: "one SMPTE HD colour-bars clip at 100% (fills the canvas)",
        frames: [0, 60, 119],
        sources: &["bars"],
    },
    Golden {
        name: "scaled_offset",
        about: "a moving test pattern at 50% scale, moved 96 px left and 54 px down",
        frames: [0, 45, 110],
        sources: &["testsrc"],
    },
    Golden {
        name: "opacity_over_track",
        about: "the test pattern at 50% opacity on V2 over colour bars on V1",
        frames: [0, 45, 110],
        sources: &["bars", "testsrc"],
    },
    Golden {
        name: "rotated_phone",
        about: "a clip with a 90° display-matrix rotation (portrait), fitted into the canvas",
        frames: [0, 45, 110],
        sources: &["phone"],
    },
    Golden {
        name: "color_tags",
        about: "BT.601 limited-range bars (left) beside the same samples tagged BT.709 full range \
                (right), each at 50%",
        frames: [0, 45, 110],
        sources: &["bars_bt601", "bars_full"],
    },
    Golden {
        name: "cut_at_45",
        about: "the test pattern split at frame 45, its right half replaced by colour bars: \
                frames 44 and 45 sit on either side of the cut",
        frames: [20, 44, 45],
        sources: &["testsrc", "bars"],
    },
    Golden {
        name: "flipped_phone",
        about: "the 90° display-rotated phone clip with 10% cropped from the left of the upright \
                picture, flipped left-right, then turned another 90° clockwise by its transform",
        frames: [0, 45, 110],
        sources: &["phone"],
    },
    Golden {
        name: "screen_keyframed",
        about: "the test pattern at half size in screen mode over colour bars, its position and \
                rotation keyframed from left to right over the 4 s",
        frames: [0, 45, 110],
        sources: &["bars", "testsrc"],
    },
    Golden {
        name: "side_by_side",
        about: "two colour-bars clips arranged by item.apply_layout (side by side, fill with a \
                centred crop): each half of the frame is filled, with no bars",
        frames: [0, 60, 119],
        sources: &["bars"],
    },
];

/// The negative test: `scaled_offset` one pixel to the right.
pub const NEGATIVE: Golden = Golden {
    name: "scaled_offset_nudged",
    about: "scaled_offset moved 1 px to the right: must fail against scaled_offset's references",
    frames: PROJECTS[1].frames,
    sources: &["testsrc"],
};

/// The project whose references [`NEGATIVE`] is compared with.
pub const NEGATIVE_AGAINST: &str = "scaled_offset";

/// The `ffmpeg` of the FFmpeg bundle (`$FFMPEG_DIR/bin`), else the one on `PATH`.
pub fn ffmpeg() -> PathBuf {
    if let Ok(dir) = std::env::var("FFMPEG_DIR") {
        let exe = Path::new(&dir)
            .join("bin")
            .join(format!("ffmpeg{}", std::env::consts::EXE_SUFFIX));
        if exe.is_file() {
            return exe;
        }
    }
    PathBuf::from("ffmpeg")
}

/// The FFmpeg version line (`ffmpeg version n8.1.3-...`), for the update log.
pub fn ffmpeg_version() -> String {
    Command::new(ffmpeg())
        .args(["-hide_banner", "-version"])
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .map(|l| l.split_whitespace().nth(2).unwrap_or("unknown").to_owned())
        })
        .unwrap_or_else(|| "unknown".into())
}

fn run_ffmpeg(args: &[&str]) -> Result<(), String> {
    let out = Command::new(ffmpeg())
        .args(["-v", "error", "-nostdin", "-y"])
        .args(args)
        .output()
        .map_err(|e| {
            format!(
                "cannot run {} ({e}); source scripts/ffmpeg/env.sh (`just golden` does)",
                ffmpeg().display()
            )
        })?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "ffmpeg {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Generates the [`SOURCES`] files into `dir`. A file whose recipe (graph, rotation and FFmpeg
/// version, kept in `<name>.recipe`) is unchanged is reused.
pub fn generate_sources(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let version = ffmpeg_version();
    for s in SOURCES {
        let started = std::time::Instant::now();
        let out = dir.join(format!("{}.mp4", s.name));
        let stamp = dir.join(format!("{}.recipe", s.name));
        let recipe = format!("{}\nrotation {:?}\nffmpeg {version}\n", s.graph, s.rotation);
        if out.is_file() && std::fs::read_to_string(&stamp).is_ok_and(|r| r == recipe) {
            continue;
        }
        let _ = std::fs::remove_file(&stamp);
        let encoded = if s.rotation.is_some() {
            dir.join(format!("{}.unrotated.mp4", s.name))
        } else {
            out.clone()
        };
        let encoded_arg = encoded.to_string_lossy().into_owned();
        run_ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            s.graph,
            "-c:v",
            "libx264",
            "-qp",
            "0",
            "-preset",
            "ultrafast",
            "-threads",
            "1",
            "-an",
            "-fflags",
            "+bitexact",
            "-flags:v",
            "+bitexact",
            &encoded_arg,
        ])?;
        if let Some(deg) = s.rotation {
            // A display matrix survives only a stream copy (FFmpeg rotates pixels when it
            // re-encodes a rotated input).
            let deg = deg.to_string();
            let out_arg = out.to_string_lossy().into_owned();
            run_ffmpeg(&[
                "-display_rotation:v:0",
                &deg,
                "-i",
                &encoded_arg,
                "-c",
                "copy",
                "-fflags",
                "+bitexact",
                &out_arg,
            ])?;
            let _ = std::fs::remove_file(&encoded);
        }
        std::fs::write(&stamp, recipe).map_err(|e| format!("{}: {e}", stamp.display()))?;
        println!(
            "  source   {:<22} ({:.1} s)",
            format!("{}.mp4", s.name),
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

/// The ids `project.new` returned.
struct NewProject {
    sequence: Value,
    video_track: Value,
}

/// The timeline steps of `project` (one `cutweave_execute` batch, one transaction): `media`
/// holds the imported media ids in [`Golden::sources`] order.
fn timeline_steps(project: &Golden, ids: &NewProject, media: &[Value]) -> Value {
    let (seq, v1) = (&ids.sequence, &ids.video_track);
    let insert = |track: Value, media: &Value, at: Value| {
        json!({"cmd": "timeline.insert", "args": {
            "sequence": seq, "track": track, "media": media, "at": at, "link": false
        }})
    };
    let at_0 = json!(0);
    let props = |step: usize, props: Value| {
        json!({"cmd": "item.set_props", "args": {
            "item": {"$ref": format!("steps[{step}].output.clips[0]")}, "props": props
        }})
    };
    let new_track = json!({"cmd": "track.add", "args": {"sequence": seq, "kind": "video"}});
    let track_of = |step: usize| json!({"$ref": format!("steps[{step}].output.track")});
    // Half size, the anchor `(x, y)` pixels from the canvas centre. Positions are in canvas
    // units (1.0 = the canvas width horizontally, its height vertically).
    let half_at = |x: f64, y: f64| {
        json!({"transform": {
            "scale": {"x": 0.5, "y": 0.5},
            "position": {"x": x / f64::from(WIDTH), "y": y / f64::from(HEIGHT)}
        }})
    };
    match project.name {
        "bars_100" | "rotated_phone" => json!([insert(v1.clone(), &media[0], at_0)]),
        "scaled_offset" => json!([
            insert(v1.clone(), &media[0], at_0),
            props(0, half_at(-96.0, 54.0)),
        ]),
        "scaled_offset_nudged" => json!([
            insert(v1.clone(), &media[0], at_0),
            props(0, half_at(-95.0, 54.0)),
        ]),
        "opacity_over_track" => json!([
            insert(v1.clone(), &media[0], at_0.clone()),
            new_track,
            insert(track_of(1), &media[1], at_0),
            props(2, json!({"opacity": 0.5})),
        ]),
        "color_tags" => json!([
            insert(v1.clone(), &media[0], at_0.clone()),
            props(0, half_at(-160.0, 0.0)),
            new_track,
            insert(track_of(2), &media[1], at_0),
            props(3, half_at(160.0, 0.0)),
        ]),
        "cut_at_45" => json!([
            insert(v1.clone(), &media[0], at_0),
            {"cmd": "clip.split", "args": {
                "item": {"$ref": "steps[0].output.clips[0]"}, "at": "f:45"
            }},
            {"cmd": "item.delete", "args": {"items": [{"$ref": "steps[1].output.right"}]}},
            {"cmd": "timeline.insert", "args": {
                "sequence": seq, "track": v1, "media": &media[1], "at": "f:45",
                "duration": "f:75", "link": false
            }},
        ]),
        "flipped_phone" => json!([
            insert(v1.clone(), &media[0], at_0),
            props(
                0,
                json!({
                    "crop": {"left": 0.1},
                    "flip": {"horizontal": true},
                    "transform": {"rotation": 90.0}
                })
            ),
        ]),
        "screen_keyframed" => json!([
            insert(v1.clone(), &media[0], at_0.clone()),
            new_track,
            insert(track_of(1), &media[1], at_0),
            props(2, json!({
                "blend_mode": "screen",
                "transform": {"scale": {"x": 0.5, "y": 0.5}}
            })),
            {"cmd": "keyframe.set", "args": {
                "item": {"$ref": "steps[2].output.clips[0]"}, "prop": "transform.position",
                "keys": [
                    {"at": "f:0", "value": {"x": -0.25, "y": 0.1}},
                    {"at": "f:119", "value": {"x": 0.25, "y": -0.1}}
                ]
            }},
            {"cmd": "keyframe.set", "args": {
                "item": {"$ref": "steps[2].output.clips[0]"}, "prop": "transform.rotation",
                "keys": [{"at": "f:0", "value": -20.0}, {"at": "f:119", "value": 25.0}]
            }},
        ]),
        "side_by_side" => json!([
            insert(v1.clone(), &media[0], at_0.clone()),
            new_track,
            insert(track_of(1), &media[0], at_0),
            {"cmd": "item.apply_layout", "args": {
                "items": [
                    {"$ref": "steps[0].output.clips[0]"},
                    {"$ref": "steps[2].output.clips[0]"}
                ],
                "layout": "side_by_side", "fill": "crop"
            }},
        ]),
        other => json!([{"cmd": "no.such.golden.project", "args": {"name": other}}]),
    }
}

/// A `cutweave mcp --headless` session over stdio (raw JSON-RPC lines).
struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Mcp {
    fn start(cutweave: &Path, root: &Path, seed: u64) -> Result<Mcp, String> {
        let mut child = Command::new(cutweave)
            .args(["mcp", "--headless", "--no-log", "--deterministic-ids"])
            .arg(seed.to_string())
            .arg("--allow-root")
            .arg(root)
            // The analysis jobs' caches stay out of the person's cache folder.
            .arg("--cache-dir")
            .arg(root.join("cache").join("build"))
            // Building projects draws nothing that matters: keep it off the hardware GPUs.
            .env("CUTWEAVE_GPU", "software")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", cutweave.display()))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().map(BufReader::new).ok_or("no stdout")?;
        let mut mcp = Mcp {
            child,
            stdin,
            stdout,
            next_id: 1,
        };
        let init = mcp.exchange(&json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25", "capabilities": {},
                "clientInfo": {"name": "xtask-golden", "version": "1"}
            }
        }))?;
        if init.get("result").is_none() {
            return Err(format!("MCP initialize failed: {init}"));
        }
        mcp.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        Ok(mcp)
    }

    fn send(&mut self, frame: &Value) -> Result<(), String> {
        let stdin = self.stdin.as_mut().ok_or("MCP stdin closed")?;
        writeln!(stdin, "{frame}")
            .and_then(|()| stdin.flush())
            .map_err(|e| format!("MCP write: {e}"))
    }

    fn exchange(&mut self, request: &Value) -> Result<Value, String> {
        self.send(request)?;
        loop {
            let mut line = String::new();
            let n = self
                .stdout
                .read_line(&mut line)
                .map_err(|e| format!("MCP read: {e}"))?;
            if n == 0 {
                return Err("the MCP engine closed its stdout".into());
            }
            let frame: Value = serde_json::from_str(line.trim_end())
                .map_err(|e| format!("not a JSON-RPC line ({e}): {line:?}"))?;
            if frame.get("id") == request.get("id") {
                return Ok(frame);
            }
        }
    }

    /// `cutweave_execute{steps}`: the outputs of the steps.
    fn execute(&mut self, steps: &Value) -> Result<Vec<Value>, String> {
        let id = self.next_id;
        self.next_id += 1;
        let reply = self.exchange(&json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": "cutweave_execute", "arguments": {"steps": steps}}
        }))?;
        let result = reply.get("result").cloned().unwrap_or(Value::Null);
        if reply.get("error").is_some() || result["isError"] == true {
            return Err(format!("cutweave_execute {steps}: {reply}"));
        }
        Ok(result["structuredContent"]["steps"]
            .as_array()
            .map_or_else(Vec::new, |steps| {
                steps.iter().map(|s| s["output"].clone()).collect()
            }))
    }

    /// One command, alone.
    fn one(&mut self, cmd: &str, args: &Value) -> Result<Value, String> {
        self.execute(&json!([{"cmd": cmd, "args": args}]))?
            .into_iter()
            .next()
            .ok_or_else(|| format!("{cmd} returned nothing"))
    }

    /// Closes stdin (the session ends) and waits for a clean exit.
    fn close(mut self) -> Result<(), String> {
        drop(self.stdin.take());
        let status = self.child.wait().map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("the MCP engine exited with {status}"))
        }
    }
}

/// A project on disk.
#[derive(Clone, Debug)]
pub struct Built {
    /// The recipe.
    pub golden: Golden,
    /// Its `.cutweave` file.
    pub document: PathBuf,
}

/// Builds [`PROJECTS`] and [`NEGATIVE`] into `work/projects/` from the sources in
/// `work/sources/` with `cutweave` (an FFmpeg build).
pub fn build_projects(cutweave: &Path, work: &Path) -> Result<Vec<Built>, String> {
    let dir = work.join("projects");
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let sources = work.join("sources");
    let started = std::time::Instant::now();
    let mut mcp = Mcp::start(cutweave, work, 1)?;
    println!(
        "  built    {:<22} ({:.1} s)",
        "(MCP engine started)",
        started.elapsed().as_secs_f64()
    );
    let mut built = Vec::new();
    for golden in PROJECTS.iter().chain([&NEGATIVE]) {
        let started = std::time::Instant::now();
        let result = build_one(&mut mcp, golden, &sources, &dir);
        match result {
            Ok(document) => {
                println!(
                    "  built    {:<22} ({:.1} s)",
                    golden.name,
                    started.elapsed().as_secs_f64()
                );
                built.push(Built {
                    golden: *golden,
                    document,
                });
            }
            Err(e) => {
                let _ = mcp.close();
                return Err(format!("golden project `{}`: {e}", golden.name));
            }
        }
    }
    let started = std::time::Instant::now();
    mcp.close()?;
    println!(
        "  built    {:<22} ({:.1} s)",
        "(MCP engine stopped)",
        started.elapsed().as_secs_f64()
    );
    Ok(built)
}

fn build_one(
    mcp: &mut Mcp,
    golden: &Golden,
    sources: &Path,
    dir: &Path,
) -> Result<PathBuf, String> {
    let created = mcp.one(
        "project.new",
        &json!({"name": golden.name, "format": {
            "width": WIDTH, "height": HEIGHT, "frame_rate": {"num": FPS, "den": 1}
        }}),
    )?;
    let ids = NewProject {
        sequence: created["sequence"].clone(),
        video_track: created["video_track"].clone(),
    };
    let paths: Vec<String> = golden
        .sources
        .iter()
        .map(|s| {
            sources
                .join(format!("{s}.mp4"))
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let imported = mcp.one("media.import", &json!({"paths": paths}))?;
    if imported["failed"].as_array().is_some_and(|f| !f.is_empty()) {
        return Err(format!("media.import failed: {}", imported["failed"]));
    }
    // Analysis jobs (index, thumbnails, waveform) finish before anything else runs.
    for job in imported["job_ids"].as_array().cloned().unwrap_or_default() {
        mcp.one("job.wait", &json!({"job_id": job, "timeout_ms": 60_000}))?;
    }
    let media: Vec<Value> = imported["media_ids"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if media.len() != golden.sources.len() {
        return Err(format!("media.import returned {media:?}"));
    }
    mcp.execute(&timeline_steps(golden, &ids, &media))?;
    let folder = dir.join(golden.name);
    let saved = mcp.one(
        "project.save_as",
        &json!({"path": folder.to_string_lossy()}),
    )?;
    mcp.one("project.close", &json!({}))?;
    saved["path"]
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(|| format!("project.save_as returned {saved}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nine_projects_and_twenty_seven_frames() {
        assert_eq!(PROJECTS.len(), 9);
        let frames: usize = PROJECTS.iter().map(|p| p.frames.len()).sum();
        assert_eq!(frames, 27);
        let mut names: Vec<_> = PROJECTS.iter().map(|p| p.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 9);
    }

    #[test]
    fn every_project_has_steps_and_known_sources() {
        let ids = NewProject {
            sequence: json!("seq_x"),
            video_track: json!("trk_v1"),
        };
        for p in PROJECTS.iter().chain([&NEGATIVE]) {
            for s in p.sources {
                assert!(SOURCES.iter().any(|src| src.name == *s), "{}: {s}", p.name);
            }
            let media: Vec<Value> = p.sources.iter().map(|s| json!(s)).collect();
            let steps = timeline_steps(p, &ids, &media);
            assert!(
                !steps.to_string().contains("no.such.golden.project"),
                "{}",
                p.name
            );
            // Frames lie inside the 4 s sources.
            assert!(p.frames.iter().all(|f| *f < 4 * FPS), "{}", p.name);
        }
    }

    #[test]
    fn the_negative_test_moves_scaled_offset_by_one_pixel() {
        assert_eq!(NEGATIVE.frames, PROJECTS[1].frames);
        assert_eq!(PROJECTS[1].name, NEGATIVE_AGAINST);
        let ids = NewProject {
            sequence: json!("seq_x"),
            video_track: json!("trk_v1"),
        };
        let x = |p: &Golden| {
            timeline_steps(p, &ids, &[json!("m")])[1]["args"]["props"]["transform"]["position"]["x"]
                .as_f64()
                .unwrap()
        };
        // One pixel of the 640 px canvas, in canvas units.
        let pixel = 1.0 / f64::from(WIDTH);
        assert!((x(&NEGATIVE) - x(&PROJECTS[1]) - pixel).abs() < 1e-12);
    }
}
