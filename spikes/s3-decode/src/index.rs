// SPDX-License-Identifier: GPL-3.0-or-later
//! Frame index by packet scan (arch §7): every video packet in demux (decode) order, the frames in
//! presentation order, and for each frame the keyframe packet a seek must start decoding from.
//!
//! No decoding happens here; the scan reads packets only. VFR needs no special case: frames are
//! addressed by their own pts, never by `n / fps`.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use ffmpeg_next as ffmpeg;

use crate::error::{Error, Result, ff};
use crate::ffi;

/// One demuxed video packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PacketEntry {
    pub pts: Option<i64>,
    pub dts: Option<i64>,
    pub key: bool,
    /// `AV_PKT_FLAG_DISCARD`: decoded for reference, never output (edit-list pre-roll).
    pub discard: bool,
    /// Byte position in the file, -1 if unknown.
    pub pos: i64,
    pub size: u32,
}

impl PacketEntry {
    /// The timestamp the decoder will stamp on this packet's frame.
    pub fn frame_pts(&self) -> Option<i64> {
        self.pts.or(self.dts)
    }
}

/// One output frame, in presentation order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameEntry {
    pub pts: i64,
    /// Index of this frame's packet in `FrameIndex::packets` (decode order).
    pub decode_idx: usize,
    /// Decode index of the keyframe packet to start decoding from to reach this frame.
    pub start_idx: usize,
}

/// The packet-scan index of one video stream.
#[derive(Clone)]
pub struct FrameIndex {
    pub stream: usize,
    /// Stream time base (num, den).
    pub time_base: (i32, i32),
    /// All packets of the stream in demux order.
    pub packets: Vec<PacketEntry>,
    /// Decodable frames in presentation order.
    pub frames: Vec<FrameEntry>,
    /// Packets with a pts that precede the first usable keyframe (not decodable after a seek).
    pub undecodable: usize,
    /// Corrupt packets the demuxer skipped (`AVERROR_INVALIDDATA`) during the scan.
    pub demux_errors: u32,
    /// The error that ended the scan early, if it was not a clean EOF.
    pub ended_by: Option<String>,
    /// Wall time of the packet scan.
    pub scan_time: Duration,
    /// Seek with the keyframe's pts instead of its dts (Matroska: cues are pts, and the demuxer
    /// reports no dts for the first packets of a cluster run, which it reconstructs differently
    /// after a seek).
    pub seek_by_pts: bool,
    lookup: HashMap<(i64, i64), usize>,
    by_pts: HashMap<i64, usize>,
    by_pos: HashMap<i64, usize>,
}

const NOPTS: i64 = i64::MIN;

impl FrameIndex {
    /// Opens `path`, picks the best video stream and scans all its packets.
    pub fn scan(path: &Path) -> Result<Self> {
        let started = Instant::now();
        let p = path.to_str().ok_or(Error::NonUtf8Path)?;
        let mut input = ffmpeg::format::input(p).map_err(ff("avformat_open_input"))?;
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or(Error::NoVideoStream)?;
        let (stream_idx, tb) = (stream.index(), stream.time_base());
        let mut packets = Vec::new();
        let mut demux_errors = 0u32;
        let mut ended_by = None;
        loop {
            let mut pkt = ffmpeg::Packet::empty();
            match pkt.read(&mut input) {
                Ok(()) => {
                    if pkt.stream() == stream_idx {
                        packets.push(PacketEntry {
                            pts: pkt.pts(),
                            dts: pkt.dts(),
                            key: pkt.is_key(),
                            discard: ffi::packet_is_discard(&pkt),
                            pos: pkt.position() as i64,
                            size: u32::try_from(pkt.size()).unwrap_or(u32::MAX),
                        });
                    }
                }
                Err(ffmpeg::Error::Eof) => break,
                Err(ffmpeg::Error::InvalidData) => {
                    demux_errors += 1;
                    if demux_errors > 10_000 {
                        ended_by = Some("too many corrupt packets".to_string());
                        break;
                    }
                }
                Err(e) => {
                    ended_by = Some(e.to_string());
                    break;
                }
            }
        }
        let seek_by_pts = input.format().name().contains("matroska");
        let mut index =
            Self::from_packets(stream_idx, (tb.numerator(), tb.denominator()), packets)?;
        index.seek_by_pts = seek_by_pts;
        index.demux_errors = demux_errors;
        index.ended_by = ended_by;
        index.scan_time = started.elapsed();
        Ok(index)
    }

    /// Builds the index from packets in demux order (pure; unit-tested).
    ///
    /// Seek-start rule: a frame T starts from the **latest keyframe K in decode order with
    /// `decode(K) <= decode(T)` and `pts(K) <= pts(T)`**. Frames decoded after K but shown before it
    /// (open-GOP leading / RASL pictures) may reference the previous GOP, so they fall back to the
    /// previous keyframe. Packets before the first keyframe cannot be reached by a seek.
    pub fn from_packets(
        stream: usize,
        time_base: (i32, i32),
        packets: Vec<PacketEntry>,
    ) -> Result<Self> {
        let mut frames = Vec::with_capacity(packets.len());
        let mut keys: Vec<usize> = Vec::new();
        let mut undecodable = 0;
        let mut lookup = HashMap::with_capacity(packets.len());
        let mut by_pts = HashMap::with_capacity(packets.len());
        let mut by_pos = HashMap::with_capacity(packets.len());
        for (d, p) in packets.iter().enumerate() {
            lookup
                .entry((p.dts.unwrap_or(NOPTS), p.pts.unwrap_or(NOPTS)))
                .or_insert(d);
            if let Some(pts) = p.pts {
                by_pts.entry(pts).or_insert(d);
            }
            if p.pos >= 0 {
                by_pos.entry(p.pos).or_insert(d);
            }
            let Some(pts) = p.frame_pts() else {
                return Err(Error::NoTimestamps { decode_idx: d });
            };
            if p.key {
                keys.push(d);
            }
            if p.discard {
                continue;
            }
            // Scan back over keyframes (normally one step) for the latest one shown at or before T.
            let start = keys
                .iter()
                .rev()
                .copied()
                .find(|&k| packets[k].frame_pts().is_some_and(|kp| kp <= pts));
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
            return Err(Error::EmptyIndex);
        }
        frames.sort_by_key(|f| f.pts);
        Ok(FrameIndex {
            stream,
            time_base,
            packets,
            frames,
            undecodable,
            demux_errors: 0,
            ended_by: None,
            scan_time: Duration::ZERO,
            seek_by_pts: false,
            lookup,
            by_pts,
            by_pos,
        })
    }

    /// Decode index of a packet seen after a seek: by (dts, pts), then pts alone (dts can be
    /// reconstructed differently after a seek), then byte position.
    pub fn locate(&self, dts: Option<i64>, pts: Option<i64>, pos: i64) -> Option<usize> {
        if let Some(&d) = self
            .lookup
            .get(&(dts.unwrap_or(NOPTS), pts.unwrap_or(NOPTS)))
        {
            return Some(d);
        }
        if let Some(d) = pts.and_then(|p| self.by_pts.get(&p)) {
            return Some(*d);
        }
        if pos >= 0 {
            return self.by_pos.get(&pos).copied();
        }
        None
    }

    /// Presentation index of the frame with this pts.
    pub fn frame_by_pts(&self, pts: i64) -> Option<usize> {
        self.frames.binary_search_by_key(&pts, |f| f.pts).ok()
    }

    /// Timestamp to hand the demuxer when seeking to keyframe packet `k`.
    ///
    /// MP4/MOV index entries are DTS and MPEG-TS bisects on DTS, so dts is used there; Matroska
    /// cues are pts. Landing too late is detected and retried by the caller; landing early only
    /// costs dropped (never decoded) packets.
    pub fn seek_ts(&self, k: usize) -> i64 {
        let p = &self.packets[k];
        let ts = if self.seek_by_pts {
            p.pts.or(p.dts)
        } else {
            p.dts.or(p.pts)
        };
        ts.unwrap_or(i64::MIN)
    }

    /// Keyframe decode indices that precede `k` (for landing retries), nearest first.
    pub fn keys_before(&self, k: usize) -> impl Iterator<Item = usize> + '_ {
        self.packets[..k]
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, p)| p.key)
            .map(|(i, _)| i)
    }

    /// Keyframe interval statistics in frames, in presentation order: (count, min, max, mean).
    /// (In decode order an open GOP's interval looks one frame longer or shorter.)
    pub fn gop_stats(&self) -> (usize, usize, usize, f64) {
        let keys: Vec<usize> = self
            .frames
            .iter()
            .enumerate()
            .filter(|(_, f)| self.packets[f.decode_idx].key)
            .map(|(i, _)| i)
            .collect();
        if keys.len() < 2 {
            return (
                keys.len(),
                self.frames.len(),
                self.frames.len(),
                self.frames.len() as f64,
            );
        }
        let gaps: Vec<usize> = keys.windows(2).map(|w| w[1] - w[0]).collect();
        let mean = gaps.iter().sum::<usize>() as f64 / gaps.len() as f64;
        (
            keys.len(),
            *gaps.iter().min().unwrap_or(&0),
            *gaps.iter().max().unwrap_or(&0),
            mean,
        )
    }

    /// Distinct frame durations (pts deltas in presentation order), for VFR detection.
    pub fn frame_deltas(&self) -> (i64, i64) {
        let d: Vec<i64> = self
            .frames
            .windows(2)
            .map(|w| w[1].pts - w[0].pts)
            .collect();
        (
            d.iter().copied().min().unwrap_or(0),
            d.iter().copied().max().unwrap_or(0),
        )
    }

    /// True when frame durations differ by more than one time-base tick plus 1 % (rounding of
    /// CFR rates such as 1/60 s in a 1 ms Matroska time base is not VFR).
    pub fn is_vfr(&self) -> bool {
        let (lo, hi) = self.frame_deltas();
        hi - lo > 1 + hi / 100
    }

    /// Largest number of packets a seek has to feed the decoder (decode-forward distance).
    pub fn max_decode_distance(&self) -> usize {
        self.frames
            .iter()
            .map(|f| f.decode_idx - f.start_idx + 1)
            .max()
            .unwrap_or(0)
    }

    /// Seconds per time-base tick.
    pub fn tick(&self) -> f64 {
        f64::from(self.time_base.0) / f64::from(self.time_base.1.max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkt(pts: i64, dts: i64, key: bool) -> PacketEntry {
        PacketEntry {
            pts: Some(pts),
            dts: Some(dts),
            key,
            discard: false,
            pos: -1,
            size: 1,
        }
    }

    /// Closed GOP, I P B B (decode order) = I B B P (display), two GOPs of 4.
    #[test]
    fn closed_gop_with_b_frames_starts_at_own_gop_keyframe() {
        let p = vec![
            pkt(0, -2, true),
            pkt(3, -1, false),
            pkt(1, 0, false),
            pkt(2, 1, false),
            pkt(4, 2, true),
            pkt(7, 3, false),
            pkt(5, 4, false),
            pkt(6, 5, false),
        ];
        let ix = FrameIndex::from_packets(0, (1, 1), p).expect("index");
        let pts: Vec<i64> = ix.frames.iter().map(|f| f.pts).collect();
        assert_eq!(pts, (0..8).collect::<Vec<_>>());
        let starts: Vec<usize> = ix.frames.iter().map(|f| f.start_idx).collect();
        assert_eq!(starts, vec![0, 0, 0, 0, 4, 4, 4, 4]);
        assert_eq!(ix.max_decode_distance(), 4);
    }

    /// Open GOP: the second keyframe (pts 6) is followed in decode order by two leading B-frames
    /// (pts 4, 5) that reference the previous GOP, so they must start from the first keyframe.
    #[test]
    fn open_gop_leading_frames_start_from_previous_keyframe() {
        let p = vec![
            pkt(0, -1, true),
            pkt(3, 0, false),
            pkt(1, 1, false),
            pkt(2, 2, false),
            pkt(6, 3, true),
            pkt(4, 4, false),
            pkt(5, 5, false),
            pkt(7, 6, false),
        ];
        let ix = FrameIndex::from_packets(0, (1, 1), p).expect("index");
        let by_pts = |t: i64| ix.frames[ix.frame_by_pts(t).expect("frame")];
        assert_eq!(by_pts(4).start_idx, 0, "leading B of GOP 2 needs GOP 1");
        assert_eq!(by_pts(5).start_idx, 0);
        assert_eq!(by_pts(6).start_idx, 4, "the CRA itself");
        assert_eq!(by_pts(7).start_idx, 4, "trailing picture");
    }

    #[test]
    fn discard_packets_are_not_frames_and_leading_packets_are_undecodable() {
        let mut p = vec![
            pkt(-1, -3, false),
            pkt(0, -2, true),
            pkt(2, -1, false),
            pkt(1, 0, false),
        ];
        p.push(PacketEntry {
            discard: true,
            ..pkt(3, 1, false)
        });
        let ix = FrameIndex::from_packets(0, (1, 1), p).expect("index");
        assert_eq!(ix.undecodable, 1);
        assert_eq!(
            ix.frames.iter().map(|f| f.pts).collect::<Vec<_>>(),
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
        assert!(
            !FrameIndex::from_packets(0, (1, 1000), cfr)
                .expect("index")
                .is_vfr()
        );
        let vfr: Vec<_> = [0, 33, 66, 108, 141, 183]
            .iter()
            .map(|&t| pkt(t, t, t == 0))
            .collect();
        assert!(
            FrameIndex::from_packets(0, (1, 1000), vfr)
                .expect("index")
                .is_vfr()
        );
    }

    #[test]
    fn packets_without_timestamps_are_an_error_not_a_panic() {
        let p = vec![PacketEntry {
            pts: None,
            dts: None,
            key: true,
            discard: false,
            pos: 0,
            size: 1,
        }];
        assert!(matches!(
            FrameIndex::from_packets(0, (1, 1), p),
            Err(Error::NoTimestamps { decode_idx: 0 })
        ));
        assert!(matches!(
            FrameIndex::from_packets(0, (1, 1), vec![]),
            Err(Error::EmptyIndex)
        ));
    }
}
