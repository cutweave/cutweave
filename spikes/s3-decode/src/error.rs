// SPDX-License-Identifier: GPL-3.0-or-later
//! Spike error type. Every FFmpeg failure is mapped to a value; nothing in the decode path panics.

use std::fmt;

use ffmpeg_next as ffmpeg;

/// Result alias for the spike.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong opening, indexing, seeking or verifying a file.
#[derive(Debug)]
pub enum Error {
    /// An FFmpeg call failed (`op` names the call).
    Ffmpeg {
        op: &'static str,
        err: ffmpeg::Error,
    },
    /// The file has no video stream (or no decoder for it).
    NoVideoStream,
    /// ffmpeg-next panics on non-UTF-8 paths (`format::input` unwraps `to_str`), so they are refused up front.
    NonUtf8Path,
    /// The packet scan found no decodable frame.
    EmptyIndex,
    /// A packet had neither pts nor dts, so it cannot be placed on the timeline.
    NoTimestamps { decode_idx: usize },
    /// Presentation index out of range.
    NoSuchFrame(usize),
    /// After a seek the demuxer never returned the wanted keyframe packet.
    SeekLanding {
        key_decode_idx: usize,
        attempts: u32,
    },
    /// The decoder produced a frame past the target without producing the target.
    Overshoot { wanted: i64, got: i64 },
    /// The stream ended before the target frame came out of the decoder.
    EndOfStream { wanted: Option<i64> },
    /// The decoder returned a frame without any timestamp.
    FrameWithoutPts,
    /// A safety cap (packets read for one request) was hit: corrupt input.
    Runaway { packets: u64 },
    /// ffprobe/ffmpeg ground truth could not be produced or parsed.
    Truth(String),
    /// Filesystem or process I/O.
    Io(std::io::Error),
}

impl Error {
    /// Short machine-readable kind, used in CSV output.
    pub fn kind(&self) -> &'static str {
        match self {
            Error::Ffmpeg { .. } => "ffmpeg",
            Error::NoVideoStream => "no_video_stream",
            Error::NonUtf8Path => "non_utf8_path",
            Error::EmptyIndex => "empty_index",
            Error::NoTimestamps { .. } => "no_timestamps",
            Error::NoSuchFrame(_) => "no_such_frame",
            Error::SeekLanding { .. } => "seek_landing",
            Error::Overshoot { .. } => "overshoot",
            Error::EndOfStream { .. } => "end_of_stream",
            Error::FrameWithoutPts => "frame_without_pts",
            Error::Runaway { .. } => "runaway",
            Error::Truth(_) => "truth",
            Error::Io(_) => "io",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Ffmpeg { op, err } => write!(f, "{op}: {err}"),
            Error::NoVideoStream => write!(f, "no decodable video stream"),
            Error::NonUtf8Path => write!(f, "path is not valid UTF-8"),
            Error::EmptyIndex => write!(f, "packet scan found no decodable frame"),
            Error::NoTimestamps { decode_idx } => write!(f, "packet {decode_idx} has no pts/dts"),
            Error::NoSuchFrame(i) => write!(f, "no frame {i}"),
            Error::SeekLanding {
                key_decode_idx,
                attempts,
            } => {
                write!(
                    f,
                    "seek never reached keyframe packet {key_decode_idx} ({attempts} attempts)"
                )
            }
            Error::Overshoot { wanted, got } => {
                write!(f, "decoder skipped pts {wanted} (next was {got})")
            }
            Error::EndOfStream { wanted } => write!(f, "stream ended before pts {wanted:?}"),
            Error::FrameWithoutPts => write!(f, "decoded frame has no timestamp"),
            Error::Runaway { packets } => write!(f, "gave up after {packets} packets"),
            Error::Truth(s) => write!(f, "ground truth: {s}"),
            Error::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// Wraps an FFmpeg error with the name of the call that failed.
pub fn ff(op: &'static str) -> impl FnOnce(ffmpeg::Error) -> Error {
    move |err| Error::Ffmpeg { op, err }
}
