// SPDX-License-Identifier: GPL-3.0-or-later
//! Frame-exactness verification of one file against ffprobe/ffmpeg ground truth.
//!
//! Four passes, each frame checked three ways (pts vs ffprobe, Y-plane CRC vs ffmpeg, burned-in
//! barcode vs presentation index):
//! 1. **sequential**: decode every frame from the start;
//! 2. **random**: N seeded random seeks, each a cold keyframe seek + decode-forward
//!    (prints `frame-exact: N/N`);
//! 3. **edges**: first and last four frames, every keyframe and its neighbours, every open-GOP leading
//!    frame, forward and then in reverse (backward scrubbing);
//! 4. **scrub**: a forward sweep and single-frame stepping with `forward_continue` on.

use std::path::Path;

use crate::error::Result;
use crate::frame_check::Luma;
use crate::index::FrameIndex;
use crate::source::{Options, Stats, VideoSource};
use crate::truth::{self, Truth};
use crate::util::{Crc32, Rng};

/// What to run.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub seeks: usize,
    pub seed: u64,
    pub opts: Options,
    /// Check the burned-in barcode (generated media only).
    pub barcode: bool,
}

/// Pass/fail counts of one pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct Tally {
    pub ok: usize,
    pub total: usize,
}

impl Tally {
    fn add(&mut self, ok: bool) {
        self.total += 1;
        self.ok += usize::from(ok);
    }

    /// `ok/total`.
    pub fn show(&self) -> String {
        format!("{}/{}", self.ok, self.total)
    }

    /// Every check passed (and there was at least one).
    pub fn all(&self) -> bool {
        self.total > 0 && self.ok == self.total
    }
}

/// Result of verifying one file.
#[derive(Debug, Default)]
pub struct Report {
    pub file: String,
    pub codec: String,
    pub pix_fmt: String,
    pub width: u32,
    pub height: u32,
    pub frames: usize,
    pub packets: usize,
    pub keyframes: usize,
    pub gop_max: usize,
    pub max_decode_distance: usize,
    pub vfr: bool,
    pub open_gop_leading: usize,
    pub time_base: (i32, i32),
    pub index_scan_ms: f64,
    /// Index packets vs `ffprobe -show_packets` (pts, dts, key, discard).
    pub packets_match: Tally,
    /// Index presentation-order pts vs `ffprobe -show_frames`.
    pub frames_match: Tally,
    /// Truth CRC rows whose pts agrees with the ffprobe frame pts.
    pub crc_rows_aligned: Tally,
    /// `ffmpeg -vf select=eq(n\,N)` CRCs (first, last, one random frame) equal the framehash rows.
    pub select_spot: Tally,
    pub sequential: Tally,
    pub random: Tally,
    pub edges: Tally,
    pub scrub: Tally,
    pub barcode_checked: bool,
    pub stats: Stats,
    pub failures: Vec<String>,
}

impl Report {
    /// All passes exact.
    pub fn exact(&self) -> bool {
        self.packets_match.all()
            && self.frames_match.all()
            && self.select_spot.all()
            && self.sequential.all()
            && self.random.all()
            && self.edges.all()
            && self.scrub.all()
    }

    fn fail(&mut self, msg: String) {
        if self.failures.len() < 20 {
            self.failures.push(msg);
        }
    }
}

struct Checker<'a> {
    truth: &'a Truth,
    crc: Crc32,
    barcode: bool,
}

impl Checker<'_> {
    /// Checks the frame the source returned for presentation index `pi`. Returns a reason on failure.
    fn check(
        &self,
        index: &FrameIndex,
        pi: usize,
        frame: &ffmpeg_next::frame::Video,
    ) -> std::result::Result<(), String> {
        let got = frame.pts().or_else(|| frame.timestamp());
        let want = index.frames[pi].pts;
        if got != Some(want) {
            return Err(format!("frame {pi}: pts {got:?} != index {want}"));
        }
        if let Some(t) = self.truth.frames.get(pi).and_then(truth::TruthFrame::ts)
            && t != want
        {
            return Err(format!("frame {pi}: pts {want} != ffprobe {t}"));
        }
        let luma = Luma::of(frame)
            .ok_or_else(|| format!("frame {pi}: unsupported pixel format {:?}", frame.format()))?;
        match self.truth.crcs.get(pi) {
            Some(&(_, crc)) => {
                let c = luma.crc(&self.crc);
                if c != crc {
                    return Err(format!(
                        "frame {pi} (pts {want}): Y crc {c:08x} != ffmpeg {crc:08x}"
                    ));
                }
            }
            None => return Err(format!("frame {pi}: no reference CRC")),
        }
        if self.barcode {
            match luma.barcode() {
                Some(n) if n as usize == pi => {}
                other => return Err(format!("frame {pi}: barcode {other:?}")),
            }
        }
        Ok(())
    }
}

/// Verifies `path` (video stream picked by `av_find_best_stream`).
pub fn verify(out_dir: &Path, path: &Path, cfg: Config) -> Result<Report> {
    let index = FrameIndex::scan(path)?;
    let truth = truth::collect(out_dir, path, index.stream)?;
    let mut r = Report {
        file: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        frames: index.frames.len(),
        packets: index.packets.len(),
        keyframes: index.gop_stats().0,
        gop_max: index.gop_stats().2,
        max_decode_distance: index.max_decode_distance(),
        vfr: index.is_vfr(),
        time_base: index.time_base,
        index_scan_ms: index.scan_time.as_secs_f64() * 1e3,
        barcode_checked: cfg.barcode,
        ..Report::default()
    };
    compare_index(&index, &truth, &mut r);
    // The framehash rows must be the frames `-vf select=eq(n\,N)` picks (WBS wording): spot-check.
    let mut rng = Rng::new(cfg.seed ^ 0x5E1E);
    for n in [0, r.frames - 1, rng.below(r.frames)] {
        let ok = truth::select_crc(path, index.stream, n)
            .is_ok_and(|c| truth.crcs.get(n).is_some_and(|row| row.1 == c));
        r.select_spot.add(ok);
        if !ok {
            r.fail(format!(
                "select=eq(n,{n}) CRC differs from framehash row {n}"
            ));
        }
    }
    let leading = leading_frames(&index);
    r.open_gop_leading = leading.len();

    let checker = Checker {
        truth: &truth,
        crc: Crc32::default(),
        barcode: cfg.barcode,
    };
    let mut src = VideoSource::open_with_index(
        path,
        index,
        Options {
            forward_continue: false,
            ..cfg.opts
        },
    )?;
    {
        let d = src.decoder();
        r.codec = d.codec().map(|c| c.name().to_string()).unwrap_or_default();
        r.width = d.width();
        r.height = d.height();
    }

    // 1. Sequential.
    src.rewind()?;
    for pi in 0..r.frames {
        let res = match src.next_frame() {
            Ok(true) => {
                if pi == 0 {
                    r.pix_fmt = format!("{:?}", src.frame().format());
                }
                checker.check(&src.index, pi, src.frame())
            }
            Ok(false) => Err(format!("sequential: stream ended at frame {pi}")),
            Err(e) => Err(format!("sequential frame {pi}: {e}")),
        };
        r.sequential.add(res.is_ok());
        if let Err(m) = res {
            r.fail(m);
            if r.sequential.total - r.sequential.ok > 20 {
                break;
            }
        }
    }

    // 2. Random cold seeks.
    let mut rng = Rng::new(cfg.seed);
    for _ in 0..cfg.seeks {
        let pi = rng.below(r.frames);
        let res = seek_check(&mut src, &checker, pi);
        r.random.add(res.is_ok());
        if let Err(m) = res {
            r.fail(format!("random: {m}"));
        }
    }

    // 3. Edges, forward then reverse.
    let mut edges = edge_frames(&src.index, &leading);
    let rev: Vec<usize> = edges.iter().rev().copied().collect();
    edges.extend(rev);
    for pi in edges {
        let res = seek_check(&mut src, &checker, pi);
        r.edges.add(res.is_ok());
        if let Err(m) = res {
            r.fail(format!("edge: {m}"));
        }
    }

    // 4. Scrub: forward sweep (~120 positions) and 45 single-frame steps from the middle.
    src.set_options(cfg.opts.skip_nonref, true);
    let step = (r.frames / 120).max(1);
    let mid = r.frames / 2;
    let sweep: Vec<usize> = (0..r.frames)
        .step_by(step)
        .chain(mid..(mid + 45).min(r.frames))
        .collect();
    for pi in sweep {
        let res = seek_check(&mut src, &checker, pi);
        r.scrub.add(res.is_ok());
        if let Err(m) = res {
            r.fail(format!("scrub: {m}"));
        }
    }
    r.stats = src.stats;
    Ok(r)
}

fn seek_check(
    src: &mut VideoSource,
    checker: &Checker<'_>,
    pi: usize,
) -> std::result::Result<(), String> {
    src.seek_frame(pi).map_err(|e| format!("seek {pi}: {e}"))?;
    checker.check(&src.index, pi, src.frame())
}

fn compare_index(index: &FrameIndex, truth: &Truth, r: &mut Report) {
    if index.packets.len() != truth.packets.len() {
        r.fail(format!(
            "index has {} packets, ffprobe {}",
            index.packets.len(),
            truth.packets.len()
        ));
    }
    for (i, (a, b)) in index.packets.iter().zip(&truth.packets).enumerate() {
        let ok = a.pts == b.pts && a.dts == b.dts && a.key == b.key && a.discard == b.discard;
        r.packets_match.add(ok);
        if !ok {
            r.fail(format!("packet {i}: index {a:?} vs ffprobe {b:?}"));
        }
    }
    for _ in truth.packets.len()..index.packets.len().max(truth.packets.len()) {
        r.packets_match.add(false);
    }
    if index.frames.len() != truth.frames.len() {
        r.fail(format!(
            "index has {} frames, ffprobe decoded {}",
            index.frames.len(),
            truth.frames.len()
        ));
    }
    for (i, (a, b)) in index.frames.iter().zip(&truth.frames).enumerate() {
        let ok = Some(a.pts) == b.ts();
        r.frames_match.add(ok);
        if !ok {
            r.fail(format!(
                "frame {i}: index pts {} vs ffprobe {:?}",
                a.pts,
                b.ts()
            ));
        }
    }
    for _ in index.frames.len().min(truth.frames.len())..index.frames.len().max(truth.frames.len())
    {
        r.frames_match.add(false);
    }
    for (row, f) in truth.crcs.iter().zip(&truth.frames) {
        r.crc_rows_aligned.add(Some(row.0) == f.ts());
    }
}

/// Open-GOP leading frames: shown before the keyframe that precedes them in decode order.
pub fn leading_frames(index: &FrameIndex) -> Vec<usize> {
    let mut last_key = None;
    let mut leading_pts = Vec::new();
    for (d, p) in index.packets.iter().enumerate() {
        if p.key {
            last_key = Some(d);
        } else if let (Some(k), Some(pts)) = (last_key, p.frame_pts())
            && index.packets[k].frame_pts().is_some_and(|kp| pts < kp)
        {
            leading_pts.push(pts);
        }
    }
    leading_pts
        .iter()
        .filter_map(|&t| index.frame_by_pts(t))
        .collect()
}

/// First frame, last four frames, keyframes with neighbours, leading frames (capped at 300,
/// evenly sampled).
fn edge_frames(index: &FrameIndex, leading: &[usize]) -> Vec<usize> {
    let n = index.frames.len();
    // The last few frames: reaching them needs an EOF drain, which can leave frames behind in
    // the decoder (see `VideoSource::land_at`).
    let mut v: Vec<usize> = std::iter::once(0).chain(n.saturating_sub(4)..n).collect();
    for (pi, f) in index.frames.iter().enumerate() {
        if index.packets[f.decode_idx].key {
            v.extend([pi.saturating_sub(1), pi, (pi + 1).min(n - 1)]);
        }
    }
    v.extend_from_slice(leading);
    v.sort_unstable();
    v.dedup();
    if v.len() > 300 {
        let step = v.len() as f64 / 300.0;
        v = (0..300).map(|i| v[(i as f64 * step) as usize]).collect();
    }
    v
}
