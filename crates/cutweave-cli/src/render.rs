// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave render --frames T1,T2,... --out-dir DIR`: still frames of a project as PNG files,
//! the golden-frame runner of WP-1O.01 (arch §8, §20).
//!
//! A one-shot engine with the full subsystems opens the project and every time is rendered by
//! the `render.frame` query (PNG, at the sequence size, through the engine's dispatcher like any
//! client), so the frames are what an agent or the preview sees. `CUTWEAVE_GPU` picks the adapter
//! (`software`: WARP on Windows, lavapipe on Linux).
//!
//! With `--check-determinism` every frame is also rendered
//!
//! 1. a second time by `render.frame`, in reverse order (warm caches, another decode order), and
//! 2. through the export pipeline's readback (`cutweave_engine::export::pipeline::render_frame`,
//!    which an export runs for every frame before encoding) on a compositor of its own on the
//!    same adapter, with the export's own sequential decoders,
//!
//! and the command fails unless all three PNGs are byte-identical (the "preview matches export"
//! guarantee and the same-adapter rule of the tolerance policy). The result then names the
//! adapter, whose key names the golden reference set (`AdapterInfo::key`). A GPU that could not
//! be opened (the engine fell back to another adapter or to the CPU) fails the check too.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cutweave_api::Credential;
use cutweave_engine::export::pipeline::render_frame as export_frame;
use cutweave_engine::export::{DecoderSources, ExportSources};
use cutweave_engine::{
    AudioConfig, Call, Connection, Engine, EngineConfig, EngineMode, FullConfig, FullHandle,
    FullSubsystems,
};
use cutweave_model::{Project, SequenceId, Time};
use cutweave_render::encode::encode_png;
use cutweave_render::gpu::{
    AdapterChoice, AdapterInfo, Compositor, CompositorOptions, GpuContext, GpuFrameBackend,
};
use serde_json::{Value, json};

use crate::cli::RenderCmd;
use crate::failure::Failure;

/// Most times one invocation renders.
const MAX_FRAMES: usize = 256;

/// The longest side `render.frame` accepts (it never renders above the sequence size).
const MAX_SIZE: u32 = cutweave_render::query::MAX_FRAME_SIZE;

/// One rendered time.
struct Shot {
    /// The time as given.
    spec: String,
    /// The resolved time.
    time: Time,
    /// `render.frame`'s output without the image.
    meta: Value,
    /// The PNG.
    png: Vec<u8>,
    /// The file it was written to.
    file: PathBuf,
}

/// `cutweave render --frames`.
///
/// # Errors
/// Usage errors (exit 2) for bad times or a missing project; the engine's error for a failing
/// `render.frame`; `nondeterministic` (exit 1) when `--check-determinism` finds a difference.
pub fn frames(cmd: &RenderCmd, frames: &str, out_dir: &Path) -> Result<Value, Failure> {
    let specs = parse_times(frames)?;
    if !cmd.project.exists() {
        return Err(Failure::usage(format!(
            "no project at {}",
            cmd.project.display()
        )));
    }
    std::fs::create_dir_all(out_dir)
        .map_err(|e| Failure::error("io", format!("cannot create {}: {e}", out_dir.display())))?;

    let mut media = FullConfig::native();
    if let Some(dir) = &cmd.cache_dir {
        media = media.with_cache_dir(dir);
    }
    let full = FullSubsystems::new(media);
    let handle = full.handle();
    let mut config = EngineConfig::new(EngineMode::Oneshot);
    config.project = Some(cmd.project.clone());
    config.deterministic_ids = cmd.deterministic_ids;
    config.allowed_roots.clone_from(&cmd.allow_root);
    config.audio = AudioConfig::Null;
    config.subsystems = Some(Box::new(full));
    let engine = Engine::start(config).map_err(|e| Failure::error(e.code(), e.to_string()))?;
    let result = render_all(cmd, &specs, out_dir, &engine, &handle);
    engine.shutdown();
    result
}

/// Everything after the engine started (so the caller always shuts it down).
fn render_all(
    cmd: &RenderCmd,
    specs: &[String],
    out_dir: &Path,
    engine: &Engine,
    handle: &FullHandle,
) -> Result<Value, Failure> {
    let conn = engine
        .handle()
        .connect(&Credential::Agent("cli".into()))
        .map_err(|e| Failure::from_cmd(&e))?;
    let shots = first_pass(&conn, cmd.sequence.as_deref(), specs, out_dir)?;
    let sequence = shots
        .first()
        .and_then(|s| s.meta.get("sequence"))
        .cloned()
        .unwrap_or(Value::Null);
    let (adapter, checks) = if cmd.check_determinism {
        let (info, report) = check(&conn, engine, &sequence, &shots)?;
        (adapter_json(&info), Some(report))
    } else {
        (Value::Null, None)
    };
    let degraded: Vec<Value> = handle
        .degraded()
        .into_iter()
        .map(|(subsystem, reason)| json!({"subsystem": subsystem, "reason": reason}))
        .collect();
    let gpu_lost = !handle.has_gpu() || !degraded.is_empty();
    let frames: Vec<Value> = shots
        .iter()
        .enumerate()
        .map(|(i, s)| frame_json(s, checks.as_ref().map(|r| r[i])))
        .collect();
    let first = shots.first().map(|s| &s.meta);
    let out = json!({
        "project": cmd.project.to_string_lossy(),
        "sequence": sequence,
        "width": first.and_then(|m| m.get("width")).cloned(),
        "height": first.and_then(|m| m.get("height")).cloned(),
        "adapter": adapter,
        "degraded": degraded,
        "frames": frames,
    });
    let Some(report) = &checks else {
        return Ok(out);
    };
    if gpu_lost {
        return Err(Failure {
            json: json!({
                "code": "degraded",
                "message": "the engine did not render on the requested GPU adapter (it fell \
                            back to another one or to the CPU), so the frames cannot be \
                            checked against that adapter",
                "result": out,
            }),
            exit: crate::failure::EXIT_COMMAND,
        });
    }
    let differing: Vec<String> = shots
        .iter()
        .zip(report)
        .filter(|(_, (repeat, export))| !(*repeat && *export))
        .map(|(s, (repeat, export))| {
            let mut why = Vec::new();
            if !repeat {
                why.push("second render differs");
            }
            if !export {
                why.push("export readback differs");
            }
            format!("{} ({})", s.spec, why.join(", "))
        })
        .collect();
    if differing.is_empty() {
        return Ok(out);
    }
    Err(Failure {
        json: json!({
            "code": "nondeterministic",
            "message": format!(
                "frames are not bit-identical on one adapter: {}; the variants are next to the \
                 frames as `<name>.repeat.png` / `<name>.export.png`",
                differing.join("; ")
            ),
            "result": out,
        }),
        exit: crate::failure::EXIT_COMMAND,
    })
}

/// Pass 1: every time through `render.frame`, written to `out_dir` as `f<frame>.png`.
fn first_pass(
    conn: &Connection,
    sequence: Option<&str>,
    specs: &[String],
    out_dir: &Path,
) -> Result<Vec<Shot>, Failure> {
    let mut shots = Vec::with_capacity(specs.len());
    let mut names = BTreeSet::new();
    for spec in specs {
        let (meta, png) = render_frame(conn, sequence, spec)?;
        let time = flicks_of(&meta)?;
        let name = match meta.get("frame").and_then(Value::as_u64) {
            Some(frame) => format!("f{frame:05}.png"),
            None => format!("t{}.png", time.flicks()),
        };
        if !names.insert(name.clone()) {
            return Err(Failure::usage(format!(
                "`{spec}` is the same frame as an earlier time ({name})"
            )));
        }
        let file = out_dir.join(&name);
        write(&file, &png)?;
        shots.push(Shot {
            spec: spec.clone(),
            time,
            meta,
            png,
            file,
        });
    }
    Ok(shots)
}

/// One frame of the result; `check` is `(second render identical, export readback identical)`.
fn frame_json(s: &Shot, check: Option<(bool, bool)>) -> Value {
    let count = |key: &str, reason: Option<&str>| {
        s.meta
            .get(key)
            .and_then(Value::as_array)
            .map_or(0, |items| {
                items
                    .iter()
                    .filter(|i| reason.is_none_or(|r| i["reason"] == r))
                    .count()
            })
    };
    let mut frame = json!({
        "t": s.spec,
        "time": s.time.flicks(),
        "frame": s.meta.get("frame").cloned().unwrap_or(Value::Null),
        "timecode": s.meta.get("timecode").cloned().unwrap_or(Value::Null),
        "file": s.file.to_string_lossy(),
        "bytes": s.png.len(),
        "visible": count("items_visible", None),
        "unavailable": count("items_hidden", Some("unavailable")),
        "notes": s.meta.get("notes").cloned().unwrap_or_else(|| json!([])),
    });
    if let (Some((repeat, export)), Value::Object(map)) = (check, &mut frame) {
        map.insert("repeat_identical".into(), json!(repeat));
        map.insert("export_identical".into(), json!(export));
    }
    frame
}

/// The second `render.frame` pass and the export readback of every shot: `(adapter, [(second
/// render identical, export readback identical)])`. Differing variants are written next to the
/// frame for inspection.
fn check(
    conn: &Connection,
    engine: &Engine,
    sequence: &Value,
    shots: &[Shot],
) -> Result<(AdapterInfo, Vec<(bool, bool)>), Failure> {
    let mut report = vec![(true, true); shots.len()];

    // Pass 2: `render.frame` again, last time first.
    let seq_arg = sequence.as_str();
    for (i, shot) in shots.iter().enumerate().rev() {
        let (_, png) = render_frame(conn, seq_arg, &shot.time.flicks().to_string())?;
        if png != shot.png {
            report[i].0 = false;
            write(&variant(&shot.file, "repeat"), &png)?;
        }
    }

    // Pass 3: the export pipeline's readback, on a compositor and decoders of its own.
    let doc: Project = engine
        .handle()
        .doc()
        .map_err(|e| Failure::from_cmd(&e))?
        .committed_snapshot()
        .doc
        .clone();
    let seq: SequenceId = serde_json::from_value(sequence.clone())
        .map_err(|e| Failure::error("internal", format!("render.frame named no sequence: {e}")))?;
    let media = FullConfig::native().media.ok_or_else(|| {
        Failure::error(
            "unsupported",
            "--check-determinism needs the media backend (a build with the `ffmpeg` feature)",
        )
    })?;
    let ctx = GpuContext::new(&AdapterChoice::from_env_or(AdapterChoice::Auto))
        .map_err(|e| Failure::error("gpu", format!("cannot open the GPU adapter: {e}")))?;
    let info = ctx.info().clone();
    let compositor = Compositor::new(ctx, CompositorOptions::default())
        .map_err(|e| Failure::error("gpu", format!("cannot create a compositor: {e}")))?;
    let options = compositor.options().graph;
    let source = DecoderSources::new(media).video(&doc);
    let backend = GpuFrameBackend::new(compositor, Arc::clone(&source)).shared();
    for (i, shot) in shots.iter().enumerate() {
        let size = (
            dimension(&shot.meta, "width")?,
            dimension(&shot.meta, "height")?,
        );
        let (image, _missing) =
            export_frame(&doc, seq, shot.time, size, &backend, &*source, &options)
                .map_err(|e| Failure::error("render_failed", e.to_string()))?;
        let mut png = Vec::new();
        encode_png(&image, &mut png)
            .map_err(|e| Failure::error("internal", format!("PNG encoding failed: {e}")))?;
        if png != shot.png {
            report[i].1 = false;
            write(&variant(&shot.file, "export"), &png)?;
        }
    }
    Ok((info, report))
}

/// `render.frame` at `t` as PNG at the sequence size: `(output without the image, PNG bytes)`.
fn render_frame(
    conn: &Connection,
    sequence: Option<&str>,
    t: &str,
) -> Result<(Value, Vec<u8>), Failure> {
    let t_arg = t
        .parse::<i64>()
        .map_or_else(|_| json!(t), |flicks| json!(flicks));
    let mut params = json!({"t": t_arg, "max_size": MAX_SIZE, "format": "png"});
    if let (Some(seq), Value::Object(map)) = (sequence, &mut params) {
        map.insert("sequence".into(), json!(seq));
    }
    let reply = conn
        .call(Call::new("render.frame", params))
        .map_err(|e| Failure::from_cmd(&e))?;
    let mut meta = reply.output;
    let data = meta
        .as_object_mut()
        .and_then(|m| m.remove("data"))
        .and_then(|d| d.as_str().map(str::to_owned))
        .ok_or_else(|| Failure::error("internal", "render.frame returned no image data"))?;
    let png = unbase64(&data)
        .ok_or_else(|| Failure::error("internal", "render.frame returned malformed base64"))?;
    Ok((meta, png))
}

/// The times of `--frames`, in order (1..=256 of them).
fn parse_times(frames: &str) -> Result<Vec<String>, Failure> {
    let specs: Vec<String> = frames
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    if specs.is_empty() || specs.len() > MAX_FRAMES {
        return Err(Failure::usage(format!(
            "--frames needs 1..={MAX_FRAMES} times separated by commas (`f:45`, `1.5s`, a \
             timecode or flicks)"
        )));
    }
    Ok(specs)
}

fn flicks_of(meta: &Value) -> Result<Time, Failure> {
    meta.get("time")
        .and_then(Value::as_i64)
        .and_then(|f| Time::from_flicks(f).ok())
        .ok_or_else(|| Failure::error("internal", "render.frame returned no time"))
}

fn dimension(meta: &Value, key: &str) -> Result<u32, Failure> {
    meta.get(key)
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| Failure::error("internal", format!("render.frame returned no {key}")))
}

fn adapter_json(info: &AdapterInfo) -> Value {
    json!({
        "key": info.key,
        "name": info.name,
        "backend": info.backend,
        "device_type": info.device_type,
        "driver": info.driver,
        "software": info.software,
    })
}

/// `f00045.png` → `f00045.<tag>.png`.
fn variant(file: &Path, tag: &str) -> PathBuf {
    let stem = file
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    file.with_file_name(format!("{stem}.{tag}.png"))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
    std::fs::write(path, bytes)
        .map_err(|e| Failure::error("io", format!("cannot write {}: {e}", path.display())))
}

/// Standard base64 (with padding) to bytes; `None` for anything else.
fn unbase64(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes = s.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let pad = chunk.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 {
            return None;
        }
        let mut n = 0u32;
        for &c in &chunk[..4 - pad] {
            n = (n << 6) | val(c)?;
        }
        n <<= 6 * u32::try_from(pad).ok()?;
        out.extend_from_slice(&n.to_be_bytes()[1..4 - pad]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_the_render_encoder() {
        for len in 0..40usize {
            let data: Vec<u8> = (0..len)
                .map(|i| u8::try_from((i * 37 + 11) % 256).unwrap_or(0))
                .collect();
            let text = cutweave_render::encode::base64(&data);
            assert_eq!(unbase64(&text).as_deref(), Some(&data[..]), "length {len}");
        }
        assert_eq!(unbase64("abc"), None);
        assert_eq!(unbase64("a$cd"), None);
    }

    #[test]
    fn times_are_split_on_commas() {
        assert_eq!(
            parse_times(" f:0, 1.5s ,,00:00:02:00").unwrap(),
            ["f:0", "1.5s", "00:00:02:00"]
        );
        assert!(parse_times(" , ").is_err());
    }

    #[test]
    fn variants_sit_next_to_the_frame() {
        assert_eq!(
            variant(Path::new("out/f00045.png"), "export"),
            Path::new("out/f00045.export.png")
        );
    }
}
