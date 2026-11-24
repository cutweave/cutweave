// SPDX-License-Identifier: GPL-3.0-or-later
//! Failure handling on broken input: every stage must end in a value (frames decoded or a typed
//! error), never a panic, a hang or a crash.
//!
//! `check` runs in-process under `catch_unwind` (catches Rust panics, including ones inside
//! ffmpeg-next). The CLI's `robust --isolate` additionally runs each file in a child process so a
//! native crash inside FFmpeg (access violation, abort) is observed as an exit status instead of
//! taking the harness down.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::time::Instant;

use crate::source::{Options, VideoSource};
use crate::util::Rng;

/// What happened to one file.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub file: String,
    pub bytes: u64,
    /// Stage reached: `open` (index scan + decoder), `decode`, `seek`, `done`.
    pub stage: String,
    /// Error kind and message if a stage returned an error.
    pub error: Option<String>,
    pub frames_indexed: usize,
    pub frames_decoded: usize,
    pub seeks_ok: usize,
    pub seeks_err: usize,
    pub decode_errors: u64,
    pub demux_errors: u64,
    pub panicked: Option<String>,
    pub ms: f64,
    /// Frames FFmpeg returned with concealed errors (corrupt flag / decode_error_flags).
    pub corrupt_frames: u64,
}

impl Outcome {
    /// One CSV-safe line.
    pub fn line(&self) -> String {
        format!(
            "{},{},{},{},{},{},{},{},{},{},{},{:.1},{}",
            self.file,
            self.bytes,
            self.stage,
            self.error.as_deref().unwrap_or("").replace(',', ";"),
            self.frames_indexed,
            self.frames_decoded,
            self.seeks_ok,
            self.seeks_err,
            self.decode_errors,
            self.demux_errors,
            self.panicked.as_deref().unwrap_or("").replace(',', ";"),
            self.ms,
            self.corrupt_frames
        )
    }

    /// CSV header matching `line`.
    pub const HEADER: &'static str = "file,bytes,stage,error,frames_indexed,frames_decoded,seeks_ok,seeks_err,decode_errors,demux_errors,panic,ms,corrupt_frames";
}

/// Opens, decodes everything and does 25 random seeks, catching panics.
pub fn check(path: &Path) -> Outcome {
    let started = Instant::now();
    let mut o = Outcome {
        file: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        bytes: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        stage: "open".into(),
        ..Outcome::default()
    };
    let res = catch_unwind(AssertUnwindSafe(|| run(path, &mut o)));
    if let Err(p) = res {
        o.panicked = Some(
            p.downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_string()))
                .unwrap_or_else(|| "panic".into()),
        );
    }
    o.ms = started.elapsed().as_secs_f64() * 1e3;
    o
}

fn run(path: &Path, o: &mut Outcome) {
    let mut src = match VideoSource::open(path, Options::default()) {
        Ok(s) => s,
        Err(e) => {
            o.error = Some(format!("{}: {e}", e.kind()));
            return;
        }
    };
    o.frames_indexed = src.index.frames.len();
    o.stage = "decode".into();
    let cap = o.frames_indexed * 2 + 1000;
    loop {
        match src.next_frame() {
            Ok(true) => {
                o.frames_decoded += 1;
                if o.frames_decoded > cap {
                    o.error = Some("runaway: more frames than indexed".into());
                    break;
                }
            }
            Ok(false) => break,
            Err(e) => {
                o.error = Some(format!("{}: {e}", e.kind()));
                break;
            }
        }
    }
    o.stage = "seek".into();
    let mut rng = Rng::new(0xB20C);
    for _ in 0..25 {
        match src.seek_frame(rng.below(o.frames_indexed)) {
            Ok(()) => o.seeks_ok += 1,
            Err(e) => {
                o.seeks_err += 1;
                if o.error.is_none() {
                    o.error = Some(format!("{}: {e}", e.kind()));
                }
            }
        }
    }
    o.decode_errors = src.stats.decode_errors;
    o.corrupt_frames = src.stats.corrupt_frames;
    o.demux_errors = src.stats.demux_errors + u64::from(src.index.demux_errors);
    o.stage = "done".into();
}

/// Deterministic corruptions of a valid file for the fuzz pass: (label, bytes).
/// Lazy: one corrupted copy in memory at a time (sources can be ~100 MB).
pub fn mutate(data: &[u8], cases: usize, seed: u64) -> impl Iterator<Item = (String, Vec<u8>)> {
    let mut rng = Rng::new(seed);
    (0..cases).map(move |i| {
        let mut d = data.to_vec();
        let len = d.len().max(1);
        let label = match i % 5 {
            0 => {
                let cut = rng.below(len);
                d.truncate(cut);
                format!("truncate@{cut}")
            }
            1 => {
                let n = 1 + rng.below(64);
                for _ in 0..n {
                    let at = rng.below(len);
                    d[at] ^= 1 << rng.below(8);
                }
                format!("bitflip x{n}")
            }
            2 => {
                let at = rng.below(len);
                let n = (1 + rng.below(64 * 1024)).min(len - at);
                d[at..at + n].fill(0);
                format!("zero {n}B @{at}")
            }
            3 => {
                let n = (1 + rng.below(4096)).min(len);
                for b in &mut d[..n] {
                    *b = (rng.next_u64() & 0xFF) as u8;
                }
                format!("header garbage {n}B")
            }
            _ => {
                // Swap two 4 KiB blocks: valid data in the wrong place.
                let a = rng.below(len.saturating_sub(4096).max(1));
                let b = rng.below(len.saturating_sub(4096).max(1));
                if a + 4096 <= len && b + 4096 <= len && (a + 4096 <= b || b + 4096 <= a) {
                    let tmp: Vec<u8> = d[a..a + 4096].to_vec();
                    d.copy_within(b..b + 4096, a);
                    d[b..b + 4096].copy_from_slice(&tmp);
                }
                format!("swap 4KiB @{a}<->{b}")
            }
        };
        (label, d)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutations_are_deterministic_and_change_the_data() {
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let a: Vec<_> = mutate(&data, 10, 1).collect();
        let b: Vec<_> = mutate(&data, 10, 1).collect();
        assert_eq!(a.len(), 10);
        for ((la, da), (lb, db)) in a.iter().zip(&b) {
            assert_eq!(la, lb);
            assert_eq!(da, db);
        }
        assert!(a.iter().filter(|(_, d)| *d != data).count() >= 8);
    }

    #[test]
    fn missing_file_is_an_error_not_a_panic() {
        let o = check(Path::new("definitely-missing-file.mp4"));
        assert!(o.panicked.is_none());
        assert_eq!(o.stage, "open");
        assert!(o.error.is_some());
    }
}
