// SPDX-License-Identifier: GPL-3.0-or-later
//! WCAG contrast maths and the usage-note driven contrast gate.

use std::fmt::Write as _;

use super::model::{Rgba, Tokens};

/// Minimum ratio for text and icons used as text.
pub const TEXT_MIN: f64 = 4.5;
/// Minimum ratio for non-text marks (WCAG 1.4.11).
pub const MARK_MIN: f64 = 3.0;

/// Surfaces that `every surface` in a usage note expands to.
const SURFACES: &[&str] = &["surface-0", "surface-1", "surface-2", "surface-3"];
/// Fill tokens that act as a ground only when a note says `on <fill>`.
const FILLS: &[&str] = &["marigold", "agent", "danger"];

/// One checked foreground/ground pair.
#[derive(Debug, Clone)]
pub struct PairResult {
    pub theme: String,
    pub fg: String,
    pub ground: String,
    pub min: f64,
    pub ratio: f64,
    /// True when the grounds came from the built-in default instead of the usage note.
    pub defaulted: bool,
}

impl PairResult {
    pub fn passes(&self) -> bool {
        self.ratio + 1e-9 >= self.min
    }
}

fn channel_to_linear(c: f64) -> f64 {
    let c = c / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG relative luminance of opaque channels in 0..=255.
pub fn luminance(rgb: [f64; 3]) -> f64 {
    0.2126 * channel_to_linear(rgb[0])
        + 0.7152 * channel_to_linear(rgb[1])
        + 0.0722 * channel_to_linear(rgb[2])
}

/// Composites `fg` (with its alpha) over the opaque colour `bg`, in sRGB space.
pub fn blend(fg: Rgba, bg: [f64; 3]) -> [f64; 3] {
    let a = f64::from(fg.a) / 255.0;
    let mix = |f: u8, b: f64| f64::from(f) * a + b * (1.0 - a);
    [mix(fg.r, bg[0]), mix(fg.g, bg[1]), mix(fg.b, bg[2])]
}

fn opaque(c: Rgba) -> [f64; 3] {
    [f64::from(c.r), f64::from(c.g), f64::from(c.b)]
}

/// WCAG contrast ratio of `fg` drawn over the opaque `ground` (fg alpha is blended).
pub fn contrast(fg: Rgba, ground: [f64; 3]) -> f64 {
    let l1 = luminance(blend(fg, ground));
    let l2 = luminance(ground);
    let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

#[derive(Clone, Copy, PartialEq)]
enum Class {
    Text,
    Mark,
}

fn classify(name: &str, usage: &str) -> Option<Class> {
    if name == "text-disabled" || usage.contains("exempt from contrast") {
        return None;
    }
    if name.starts_with("text-")
        || name.ends_with("-text")
        || name.starts_with("on-")
        || matches!(name, "success" | "warning" | "danger")
    {
        return Some(Class::Text);
    }
    if matches!(name, "border-control" | "focus-ring" | "agent")
        || (name.starts_with("clip-") && !name.ends_with("-body"))
    {
        return Some(Class::Mark);
    }
    None
}

fn words(usage: &str) -> Vec<String> {
    usage
        .to_ascii_lowercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '*')))
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Names of the ground tokens a usage note refers to, in note order, de-duplicated.
pub fn parse_grounds(tokens: &Tokens, self_name: &str, usage: &str) -> Vec<String> {
    let known = |n: &str| tokens.colors.iter().any(|t| t.name == n);
    let is_surface_like = |n: &str| {
        n.starts_with("surface-") && n != "surface-preview"
            || n == "timeline-lane"
            || n == "timeline-lane-alt"
            || (n.starts_with("clip-") && n.ends_with("-body"))
    };
    let ws = words(usage);
    let mut out: Vec<String> = Vec::new();
    let mut push = |n: &str| {
        if !out.iter().any(|o| o == n) {
            out.push(n.to_owned());
        }
    };
    let mut i = 0;
    while i < ws.len() {
        let w = ws[i].as_str();
        let prev_on = i > 0 && ws[i - 1] == "on";
        if w == "every" && ws.get(i + 1).is_some_and(|n| n == "surface") {
            for s in SURFACES {
                push(s);
            }
        } else if w == "every" && ws.get(i + 1).is_some_and(|n| n == "clip-*-body") {
            for t in &tokens.colors {
                if t.name.starts_with("clip-") && t.name.ends_with("-body") {
                    push(&t.name);
                }
            }
        } else if w.starts_with("surface-")
            && ws.get(i + 1).is_some_and(|n| n == "to")
            && ws.get(i + 2).is_some_and(|n| n.starts_with("surface-"))
        {
            // "surface-0 to surface-3": inclusive numeric range.
            let n = |s: &str| s.trim_start_matches("surface-").parse::<u32>().ok();
            if let (Some(lo), Some(hi)) = (n(w), n(&ws[i + 2])) {
                (lo..=hi)
                    .map(|k| format!("surface-{k}"))
                    .filter(|s| known(s))
                    .for_each(|s| push(&s));
            }
            i += 2;
        } else if w != self_name
            && ((known(w) && is_surface_like(w)) || (prev_on && FILLS.contains(&w)))
        {
            push(w);
        }
        i += 1;
    }
    out
}

/// Fallback grounds for marks whose note names none.
fn default_grounds(name: &str) -> &'static [&'static str] {
    if name == "agent" {
        &["surface-1", "surface-2", "timeline-lane"]
    } else if name.starts_with("clip-") {
        &["timeline-lane", "timeline-lane-alt"]
    } else {
        &[]
    }
}

struct Spec {
    fg: String,
    /// Ground token and (for translucent grounds) the surfaces it is composited over.
    ground: String,
    over: Option<String>,
    min: f64,
    defaulted: bool,
}

fn specs(tokens: &Tokens) -> Result<Vec<Spec>, String> {
    let mut out = Vec::new();
    for tok in &tokens.colors {
        if let Some(class) = classify(&tok.name, &tok.usage) {
            let min = if class == Class::Text {
                TEXT_MIN
            } else {
                MARK_MIN
            };
            let mut grounds = parse_grounds(tokens, &tok.name, &tok.usage);
            let mut defaulted = false;
            if grounds.is_empty() {
                grounds = default_grounds(&tok.name)
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect();
                defaulted = true;
            }
            if grounds.is_empty() {
                return Err(format!(
                    "contrast: usage note of `{}` names no ground to check against",
                    tok.name
                ));
            }
            for g in grounds {
                out.push(Spec {
                    fg: tok.name.clone(),
                    ground: g,
                    over: None,
                    min,
                    defaulted,
                });
            }
        }
        // "keep on-x text": the hover fill must still carry the on-x text.
        let ws = words(&tok.usage);
        for pair in ws.windows(2) {
            if pair[0] == "keep" && pair[1].starts_with("on-") {
                out.push(Spec {
                    fg: pair[1].clone(),
                    ground: tok.name.clone(),
                    over: None,
                    min: TEXT_MIN,
                    defaulted: false,
                });
            }
        }
        // Translucent fills that sit behind text-primary are checked over the panel surfaces.
        if tok.usage.contains("behind text-primary") {
            for s in ["surface-1", "surface-2"] {
                out.push(Spec {
                    fg: "text-primary".into(),
                    ground: tok.name.clone(),
                    over: Some(s.into()),
                    min: TEXT_MIN,
                    defaulted: false,
                });
            }
        }
    }
    Ok(out)
}

/// Evaluates every declared pair in every theme.
pub fn check(tokens: &Tokens) -> Result<Vec<PairResult>, String> {
    let mut results = Vec::new();
    for (ti, theme) in tokens.themes.iter().enumerate() {
        for s in specs(tokens)? {
            let fg = tokens.resolve_color(&s.fg, ti)?;
            let g = tokens.resolve_color(&s.ground, ti)?;
            let (ground_rgb, label) = if let Some(base) = &s.over {
                let b = tokens.resolve_color(base, ti)?;
                (blend(g, opaque(b)), format!("{} over {base}", s.ground))
            } else if g.a == 255 {
                (opaque(g), s.ground.clone())
            } else {
                return Err(format!("ground `{}` is translucent", s.ground));
            };
            results.push(PairResult {
                theme: theme.clone(),
                fg: s.fg,
                ground: label,
                min: s.min,
                ratio: contrast(fg, ground_rgb),
                defaulted: s.defaulted,
            });
        }
    }
    Ok(results)
}

/// Renders the pairs as a fixed-width table.
pub fn table(results: &[PairResult]) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "{:<6} {:<18} {:<30} {:>5} {:>7}  result",
        "theme", "token", "ground", "min", "ratio"
    );
    for r in results {
        let _ = writeln!(
            s,
            "{:<6} {:<18} {:<30} {:>5.1} {:>7.2}  {}{}",
            r.theme,
            r.fg,
            r.ground,
            r.min,
            r.ratio,
            if r.passes() { "ok" } else { "FAIL" },
            if r.defaulted {
                " (default grounds)"
            } else {
                ""
            }
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::model::parse_hex;

    fn ratio(fg: &str, bg: &str) -> f64 {
        let b = parse_hex(bg).unwrap();
        contrast(parse_hex(fg).unwrap(), opaque(b))
    }

    #[test]
    fn wcag_known_pairs() {
        assert!((ratio("#000000", "#ffffff") - 21.0).abs() < 1e-9);
        assert!((ratio("#ffffff", "#ffffff") - 1.0).abs() < 1e-9);
        // Widely published reference values.
        assert!((ratio("#767676", "#ffffff") - 4.54).abs() < 0.01);
        assert!((ratio("#777777", "#ffffff") - 4.48).abs() < 0.01);
        assert!((ratio("#0000ff", "#ffffff") - 8.59).abs() < 0.01);
        // Order of operands does not matter for an opaque pair.
        assert!((ratio("#ffffff", "#767676") - ratio("#767676", "#ffffff")).abs() < 1e-12);
    }

    #[test]
    fn alpha_is_blended_over_the_ground() {
        // 50% black on white is #808080 -> 3.95:1.
        let half_black = ratio("#00000080", "#ffffff");
        let grey = ratio("#7f7f7f", "#ffffff");
        assert!((half_black - grey).abs() < 0.05, "{half_black} vs {grey}");
        // Fully transparent foreground is the ground itself.
        assert!((ratio("#ffffff00", "#ffffff") - 1.0).abs() < 1e-9);
    }

    fn fixture() -> Tokens {
        Tokens::parse(
            r##"{"name":"T","color":{"themes":[{"id":"dark"},{"id":"light"}],"tokens":[
            {"name":"surface-0","value":"#111111","usage":""},
            {"name":"surface-1","value":"#181818","usage":""},
            {"name":"surface-2","value":"#202020","usage":""},
            {"name":"surface-3","value":"#2a2a2a","usage":""},
            {"name":"timeline-lane","value":"#161616","usage":""},
            {"name":"clip-a-body","value":"#1b3245","usage":""},
            {"name":"clip-b-body","value":"#183626","usage":""},
            {"name":"marigold","value":"#f2b33d","usage":""}
            ]},"type":{"families":{},"groups":[]}}"##,
        )
        .unwrap()
    }

    #[test]
    fn grounds_are_parsed_from_usage_notes() {
        let t = fixture();
        let g = |u: &str| parse_grounds(&t, "x", u);
        assert_eq!(
            g("on surface-0 to surface-2 and on every clip-*-body."),
            [
                "surface-0",
                "surface-1",
                "surface-2",
                "clip-a-body",
                "clip-b-body"
            ]
        );
        assert_eq!(
            g("On every surface and on timeline-lane."),
            [
                "surface-0",
                "surface-1",
                "surface-2",
                "surface-3",
                "timeline-lane"
            ]
        );
        assert_eq!(
            g("Text on marigold fills (buttons)."),
            ["marigold"],
            "fills count only after `on`"
        );
        assert_eq!(
            parse_grounds(&t, "marigold", "Marigold as text"),
            Vec::<String>::new()
        );
        assert_eq!(g("no grounds here"), Vec::<String>::new());
    }
}
