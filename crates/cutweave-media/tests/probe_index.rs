// SPDX-License-Identifier: GPL-3.0-or-later
//! Probe, frame index and content hash against real media (WP-1D.02 acceptance), compared with
//! `ffprobe` on every file of the small-tier corpus.
//!
//! Needs the FFmpeg bundle (`just test-ffmpeg`, docs/dev/ffmpeg.md), the corpus
//! (`cargo xtask fetch-corpus --tier small`, or `CUTWEAVE_CORPUS_DIR`) and `ffprobe` (the bundle's
//! `bin/`, or on `PATH`). A missing corpus, `ffprobe` or file skips the test with a message; the
//! tests never touch the network. The 60 s 4K30 clip of the index-speed criterion is generated
//! with the FFmpeg CLI into the git-ignored corpus directory on first use.
#![cfg(feature = "ffmpeg")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration as Wall, Instant};

use cutweave_media::{FfmpegBackend, FrameIndex, HashMode, MediaBackend, MediaError, content_hash};
use cutweave_model::{ColorRange, FLICKS_PER_SECOND, FrameRate, StreamKind};
use serde_json::Value;

fn backend() -> FfmpegBackend {
    FfmpegBackend::new().expect("the loaded FFmpeg matches the build")
}

fn corpus_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("CUTWEAVE_CORPUS_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/corpus"),
        PathBuf::from,
    );
    dir.is_dir().then_some(dir)
}

/// A corpus file by name, or a skip message.
fn corpus_file(name: &str) -> Option<PathBuf> {
    let Some(dir) = corpus_dir() else {
        eprintln!("skipping: no corpus (cargo xtask fetch-corpus --tier small)");
        return None;
    };
    let path = dir.join(name);
    if path.is_file() {
        Some(path)
    } else {
        eprintln!("skipping: {name} is not in {}", dir.display());
        None
    }
}

/// The FFmpeg tool `name` (`ffprobe`, `ffmpeg`): the bundle's, else the one on `PATH`.
fn tool(name: &str) -> Option<PathBuf> {
    let exe = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let in_bundle = std::env::var_os("FFMPEG_DIR")
        .map(|d| PathBuf::from(d).join("bin").join(&exe))
        .filter(|p| p.is_file());
    let candidate = in_bundle.unwrap_or_else(|| PathBuf::from(&exe));
    Command::new(&candidate)
        .arg("-version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| candidate)
}

fn ffprobe_json(path: &Path, args: &[&str]) -> Option<Value> {
    let Some(ffprobe) = tool("ffprobe") else {
        eprintln!("skipping: no ffprobe (the bundle's bin/ or PATH)");
        return None;
    };
    let out = Command::new(ffprobe)
        .args(["-v", "error", "-of", "json"])
        .args(args)
        .arg(path)
        .output()
        .expect("run ffprobe");
    assert!(out.status.success(), "ffprobe failed: {out:?}");
    Some(serde_json::from_slice(&out.stdout).expect("ffprobe json"))
}

/// Every media file of the corpus directory.
fn corpus_media() -> Vec<PathBuf> {
    let Some(dir) = corpus_dir() else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("read corpus dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                [
                    "mp4", "mov", "mkv", "webm", "ts", "wav", "ac3", "png", "jpg",
                ]
                .contains(&e)
            })
        })
        // The deliberately truncated file has its own test.
        .filter(|p| {
            !p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("broken_")
        })
        .collect();
    files.sort();
    files
}

fn parse_ratio(s: &str) -> Option<(u32, u32)> {
    let (n, d) = s.split_once('/')?;
    let (n, d): (u32, u32) = (n.parse().ok()?, d.parse().ok()?);
    (n > 0 && d > 0).then_some((n, d))
}

fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// ffprobe prints "unknown" for an unspecified color tag.
fn tag<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    str_field(v, key).filter(|s| *s != "unknown" && *s != "unspecified")
}

fn first_stream<'a>(streams: &'a [Value], kind: &str) -> Option<&'a Value> {
    streams
        .iter()
        .find(|s| str_field(s, "codec_type") == Some(kind))
}

#[test]
#[allow(clippy::too_many_lines)] // one pass over each file, field by field
fn probe_matches_ffprobe_on_every_corpus_file() {
    let files = corpus_media();
    if files.is_empty() {
        eprintln!("skipping: no corpus files");
        return;
    }
    let backend = backend();
    let mut checked = 0;
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let Some(truth) = ffprobe_json(path, &["-show_streams", "-show_format"]) else {
            return;
        };
        let t0 = Instant::now();
        let info = backend
            .probe(path)
            .unwrap_or_else(|e| panic!("{name}: probe failed: {e}"));
        let took = t0.elapsed();
        let streams = truth["streams"].as_array().expect("streams");
        let format_name = str_field(&truth["format"], "format_name").unwrap_or("");
        let still = format_name.contains("image2") || format_name.ends_with("_pipe");

        // Streams: same count, order, kinds and codecs.
        assert_eq!(info.streams.len(), streams.len(), "{name}: stream count");
        for (ours, theirs) in info.streams.iter().zip(streams) {
            let want_kind = match str_field(theirs, "codec_type") {
                Some("video") => StreamKind::Video,
                Some("audio") => StreamKind::Audio,
                Some("subtitle") => StreamKind::Subtitle,
                Some("attachment") => StreamKind::Attachment,
                _ => StreamKind::Data,
            };
            assert_eq!(ours.kind, want_kind, "{name}: stream {} kind", ours.index);
            assert_eq!(u64::from(ours.index), theirs["index"].as_u64().unwrap());
            assert_eq!(
                ours.codec.as_deref(),
                str_field(theirs, "codec_name"),
                "{name}: stream {} codec",
                ours.index
            );
            assert_eq!(
                ours.profile.as_deref(),
                str_field(theirs, "profile"),
                "{name}: stream {} profile",
                ours.index
            );
            if want_kind == StreamKind::Video {
                assert_eq!(
                    ours.width.map(u64::from),
                    theirs["width"].as_u64(),
                    "{name}: width"
                );
                assert_eq!(
                    ours.height.map(u64::from),
                    theirs["height"].as_u64(),
                    "{name}: height"
                );
                assert_eq!(
                    ours.pixel_format.as_deref(),
                    str_field(theirs, "pix_fmt"),
                    "{name}: pixel format"
                );
            }
        }

        // Frame rate (average, as ffprobe's avg_frame_rate) and rotation, color of the main
        // video stream.
        let video = first_stream(streams, "video");
        let mut frame_ms = 1.0; // tolerance for the duration check
        if let Some(v) = video.filter(|_| !still) {
            let want = str_field(v, "avg_frame_rate")
                .and_then(parse_ratio)
                .or_else(|| str_field(v, "r_frame_rate").and_then(parse_ratio));
            match (info.fps, want) {
                (Some(ours), Some((n, d))) => {
                    assert_eq!(
                        u64::from(ours.num()) * u64::from(d),
                        u64::from(n) * u64::from(ours.den()),
                        "{name}: fps {ours} vs {n}/{d}"
                    );
                    frame_ms = 1000.0 / ours.as_f64();
                }
                (None, None) => {}
                (ours, theirs) => panic!("{name}: fps {ours:?} vs {theirs:?}"),
            }
        } else {
            assert_eq!(info.fps, None, "{name}: no frame rate for stills and audio");
        }
        if let Some(v) = video {
            // ffprobe reports the display matrix's counter-clockwise angle; ours is clockwise.
            let ccw = v["side_data_list"]
                .as_array()
                .and_then(|l| l.iter().find_map(|sd| sd["rotation"].as_f64()))
                .unwrap_or(0.0);
            let cw = (-ccw).rem_euclid(360.0).round() % 360.0;
            assert!(
                (f64::from(info.rotation) - cw).abs() < 0.5,
                "{name}: rotation {} vs {cw}",
                info.rotation
            );
            assert_eq!(
                info.color
                    .primaries
                    .as_ref()
                    .map(cutweave_model::ColorPrimaries::as_str),
                tag(v, "color_primaries"),
                "{name}: primaries"
            );
            assert_eq!(
                info.color
                    .transfer
                    .as_ref()
                    .map(cutweave_model::TransferCharacteristics::as_str),
                tag(v, "color_transfer"),
                "{name}: transfer"
            );
            assert_eq!(
                info.color
                    .matrix
                    .as_ref()
                    .map(cutweave_model::MatrixCoefficients::as_str),
                tag(v, "color_space"),
                "{name}: matrix"
            );
            let range = match str_field(v, "color_range") {
                Some("tv") => Some(ColorRange::Limited),
                Some("pc") => Some(ColorRange::Full),
                _ => None,
            };
            assert_eq!(info.color.range, range, "{name}: range");
        } else {
            assert_eq!(info.rotation, 0, "{name}");
        }

        // Audio: first stream's rate and channels.
        match first_stream(streams, "audio") {
            Some(a) => {
                let audio = info
                    .audio
                    .unwrap_or_else(|| panic!("{name}: no audio info"));
                let rate: u64 = str_field(a, "sample_rate").unwrap().parse().unwrap();
                assert_eq!(u64::from(audio.rate.hz()), rate, "{name}: sample rate");
                assert_eq!(
                    Some(u64::from(audio.channels)),
                    a["channels"].as_u64(),
                    "{name}: channels"
                );
            }
            None => assert_eq!(info.audio, None, "{name}"),
        }

        // Duration within one frame of the container's.
        let want = str_field(&truth["format"], "duration").and_then(|d| d.parse::<f64>().ok());
        if still {
            assert_eq!(info.duration, None, "{name}: stills have no duration");
        } else if let Some(want) = want {
            let ours = info
                .duration
                .unwrap_or_else(|| panic!("{name}: no duration"))
                .as_secs_f64();
            let diff_ms = (ours - want).abs() * 1000.0;
            assert!(
                diff_ms <= frame_ms,
                "{name}: duration {ours} vs {want} ({diff_ms:.2} ms > {frame_ms:.2} ms)"
            );
        }

        eprintln!(
            "probe {name}: {:?} {}x{} fps {:?} vfr {} rot {} dur {:?} ({:.1} ms)",
            info.streams.first().map(|s| s.kind),
            info.streams.first().and_then(|s| s.width).unwrap_or(0),
            info.streams.first().and_then(|s| s.height).unwrap_or(0),
            info.fps.map(FrameRate::as_f64),
            info.vfr,
            info.rotation,
            info.duration.map(cutweave_model::Duration::as_secs_f64),
            took.as_secs_f64() * 1e3
        );
        checked += 1;
    }
    eprintln!("probe matched ffprobe on {checked} corpus files");
}

#[test]
fn vfr_is_flagged_only_on_the_vfr_clip_and_agrees_with_the_index() {
    let files = corpus_media();
    if files.is_empty() {
        eprintln!("skipping: no corpus files");
        return;
    }
    let backend = backend();
    let mut vfr_seen = false;
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let info = backend.probe(path).unwrap();
        if info.streams.iter().all(|s| s.kind != StreamKind::Video) || info.fps.is_none() {
            continue;
        }
        let index = backend.index_media(path).unwrap();
        assert_eq!(
            info.vfr,
            index.is_vfr(),
            "{name}: probe and index disagree on VFR"
        );
        let want = name.starts_with("vfr_");
        assert_eq!(info.vfr, want, "{name}: vfr flag");
        vfr_seen |= info.vfr;
    }
    if files
        .iter()
        .any(|p| p.file_name().unwrap().to_string_lossy().starts_with("vfr_"))
    {
        assert!(vfr_seen);
    }
}

#[test]
fn the_phone_clip_is_portrait_rotated_and_variable_rate() {
    let Some(path) = corpus_file("vfr_phone_rotated90.mp4") else {
        return;
    };
    let info = backend().probe(&path).unwrap();
    assert!(info.vfr);
    // The corpus recipe tags the display matrix for a 90 degree counter-clockwise rotation.
    assert_eq!(info.rotation, 270);
    let v = &info.streams[0];
    assert_eq!((v.width, v.height), (Some(1080), Some(1920)));
    assert!(info.audio.is_some());
}

#[test]
fn a_broken_mp4_is_corrupt_within_two_seconds() {
    let Some(path) = corpus_file("broken_truncated_mp4.mp4") else {
        return;
    };
    let backend = backend();
    let t0 = Instant::now();
    let err = backend
        .probe(&path)
        .expect_err("a truncated mp4 must not probe");
    let took = t0.elapsed();
    assert!(matches!(err, MediaError::Corrupt { .. }), "{err:?}");
    assert!(took < Wall::from_secs(2), "probe took {took:?}");
    // Indexing it is an error too, or a partial index that says where the scan stopped; never a
    // panic.
    match backend.index_media(&path) {
        Err(MediaError::Corrupt { .. }) => {}
        Ok(index) => eprintln!(
            "broken mp4: partial index of {} frames, ended by {:?}",
            index.frame_count(),
            index.scan().ended_by
        ),
        Err(e) => panic!("unexpected {e:?}"),
    }
    eprintln!("broken mp4 probe: {err} in {took:?}");
}

/// ffprobe's packets of the first video stream: `(pts, dts, key, pos)` in demux order.
/// One packet as ffprobe lists it: `(pts, dts, key, pos)`.
type Packet = (Option<i64>, Option<i64>, bool, Option<i64>);

fn ffprobe_packets(path: &Path) -> Option<Vec<Packet>> {
    let truth = ffprobe_json(
        path,
        &[
            "-select_streams",
            "v:0",
            "-show_entries",
            "packet=pts,dts,flags,pos",
        ],
    )?;
    let int = |p: &Value, k: &str| p.get(k).and_then(Value::as_i64);
    let pos = |p: &Value| str_field(p, "pos").and_then(|s| s.parse::<i64>().ok());
    Some(
        truth["packets"]
            .as_array()
            .expect("packets")
            .iter()
            .map(|p| {
                let key = str_field(p, "flags").is_some_and(|f| f.contains('K'));
                (int(p, "pts"), int(p, "dts"), key, pos(p))
            })
            .collect(),
    )
}

fn assert_index_matches_ffprobe(name: &str, path: &Path, index: &FrameIndex) -> Option<()> {
    let packets = ffprobe_packets(path)?;
    assert_eq!(index.entries().len(), packets.len(), "{name}: packet count");
    for (i, (entry, (pts, dts, key, pos))) in index.entries().iter().zip(&packets).enumerate() {
        assert_eq!(entry.pts, *pts, "{name}: packet {i} pts");
        assert_eq!(entry.dts, *dts, "{name}: packet {i} dts");
        assert_eq!(entry.is_key, *key, "{name}: packet {i} key flag");
        if let Some(pos) = pos {
            assert_eq!(entry.pos, *pos, "{name}: packet {i} pos");
        }
    }
    // The keyframe pts list, as the acceptance criterion states it.
    let mut want: Vec<i64> = packets
        .iter()
        .filter(|p| p.2)
        .filter_map(|p| p.0.or(p.1))
        .collect();
    want.sort_unstable();
    let mut got: Vec<i64> = index
        .entries()
        .iter()
        .filter(|e| e.is_key)
        .filter_map(cutweave_media::IndexEntry::frame_pts)
        .collect();
    got.sort_unstable();
    assert_eq!(got, want, "{name}: keyframe pts");
    Some(())
}

#[test]
fn the_index_equals_ffprobe_packets_on_every_video_file() {
    let files = corpus_media();
    if files.is_empty() {
        eprintln!("skipping: no corpus files");
        return;
    }
    let backend = backend();
    let mut checked = 0;
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let info = backend.probe(path).unwrap();
        if info.streams.iter().all(|s| s.kind != StreamKind::Video) {
            // Audio-only: no pictures to index.
            assert!(
                matches!(
                    backend.index_media(path),
                    Err(MediaError::NoStream {
                        kind: StreamKind::Video,
                        ..
                    })
                ),
                "{name}"
            );
            continue;
        }
        let t0 = Instant::now();
        let index = backend.index_media(path).unwrap();
        let took = t0.elapsed();
        if assert_index_matches_ffprobe(&name, path, &index).is_none() {
            return;
        }
        // Frames are in presentation order with unique times; the GOP map covers every frame.
        let frames = index.frames();
        assert!(
            frames.windows(2).all(|w| w[0].pts < w[1].pts),
            "{name}: frame order"
        );
        let gops = index.gops();
        assert_eq!(
            gops.iter().map(|g| g.frames).sum::<usize>(),
            frames.len(),
            "{name}: gops"
        );
        assert!(
            gops.windows(2)
                .all(|w| w[0].first_frame + w[0].frames == w[1].first_frame)
        );
        // The index and the probe agree on the length, within a frame.
        if let (Some(d), Some(probe)) = (index.duration(), info.duration)
            && info.fps.is_some()
        {
            let frame = 1.0 / info.fps.unwrap().as_f64();
            // Audio can be longer than the pictures: only the pictures may be shorter.
            assert!(
                d.as_secs_f64() <= probe.as_secs_f64() + frame,
                "{name}: index {d} vs probe {probe}"
            );
        }
        // The stored form round-trips.
        let bytes = index.to_bytes();
        assert_eq!(FrameIndex::from_bytes(&bytes).unwrap(), index, "{name}");
        eprintln!(
            "index {name}: {} frames, {} gops, {} B stored, built in {:.1} ms",
            frames.len(),
            gops.len(),
            bytes.len(),
            took.as_secs_f64() * 1e3
        );
        checked += 1;
    }
    eprintln!("index equals ffprobe packets on {checked} video files");
}

/// The 60 s 4K30 long-GOP clip (criterion: the index builds in < 3 s, recorded not gating).
fn long_clip() -> Option<PathBuf> {
    let dir = corpus_dir()?;
    let path = dir.join("h264_4k30_longgop_60s.mp4");
    if path.is_file() {
        return Some(path);
    }
    let Some(ffmpeg) = tool("ffmpeg") else {
        eprintln!("skipping: no 60 s 4K clip and no ffmpeg CLI to generate it");
        return None;
    };
    eprintln!("generating {} (60 s of 4K30)", path.display());
    // Generate under a name that no scan of the corpus folder matches (its extension is not a
    // media one) and that is unique to this process, then publish it with one rename: a test
    // process listing the folder never sees a half-written clip, and two processes generating at
    // once (nextest runs every test in its own process) do not write the same file.
    let tmp = path.with_extension(format!("mp4.{}.part", std::process::id()));
    let status = Command::new(ffmpeg)
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=3840x2160:rate=30:duration=60",
        ])
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=60",
        ])
        .args([
            "-c:v", "libx264", "-preset", "veryfast", "-crf", "28", "-g", "250", "-bf", "3",
        ])
        .args([
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            "-shortest",
            "-bitexact",
        ])
        .args(["-movflags", "+faststart", "-f", "mp4"])
        .arg(&tmp)
        .status()
        .ok()?;
    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        eprintln!("skipping: ffmpeg could not generate the clip");
        return None;
    }
    // Renaming over an existing file fails on Windows: another process won the race, which is
    // fine, its clip is identical (`-bitexact`).
    if std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    path.is_file().then_some(path)
}

#[test]
fn the_4k30_long_gop_minute_indexes_fast_and_matches_ffprobe() {
    let Some(path) = long_clip() else { return };
    let backend = backend();

    let t0 = Instant::now();
    let info = backend.probe(&path).unwrap();
    let probe_took = t0.elapsed();

    let t0 = Instant::now();
    let index = backend.index_media(&path).unwrap();
    let index_took = t0.elapsed();

    assert_eq!(index.frame_count(), 1800);
    assert!(!index.is_vfr() && !info.vfr);
    assert_eq!(info.fps, Some(FrameRate::FPS_30));
    let (keys, min, max, _) = index.gop_stats();
    assert_eq!(
        (keys, max),
        (8, 250),
        "a 250-frame GOP: keyframes at 0, 250, ..."
    );
    assert!(min <= 250);
    // Within a frame of the 60 s of pictures.
    let secs = index.duration().unwrap().as_secs_f64();
    assert!((secs - 60.0).abs() < 1.0 / 30.0 + 1e-9, "{secs}");
    assert_eq!(
        index.time_of(30).map(cutweave_model::Time::flicks),
        Some(FLICKS_PER_SECOND),
        "frame 30 of a 30 fps clip is at 1 s"
    );

    if assert_index_matches_ffprobe("4k30 60 s", &path, &index).is_none() {
        return;
    }

    let t0 = Instant::now();
    let bytes = index.to_bytes();
    let back = FrameIndex::from_bytes(&bytes).unwrap();
    let blob_took = t0.elapsed();
    assert_eq!(back, index);

    let t0 = Instant::now();
    let fast = content_hash(&path, HashMode::Fast).unwrap();
    let fast_took = t0.elapsed();
    let t0 = Instant::now();
    let full = content_hash(&path, HashMode::Full).unwrap();
    let full_took = t0.elapsed();
    assert_ne!(fast.to_string(), full.to_string());

    eprintln!(
        "RECORD 4K30 60 s long-GOP ({} MB): probe {:.1} ms, index {:.0} ms ({} packets, {} keyframes), \
         blob {} B (round trip {:.2} ms), fast hash {:.2} ms, full hash {:.0} ms",
        std::fs::metadata(&path).unwrap().len() / 1_000_000,
        probe_took.as_secs_f64() * 1e3,
        index_took.as_secs_f64() * 1e3,
        index.entries().len(),
        keys,
        bytes.len(),
        blob_took.as_secs_f64() * 1e3,
        fast_took.as_secs_f64() * 1e3,
        full_took.as_secs_f64() * 1e3,
    );
}

#[test]
fn a_still_image_is_a_one_frame_index_without_a_duration() {
    let Some(path) = corpus_file("image_png.png") else {
        return;
    };
    let backend = backend();
    let info = backend.probe(&path).unwrap();
    assert_eq!(info.duration, None);
    assert_eq!(info.fps, None);
    assert!(!info.vfr);
    assert_eq!(info.audio, None);
    assert_eq!(info.streams.len(), 1);
}

#[test]
fn fast_hashes_of_corpus_files_are_stable_and_distinct() {
    let files = corpus_media();
    if files.is_empty() {
        eprintln!("skipping: no corpus files");
        return;
    }
    let mut seen = std::collections::HashMap::new();
    for path in &files {
        let a = content_hash(path, HashMode::Fast).unwrap();
        let b = content_hash(path, HashMode::Fast).unwrap();
        assert_eq!(a, b);
        if let Some(other) = seen.insert(a.to_string(), path.clone()) {
            panic!("{} and {} share a hash", other.display(), path.display());
        }
    }
}
