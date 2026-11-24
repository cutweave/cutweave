// SPDX-License-Identifier: GPL-3.0-or-later
//! `index_media` for the FFmpeg backend (WP-1D.02): the packet scan behind [`FrameIndex`].
//!
//! Reads every packet of the main video stream with `Packet::read` (pts, dts, key and discard
//! flags, file position), no decoding. The port of the Spike S3 scan; the index rules live in
//! [`FrameIndex::from_entries`].

use std::path::Path;

use ffmpeg_next::media::Type;
use ffmpeg_next::{Error, Packet};

use super::{input, sys};
use crate::index::{IndexEntry, ScanInfo, TimeBase};
use crate::{FrameIndex, MediaError};

/// Corrupt packets tolerated before the scan gives up (the rest of the file is likely garbage).
const MAX_DEMUX_ERRORS: u32 = 10_000;

pub(super) fn index_media(path: &Path) -> Result<FrameIndex, MediaError> {
    let mut input = input::open(path)?;
    let (stream_index, time_base) = {
        let stream = input
            .streams()
            .best(Type::Video)
            .ok_or_else(|| MediaError::NoStream {
                path: crate::path::display(path),
                kind: cutweave_model::StreamKind::Video,
                index: None,
            })?;
        let tb = stream.time_base();
        let time_base = u32::try_from(tb.numerator())
            .ok()
            .zip(u32::try_from(tb.denominator()).ok())
            .and_then(|(num, den)| TimeBase::new(num, den))
            .ok_or_else(|| MediaError::Corrupt {
                path: crate::path::display(path),
                message: format!(
                    "the video stream has an invalid time base {}/{}",
                    tb.numerator(),
                    tb.denominator()
                ),
            })?;
        (stream.index(), time_base)
    };

    let mut entries = Vec::new();
    let mut demux_errors = 0u32;
    let mut ended_by = None;
    loop {
        let mut pkt = Packet::empty();
        match pkt.read(&mut input) {
            Ok(()) => {
                if pkt.stream() == stream_index {
                    entries.push(IndexEntry {
                        pts: pkt.pts(),
                        dts: pkt.dts(),
                        is_key: pkt.is_key(),
                        discard: sys::packet_is_discard(&pkt),
                        pos: i64::try_from(pkt.position()).unwrap_or(-1),
                    });
                }
            }
            Err(Error::Eof) => break,
            Err(Error::InvalidData) => {
                demux_errors += 1;
                if demux_errors > MAX_DEMUX_ERRORS {
                    ended_by = Some("too many corrupt packets".to_owned());
                    break;
                }
            }
            Err(e) => {
                ended_by = Some(e.to_string());
                break;
            }
        }
    }

    let scan = ScanInfo {
        // Matroska cues are pts, and its demuxer reconstructs dts differently after a seek.
        seek_by_pts: input.format().name().contains("matroska"),
        demux_errors,
        ended_by,
    };
    let stream = u32::try_from(stream_index).unwrap_or(u32::MAX);
    FrameIndex::from_entries(stream, time_base, entries, scan).map_err(|e| e.into_media(path))
}
