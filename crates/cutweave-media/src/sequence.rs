// SPDX-License-Identifier: GPL-3.0-or-later
//! Image sequences (ADR-0035, WP-2F.05c): a numbered run of pictures read as one video.
//!
//! A sequence reaches a [`MediaBackend`](crate::MediaBackend) as a **locator**: a path that
//! names the run's pattern in its directory and carries the start number, the count, the frame
//! rate and the missing numbers ([`ImageSequence::locator`]; a media asset gives it with
//! `MediaAsset::source_path`). Every backend call that takes a path takes a locator, so the
//! decoder pool, the caches, proxies and export handle a sequence like any video file:
//!
//! - `probe` reads the **first file only** and reports the picture with the run's duration
//!   (`count / fps`) and frame rate;
//! - `index_media` **computes** the frame index (frame `n` is file `first + n`); nothing is
//!   scanned;
//! - `open_decoder` returns a [`SequenceDecoder`]: the backend decodes each unbroken stretch of
//!   files (FFmpeg: its `image2` demuxer on the pattern and a start number), and a missing
//!   number shows the last picture before it, flagged [`VideoFrame::damaged`];
//! - [`content_hash`](crate::content_hash) of a locator is the first file's hash mixed with the
//!   run, so a sequence and a still of its first picture never share cache entries.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cutweave_model::{ImageSequence, ProbeInfo, StreamKind, Time};

use crate::backend::VideoDecoder;
use crate::index::{IndexEntry, ScanInfo, TimeBase};
use crate::{CancelToken, FrameIndex, MediaError, VideoFrame};

/// An image sequence as a media backend reads it: the directory and the run.
#[derive(Clone, Debug, PartialEq)]
pub struct SequenceSource {
    /// The directory the files are in.
    pub dir: PathBuf,
    /// The run: pattern, first number, count, frame rate, missing numbers.
    pub run: ImageSequence,
}

impl SequenceSource {
    /// The sequence `path` is the locator of; `None` for any other path (an ordinary file).
    #[must_use]
    pub fn parse(path: &Path) -> Option<SequenceSource> {
        let (dir, run) = ImageSequence::parse_locator(path.to_str()?)?;
        Some(SequenceSource {
            dir: PathBuf::from(dir),
            run,
        })
    }

    /// The locator of this sequence (what [`parse`](Self::parse) reads).
    #[must_use]
    pub fn locator(&self) -> PathBuf {
        PathBuf::from(self.run.locator(&self.first_file().to_string_lossy()))
    }

    /// The file of `number`.
    #[must_use]
    pub fn file(&self, number: u32) -> PathBuf {
        // The pattern was validated when the locator was parsed.
        self.dir
            .join(self.run.file_name(number).unwrap_or_default())
    }

    /// The first file of the run (always present at import).
    #[must_use]
    pub fn first_file(&self) -> PathBuf {
        self.file(self.run.first)
    }

    /// The pattern in its directory (`/renders/shot_%04d.png`): what FFmpeg's `image2` demuxer
    /// opens.
    #[must_use]
    pub fn pattern_path(&self) -> PathBuf {
        self.dir.join(&self.run.pattern)
    }

    /// The time base of the computed index: one tick is one frame.
    #[must_use]
    pub fn time_base(&self) -> Option<TimeBase> {
        TimeBase::new(self.run.fps.den(), self.run.fps.num())
    }

    /// The frame index of `frames` frames at the run's rate, computed: every frame is a
    /// keyframe, frame `n` has pts `n` in a time base of one frame. Nothing is read.
    pub fn index_of(&self, frames: u32) -> Result<FrameIndex, MediaError> {
        let invalid = |message: String| MediaError::Invalid { message };
        let time_base = self
            .time_base()
            .ok_or_else(|| invalid("the image sequence has no usable frame rate".to_owned()))?;
        let entries = (0..i64::from(frames))
            .map(|n| IndexEntry {
                pts: Some(n),
                dts: Some(n),
                is_key: true,
                discard: false,
                pos: -1,
            })
            .collect();
        FrameIndex::from_entries(0, time_base, entries, ScanInfo::default())
            .map_err(|e| invalid(format!("the image sequence has no frames: {e}")))
    }

    /// The frame index of the whole run (missing numbers included: they are frames too).
    pub fn index(&self) -> Result<FrameIndex, MediaError> {
        self.index_of(self.run.count)
    }

    /// The probe of the sequence from the probe of its first file: the same picture, with the
    /// run's duration and frame rate, and no sound.
    #[must_use]
    pub fn probe(&self, mut first: ProbeInfo) -> ProbeInfo {
        first.streams.retain(|s| s.kind == StreamKind::Video);
        first.duration = self.run.duration();
        first.fps = Some(self.run.fps);
        first.vfr = false;
        first.audio = None;
        first
    }

    /// The text mixed into the first file's hash to make the sequence's content hash.
    pub(crate) fn hash_salt(&self) -> String {
        format!(
            "image-sequence;{};{};{};{}x{}",
            self.run.pattern,
            self.run.first,
            self.run.count,
            self.run.fps.num(),
            self.run.fps.den()
        )
    }
}

/// One unbroken stretch of files a [`SequenceDecoder`] reads through one inner decoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    /// The number of its first file.
    pub first: u32,
    /// How many files it has.
    pub count: u32,
}

/// Opens the decoder of one [`Segment`]: frame `k` of it is the file `segment.first + k`, at
/// the run's frame rate.
pub type SegmentOpener =
    Box<dyn FnMut(Segment) -> Result<Box<dyn VideoDecoder>, MediaError> + Send>;

/// The [`VideoDecoder`] of an image sequence: frame `n` is the file `first + n`, read through
/// the decoder of the segment that holds it; a missing number shows the last picture before
/// it, flagged damaged. Frames carry the times of the sequence's computed index.
pub struct SequenceDecoder {
    run: ImageSequence,
    index: Arc<FrameIndex>,
    segments: Vec<Segment>,
    open: SegmentOpener,
    /// The open segment decoder and the position of its segment in `segments`.
    current: Option<(usize, Box<dyn VideoDecoder>)>,
    /// The frame `next_frame` returns next.
    next: u32,
    cancel: Option<CancelToken>,
}

impl SequenceDecoder {
    /// A decoder for `run` with the computed `index` of the whole run. `segments` are the
    /// stretches `open` can decode, ascending; together they must hold every number that is
    /// not missing ([`ImageSequence::segments`], or one segment over the whole run for a
    /// backend that needs no files).
    #[must_use]
    pub fn new(
        run: ImageSequence,
        index: Arc<FrameIndex>,
        segments: Vec<Segment>,
        open: SegmentOpener,
    ) -> SequenceDecoder {
        SequenceDecoder {
            run,
            index,
            segments,
            open,
            current: None,
            next: 0,
            cancel: None,
        }
    }

    fn frame(&mut self, n: u32) -> Result<VideoFrame, MediaError> {
        let number = self.run.number_for_frame(n);
        let stands_in = number != self.run.first.saturating_add(n);
        let at = self
            .segments
            .partition_point(|s| s.first <= number)
            .checked_sub(1)
            .filter(|&i| number - self.segments[i].first < self.segments[i].count)
            .ok_or_else(|| MediaError::Invalid {
                message: format!("picture {number} of the image sequence is in no segment"),
            })?;
        let segment = self.segments[at];
        if self.current.as_ref().is_none_or(|(i, _)| *i != at) {
            // Close the previous segment's decoder before opening the next one.
            self.current = None;
            let mut decoder = (self.open)(segment)?;
            decoder.set_cancel(self.cancel.clone());
            self.current = Some((at, decoder));
        }
        let local = self
            .run
            .fps
            .frame_to_time(i64::from(number - segment.first))
            .map_err(|_| MediaError::OutOfRange { t: Time::MAX })?;
        let Some((_, decoder)) = self.current.as_mut() else {
            return Err(MediaError::Backend {
                message: "the image sequence decoder is not open".to_owned(),
            });
        };
        let mut frame = match decoder.seek_exact(local) {
            Ok(frame) => frame,
            Err(e) => {
                if !matches!(e, MediaError::Cancelled) {
                    // A decoder that failed is not trusted with the next frame.
                    self.current = None;
                }
                return Err(e);
            }
        };
        frame.pts = self
            .index
            .time_of(n as usize)
            .ok_or(MediaError::OutOfRange { t: Time::MAX })?;
        frame.damaged |= stands_in;
        self.next = n + 1;
        Ok(frame)
    }
}

impl VideoDecoder for SequenceDecoder {
    fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        if self.next >= self.run.count {
            return Ok(None);
        }
        self.frame(self.next).map(Some)
    }

    fn seek_exact(&mut self, t: Time) -> Result<VideoFrame, MediaError> {
        if self.index.end_time().is_some_and(|end| t >= end) {
            return Err(MediaError::OutOfRange { t });
        }
        let n = u32::try_from(self.index.frame_at(t)).unwrap_or(u32::MAX);
        self.frame(n.min(self.run.count.saturating_sub(1)))
    }

    fn index(&self) -> Arc<FrameIndex> {
        Arc::clone(&self.index)
    }

    fn set_cancel(&mut self, cancel: Option<CancelToken>) {
        if let Some((_, decoder)) = self.current.as_mut() {
            decoder.set_cancel(cancel.clone());
        }
        self.cancel = cancel;
    }
}

impl std::fmt::Debug for SequenceDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SequenceDecoder")
            .field("run", &self.run)
            .field("next", &self.next)
            .finish_non_exhaustive()
    }
}

/// The segments of `run` ([`ImageSequence::segments`]) as [`Segment`]s.
#[must_use]
pub fn segments_of(run: &ImageSequence) -> Vec<Segment> {
    run.segments()
        .into_iter()
        .map(|(first, count)| Segment { first, count })
        .collect()
}

#[cfg(test)]
mod tests {
    use cutweave_model::{Duration, Extra, FrameRate};

    use super::*;
    use crate::testing::{FakeMedia, FakeVideo, fake_frame_number};
    use crate::{DecodeOpts, FakeBackend, MediaBackend};

    fn run(first: u32, count: u32, missing: &[u32]) -> ImageSequence {
        ImageSequence {
            pattern: "f_%04d.png".into(),
            first,
            count,
            fps: FrameRate::FPS_30,
            missing: missing.to_vec(),
            extra: Extra::new(),
        }
    }

    fn source(first: u32, count: u32, missing: &[u32]) -> SequenceSource {
        SequenceSource {
            dir: PathBuf::from("/renders"),
            run: run(first, count, missing),
        }
    }

    #[test]
    fn a_locator_parses_to_its_files_and_other_paths_do_not() {
        let src = source(98, 300, &[120]);
        let back = SequenceSource::parse(&src.locator()).expect("a locator");
        assert_eq!(back.run, src.run);
        assert_eq!(back.first_file(), src.first_file());
        assert!(src.first_file().ends_with("f_0098.png"));
        assert!(src.pattern_path().ends_with("f_%04d.png"));
        assert_eq!(SequenceSource::parse(&src.first_file()), None);
        assert_eq!(SequenceSource::parse(&src.pattern_path()), None);
    }

    #[test]
    fn the_index_is_computed_one_keyframe_per_picture() {
        let index = source(1, 300, &[7]).index().expect("index");
        assert_eq!(index.frame_count(), 300);
        assert_eq!(index.max_decode_distance(), 1);
        assert_eq!(index.duration(), Some(Duration::from_secs(10).unwrap()));
        assert_eq!(index.time_of(30), Some(Time::from_secs(1).unwrap()));
        assert_eq!(index.frame_at(Time::from_secs(1).unwrap()), 30);
        assert!(!index.is_vfr());
    }

    #[test]
    fn the_probe_is_the_first_picture_with_the_runs_length() {
        let first = FakeMedia::video(FakeVideo::new(64, 48, FrameRate::FPS_25, 1))
            .probe()
            .expect("probe");
        let probe = source(1, 300, &[]).probe(first);
        assert_eq!(probe.duration, Some(Duration::from_secs(10).unwrap()));
        assert_eq!(probe.fps, Some(FrameRate::FPS_30));
        assert_eq!(probe.streams.len(), 1);
        assert_eq!(probe.streams[0].width, Some(64));
        assert_eq!(probe.audio, None);
    }

    /// The fake backend serves a sequence whose first file is registered: frame `n` carries
    /// `n`, and a missing number shows the picture before it, flagged.
    #[test]
    fn frames_are_exact_and_a_missing_one_shows_the_picture_before_it() {
        let src = source(10, 8, &[12, 13, 15]);
        let backend = FakeBackend::new().with_media(
            src.first_file(),
            FakeMedia::video(FakeVideo::new(16, 8, FrameRate::FPS_30, 1)),
        );
        let locator = src.locator();
        let probe = backend.probe(&locator).expect("probe");
        assert_eq!(probe.fps, Some(FrameRate::FPS_30));
        let mut decoder = backend
            .open_decoder(&locator, &DecodeOpts::default())
            .expect("decoder");
        assert_eq!(decoder.index().frame_count(), 8);
        let mut shown = Vec::new();
        while let Some(frame) = decoder.next_frame().expect("frame") {
            let n = i64::try_from(shown.len()).unwrap();
            assert_eq!(frame.pts, FrameRate::FPS_30.frame_to_time(n).unwrap());
            shown.push((fake_frame_number(&frame).expect("number"), frame.damaged));
        }
        assert_eq!(
            shown,
            [
                (0, false),
                (1, false),
                (1, true),
                (1, true),
                (4, false),
                (4, true),
                (6, false),
                (7, false)
            ]
        );
        // Random access gives the same pictures, and the end is out of range.
        let at = |n: i64| FrameRate::FPS_30.frame_to_time(n).unwrap();
        let frame = decoder.seek_exact(at(5)).expect("seek");
        assert_eq!((fake_frame_number(&frame), frame.damaged), (Some(4), true));
        assert_eq!(
            fake_frame_number(&decoder.next_frame().unwrap().unwrap()),
            Some(6)
        );
        assert_eq!(
            fake_frame_number(&decoder.seek_exact(at(0)).unwrap()),
            Some(0)
        );
        assert!(matches!(
            decoder.seek_exact(at(8)),
            Err(MediaError::OutOfRange { .. })
        ));
    }
}
