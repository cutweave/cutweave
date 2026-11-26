// SPDX-License-Identifier: GPL-3.0-or-later
//! A source played backwards (WP-2D.01): a reversed clip reads its media from the latest position
//! down to the start, in blocks that are decoded forwards and handed out frame by frame in
//! reverse. The frames are exactly the source's, so the result is the source reversed sample for
//! sample (no joins to smooth). Runs on the mixer's worker threads like every source.

use crate::error::SourceError;
use crate::source::{PcmFormat, PcmSource};

/// Frames decoded per block (about 0.17 s at 48 kHz): one seek of the inner source per block.
const BLOCK: u64 = 8192;

/// `inner` played backwards: after `seek(n)` the frames come as inner frames `n`, `n − 1`, … `0`,
/// then the source ends.
pub(crate) struct Reversed {
    inner: Box<dyn PcmSource>,
    channels: usize,
    /// The inner frame just after the next block to decode (exclusive end of what is left).
    pos: u64,
    /// The current block, already reversed: `len` frames, `at` of them handed out.
    block: Vec<f32>,
    at: usize,
    len: usize,
}

impl Reversed {
    pub(crate) fn new(inner: Box<dyn PcmSource>) -> Reversed {
        let channels = usize::from(inner.format().channels).max(1);
        Reversed {
            inner,
            channels,
            pos: 0,
            block: Vec::new(),
            at: 0,
            len: 0,
        }
    }

    /// Decodes the block before `pos` and reverses it; `false` at the start of the source.
    fn refill(&mut self) -> Result<bool, SourceError> {
        if self.pos == 0 {
            return Ok(false);
        }
        let start = self.pos.saturating_sub(BLOCK);
        let frames = usize::try_from(self.pos - start).unwrap_or(0);
        let c = self.channels;
        self.block.resize(frames * c, 0.0);
        self.inner.seek(start)?;
        let mut got = 0;
        while got < frames {
            let n = self.inner.read(&mut self.block[got * c..frames * c])?;
            if n == 0 {
                break;
            }
            got += n;
        }
        // Past the end of the stream is silence.
        self.block[got * c..].fill(0.0);
        // Reverse the order of the frames, keeping each frame's channels in order.
        for i in 0..frames / 2 {
            let j = frames - 1 - i;
            for ch in 0..c {
                self.block.swap(i * c + ch, j * c + ch);
            }
        }
        self.pos = start;
        self.at = 0;
        self.len = frames;
        Ok(true)
    }
}

impl PcmSource for Reversed {
    fn format(&self) -> PcmFormat {
        self.inner.format()
    }

    /// The next frame read is inner frame `frame`, then the ones before it.
    fn seek(&mut self, frame: u64) -> Result<(), SourceError> {
        self.pos = frame.saturating_add(1);
        self.at = 0;
        self.len = 0;
        Ok(())
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, SourceError> {
        if self.at == self.len && !self.refill()? {
            return Ok(0);
        }
        let c = self.channels;
        let n = (out.len() / c).min(self.len - self.at);
        out[..n * c].copy_from_slice(&self.block[self.at * c..(self.at + n) * c]);
        self.at += n;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation
    )]

    use cutweave_model::SampleRate;

    use super::*;

    /// Frame `n` holds `(n, -n)`; `len` frames.
    struct Ramp {
        len: u64,
        pos: u64,
    }

    impl PcmSource for Ramp {
        fn format(&self) -> PcmFormat {
            PcmFormat::new(SampleRate::HZ_48_000, 2)
        }
        fn seek(&mut self, frame: u64) -> Result<(), SourceError> {
            self.pos = frame;
            Ok(())
        }
        fn read(&mut self, out: &mut [f32]) -> Result<usize, SourceError> {
            // Short reads on purpose: at most 1000 frames.
            let n = (out.len() / 2)
                .min(1000)
                .min(self.len.saturating_sub(self.pos) as usize);
            for i in 0..n {
                let f = (self.pos + i as u64) as f32;
                out[2 * i] = f;
                out[2 * i + 1] = -f;
            }
            self.pos += n as u64;
            Ok(n)
        }
    }

    #[test]
    fn frames_come_backwards_across_blocks_and_end_at_the_start() {
        let mut r = Reversed::new(Box::new(Ramp {
            len: 20_000,
            pos: 0,
        }));
        r.seek(19_999).unwrap();
        let mut got = Vec::new();
        let mut buf = vec![0.0; 2 * 777];
        loop {
            let n = r.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..2 * n]);
        }
        assert_eq!(got.len(), 2 * 20_000);
        for (i, f) in got.as_chunks::<2>().0.iter().enumerate() {
            let want = (19_999 - i) as f32;
            assert_eq!(f, &[want, -want], "frame {i}");
        }
    }

    #[test]
    fn past_the_end_is_silence_then_the_last_frames() {
        let mut r = Reversed::new(Box::new(Ramp { len: 10, pos: 0 }));
        r.seek(12).unwrap();
        let mut buf = vec![1.0; 2 * 13];
        let mut n = 0;
        while n < 13 {
            n += r.read(&mut buf[2 * n..]).unwrap();
        }
        assert_eq!(&buf[..6], &[0.0; 6]);
        assert_eq!(&buf[6..8], &[9.0, -9.0]);
        assert_eq!(&buf[24..26], &[0.0, -0.0]);
        assert_eq!(r.read(&mut buf).unwrap(), 0);
    }
}
