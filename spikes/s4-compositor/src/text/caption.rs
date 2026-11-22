//! Caption animators (§11 caption styles): frame number → [`TextSpan`]s. Pure functions of the
//! frame (no clock, no RNG), so a caption frame is reproducible anywhere (§8).
//!
//! * [`pop_karaoke`]: words pop in on their start time (scale 0.55 → overshoot → 1, fade, rise),
//!   the active word gets a karaoke sweep; spoken words return to the base fill.
//! * [`glyph_storm`]: per-glyph wobble (transform, colour, opacity) for the 1000-glyph cost
//!   scene.

use crate::scene::{TextHighlight, TextSpan};

/// Word timing in sequence frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WordTiming {
    pub range: [u32; 2],
    pub start: i64,
    pub end: i64,
}

/// Splits on ASCII spaces and spaces words `step` frames apart starting at `first`.
#[must_use]
pub fn word_timings(text: &str, first: i64, step: i64) -> Vec<WordTiming> {
    let mut out: Vec<WordTiming> = Vec::new();
    let mut start = None;
    for (i, c) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        match (c.is_whitespace(), start) {
            (false, None) => start = Some(i),
            (true, Some(s)) => {
                let n = out.len() as i64;
                out.push(WordTiming {
                    range: [s as u32, i as u32],
                    start: first + n * step,
                    end: first + (n + 1) * step,
                });
                start = None;
            }
            _ => {}
        }
    }
    out
}

/// easeOutBack (overshoot ≈ 10%).
#[must_use]
pub fn ease_out_back(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let c1 = 1.701_58;
    let c3 = c1 + 1.0;
    let u = t - 1.0;
    1.0 + c3 * u * u * u + c1 * u * u
}

/// Pop + karaoke caption style.
#[must_use]
pub fn pop_karaoke(
    words: &[WordTiming],
    frame: i64,
    size_px: f32,
    highlight_srgb: [f32; 4],
) -> Vec<TextSpan> {
    const POP_FRAMES: f32 = 14.0;
    words
        .iter()
        .map(|w| {
            let mut s = TextSpan::new(w.range);
            if frame < w.start {
                s.opacity = 0.0;
                return s;
            }
            let t = (frame - w.start) as f32 / POP_FRAMES;
            if t < 1.0 {
                let k = 0.55 + 0.45 * ease_out_back(t);
                s.scale = [k, k];
                s.opacity = (t * 3.5).min(1.0);
                s.offset_px = [0.0, (1.0 - ease_out_back(t)) * 0.25 * size_px];
            }
            if frame < w.end {
                let p = (frame - w.start) as f32 / (w.end - w.start).max(1) as f32;
                s.highlight = Some(TextHighlight {
                    color_srgb: highlight_srgb,
                    progress: p,
                });
            }
            s
        })
        .collect()
}

/// `n` letters in 8-letter words (no spaces counted), for the glyph-cost scene.
#[must_use]
pub fn storm_text(n: usize) -> String {
    const WORDS: [&str; 4] = ["CUTWEAVE", "captions", "KARAOKEs", "popping!"];
    let mut s = String::new();
    let mut letters = 0;
    let mut i = 0;
    while letters < n {
        let w = WORDS[i % WORDS.len()];
        let take = (n - letters).min(w.len());
        if !s.is_empty() {
            s.push(' ');
        }
        s.push_str(&w[..take]);
        letters += take;
        i += 1;
    }
    s
}

/// Per-glyph animation of every non-space character of `text`.
#[must_use]
pub fn glyph_storm(text: &str, frame: i64) -> Vec<TextSpan> {
    let f = frame as f32 / 60.0;
    text.char_indices()
        .filter(|(_, c)| !c.is_whitespace())
        .enumerate()
        .map(|(n, (i, c))| {
            let ph = n as f32 * 0.37;
            let mut s = TextSpan::new([i as u32, (i + c.len_utf8()) as u32]);
            s.per_glyph = true;
            s.rotation_deg = 18.0 * (f * 2.1 + ph).sin();
            let k = 1.0 + 0.2 * (f * 3.3 + ph * 1.3).sin();
            s.scale = [k, k];
            s.offset_px = [0.0, 6.0 * (f * 4.0 + ph).sin()];
            s.opacity = 0.75 + 0.25 * (f * 1.7 + ph).cos();
            let hue = (f * 0.25 + n as f32 * 0.013).fract();
            s.fill_srgb = Some(hue_to_srgb(hue));
            s
        })
        .collect()
}

fn hue_to_srgb(h: f32) -> [f32; 4] {
    let f = |n: f32| {
        let k = (n + h * 6.0) % 6.0;
        1.0 - 0.55 * (k.min(4.0 - k).clamp(0.0, 1.0))
    };
    [f(5.0), f(3.0), f(1.0), 1.0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_pop_in_order_and_karaoke_progresses() {
        let text = "one two three";
        let w = word_timings(text, 10, 20);
        assert_eq!(w.len(), 3);
        assert_eq!(&text[w[1].range[0] as usize..w[1].range[1] as usize], "two");
        let s = pop_karaoke(&w, 5, 64.0, [1.0, 0.8, 0.0, 1.0]);
        assert!(
            s.iter().all(|s| s.opacity == 0.0),
            "nothing before the first word"
        );
        let s = pop_karaoke(&w, 39, 64.0, [1.0, 0.8, 0.0, 1.0]);
        assert_eq!(s[0].highlight, None, "spoken word is plain again");
        let hl = s[1].highlight.expect("active word");
        assert!((hl.progress - 0.45).abs() < 1e-6);
        assert!(s[1].scale[0] > 1.0, "overshoot mid-pop");
        assert_eq!(s[2].opacity, 0.0);
        assert_eq!(
            pop_karaoke(&w, 39, 64.0, [1.0; 4]),
            pop_karaoke(&w, 39, 64.0, [1.0; 4])
        );
    }

    #[test]
    fn storm_has_exactly_n_letters() {
        let t = storm_text(1000);
        assert_eq!(t.chars().filter(|c| !c.is_whitespace()).count(), 1000);
        assert_eq!(glyph_storm(&t, 3).len(), 1000);
    }
}
