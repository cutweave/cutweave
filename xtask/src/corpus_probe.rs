// SPDX-License-Identifier: GPL-3.0-or-later
//! Probe a corpus file with `ffprobe` and compare it with the item's `expect` table in
//! `testdata/corpus.toml` (issue #108). A generator recipe that silently produces the wrong media
//! (constant frame rate instead of VFR, a missing rotation, one frame size instead of three) fails here.

use crate::corpus::{Expect, Item};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

/// True when `ffprobe` can be run.
pub fn have_ffprobe() -> bool {
    Command::new("ffprobe")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn ffprobe(args: &[&str], file: &Path) -> Result<std::process::Output, String> {
    Command::new("ffprobe")
        .args(["-v", "error"])
        .args(args)
        .arg(file)
        .output()
        .map_err(|e| format!("cannot run ffprobe: {e}"))
}

/// `"30000/1001"` to a number; `"0/0"` is None.
fn rate(s: &str) -> Option<f64> {
    let (n, d) = s.split_once('/')?;
    let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
    (d != 0.0 && n != 0.0).then(|| n / d)
}

fn csv_numbers(out: &[u8]) -> Vec<Vec<f64>> {
    String::from_utf8_lossy(out)
        .lines()
        .map(|l| {
            l.split(',')
                .filter(|f| !f.is_empty())
                .filter_map(|f| f.trim().parse().ok())
                .collect::<Vec<f64>>()
        })
        .filter(|v: &Vec<f64>| !v.is_empty())
        .collect()
}

/// True when the video packet presentation times are not evenly spaced.
fn is_vfr(file: &Path) -> Result<bool, String> {
    let out = ffprobe(
        &[
            "-select_streams",
            "v:0",
            "-show_entries",
            "packet=pts_time",
            "-of",
            "csv=p=0",
        ],
        file,
    )?;
    let mut pts: Vec<f64> = csv_numbers(&out.stdout).into_iter().map(|v| v[0]).collect();
    pts.sort_by(f64::total_cmp);
    let deltas: Vec<f64> = pts.windows(2).map(|w| w[1] - w[0]).collect();
    if deltas.is_empty() {
        return Err("fewer than two video packets, cannot tell CFR from VFR".into());
    }
    let (min, max) = deltas
        .iter()
        .fold((f64::MAX, f64::MIN), |(lo, hi), d| (lo.min(*d), hi.max(*d)));
    // Timebase rounding moves a CFR delta by well under a millisecond; VFR gaps differ by a frame.
    Ok(max - min > 0.002)
}

/// Distinct decoded frame sizes of the first video stream, as `WxH`, in order of first appearance.
fn frame_sizes(file: &Path) -> Result<Vec<String>, String> {
    let out = ffprobe(
        &[
            "-select_streams",
            "v:0",
            "-show_entries",
            "frame=width,height",
            "-of",
            "csv=p=0",
        ],
        file,
    )?;
    let mut seen: Vec<String> = Vec::new();
    for v in csv_numbers(&out.stdout) {
        if v.len() >= 2 {
            let s = format!("{}x{}", v[0], v[1]);
            if !seen.contains(&s) {
                seen.push(s);
            }
        }
    }
    Ok(seen)
}

fn stream_of<'a>(streams: &'a [Value], kind: &str) -> Option<&'a Value> {
    streams.iter().find(|s| s["codec_type"] == kind)
}

fn str_of<'a>(s: &'a Value, key: &str) -> &'a str {
    s[key].as_str().unwrap_or("")
}

/// Compare `file` with the item's expectations. Returns every mismatch, or an empty list.
#[allow(clippy::too_many_lines, clippy::cast_possible_truncation)] // one flat list of independent checks
pub fn check(item: &Item, exp: &Expect, file: &Path) -> Result<Vec<String>, String> {
    let mut bad = Vec::new();
    let out = ffprobe(&["-show_format", "-show_streams", "-of", "json"], file)?;
    if exp.broken {
        // A truncated container must not probe cleanly: no streams, or ffprobe errors out.
        let streams = serde_json::from_slice::<Value>(&out.stdout)
            .ok()
            .and_then(|v| v["streams"].as_array().map(Vec::len))
            .unwrap_or(0);
        if out.status.success() && streams > 0 {
            bad.push("expected a file that does not probe, but ffprobe read it".into());
        }
        return Ok(bad);
    }
    if !out.status.success() {
        return Ok(vec![format!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )]);
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("ffprobe json: {e}"))?;
    let streams = v["streams"].as_array().cloned().unwrap_or_default();

    if !streams
        .iter()
        .any(|s| str_of(s, "codec_name") == item.codec)
    {
        let names: Vec<&str> = streams.iter().map(|s| str_of(s, "codec_name")).collect();
        bad.push(format!("codec: expected {}, found {names:?}", item.codec));
    }
    let video = stream_of(&streams, "video");
    let audio = stream_of(&streams, "audio");

    if exp.audio.is_some_and(|want| want != audio.is_some()) {
        bad.push(format!(
            "audio stream: expected {:?}, found {}",
            exp.audio,
            audio.is_some()
        ));
    }
    if let Some(a) = audio {
        if let Some(want) = exp.channels {
            let got = a["channels"].as_u64().unwrap_or(0);
            if got != u64::from(want) {
                bad.push(format!("channels: expected {want}, found {got}"));
            }
        }
        if let Some(want) = exp.sample_rate {
            let got: u64 = str_of(a, "sample_rate").parse().unwrap_or(0);
            if got != u64::from(want) {
                bad.push(format!("sample_rate: expected {want}, found {got}"));
            }
        }
    } else if exp.channels.is_some() || exp.sample_rate.is_some() {
        bad.push("channels/sample_rate expected but there is no audio stream".into());
    }

    let video_wanted = exp.width.is_some()
        || exp.height.is_some()
        || exp.fps.is_some()
        || exp.vfr.is_some()
        || exp.rotation.is_some()
        || exp.pix_fmt.is_some()
        || exp.color_transfer.is_some()
        || exp.frame_sizes.is_some();
    if let Some(vs) = video {
        if let Some(want) = exp.width {
            let got = vs["width"].as_u64().unwrap_or(0);
            if got != u64::from(want) {
                bad.push(format!("width: expected {want}, found {got}"));
            }
        }
        if let Some(want) = exp.height {
            let got = vs["height"].as_u64().unwrap_or(0);
            if got != u64::from(want) {
                bad.push(format!("height: expected {want}, found {got}"));
            }
        }
        if let Some(want) = exp.fps {
            let got = rate(str_of(vs, "r_frame_rate")).unwrap_or(0.0);
            if (got - want).abs() > want * 0.001 {
                bad.push(format!("fps: expected {want}, found {got}"));
            }
        }
        if let Some(want) = exp.pix_fmt.as_deref() {
            let got = str_of(vs, "pix_fmt");
            if got != want {
                bad.push(format!("pix_fmt: expected {want}, found {got}"));
            }
        }
        if let Some(want) = exp.color_transfer.as_deref() {
            let got = str_of(vs, "color_transfer");
            if got != want {
                bad.push(format!("color_transfer: expected {want}, found {got}"));
            }
        }
        if let Some(want) = exp.rotation {
            // ffprobe's `rotation` is counter-clockwise; the manifest and the engine use clockwise.
            let ccw = vs["side_data_list"]
                .as_array()
                .into_iter()
                .flatten()
                .find_map(|sd| sd["rotation"].as_f64())
                .unwrap_or(0.0);
            let got = (360 - (ccw.round() as i64).rem_euclid(360)).rem_euclid(360);
            if got != i64::from(want) {
                bad.push(format!(
                    "rotation (clockwise): expected {want}, found {got}"
                ));
            }
        }
        if let Some(want) = exp.vfr {
            match is_vfr(file) {
                Ok(got) if got != want => bad.push(format!(
                    "frame-rate mode: expected {}, found {}",
                    if want { "VFR" } else { "CFR" },
                    if got { "VFR" } else { "CFR" }
                )),
                Ok(_) => {}
                Err(e) => bad.push(e),
            }
        }
        if let Some(want) = &exp.frame_sizes {
            let got = frame_sizes(file)?;
            if &got != want {
                bad.push(format!(
                    "frame sizes in order: expected {want:?}, found {got:?}"
                ));
            }
        }
    } else if video_wanted {
        bad.push("video properties expected but there is no video stream".into());
    }

    if let Some(want) = exp.duration {
        let got: f64 = str_of(&v["format"], "duration").parse().unwrap_or(0.0);
        if (got - want).abs() > exp.duration_tolerance.unwrap_or(0.25) {
            bad.push(format!("duration: expected {want} s, found {got:.3} s"));
        }
    }
    Ok(bad)
}
