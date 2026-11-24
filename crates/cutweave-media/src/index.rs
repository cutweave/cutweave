// SPDX-License-Identifier: GPL-3.0-or-later
//! [`FrameIndex`]: the packet-scan index of one video stream (arch §7, ADR-0003).
//!
//! Every video packet in demux (decode) order, the frames in presentation order, and for each
//! frame the keyframe packet a seek must start decoding from. No decoding happens to build it, and
//! it is plain data: backends build it ([`MediaBackend::index_media`](crate::MediaBackend)),
//! `cutweave-cache` stores it in `cache.sqlite` (`to_bytes` / `from_bytes`), and the decoder
//! (WP-1D.03) seeks with it.
//!
//! **VFR needs no special case.** Frames are addressed by their own pts, never by `n / fps`
//! (arch §4.1): [`FrameIndex::frame_at`] maps a [`Time`] to a frame number through the table.
//!
//! Timestamps stay in the stream time base ([`TimeBase`] ticks) until mapped to flicks by
//! [`FrameIndex::time_of`] and [`FrameIndex::frame_at`].
//!
//! The port of the validated Spike S3 index (`spikes/s3-decode/src/index.rs`), plus the serialized
//! form and the GOP map.

use std::collections::HashMap;
use std::sync::OnceLock;

use cutweave_model::{Duration, Time};

use crate::MediaError;

/// A stream time base: one tick is `num / den` seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TimeBase {
    /// Numerator (> 0).
    pub num: u32,
    /// Denominator (> 0).
    pub den: u32,
}

impl TimeBase {
    /// The flick time base: one tick is one flick (used by synthetic media).
    pub const FLICKS: TimeBase = TimeBase {
        num: 1,
        den: 705_600_000,
    };

    /// `num / den`, or `None` if either is zero.
    #[must_use]
    pub const fn new(num: u32, den: u32) -> Option<TimeBase> {
        if num == 0 || den == 0 {
            None
        } else {
            Some(TimeBase { num, den })
        }
    }

    /// Seconds per tick.
    #[must_use]
    pub fn secs_per_tick(self) -> f64 {
        f64::from(self.num) / f64::from(self.den)
    }

    /// `ticks` as a time in flicks, rounded to the nearest flick (halves away from zero). `None`
    /// when the result is outside the range a [`Time`] can hold.
    #[must_use]
    pub fn to_time(self, ticks: i64) -> Option<Time> {
        let num = i128::from(ticks)
            * i128::from(self.num)
            * i128::from(cutweave_model::FLICKS_PER_SECOND);
        let den = i128::from(self.den);
        let half = den / 2;
        let flicks = if num >= 0 {
            (num + half) / den
        } else {
            -((-num + half) / den)
        };
        Time::from_flicks(i64::try_from(flicks).ok()?).ok()
    }

    /// The largest tick count whose time is at or before `t` (floor), saturating at the `i64`
    /// range.
    #[must_use]
    pub fn floor_ticks(self, t: Time) -> i64 {
        let num = i128::from(t.flicks()) * i128::from(self.den);
        let den = i128::from(self.num) * i128::from(cutweave_model::FLICKS_PER_SECOND);
        let ticks = num.div_euclid(den);
        i64::try_from(ticks).unwrap_or(if ticks < 0 { i64::MIN } else { i64::MAX })
    }
}

/// One demuxed video packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexEntry {
    /// Presentation timestamp in ticks, if the demuxer reported one.
    pub pts: Option<i64>,
    /// Decode timestamp in ticks, if the demuxer reported one.
    pub dts: Option<i64>,
    /// A keyframe (random access point).
    pub is_key: bool,
    /// `AV_PKT_FLAG_DISCARD`: decoded for reference, never output (edit-list pre-roll).
    pub discard: bool,
    /// Byte position in the file, -1 if unknown.
    pub pos: i64,
}

impl IndexEntry {
    /// The timestamp the decoder stamps on this packet's frame: pts, else dts.
    #[must_use]
    pub fn frame_pts(&self) -> Option<i64> {
        self.pts.or(self.dts)
    }
}

/// One output frame, in presentation order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameEntry {
    /// Presentation timestamp in ticks.
    pub pts: i64,
    /// Index of this frame's packet in [`FrameIndex::entries`] (decode order).
    pub decode_idx: usize,
    /// Decode index of the keyframe packet decoding must start from to reach this frame.
    pub start_idx: usize,
}

/// One group of pictures: a keyframe and the frames presented before the next keyframe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gop {
    /// Decode index of the keyframe packet.
    pub key_packet: usize,
    /// Presentation index of the keyframe's frame.
    pub first_frame: usize,
    /// Frames in presentation order from `first_frame` up to the next keyframe.
    pub frames: usize,
}

/// Why an index could not be built or read.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum IndexError {
    /// A packet has neither pts nor dts, so its frame cannot be placed on the timeline.
    #[error("packet {decode_idx} has no timestamps")]
    NoTimestamps {
        /// Decode index of the packet.
        decode_idx: usize,
    },
    /// The stream has no decodable frames (no packets, or no keyframe).
    #[error("the stream has no decodable frames")]
    Empty,
    /// A stored index is damaged or from a newer format.
    #[error("stored frame index is unreadable: {0}")]
    BadBlob(&'static str),
}

impl IndexError {
    /// This error as the [`MediaError`] for `path`.
    #[must_use]
    pub fn into_media(self, path: &std::path::Path) -> MediaError {
        MediaError::Corrupt {
            path: crate::path::display(path),
            message: self.to_string(),
        }
    }
}

/// How the packet scan went, besides the packets.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanInfo {
    /// Seek with the keyframe's pts instead of its dts (Matroska: cues are pts).
    pub seek_by_pts: bool,
    /// Corrupt packets the demuxer skipped during the scan.
    pub demux_errors: u32,
    /// The error that ended the scan early, if it was not a clean end of file.
    pub ended_by: Option<String>,
}

const NOPTS: i64 = i64::MIN;

/// Lookup tables for [`FrameIndex::locate`], built on first use.
#[derive(Clone, Debug)]
struct Lookup {
    dts_pts: HashMap<(i64, i64), usize>,
    pts: HashMap<i64, usize>,
    pos: HashMap<i64, usize>,
}

/// The packet-scan index of one video stream.
#[derive(Clone, Debug)]
pub struct FrameIndex {
    stream: u32,
    time_base: TimeBase,
    entries: Vec<IndexEntry>,
    frames: Vec<FrameEntry>,
    undecodable: usize,
    scan: ScanInfo,
    lookup: OnceLock<Lookup>,
}

impl PartialEq for FrameIndex {
    fn eq(&self, other: &FrameIndex) -> bool {
        // Everything else is derived from these.
        self.stream == other.stream
            && self.time_base == other.time_base
            && self.entries == other.entries
            && self.scan == other.scan
    }
}

impl Eq for FrameIndex {}

impl FrameIndex {
    /// Builds the index from packets in demux order.
    ///
    /// Seek-start rule: a frame T starts from the **latest keyframe K in decode order with
    /// `decode(K) <= decode(T)` and `pts(K) <= pts(T)`**. Frames decoded after K but shown before
    /// it (open-GOP leading / RASL pictures) may reference the previous GOP, so they fall back to
    /// the previous keyframe. Packets before the first keyframe cannot be reached by a seek and
    /// are counted in [`undecodable`](Self::undecodable).
    pub fn from_entries(
        stream: u32,
        time_base: TimeBase,
        entries: Vec<IndexEntry>,
        scan: ScanInfo,
    ) -> Result<FrameIndex, IndexError> {
        let mut frames = Vec::with_capacity(entries.len());
        let mut keys: Vec<usize> = Vec::new();
        let mut undecodable = 0;
        for (d, p) in entries.iter().enumerate() {
            let Some(pts) = p.frame_pts() else {
                return Err(IndexError::NoTimestamps { decode_idx: d });
            };
            if p.is_key {
                keys.push(d);
            }
            if p.discard {
                continue;
            }
            // Scan back over keyframes (normally one step) for the latest one shown at or
            // before this frame.
            let start = keys
                .iter()
                .rev()
                .copied()
                .find(|&k| entries[k].frame_pts().is_some_and(|kp| kp <= pts));
            match start {
                Some(start_idx) => frames.push(FrameEntry {
                    pts,
                    decode_idx: d,
                    start_idx,
                }),
                None => undecodable += 1,
            }
        }
        if frames.is_empty() {
            return Err(IndexError::Empty);
        }
        frames.sort_by_key(|f| f.pts);
        Ok(FrameIndex {
            stream,
            time_base,
            entries,
            frames,
            undecodable,
            scan,
            lookup: OnceLock::new(),
        })
    }

    /// Container index of the indexed video stream.
    #[must_use]
    pub fn stream(&self) -> u32 {
        self.stream
    }

    /// The stream time base all timestamps are in.
    #[must_use]
    pub fn time_base(&self) -> TimeBase {
        self.time_base
    }

    /// All packets of the stream in demux (decode) order.
    #[must_use]
    pub fn entries(&self) -> &[IndexEntry] {
        &self.entries
    }

    /// The decodable frames in presentation order.
    #[must_use]
    pub fn frames(&self) -> &[FrameEntry] {
        &self.frames
    }

    /// Number of decodable frames.
    #[must_use]
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Packets with a timestamp that precede the first usable keyframe (not decodable after a
    /// seek).
    #[must_use]
    pub fn undecodable(&self) -> usize {
        self.undecodable
    }

    /// How the scan went.
    #[must_use]
    pub fn scan(&self) -> &ScanInfo {
        &self.scan
    }

    /// The time of frame `n` (presentation order) in flicks. `None` past the end, or if the time
    /// does not fit a [`Time`].
    #[must_use]
    pub fn time_of(&self, n: usize) -> Option<Time> {
        self.time_base.to_time(self.frames.get(n)?.pts)
    }

    /// The presentation index of the frame on screen at `t`: the last frame whose time (as
    /// [`time_of`](Self::time_of) reports it) is at or before `t`, or the first frame for a `t`
    /// before it. (An index always has a frame.) `frame_at(time_of(n)) == n` for every frame,
    /// also when a tick is not a whole number of flicks (MP4's 1/30720 at 60 fps).
    #[must_use]
    pub fn frame_at(&self, t: Time) -> usize {
        // Compare the frames' rounded times, not ticks: `floor_ticks(time_of(n))` falls one
        // tick short when `time_of` rounded down.
        let after = self.frames.partition_point(|f| {
            self.time_base
                .to_time(f.pts)
                .map_or(f.pts < 0, |ft| ft <= t)
        });
        after.saturating_sub(1)
    }

    /// Presentation index of the frame with exactly this pts (ticks).
    #[must_use]
    pub fn frame_by_pts(&self, pts: i64) -> Option<usize> {
        self.frames.binary_search_by_key(&pts, |f| f.pts).ok()
    }

    /// Where the stream starts: the time of the first frame.
    #[must_use]
    pub fn start_time(&self) -> Option<Time> {
        self.time_of(0)
    }

    /// Length of the stream from the first frame to the end of the last one: the last frame is
    /// given the duration of the gap before it. `None` if it does not fit a [`Duration`].
    #[must_use]
    pub fn duration(&self) -> Option<Duration> {
        let first = self.frames.first()?.pts;
        let last = self.frames.last()?.pts;
        let tail = match self.frames.len() {
            0 | 1 => 0,
            n => last - self.frames[n - 2].pts,
        };
        let ticks = last.checked_sub(first)?.checked_add(tail)?;
        Duration::from_flicks(self.time_base.to_time(ticks)?.flicks()).ok()
    }

    /// When the last frame stops being on screen: it lasts as long as the gap before it. A
    /// decoder answers seeks at or past it with [`MediaError::OutOfRange`]. `None` for a stream
    /// of one picture (a still has no end).
    #[must_use]
    pub fn end_time(&self) -> Option<Time> {
        let n = self.frames.len();
        if n < 2 {
            return None;
        }
        let last = self.frames[n - 1].pts;
        let end = last.saturating_add(last - self.frames[n - 2].pts);
        Some(self.time_base.to_time(end).unwrap_or(Time::MAX))
    }

    /// Presentation indices of the keyframes, in order.
    pub fn key_frames(&self) -> impl Iterator<Item = usize> + '_ {
        self.frames
            .iter()
            .enumerate()
            .filter(|(_, f)| self.entries[f.decode_idx].is_key)
            .map(|(i, _)| i)
    }

    /// The pts (ticks) of every keyframe, in presentation order.
    #[must_use]
    pub fn keyframe_pts(&self) -> Vec<i64> {
        self.key_frames().map(|i| self.frames[i].pts).collect()
    }

    /// The group-of-pictures map: one [`Gop`] per keyframe, in presentation order. Frames shown
    /// before the first keyframe cannot exist in the index, so the first GOP starts at frame 0.
    #[must_use]
    pub fn gops(&self) -> Vec<Gop> {
        let keys: Vec<usize> = self.key_frames().collect();
        keys.iter()
            .enumerate()
            .map(|(i, &first_frame)| {
                let end = keys.get(i + 1).copied().unwrap_or(self.frames.len());
                Gop {
                    key_packet: self.frames[first_frame].decode_idx,
                    first_frame,
                    frames: end - first_frame,
                }
            })
            .collect()
    }

    /// Keyframe interval statistics in frames, in presentation order: `(count, min, max, mean)`.
    /// A single keyframe (or none) reports the frame count as every length.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // counts far below 2^52
    pub fn gop_stats(&self) -> (usize, usize, usize, f64) {
        let keys: Vec<usize> = self.key_frames().collect();
        if keys.len() < 2 {
            let n = self.frames.len();
            return (keys.len(), n, n, n as f64);
        }
        let gaps: Vec<usize> = keys.windows(2).map(|w| w[1] - w[0]).collect();
        let mean = gaps.iter().sum::<usize>() as f64 / gaps.len() as f64;
        (
            keys.len(),
            gaps.iter().copied().min().unwrap_or(0),
            gaps.iter().copied().max().unwrap_or(0),
            mean,
        )
    }

    /// Smallest and largest distance between consecutive frames, in ticks.
    #[must_use]
    pub fn frame_deltas(&self) -> (i64, i64) {
        let d = self.frames.windows(2).map(|w| w[1].pts - w[0].pts);
        let lo = d.clone().min().unwrap_or(0);
        let hi = d.max().unwrap_or(0);
        (lo, hi)
    }

    /// Whether frame durations differ by more than one tick plus 1 % (rounding of constant rates
    /// such as 1/60 s in a 1 ms Matroska time base is not VFR).
    #[must_use]
    pub fn is_vfr(&self) -> bool {
        let (lo, hi) = self.frame_deltas();
        is_vfr_range(lo, hi)
    }

    /// Largest number of packets a seek has to feed the decoder (decode-forward distance).
    #[must_use]
    pub fn max_decode_distance(&self) -> usize {
        self.frames
            .iter()
            .map(|f| f.decode_idx - f.start_idx + 1)
            .max()
            .unwrap_or(0)
    }

    /// Timestamp to hand the demuxer when seeking to keyframe packet `k` (a decode index).
    ///
    /// MP4/MOV index entries are DTS and MPEG-TS bisects on DTS, so dts is used there; Matroska
    /// cues are pts ([`ScanInfo::seek_by_pts`]). Landing too late is detected and retried by the
    /// caller; landing early only costs dropped (never decoded) packets.
    #[must_use]
    pub fn seek_ts(&self, k: usize) -> i64 {
        let Some(p) = self.entries.get(k) else {
            return i64::MIN;
        };
        let ts = if self.scan.seek_by_pts {
            p.pts.or(p.dts)
        } else {
            p.dts.or(p.pts)
        };
        ts.unwrap_or(i64::MIN)
    }

    /// Keyframe decode indices that precede `k` (for landing retries), nearest first.
    pub fn keys_before(&self, k: usize) -> impl Iterator<Item = usize> + '_ {
        self.entries[..k.min(self.entries.len())]
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, p)| p.is_key)
            .map(|(i, _)| i)
    }

    /// Decode index of a packet seen after a seek: by (dts, pts), then pts alone (dts can be
    /// reconstructed differently after a seek), then byte position.
    #[must_use]
    pub fn locate(&self, dts: Option<i64>, pts: Option<i64>, pos: i64) -> Option<usize> {
        let l = self.lookup.get_or_init(|| {
            let mut l = Lookup {
                dts_pts: HashMap::with_capacity(self.entries.len()),
                pts: HashMap::with_capacity(self.entries.len()),
                pos: HashMap::with_capacity(self.entries.len()),
            };
            for (d, p) in self.entries.iter().enumerate() {
                l.dts_pts
                    .entry((p.dts.unwrap_or(NOPTS), p.pts.unwrap_or(NOPTS)))
                    .or_insert(d);
                if let Some(pts) = p.pts {
                    l.pts.entry(pts).or_insert(d);
                }
                if p.pos >= 0 {
                    l.pos.entry(p.pos).or_insert(d);
                }
            }
            l
        });
        if let Some(&d) = l.dts_pts.get(&(dts.unwrap_or(NOPTS), pts.unwrap_or(NOPTS))) {
            return Some(d);
        }
        if let Some(&d) = pts.and_then(|p| l.pts.get(&p)) {
            return Some(d);
        }
        if pos >= 0 {
            return l.pos.get(&pos).copied();
        }
        None
    }
}

/// The VFR rule on the smallest and largest frame distance (ticks): see [`FrameIndex::is_vfr`].
#[must_use]
pub fn is_vfr_range(lo: i64, hi: i64) -> bool {
    hi - lo > 1 + hi / 100
}

// ---------------------------------------------------------------------------------------------
// Serialized form (the `blob` column of `cache.sqlite`'s `frame_index` table).
//
//   magic "CWFI" | version u8 | stream varint | tb.num varint | tb.den varint | flags u8
//   | demux_errors varint | ended_by (len varint + utf-8) | packet count varint
//   | per packet: flags u8 (1 key, 2 discard, 4 has pts, 8 has dts, 16 has pos)
//                 [pts: zigzag delta from the previous pts] [dts: zigzag delta from previous dts]
//                 [pos: zigzag delta from the previous pos]
//   | 8-byte BLAKE3 checksum of everything before it
//
// Frames, the GOP map and the lookup tables are rebuilt on load: they are derived data.
// ---------------------------------------------------------------------------------------------

const MAGIC: &[u8; 4] = b"CWFI";
/// Version of the serialized form. Bump it when the layout changes; older blobs read as
/// [`IndexError::BadBlob`] and are rebuilt by the caller.
pub const BLOB_VERSION: u8 = 1;
const CHECKSUM_LEN: usize = 8;
/// Longest `ended_by` text kept.
const MAX_ENDED_BY: usize = 256;

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v & 0x7f) as u8 | 0x80);
        v >>= 7;
    }
    out.push(v.to_le_bytes()[0]); // v < 0x80 here
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)).cast_unsigned()
}

fn unzigzag(v: u64) -> i64 {
    (v >> 1).cast_signed() ^ -((v & 1).cast_signed())
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, IndexError> {
        let (&b, rest) = self
            .bytes
            .split_first()
            .ok_or(IndexError::BadBlob("truncated"))?;
        self.bytes = rest;
        Ok(b)
    }

    fn varint(&mut self) -> Result<u64, IndexError> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.byte()?;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(IndexError::BadBlob("varint too long"))
    }

    fn u32(&mut self) -> Result<u32, IndexError> {
        u32::try_from(self.varint()?).map_err(|_| IndexError::BadBlob("value out of range"))
    }

    fn delta(&mut self, base: i64) -> Result<i64, IndexError> {
        base.checked_add(unzigzag(self.varint()?))
            .ok_or(IndexError::BadBlob("timestamp overflow"))
    }
}

impl FrameIndex {
    /// The serialized form, for `cache.sqlite`. Compact (about 4 bytes per packet) and
    /// checksummed; [`from_bytes`](Self::from_bytes) is the inverse.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + self.entries.len() * 5);
        out.extend_from_slice(MAGIC);
        out.push(BLOB_VERSION);
        put_varint(&mut out, u64::from(self.stream));
        put_varint(&mut out, u64::from(self.time_base.num));
        put_varint(&mut out, u64::from(self.time_base.den));
        out.push(u8::from(self.scan.seek_by_pts));
        put_varint(&mut out, u64::from(self.scan.demux_errors));
        let ended = self.scan.ended_by.as_deref().unwrap_or("");
        let ended = &ended.as_bytes()[..floor_char_boundary(ended, MAX_ENDED_BY)];
        put_varint(&mut out, ended.len() as u64);
        out.extend_from_slice(ended);
        put_varint(&mut out, self.entries.len() as u64);
        let (mut pts, mut dts, mut pos) = (0i64, 0i64, 0i64);
        for e in &self.entries {
            let flags = u8::from(e.is_key)
                | u8::from(e.discard) << 1
                | u8::from(e.pts.is_some()) << 2
                | u8::from(e.dts.is_some()) << 3
                | u8::from(e.pos >= 0) << 4;
            out.push(flags);
            if let Some(v) = e.pts {
                put_varint(&mut out, zigzag(v.wrapping_sub(pts)));
                pts = v;
            }
            if let Some(v) = e.dts {
                put_varint(&mut out, zigzag(v.wrapping_sub(dts)));
                dts = v;
            }
            if e.pos >= 0 {
                put_varint(&mut out, zigzag(e.pos.wrapping_sub(pos)));
                pos = e.pos;
            }
        }
        let sum = blake3::hash(&out);
        out.extend_from_slice(&sum.as_bytes()[..CHECKSUM_LEN]);
        out
    }

    /// Reads a [`to_bytes`](Self::to_bytes) blob. Damaged, truncated, checksum-failing and
    /// newer-version blobs are [`IndexError::BadBlob`], never a panic.
    pub fn from_bytes(bytes: &[u8]) -> Result<FrameIndex, IndexError> {
        if bytes.len() < MAGIC.len() + 1 + CHECKSUM_LEN || &bytes[..4] != MAGIC {
            return Err(IndexError::BadBlob("not a frame index"));
        }
        if bytes[4] != BLOB_VERSION {
            return Err(IndexError::BadBlob("unsupported version"));
        }
        let (body, sum) = bytes.split_at(bytes.len() - CHECKSUM_LEN);
        if blake3::hash(body).as_bytes()[..CHECKSUM_LEN] != *sum {
            return Err(IndexError::BadBlob("checksum mismatch"));
        }
        let mut r = Reader { bytes: &body[5..] };
        let stream = r.u32()?;
        let time_base =
            TimeBase::new(r.u32()?, r.u32()?).ok_or(IndexError::BadBlob("zero time base"))?;
        let seek_by_pts = r.byte()? != 0;
        let demux_errors = r.u32()?;
        let ended_len =
            usize::try_from(r.varint()?).map_err(|_| IndexError::BadBlob("value out of range"))?;
        if ended_len > r.bytes.len() {
            return Err(IndexError::BadBlob("truncated"));
        }
        let (text, rest) = r.bytes.split_at(ended_len);
        r.bytes = rest;
        let ended_by = if text.is_empty() {
            None
        } else {
            Some(
                std::str::from_utf8(text)
                    .map_err(|_| IndexError::BadBlob("invalid text"))?
                    .to_owned(),
            )
        };
        let count =
            usize::try_from(r.varint()?).map_err(|_| IndexError::BadBlob("value out of range"))?;
        // Every packet takes at least one byte: a larger count is a lie, and must not allocate.
        if count > r.bytes.len() {
            return Err(IndexError::BadBlob("truncated"));
        }
        let mut entries = Vec::with_capacity(count);
        let (mut pts, mut dts, mut pos) = (0i64, 0i64, 0i64);
        for _ in 0..count {
            let flags = r.byte()?;
            let mut entry = IndexEntry {
                pts: None,
                dts: None,
                is_key: flags & 1 != 0,
                discard: flags & 2 != 0,
                pos: -1,
            };
            if flags & 4 != 0 {
                pts = r.delta(pts)?;
                entry.pts = Some(pts);
            }
            if flags & 8 != 0 {
                dts = r.delta(dts)?;
                entry.dts = Some(dts);
            }
            if flags & 16 != 0 {
                pos = r.delta(pos)?;
                entry.pos = pos;
            }
            entries.push(entry);
        }
        if !r.bytes.is_empty() {
            return Err(IndexError::BadBlob("trailing bytes"));
        }
        FrameIndex::from_entries(
            stream,
            time_base,
            entries,
            ScanInfo {
                seek_by_pts,
                demux_errors,
                ended_by,
            },
        )
        .map_err(|_| IndexError::BadBlob("inconsistent packets"))
    }
}

/// Largest index `<= max` that is a char boundary of `s` (`str::floor_char_boundary` is
/// unstable).
fn floor_char_boundary(s: &str, max: usize) -> usize {
    let mut i = max.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkt(pts: i64, dts: i64, key: bool) -> IndexEntry {
        IndexEntry {
            pts: Some(pts),
            dts: Some(dts),
            is_key: key,
            discard: false,
            pos: -1,
        }
    }

    const TB1: TimeBase = TimeBase { num: 1, den: 1 };

    fn index(tb: TimeBase, p: Vec<IndexEntry>) -> FrameIndex {
        FrameIndex::from_entries(0, tb, p, ScanInfo::default()).expect("index")
    }

    /// Closed GOP, I P B B (decode order) = I B B P (display), two GOPs of 4.
    #[test]
    fn closed_gop_with_b_frames_starts_at_own_gop_keyframe() {
        let ix = index(
            TB1,
            vec![
                pkt(0, -2, true),
                pkt(3, -1, false),
                pkt(1, 0, false),
                pkt(2, 1, false),
                pkt(4, 2, true),
                pkt(7, 3, false),
                pkt(5, 4, false),
                pkt(6, 5, false),
            ],
        );
        let pts: Vec<i64> = ix.frames().iter().map(|f| f.pts).collect();
        assert_eq!(pts, (0..8).collect::<Vec<_>>());
        let starts: Vec<usize> = ix.frames().iter().map(|f| f.start_idx).collect();
        assert_eq!(starts, vec![0, 0, 0, 0, 4, 4, 4, 4]);
        assert_eq!(ix.max_decode_distance(), 4);
        assert_eq!(ix.keyframe_pts(), vec![0, 4]);
        assert_eq!(
            ix.gops(),
            vec![
                Gop {
                    key_packet: 0,
                    first_frame: 0,
                    frames: 4
                },
                Gop {
                    key_packet: 4,
                    first_frame: 4,
                    frames: 4
                },
            ]
        );
        assert_eq!(ix.gop_stats(), (2, 4, 4, 4.0));
    }

    /// Open GOP: the second keyframe (pts 6) is followed in decode order by two leading B-frames
    /// (pts 4, 5) that reference the previous GOP, so they must start from the first keyframe.
    #[test]
    fn open_gop_leading_frames_start_from_previous_keyframe() {
        let ix = index(
            TB1,
            vec![
                pkt(0, -1, true),
                pkt(3, 0, false),
                pkt(1, 1, false),
                pkt(2, 2, false),
                pkt(6, 3, true),
                pkt(4, 4, false),
                pkt(5, 5, false),
                pkt(7, 6, false),
            ],
        );
        let by_pts = |t: i64| ix.frames()[ix.frame_by_pts(t).expect("frame")];
        assert_eq!(by_pts(4).start_idx, 0, "leading B of GOP 2 needs GOP 1");
        assert_eq!(by_pts(5).start_idx, 0);
        assert_eq!(by_pts(6).start_idx, 4, "the CRA itself");
        assert_eq!(by_pts(7).start_idx, 4, "trailing picture");
        // The GOP map follows presentation order: GOP 2 starts at its keyframe (frame 6).
        let gops = ix.gops();
        assert_eq!(gops.len(), 2);
        assert_eq!((gops[1].first_frame, gops[1].frames), (6, 2));
    }

    #[test]
    fn discard_packets_are_not_frames_and_leading_packets_are_undecodable() {
        let mut p = vec![
            pkt(-1, -3, false),
            pkt(0, -2, true),
            pkt(2, -1, false),
            pkt(1, 0, false),
        ];
        p.push(IndexEntry {
            discard: true,
            ..pkt(3, 1, false)
        });
        let ix = index(TB1, p);
        assert_eq!(ix.undecodable(), 1);
        assert_eq!(
            ix.frames().iter().map(|f| f.pts).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(ix.locate(Some(1), Some(3), -1), Some(4));
    }

    #[test]
    fn vfr_detection_ignores_time_base_rounding() {
        // 60 fps in a 1 ms time base: deltas 16/17.
        let cfr: Vec<_> = (0..20)
            .map(|i| pkt(i * 1000 / 60, i * 1000 / 60, i == 0))
            .collect();
        let tb = TimeBase { num: 1, den: 1000 };
        assert!(!index(tb, cfr).is_vfr());
        let vfr: Vec<_> = [0, 33, 66, 108, 141, 183]
            .iter()
            .map(|&t| pkt(t, t, t == 0))
            .collect();
        assert!(index(tb, vfr).is_vfr());
    }

    #[test]
    fn packets_without_timestamps_are_an_error_not_a_panic() {
        let p = vec![IndexEntry {
            pts: None,
            dts: None,
            is_key: true,
            discard: false,
            pos: 0,
        }];
        assert_eq!(
            FrameIndex::from_entries(0, TB1, p, ScanInfo::default()),
            Err(IndexError::NoTimestamps { decode_idx: 0 })
        );
        assert_eq!(
            FrameIndex::from_entries(0, TB1, vec![], ScanInfo::default()),
            Err(IndexError::Empty)
        );
        // Frames but no keyframe: nothing can be decoded.
        assert_eq!(
            FrameIndex::from_entries(0, TB1, vec![pkt(0, 0, false)], ScanInfo::default()),
            Err(IndexError::Empty)
        );
    }

    #[test]
    fn dts_only_packets_use_dts_as_the_frame_time() {
        let p = vec![
            IndexEntry {
                pts: None,
                ..pkt(0, 0, true)
            },
            IndexEntry {
                pts: None,
                ..pkt(0, 1, false)
            },
        ];
        let ix = index(TB1, p);
        assert_eq!(ix.keyframe_pts(), vec![0]);
        assert_eq!(ix.frames()[1].pts, 1);
    }

    #[test]
    fn times_map_through_the_table_not_through_a_rate() {
        // 1/1000 time base, irregular spacing (VFR).
        let tb = TimeBase { num: 1, den: 1000 };
        let ix = index(
            tb,
            [0, 40, 80, 150, 160]
                .iter()
                .map(|&t| pkt(t, t, t == 0))
                .collect(),
        );
        let ms = |ms: i64| Time::from_flicks(ms * 705_600).expect("time");
        assert_eq!(ix.time_of(3), Some(ms(150)));
        assert_eq!(ix.time_of(5), None);
        // On a frame, between frames, before the first, past the last.
        assert_eq!(ix.frame_at(ms(150)), 3);
        assert_eq!(ix.frame_at(ms(149)), 2);
        assert_eq!(ix.frame_at(ms(80)), 2);
        assert_eq!(ix.frame_at(Time::ZERO), 0);
        assert_eq!(ix.frame_at(Time::from_flicks(-5).expect("time")), 0);
        assert_eq!(ix.frame_at(ms(10_000)), 4);
        // The last frame lasts as long as the gap before it.
        assert_eq!(ix.duration().map(Duration::flicks), Some(170 * 705_600));
        assert_eq!(ix.start_time(), Some(Time::ZERO));
        assert_eq!(ix.end_time(), Some(ms(170)));
        // A single picture never ends.
        assert_eq!(index(tb, vec![pkt(0, 0, true)]).end_time(), None);
    }

    #[test]
    fn frame_at_inverts_time_of_when_ticks_are_not_whole_flicks() {
        // MP4's 60 fps timescale 1/30720: one tick is 22 968.75 flicks, so `time_of` rounds
        // tick 3 down to 68 906 flicks, one flick before the tick's exact start.
        let tb = TimeBase {
            num: 1,
            den: 30_720,
        };
        let ix = index(tb, (0..40).map(|t| pkt(t, t, t == 0)).collect());
        for n in 0..ix.frame_count() {
            let t = ix.time_of(n).expect("time");
            assert_eq!(ix.frame_at(t), n, "frame {n} at {t:?}");
            if n > 0 {
                let before = Time::from_flicks(t.flicks() - 1).expect("time");
                assert_eq!(ix.frame_at(before), n - 1, "just before frame {n}");
            }
        }
    }

    #[test]
    fn time_base_conversions_round_to_the_nearest_flick() {
        // 1/90000 (MPEG-TS): 1 tick = 7840 flicks exactly.
        let tb = TimeBase {
            num: 1,
            den: 90_000,
        };
        assert_eq!(tb.to_time(90_000).map(Time::flicks), Some(705_600_000));
        assert_eq!(tb.to_time(-1).map(Time::flicks), Some(-7840));
        // 1/3 s: not an integral number of flicks; rounds to nearest.
        let third = TimeBase { num: 1, den: 3 };
        assert_eq!(third.to_time(1).map(Time::flicks), Some(235_200_000));
        assert_eq!(tb.floor_ticks(Time::from_flicks(7839).expect("time")), 0);
        assert_eq!(tb.floor_ticks(Time::from_flicks(7840).expect("time")), 1);
        assert_eq!(tb.floor_ticks(Time::from_flicks(-1).expect("time")), -1);
        assert!(TimeBase::new(0, 1).is_none());
    }

    #[test]
    fn locate_finds_packets_by_timestamps_then_position() {
        let mut p = vec![pkt(0, 0, true), pkt(1, 1, false)];
        p[1].pos = 500;
        let ix = index(TB1, p);
        assert_eq!(ix.locate(Some(1), Some(1), -1), Some(1));
        assert_eq!(ix.locate(Some(99), Some(1), -1), Some(1), "pts alone");
        assert_eq!(ix.locate(None, None, 500), Some(1), "position");
        assert_eq!(ix.locate(None, None, -1), None);
        assert_eq!(ix.keys_before(1).collect::<Vec<_>>(), vec![0]);
        assert_eq!(ix.seek_ts(1), 1);
        assert_eq!(ix.seek_ts(99), i64::MIN);
    }

    fn sample() -> FrameIndex {
        let mut p = vec![
            pkt(0, -2, true),
            pkt(3, -1, false),
            pkt(1, 0, false),
            pkt(2, 1, false),
            pkt(4, 2, true),
            IndexEntry {
                pts: None,
                ..pkt(0, 3, false)
            },
            IndexEntry {
                discard: true,
                ..pkt(5, 4, false)
            },
        ];
        for (i, e) in p.iter_mut().enumerate() {
            e.pos = if i == 2 {
                -1
            } else {
                48 + 1000 * i64::try_from(i).expect("small")
            };
        }
        FrameIndex::from_entries(
            3,
            TimeBase {
                num: 1001,
                den: 30_000,
            },
            p,
            ScanInfo {
                seek_by_pts: true,
                demux_errors: 2,
                ended_by: Some("read error: ünicode".to_owned()),
            },
        )
        .expect("index")
    }

    #[test]
    fn blob_round_trips_everything_derived() {
        let ix = sample();
        let bytes = ix.to_bytes();
        let back = FrameIndex::from_bytes(&bytes).expect("round trip");
        assert_eq!(back, ix);
        assert_eq!(back.frames(), ix.frames());
        assert_eq!(back.gops(), ix.gops());
        assert_eq!(back.scan().ended_by.as_deref(), Some("read error: ünicode"));
        assert_eq!(back.stream(), 3);
    }

    #[test]
    fn blob_is_compact() {
        let p: Vec<_> = (0..30_000)
            .map(|i| IndexEntry {
                pos: 1000 + i * 4096,
                ..pkt(i * 512, i * 512 - 1024, i % 250 == 0)
            })
            .collect();
        let bytes = index(
            TimeBase {
                num: 1,
                den: 15_360,
            },
            p,
        )
        .to_bytes();
        assert!(bytes.len() < 30_000 * 8, "{} bytes", bytes.len());
    }

    #[test]
    fn damaged_blobs_are_errors_never_panics() {
        let bytes = sample().to_bytes();
        // Every truncation, and a flipped bit anywhere, fails cleanly.
        for len in 0..bytes.len() {
            assert!(FrameIndex::from_bytes(&bytes[..len]).is_err(), "len {len}");
        }
        for i in 0..bytes.len() {
            let mut bad = bytes.clone();
            bad[i] ^= 0x10;
            assert!(FrameIndex::from_bytes(&bad).is_err(), "bit flip at {i}");
        }
        let mut newer = bytes.clone();
        newer[4] = BLOB_VERSION + 1;
        assert_eq!(
            FrameIndex::from_bytes(&newer),
            Err(IndexError::BadBlob("unsupported version"))
        );
        assert!(FrameIndex::from_bytes(b"").is_err());
        assert!(FrameIndex::from_bytes(&[0xff; 64]).is_err());
    }

    #[test]
    fn a_lying_packet_count_does_not_allocate() {
        // A valid header, then a count of 2^60 with a correct checksum.
        let mut body = Vec::new();
        body.extend_from_slice(MAGIC);
        body.push(BLOB_VERSION);
        for v in [0u64, 1, 1] {
            put_varint(&mut body, v);
        }
        body.push(0);
        put_varint(&mut body, 0);
        put_varint(&mut body, 0);
        put_varint(&mut body, 1 << 60);
        let sum = blake3::hash(&body);
        body.extend_from_slice(&sum.as_bytes()[..CHECKSUM_LEN]);
        assert_eq!(
            FrameIndex::from_bytes(&body),
            Err(IndexError::BadBlob("truncated"))
        );
    }

    #[test]
    fn into_media_names_the_path() {
        let e = IndexError::Empty.into_media(std::path::Path::new("a.mp4"));
        assert!(matches!(e, MediaError::Corrupt { path, .. } if path == "a.mp4"));
    }
}
