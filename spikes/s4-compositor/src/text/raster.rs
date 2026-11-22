//! CPU reference text rasteriser (the "vector truth" for the crispness metric): the exact glyph
//! outlines (same `ttf-parser` geometry as the atlas), flattened to 1/100 px, rasterised with
//! the nonzero rule on 8 sub-scanlines per pixel row with *exact* horizontal coverage (at least
//! as accurate as the WBS's 8×8 supersampling, and free of its 1/8-px staircase under horizontal
//! motion). Independent of both GPU paths. Fill, per-span transform, opacity and the karaoke
//! sweep are supported; outline and shadow are not (they are styling, not geometry).

use super::outline::{Outline, Segment, glyph_outline};
use super::{ResolvedText, TextEngine};
use crate::{Error, Result};

/// Sub-scanlines per pixel row.
pub const SS: usize = 8;
const FLATTEN_TOL: f64 = 0.01;

struct Edge {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    dir: i32,
}

fn apply(m: &[f64; 6], p: [f64; 2]) -> [f64; 2] {
    [
        m[0] * p[0] + m[1] * p[1] + m[2],
        m[3] * p[0] + m[4] * p[1] + m[5],
    ]
}

fn flatten(seg: &Segment, out: &mut Vec<[f64; 2]>) {
    let ctrl = seg.controls();
    let mut hull = 0.0f64;
    for w in ctrl.windows(2) {
        hull += (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]);
    }
    let n = match seg {
        Segment::Line(_) => 1,
        _ => ((hull / FLATTEN_TOL).sqrt().ceil() as usize).clamp(2, 512),
    };
    for i in 1..=n {
        out.push(seg.point(i as f64 / n as f64));
    }
}

/// Edges of `outline` mapped by `m` (font units → target px).
fn edges(outline: &Outline, m: &[f64; 6]) -> (Vec<Edge>, [f64; 4]) {
    let mut out = Vec::new();
    let mut bb = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for c in &outline.contours {
        let mut pts = Vec::new();
        if let Some(first) = c.segments.first() {
            pts.push(first.start());
        }
        for s in &c.segments {
            flatten(&s.transformed(m), &mut pts);
        }
        // `transformed` maps the segment; the start point still needs mapping.
        if let Some(p) = pts.first_mut() {
            *p = apply(m, *p);
        }
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            bb = [
                bb[0].min(a[0]),
                bb[1].min(a[1]),
                bb[2].max(a[0]),
                bb[3].max(a[1]),
            ];
            if a[1] != b[1] {
                out.push(Edge {
                    x0: a[0],
                    y0: a[1],
                    x1: b[0],
                    y1: b[1],
                    dir: if b[1] > a[1] { 1 } else { -1 },
                });
            }
        }
    }
    (out, bb)
}

/// Coverage of one glyph over its pixel bounds: base and karaoke-highlighted parts, 0..=1.
struct Cov {
    x0: i64,
    y0: i64,
    w: usize,
    h: usize,
    base: Vec<f64>,
    hl: Vec<f64>,
}

/// Adds the horizontal interval `[a, b)` of one sub-scanline (weight `wt`) to `acc` (row slice
/// starting at pixel `x0`), with exact per-pixel overlap.
fn add_span(acc: &mut [f64], x0: i64, a: f64, b: f64, wt: f64) {
    let w = acc.len() as i64;
    let (a, b) = ((a - x0 as f64).max(0.0), (b - x0 as f64).min(w as f64));
    if b <= a {
        return;
    }
    let (ia, ib) = (a.floor() as i64, (b.ceil() as i64 - 1).min(w - 1));
    for i in ia.max(0)..=ib {
        let lo = a.max(i as f64);
        let hi = b.min(i as f64 + 1.0);
        if hi > lo {
            acc[i as usize] += (hi - lo) * wt;
        }
    }
}

/// `SS` sub-scanlines per pixel row with exact horizontal coverage (nonzero rule). Exact in x
/// means horizontal sub-pixel motion is tracked without the staircase of a fixed sample grid.
fn coverage(
    edges: &[Edge],
    bb: [f64; 4],
    size: (u32, u32),
    sweep: Option<(&[f64; 6], f64, f64)>,
) -> Option<Cov> {
    let x0 = (bb[0].floor() as i64).max(0);
    let y0 = (bb[1].floor() as i64).max(0);
    let x1 = (bb[2].ceil() as i64).min(i64::from(size.0));
    let y1 = (bb[3].ceil() as i64).min(i64::from(size.1));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let mut base = vec![0.0f64; w * h];
    let mut hl = vec![0.0f64; w * h];
    let mut xs: Vec<(f64, i32)> = Vec::new();
    let wt = 1.0 / SS as f64;
    for row in 0..h {
        for sy in 0..SS {
            let y = (y0 + row as i64) as f64 + (sy as f64 + 0.5) / SS as f64;
            xs.clear();
            for e in edges {
                let (ya, yb) = if e.y0 < e.y1 {
                    (e.y0, e.y1)
                } else {
                    (e.y1, e.y0)
                };
                if y >= ya && y < yb {
                    let t = (y - e.y0) / (e.y1 - e.y0);
                    xs.push((e.x0 + t * (e.x1 - e.x0), e.dir));
                }
            }
            xs.sort_by(|a, b| a.0.total_cmp(&b.0));
            // Karaoke split point on this sub-scanline (layout x is affine in target x).
            let split = sweep.map(|(inv, front, dir)| {
                let lx_at = |x: f64| inv[0] * x + inv[1] * y + inv[2];
                (lx_at, front, dir)
            });
            let r = row * w;
            let mut wind = 0;
            let mut start = 0.0;
            for &(x, d) in &xs {
                let was = wind != 0;
                wind += d;
                if !was && wind != 0 {
                    start = x;
                } else if was && wind == 0 {
                    match &split {
                        None => add_span(&mut base[r..r + w], x0, start, x, wt),
                        Some((lx_at, front, dir)) => {
                            let k = lx_at(1.0) - lx_at(0.0);
                            let lit = |px: f64| (front - lx_at(px)) * dir > 0.0;
                            let cut = if k == 0.0 {
                                x
                            } else {
                                ((front - lx_at(0.0)) / k).clamp(start, x)
                            };
                            for (a, b) in [(start, cut), (cut, x)] {
                                if b > a {
                                    let acc = if lit(0.5 * (a + b)) {
                                        &mut hl
                                    } else {
                                        &mut base
                                    };
                                    add_span(&mut acc[r..r + w], x0, a, b, wt);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Some(Cov {
        x0,
        y0,
        w,
        h,
        base,
        hl,
    })
}

/// Renders `r` (fill only) into a premultiplied linear buffer of `size`, mapping box px to
/// target px with `target` (f64 affine). Glyphs are composited in order with "over".
pub fn render(
    engine: &TextEngine,
    r: &ResolvedText,
    target: &[f64; 6],
    size: (u32, u32),
) -> Result<Vec<[f64; 4]>> {
    if r.outline.is_some() || r.shadow.is_some() {
        return Err(Error::Unsupported(
            "CPU text reference: outline / shadow (fill, spans and karaoke only)".into(),
        ));
    }
    let (w, h) = (size.0 as usize, size.1 as usize);
    let mut px = vec![[0.0f64; 4]; w * h];
    let inv_target = crate::scene::invert_affine(target);
    for g in &r.glyphs {
        let Some(face) = engine.fonts.face(g.face) else {
            continue;
        };
        let outline = glyph_outline(face, g.key.glyph, g.key.weight);
        if outline.is_empty() {
            continue;
        }
        let k = f64::from(g.size) / outline.units_per_em;
        let place = [k, 0.0, f64::from(g.pen[0]), 0.0, -k, f64::from(g.pen[1])];
        let anim = g.anim.map(f64::from);
        let layout_to_target = compose64(target, &anim);
        let m = compose64(&layout_to_target, &place);
        let (es, bb) = edges(&outline, &m);
        let inv = crate::scene::invert_affine(&layout_to_target);
        let sweep = match (g.highlight, &inv) {
            (Some(hl), Some(inv)) => Some((inv, f64::from(hl.sweep_x), f64::from(hl.dir))),
            _ => None,
        };
        let Some(cov) = coverage(&es, bb, size, sweep) else {
            continue;
        };
        let fill = g.fill.map(f64::from);
        let hlc = g.highlight.map_or(fill, |h| h.color.map(f64::from));
        let op = f64::from(g.opacity);
        for row in 0..cov.h {
            for col in 0..cov.w {
                let i = row * cov.w + col;
                if cov.base[i] <= 0.0 && cov.hl[i] <= 0.0 {
                    continue;
                }
                let (x, y) = (cov.x0 + col as i64, cov.y0 + row as i64);
                let clip = inv_target.map_or(1.0, |it| {
                    box_coverage(&it, r.box_size, [x as f64 + 0.5, y as f64 + 0.5])
                });
                let (cb, ch) = (cov.base[i].min(1.0), cov.hl[i].min(1.0));
                let src: [f64; 4] =
                    std::array::from_fn(|c| (fill[c] * cb + hlc[c] * ch) * op * clip);
                let d = &mut px[y as usize * w + x as usize];
                for c in 0..4 {
                    d[c] = src[c] + d[c] * (1.0 - src[3]);
                }
            }
        }
    }
    Ok(px)
}

/// ½-px box-edge coverage at a target pixel centre (the same rule as the compositor's layers).
#[must_use]
pub fn box_coverage(inv_target: &[f64; 6], box_size: [f32; 2], p: [f64; 2]) -> f64 {
    let u = apply(inv_target, p);
    let (bw, bh) = (f64::from(box_size[0]), f64::from(box_size[1]));
    ((u[0].min(bw - u[0]) + 0.5).clamp(0.0, 1.0)) * ((u[1].min(bh - u[1]) + 0.5).clamp(0.0, 1.0))
}

fn compose64(outer: &[f64; 6], inner: &[f64; 6]) -> [f64; 6] {
    crate::scene::compose_affine(outer, inner)
}

#[cfg(test)]
mod tests {
    use super::super::outline::Contour;
    use super::*;

    #[test]
    fn square_coverage_is_exact_on_pixel_edges_and_halves() {
        let sq = Outline {
            contours: vec![Contour {
                segments: vec![
                    Segment::Line([[0.0, 0.0], [4.0, 0.0]]),
                    Segment::Line([[4.0, 0.0], [4.0, 4.0]]),
                    Segment::Line([[4.0, 4.0], [0.0, 4.0]]),
                    Segment::Line([[0.0, 4.0], [0.0, 0.0]]),
                ],
            }],
            units_per_em: 1.0,
        };
        // Place at x = 1.5 (half pixel), y = 2.
        let m = [1.0, 0.0, 1.5, 0.0, 1.0, 2.0];
        let (es, bb) = edges(&sq, &m);
        let c = coverage(&es, bb, (10, 10), None).expect("cov");
        let at = |x: i64, y: i64| c.base[((y - c.y0) as usize) * c.w + (x - c.x0) as usize];
        assert!((at(1, 3) - 0.5).abs() < 1e-12, "half-covered left column");
        assert!((at(3, 3) - 1.0).abs() < 1e-12, "interior");
        assert!((at(5, 3) - 0.5).abs() < 1e-12, "half-covered right column");
        // Exact in x: a 0.3 px shift moves 0.3 of a pixel of coverage.
        let m = [1.0, 0.0, 1.8, 0.0, 1.0, 2.0];
        let (es, bb) = edges(&sq, &m);
        let c = coverage(&es, bb, (10, 10), None).expect("cov");
        let at = |x: i64, y: i64| c.base[((y - c.y0) as usize) * c.w + (x - c.x0) as usize];
        assert!((at(1, 3) - 0.2).abs() < 1e-12);
    }
}
