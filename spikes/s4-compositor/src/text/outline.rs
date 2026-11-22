//! Glyph outlines (font units, y up) from `ttf-parser`, at a variable-font weight.
//!
//! Shared by the MSDF generator and the CPU reference rasteriser, so both see the same geometry.

use ttf_parser::{Face, GlyphId, OutlineBuilder, Tag};

use super::fonts::FaceData;

/// 2D point in font units.
pub type P = [f64; 2];

/// One Bézier segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Segment {
    Line([P; 2]),
    Quad([P; 3]),
    Cubic([P; 4]),
}

fn lerp(a: P, b: P, t: f64) -> P {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1]]
}

fn is_zero(v: P) -> bool {
    v[0] == 0.0 && v[1] == 0.0
}

impl Segment {
    /// Start / end points.
    #[must_use]
    pub fn start(&self) -> P {
        match self {
            Self::Line(p) => p[0],
            Self::Quad(p) => p[0],
            Self::Cubic(p) => p[0],
        }
    }

    #[must_use]
    pub fn end(&self) -> P {
        match self {
            Self::Line(p) => p[1],
            Self::Quad(p) => p[2],
            Self::Cubic(p) => p[3],
        }
    }

    /// Point at `t`.
    #[must_use]
    pub fn point(&self, t: f64) -> P {
        match *self {
            Self::Line([a, b]) => lerp(a, b, t),
            Self::Quad([a, b, c]) => lerp(lerp(a, b, t), lerp(b, c, t), t),
            Self::Cubic([a, b, c, d]) => {
                let bc = lerp(b, c, t);
                lerp(lerp(lerp(a, b, t), bc, t), lerp(bc, lerp(c, d, t), t), t)
            }
        }
    }

    /// Tangent direction at `t` (not normalised; never zero for non-degenerate segments).
    #[must_use]
    pub fn direction(&self, t: f64) -> P {
        match *self {
            Self::Line([a, b]) => sub(b, a),
            Self::Quad([a, b, c]) => {
                let d = lerp(sub(b, a), sub(c, b), t);
                if is_zero(d) { sub(c, a) } else { d }
            }
            Self::Cubic([a, b, c, d]) => {
                let tangent = lerp(
                    lerp(sub(b, a), sub(c, b), t),
                    lerp(sub(c, b), sub(d, c), t),
                    t,
                );
                if is_zero(tangent) {
                    if t == 0.0 {
                        return sub(c, a);
                    }
                    if t == 1.0 {
                        return sub(d, b);
                    }
                }
                tangent
            }
        }
    }

    /// The part of the segment between `t0` and `t1` (de Casteljau).
    #[must_use]
    pub fn sub_segment(&self, t0: f64, t1: f64) -> Self {
        // Split at t1, keep the head; then split the head at t0/t1, keep the tail.
        let head = self.split(t1).0;
        if t1 <= 0.0 {
            return head;
        }
        head.split(t0 / t1).1
    }

    fn split(&self, t: f64) -> (Self, Self) {
        match *self {
            Self::Line([a, b]) => {
                let m = lerp(a, b, t);
                (Self::Line([a, m]), Self::Line([m, b]))
            }
            Self::Quad([a, b, c]) => {
                let (ab, bc) = (lerp(a, b, t), lerp(b, c, t));
                let m = lerp(ab, bc, t);
                (Self::Quad([a, ab, m]), Self::Quad([m, bc, c]))
            }
            Self::Cubic([a, b, c, d]) => {
                let (ab, bc, cd) = (lerp(a, b, t), lerp(b, c, t), lerp(c, d, t));
                let (abc, bcd) = (lerp(ab, bc, t), lerp(bc, cd, t));
                let m = lerp(abc, bcd, t);
                (Self::Cubic([a, ab, abc, m]), Self::Cubic([m, bcd, cd, d]))
            }
        }
    }

    /// Control polygon points (for bounds; a conservative hull of the curve).
    #[must_use]
    pub fn controls(&self) -> &[P] {
        match self {
            Self::Line(p) => p,
            Self::Quad(p) => p,
            Self::Cubic(p) => p,
        }
    }

    /// Applies an affine map `[a, b, c, d, e, f]` to every control point.
    #[must_use]
    pub fn transformed(&self, m: &[f64; 6]) -> Self {
        let t = |p: P| {
            [
                m[0] * p[0] + m[1] * p[1] + m[2],
                m[3] * p[0] + m[4] * p[1] + m[5],
            ]
        };
        match *self {
            Self::Line(p) => Self::Line(p.map(t)),
            Self::Quad(p) => Self::Quad(p.map(t)),
            Self::Cubic(p) => Self::Cubic(p.map(t)),
        }
    }
}

/// Closed contour.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Contour {
    pub segments: Vec<Segment>,
}

/// A glyph outline in font units (y up).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outline {
    pub contours: Vec<Contour>,
    pub units_per_em: f64,
}

impl Outline {
    /// Conservative bounds `[x0, y0, x1, y1]` (control-point hull); `None` when empty.
    #[must_use]
    pub fn bounds(&self) -> Option<[f64; 4]> {
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        let mut any = false;
        for c in &self.contours {
            for s in &c.segments {
                for p in s.controls() {
                    any = true;
                    b = [
                        b[0].min(p[0]),
                        b[1].min(p[1]),
                        b[2].max(p[0]),
                        b[3].max(p[1]),
                    ];
                }
            }
        }
        any.then_some(b)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.contours.iter().all(|c| c.segments.is_empty())
    }

    /// Segment count.
    #[must_use]
    pub fn segment_count(&self) -> usize {
        self.contours.iter().map(|c| c.segments.len()).sum()
    }

    /// Nonzero winding number at `p` (curves flattened into 16 pieces; for tests and the
    /// MSDF sign check).
    #[must_use]
    pub fn winding_at(&self, p: P) -> i32 {
        let mut w = 0;
        for c in &self.contours {
            for s in &c.segments {
                let n = if matches!(s, Segment::Line(_)) { 1 } else { 16 };
                let mut a = s.start();
                for i in 1..=n {
                    let b = s.point(i as f64 / n as f64);
                    if (a[1] <= p[1]) != (b[1] <= p[1]) {
                        let t = (p[1] - a[1]) / (b[1] - a[1]);
                        if a[0] + t * (b[0] - a[0]) > p[0] {
                            w += if b[1] > a[1] { 1 } else { -1 };
                        }
                    }
                    a = b;
                }
            }
        }
        w
    }
}

#[derive(Default)]
struct Builder {
    contours: Vec<Contour>,
    cur: Vec<Segment>,
    start: P,
    last: P,
}

impl Builder {
    fn push(&mut self, s: Segment) {
        if s.start() == s.end() && matches!(s, Segment::Line(_)) {
            return;
        }
        self.cur.push(s);
        self.last = s.end();
    }

    fn finish(&mut self) {
        if self.last != self.start {
            let s = Segment::Line([self.last, self.start]);
            self.cur.push(s);
            self.last = self.start;
        }
        if !self.cur.is_empty() {
            self.contours.push(Contour {
                segments: std::mem::take(&mut self.cur),
            });
        }
    }
}

impl OutlineBuilder for Builder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.finish();
        self.start = [f64::from(x), f64::from(y)];
        self.last = self.start;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = [f64::from(x), f64::from(y)];
        self.push(Segment::Line([self.last, p]));
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let c = [f64::from(x1), f64::from(y1)];
        let p = [f64::from(x), f64::from(y)];
        self.push(Segment::Quad([self.last, c, p]));
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let c1 = [f64::from(x1), f64::from(y1)];
        let c2 = [f64::from(x2), f64::from(y2)];
        let p = [f64::from(x), f64::from(y)];
        self.push(Segment::Cubic([self.last, c1, c2, p]));
    }

    fn close(&mut self) {
        self.finish();
    }
}

/// Parses `face` and sets the `wght` axis (clamped to its range) when the font is variable.
#[must_use]
pub fn instance(face: &FaceData, weight: u16) -> Option<Face<'_>> {
    let mut f = Face::parse(&face.data, face.index).ok()?;
    let wght = Tag::from_bytes(b"wght");
    if let Some(axis) = f.variation_axes().into_iter().find(|a| a.tag == wght) {
        let v = f32::from(weight).clamp(axis.min_value, axis.max_value);
        f.set_variation(wght, v);
    }
    Some(f)
}

/// Normalised variation coordinates (F2Dot14 bits) of `face` at `weight` (for Vello).
#[must_use]
pub fn normalized_coords(face: &FaceData, weight: u16) -> Vec<i16> {
    instance(face, weight)
        .map(|f| f.variation_coordinates().iter().map(|c| c.get()).collect())
        .unwrap_or_default()
}

/// The outline of `glyph` in `face` at `weight`. Empty (no contours) for blank glyphs.
#[must_use]
pub fn glyph_outline(face: &FaceData, glyph: u16, weight: u16) -> Outline {
    let Some(f) = instance(face, weight) else {
        return Outline::default();
    };
    let mut b = Builder::default();
    let _ = f.outline_glyph(GlyphId(glyph), &mut b);
    b.finish();
    Outline {
        contours: b.contours,
        units_per_em: f64::from(f.units_per_em()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_segment_matches_point_evaluation() {
        let c = Segment::Cubic([[0.0, 0.0], [10.0, 30.0], [40.0, -10.0], [50.0, 20.0]]);
        let s = c.sub_segment(1.0 / 3.0, 2.0 / 3.0);
        for t in [0.0, 0.25, 0.5, 1.0] {
            let a = s.point(t);
            let b = c.point(1.0 / 3.0 + t / 3.0);
            assert!((a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9);
        }
        let q = Segment::Quad([[0.0, 0.0], [5.0, 10.0], [10.0, 0.0]]);
        let s = q.sub_segment(0.0, 0.5);
        assert_eq!(s.start(), [0.0, 0.0]);
        assert_eq!(s.end(), q.point(0.5));
    }
}
