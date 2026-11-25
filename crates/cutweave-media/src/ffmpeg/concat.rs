// SPDX-License-Identifier: GPL-3.0-or-later
//! [`concat`]: joins files end to end by copying their packets (WP-2F.01: the parts of a proxy
//! become one file).
//!
//! The output gets the video and audio streams of the first part (codec parameters copied, no
//! re-encoding). Every part must have the same streams (kind and codec); each part's packets
//! are rescaled into the output's time bases and shifted, per stream, to start where that
//! stream of the previous part ended. Intra-frame video (every packet a keyframe) and PCM audio
//! join without a seam. Other streams (data, subtitles) are dropped.
//!
//! The output is written to a sibling `.<name>.<pid>-<n>.partial` and renamed over the target
//! only when complete, so a cancel, a failure or a killed process never leaves a half-joined
//! file under the target's name.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use cutweave_model::{Duration, Time};
use ffmpeg_next::codec::{self, encoder};
use ffmpeg_next::format;
use ffmpeg_next::media::Type;
use ffmpeg_next::{Error, Packet, Rational};

use super::error::media_error;
use super::input;
use crate::backend::{ConcatSpec, EncodeSummary};
use crate::path::display;
use crate::{CancelCheck, MediaError, TimeBase};

/// The muxer for `path`'s extension (`mov`, `mp4`).
fn container(path: &Path) -> Result<&'static str, MediaError> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mov") => Ok("mov"),
        Some("mp4" | "m4v") => Ok("mp4"),
        other => Err(MediaError::Unsupported {
            message: format!(
                "`{}`: cannot join into the {} container (MOV or MP4)",
                display(path),
                other.unwrap_or("unnamed")
            ),
        }),
    }
}

/// A partial output next to `target`, removed on drop unless kept.
struct Partial {
    path: PathBuf,
    keep: bool,
}

impl Partial {
    fn new(target: &Path) -> Partial {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = target
            .file_name()
            .map_or_else(|| "output".into(), |n| n.to_string_lossy());
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        Partial {
            path: target.with_file_name(format!(".{name}.{}-{n}.partial", std::process::id())),
            keep: false,
        }
    }
}

impl Drop for Partial {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// One output stream and what was written to it.
struct Out {
    medium: Type,
    codec: codec::Id,
    /// The output stream's time base (decided by the muxer's header).
    time_base: Rational,
    /// Where the next part starts, in `time_base`.
    offset: i64,
    /// Sample rate (audio), to count samples.
    rate: u32,
}

/// Joins `spec.parts` into `spec.path` (see the module docs).
pub(super) fn concat(spec: &ConcatSpec) -> Result<EncodeSummary, MediaError> {
    spec.validate()?;
    let muxer = container(&spec.path)?;
    let mut partial = Partial::new(&spec.path);
    let summary = write(spec, muxer, &partial.path)?;
    std::fs::rename(&partial.path, &spec.path).map_err(|e| MediaError::Io {
        path: display(&spec.path),
        message: format!("cannot move the joined file into place: {e}"),
    })?;
    partial.keep = true;
    Ok(summary)
}

fn cancelled(cancel: Option<&CancelCheck>) -> Result<(), MediaError> {
    if cancel.is_some_and(CancelCheck::is_cancelled) {
        Err(MediaError::Cancelled)
    } else {
        Ok(())
    }
}

fn write_error(path: &Path, e: Error) -> MediaError {
    MediaError::Backend {
        message: format!("writing `{}`: {e}", display(path)),
    }
}

/// Adds an output stream for every video and audio stream of `first_path` (copying their
/// parameters) and returns them in order.
fn add_streams(
    first_path: &Path,
    out: &mut format::context::Output,
    out_path: &Path,
) -> Result<Vec<Out>, MediaError> {
    let first = input::open(first_path)?;
    let mut outs: Vec<Out> = Vec::new();
    for ist in first.streams() {
        let params = ist.parameters();
        let medium = params.medium();
        if !matches!(medium, Type::Video | Type::Audio) {
            continue;
        }
        let rate = if medium == Type::Audio {
            codec::context::Context::from_parameters(params.clone())
                .and_then(|c| c.decoder().audio())
                .map_or(0, |a| a.rate())
        } else {
            0
        };
        let mut ost = out
            .add_stream(encoder::find(codec::Id::None))
            .map_err(|e| write_error(out_path, e))?;
        ost.set_parameters(params.clone());
        ost.set_time_base(ist.time_base());
        if medium == Type::Video {
            ost.set_avg_frame_rate(ist.avg_frame_rate());
            ost.set_rate(ist.rate());
        }
        outs.push(Out {
            medium,
            codec: params.id(),
            time_base: ist.time_base(),
            offset: 0,
            rate,
        });
    }
    if outs.is_empty() {
        return Err(MediaError::NoStream {
            path: display(first_path),
            kind: cutweave_model::StreamKind::Video,
            index: None,
        });
    }
    Ok(outs)
}

/// The part's stream indexes mapped onto `outs` (by position among video and audio streams);
/// `Invalid` when its streams are not the first part's.
fn map_streams(
    input: &format::context::Input,
    outs: &[Out],
    part: &Path,
    first_path: &Path,
) -> Result<Vec<Option<usize>>, MediaError> {
    let mismatch = || MediaError::Invalid {
        message: format!(
            "`{}` does not have the streams of `{}`; parts must match to be joined",
            display(part),
            display(first_path)
        ),
    };
    let mut map: Vec<Option<usize>> = Vec::new();
    let mut next = 0_usize;
    for ist in input.streams() {
        let params = ist.parameters();
        let medium = params.medium();
        if !matches!(medium, Type::Video | Type::Audio) {
            map.push(None);
            continue;
        }
        let fits = outs
            .get(next)
            .is_some_and(|o| o.medium == medium && o.codec == params.id());
        if !fits {
            return Err(mismatch());
        }
        map.push(Some(next));
        next += 1;
    }
    if next != outs.len() {
        return Err(mismatch());
    }
    Ok(map)
}

/// Copies the packets of `part` into `out`, after what the earlier parts wrote; returns the
/// video frames copied.
fn copy_part(
    spec: &ConcatSpec,
    part: &Path,
    outs: &mut [Out],
    out: &mut format::context::Output,
    out_path: &Path,
) -> Result<u64, MediaError> {
    let cancel = spec.cancel.as_ref();
    let mut input = input::open(part)?;
    let map = map_streams(&input, outs, part, &spec.parts[0])?;
    let time_bases: Vec<Rational> = input.streams().map(|s| s.time_base()).collect();
    let mut ends: Vec<i64> = outs.iter().map(|o| o.offset).collect();
    let mut video_frames = 0_u64;
    let mut packet = Packet::empty();
    loop {
        match packet.read(&mut input) {
            Ok(()) => {}
            Err(Error::Eof) => break,
            Err(e) => return Err(media_error(part, e)),
        }
        cancelled(cancel)?;
        let Some(Some(o)) = map.get(packet.stream()).copied() else {
            continue;
        };
        let (Some(itb), Some(stream)) = (time_bases.get(packet.stream()), outs.get(o)) else {
            continue;
        };
        packet.rescale_ts(*itb, stream.time_base);
        let shift = stream.offset;
        packet.set_pts(packet.pts().map(|p| p.saturating_add(shift)));
        packet.set_dts(packet.dts().map(|d| d.saturating_add(shift)));
        if let Some(start) = packet.pts().or(packet.dts()) {
            let end = start.saturating_add(packet.duration().max(0));
            if let Some(e) = ends.get_mut(o) {
                *e = (*e).max(end);
            }
        }
        if stream.medium == Type::Video {
            video_frames += 1;
        }
        packet.set_stream(o);
        packet.set_position(-1);
        packet
            .write_interleaved(out)
            .map_err(|e| write_error(out_path, e))?;
    }
    for (o, end) in outs.iter_mut().zip(ends) {
        o.offset = end;
    }
    Ok(video_frames)
}

/// Writes the joined file to `out_path` (the partial file).
fn write(spec: &ConcatSpec, muxer: &str, out_path: &Path) -> Result<EncodeSummary, MediaError> {
    let mut out = format::output_as(&out_path, muxer).map_err(|e| write_error(out_path, e))?;
    let mut outs = add_streams(&spec.parts[0], &mut out, out_path)?;
    out.write_header().map_err(|e| write_error(out_path, e))?;
    for (i, o) in outs.iter_mut().enumerate() {
        if let Some(s) = out.stream(i) {
            o.time_base = s.time_base();
        }
    }
    let mut video_frames = 0_u64;
    for part in &spec.parts {
        cancelled(spec.cancel.as_ref())?;
        video_frames += copy_part(spec, part, &mut outs, &mut out, out_path)?;
    }
    cancelled(spec.cancel.as_ref())?;
    out.write_trailer().map_err(|e| write_error(out_path, e))?;
    drop(out);
    Ok(summary(&outs, video_frames))
}

/// The summary of the joined streams: frames counted, samples and duration from the ends.
fn summary(outs: &[Out], video_frames: u64) -> EncodeSummary {
    let mut duration = Duration::ZERO;
    let mut audio_samples = 0_u64;
    for o in outs {
        let tb = u32::try_from(o.time_base.numerator())
            .ok()
            .zip(u32::try_from(o.time_base.denominator()).ok())
            .and_then(|(n, d)| TimeBase::new(n, d));
        let end: Option<Time> = tb.and_then(|tb| tb.to_time(o.offset));
        if let Some(d) = end.and_then(|t| Duration::try_from(t).ok()) {
            duration = duration.max(d);
        }
        if o.medium == Type::Audio && o.rate > 0 {
            let samples = Rational::new(1, i32::try_from(o.rate).unwrap_or(i32::MAX));
            let n = ffmpeg_next::rescale::Rescale::rescale(&o.offset, o.time_base, samples);
            audio_samples = audio_samples.max(u64::try_from(n).unwrap_or(0));
        }
    }
    EncodeSummary {
        video_frames,
        audio_samples,
        duration,
    }
}
