// SPDX-License-Identifier: GPL-3.0-or-later
//! Image sequences in the FFmpeg backend (ADR-0035, WP-2F.05c).
//!
//! A run of numbered pictures is decoded through FFmpeg's `image2` demuxer, opened on the
//! pattern and a start number (`input::open_sequence`), by the same decoder that reads video
//! files (`decode`): every picture is a keyframe, and packet `n` has pts `n` in a time base of
//! one frame, which is exactly the computed index ([`SequenceSource::index_of`]), so a seek is
//! exact without reading anything else.
//!
//! `image2` stops at the first number that has no file, so each unbroken stretch of the run is
//! one demuxer ([`segments_of`]); [`SequenceDecoder`] moves between them and shows the picture
//! before a missing number.

use std::sync::Arc;

use super::decode::{self, Source};
use super::hwdec;
use crate::backend::{DecodeOpts, VideoDecoder};
use crate::path::display as shown;
use crate::sequence::{Segment, SequenceDecoder, SequenceSource, segments_of};
use crate::{FrameIndex, MediaError};

/// Opens the decoder of the image sequence `run`.
pub(super) fn open(
    run: SequenceSource,
    opts: &DecodeOpts,
    policy: Arc<hwdec::Policy>,
) -> Result<Box<dyn VideoDecoder>, MediaError> {
    crate::path::check_file(&run.first_file())?;
    let index = match &opts.index {
        Some(index) => Arc::clone(index),
        None => Arc::new(run.index()?),
    };
    if index.frame_count() != run.run.count as usize || Some(index.time_base()) != run.time_base() {
        return Err(MediaError::Invalid {
            message: format!(
                "the frame index passed for `{}` is another file's",
                shown(&run.locator())
            ),
        });
    }
    let segments = segments_of(&run.run);
    let whole = Arc::clone(&index);
    let opts = DecodeOpts {
        index: None,
        ..opts.clone()
    };
    let source = run.clone();
    let open = move |segment: Segment| -> Result<Box<dyn VideoDecoder>, MediaError> {
        // A run without gaps is one segment with the index of the whole run.
        let index: Arc<FrameIndex> = if segment.count as usize == whole.frame_count() {
            Arc::clone(&whole)
        } else {
            Arc::new(source.index_of(segment.count)?)
        };
        decode::open_source(
            &source.pattern_path(),
            Source::Sequence {
                start: segment.first,
                fps: source.run.fps,
                first_file: source.file(segment.first),
            },
            index,
            &opts,
            &policy,
        )
    };
    Ok(Box::new(SequenceDecoder::new(
        run.run,
        index,
        segments,
        Box::new(open),
    )))
}
