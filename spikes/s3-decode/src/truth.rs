// SPDX-License-Identifier: GPL-3.0-or-later
//! Ground truth from the FFmpeg command-line tools of the same bundle (arch §20: frame-exact
//! seek against ffprobe truth):
//!
//! - `ffprobe -show_packets`: the packet list the index must reproduce;
//! - `ffprobe -show_frames`: the presentation-order pts list every decoded frame must match;
//! - `ffmpeg -vf extractplanes=y -f framehash -hash crc32`: one CRC of the Y plane per frame, the
//!   one-pass equivalent of `-vf select=eq(n\,N)` for every N.
//!
//! Results are cached in `s3-out/truth/` keyed by file name, size and mtime.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};

/// One packet as ffprobe reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TruthPacket {
    pub pts: Option<i64>,
    pub dts: Option<i64>,
    pub key: bool,
    pub discard: bool,
    pub pos: Option<i64>,
}

/// One decoded frame as ffprobe reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TruthFrame {
    pub pts: Option<i64>,
    pub best_effort: Option<i64>,
    pub key: bool,
    pub pict_type: String,
}

impl TruthFrame {
    /// The timestamp a frame is addressed by (pts, else best effort).
    pub fn ts(&self) -> Option<i64> {
        self.pts.or(self.best_effort)
    }
}

/// All ground truth for one stream of one file.
pub struct Truth {
    pub packets: Vec<TruthPacket>,
    pub frames: Vec<TruthFrame>,
    /// (pts, Y-plane CRC-32) per output frame, in output order.
    pub crcs: Vec<(i64, u32)>,
}

/// Locates `ffprobe`/`ffmpeg`: `$S3_FFMPEG_BIN`, then `$FFMPEG_DIR/bin`, then PATH.
pub fn tool(name: &str) -> PathBuf {
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    for dir in [
        std::env::var_os("S3_FFMPEG_BIN").map(PathBuf::from),
        std::env::var_os("FFMPEG_DIR").map(|d| PathBuf::from(d).join("bin")),
    ]
    .into_iter()
    .flatten()
    {
        let p = dir.join(&exe);
        if p.exists() {
            return p;
        }
    }
    PathBuf::from(exe)
}

fn run(tool_name: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(tool(tool_name))
        .args(args)
        .output()
        .map_err(|e| Error::Truth(format!("cannot run {tool_name}: {e}")))?;
    if !out.status.success() {
        return Err(Error::Truth(format!(
            "{tool_name} failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .take(3)
                .collect::<Vec<_>>()
                .join(" | ")
        )));
    }
    String::from_utf8(out.stdout).map_err(|e| Error::Truth(e.to_string()))
}

fn cached(
    out_dir: &Path,
    media: &Path,
    kind: &str,
    produce: impl FnOnce() -> Result<String>,
) -> Result<String> {
    let meta = fs::metadata(media)?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let name = media
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = out_dir.join("truth");
    fs::create_dir_all(&dir)?;
    let file = dir.join(format!("{name}.{}.{mtime}.{kind}.txt", meta.len()));
    if let Ok(s) = fs::read_to_string(&file) {
        return Ok(s);
    }
    let s = produce()?;
    fs::write(&file, &s)?;
    Ok(s)
}

/// Parses ffprobe `-of compact=p=0:nk=0` lines (`key=value|key=value|...`).
fn fields(line: &str) -> impl Iterator<Item = (&str, &str)> {
    line.split('|').filter_map(|kv| kv.split_once('='))
}

fn num(v: &str) -> Option<i64> {
    v.trim().parse().ok()
}

/// Collects packets, frames and Y CRCs for stream `stream` of `media`.
pub fn collect(out_dir: &Path, media: &Path, stream: usize) -> Result<Truth> {
    let m = media.to_str().ok_or(Error::NonUtf8Path)?;
    let sel = stream.to_string();
    let packets = cached(out_dir, media, "packets", || {
        run(
            "ffprobe",
            &[
                "-v",
                "error",
                "-select_streams",
                &sel,
                "-show_entries",
                "packet=pts,dts,flags,pos",
                "-of",
                "compact=p=0:nk=0",
                m,
            ],
        )
    })?;
    let frames = cached(out_dir, media, "frames", || {
        run(
            "ffprobe",
            &[
                "-v",
                "error",
                "-select_streams",
                &sel,
                "-show_entries",
                "frame=pts,best_effort_timestamp,key_frame,pict_type",
                "-of",
                "compact=p=0:nk=0",
                m,
            ],
        )
    })?;
    let map = format!("0:{stream}");
    let crcs = cached(out_dir, media, "ycrc", || {
        run(
            "ffmpeg",
            &[
                "-v",
                "error",
                "-nostdin",
                "-noautorotate",
                "-copyts",
                "-i",
                m,
                "-map",
                &map,
                "-vf",
                "extractplanes=y",
                "-fps_mode",
                "passthrough",
                "-enc_time_base",
                "demux",
                "-c:v",
                "rawvideo",
                "-f",
                "framehash",
                "-hash",
                "crc32",
                "-",
            ],
        )
    })?;
    Ok(Truth {
        packets: parse_packets(&packets),
        frames: parse_frames(&frames),
        crcs: parse_framehash(&crcs)?,
    })
}

fn parse_packets(s: &str) -> Vec<TruthPacket> {
    s.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut p = TruthPacket {
                pts: None,
                dts: None,
                key: false,
                discard: false,
                pos: None,
            };
            for (k, v) in fields(l) {
                match k {
                    "pts" => p.pts = num(v),
                    "dts" => p.dts = num(v),
                    "pos" => p.pos = num(v),
                    "flags" => {
                        p.key = v.contains('K');
                        p.discard = v.contains('D');
                    }
                    _ => {}
                }
            }
            p
        })
        .collect()
}

fn parse_frames(s: &str) -> Vec<TruthFrame> {
    s.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut f = TruthFrame {
                pts: None,
                best_effort: None,
                key: false,
                pict_type: String::new(),
            };
            for (k, v) in fields(l) {
                match k {
                    "pts" => f.pts = num(v),
                    "best_effort_timestamp" => f.best_effort = num(v),
                    "key_frame" => f.key = v.trim() == "1",
                    "pict_type" => f.pict_type = v.trim().to_string(),
                    _ => {}
                }
            }
            f
        })
        .collect()
}

/// framehash rows: `stream, dts, pts, duration, size, hash`.
fn parse_framehash(s: &str) -> Result<Vec<(i64, u32)>> {
    s.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let cols: Vec<&str> = l.split(',').map(str::trim).collect();
            let bad = || Error::Truth(format!("bad framehash line: {l}"));
            if cols.len() < 6 {
                return Err(bad());
            }
            let pts = num(cols[2]).ok_or_else(bad)?;
            let crc = u32::from_str_radix(cols[5], 16).map_err(|_| bad())?;
            Ok((pts, crc))
        })
        .collect()
}

/// Y-plane CRC of presentation frame `n` the literal way, `ffmpeg -vf select=eq(n\,N)`: used to
/// spot-check that the one-pass framehash rows are the same frames `select` picks.
pub fn select_crc(media: &Path, stream: usize, n: usize) -> Result<u32> {
    let m = media.to_str().ok_or(Error::NonUtf8Path)?;
    let map = format!("0:{stream}");
    let vf = format!("select=eq(n\\,{n}),extractplanes=y");
    let out = run(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-nostdin",
            "-noautorotate",
            "-i",
            m,
            "-map",
            &map,
            "-vf",
            &vf,
            "-fps_mode",
            "passthrough",
            "-frames:v",
            "1",
            "-c:v",
            "rawvideo",
            "-f",
            "framehash",
            "-hash",
            "crc32",
            "-",
        ],
    )?;
    parse_framehash(&out)?
        .first()
        .map(|r| r.1)
        .ok_or_else(|| Error::Truth(format!("select picked no frame {n}")))
}

/// ffprobe `nb_samples` per audio frame and the stream's sample rate/channels.
pub fn audio_samples(media: &Path, stream: usize) -> Result<(u64, u64, u64)> {
    let m = media.to_str().ok_or(Error::NonUtf8Path)?;
    let sel = stream.to_string();
    let frames = run(
        "ffprobe",
        &[
            "-v",
            "error",
            "-select_streams",
            &sel,
            "-show_entries",
            "frame=nb_samples",
            "-of",
            "compact=p=0:nk=0",
            m,
        ],
    )?;
    let total: u64 = frames
        .lines()
        .flat_map(fields)
        .filter(|(k, _)| *k == "nb_samples")
        .filter_map(|(_, v)| v.trim().parse::<u64>().ok())
        .sum();
    let info = run(
        "ffprobe",
        &[
            "-v",
            "error",
            "-select_streams",
            &sel,
            "-show_entries",
            "stream=sample_rate,channels",
            "-of",
            "compact=p=0:nk=0",
            m,
        ],
    )?;
    let (mut rate, mut ch) = (0, 0);
    for (k, v) in info.lines().flat_map(fields) {
        match k {
            "sample_rate" => rate = v.trim().parse().unwrap_or(0),
            "channels" => ch = v.trim().parse().unwrap_or(0),
            _ => {}
        }
    }
    Ok((total, rate, ch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ffprobe_compact_in_any_field_order() {
        let p = parse_packets(
            "pts=1536|dts=-512|pos=3880|flags=___\npts=0|dts=-1024|pos=48|flags=K_D\n",
        );
        assert_eq!(
            p[0],
            TruthPacket {
                pts: Some(1536),
                dts: Some(-512),
                key: false,
                discard: false,
                pos: Some(3880)
            }
        );
        assert!(p[1].key && p[1].discard);
        let f = parse_frames("key_frame=1|pts=N/A|best_effort_timestamp=7|pict_type=I|\n");
        assert_eq!(f[0].ts(), Some(7));
        assert!(f[0].key);
    }

    #[test]
    fn parses_framehash() {
        let h = parse_framehash(
            "#tb 0: 1/30\n0,          0,          0,      512,    76800, dc78d821\n",
        )
        .expect("parse");
        assert_eq!(h, vec![(0, 0xdc78_d821)]);
        assert!(parse_framehash("0, 1, 2\n").is_err());
    }
}
