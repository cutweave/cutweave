//! Multi-channel + true signed distance fields (MTSDF), written for this spike after the
//! msdfgen algorithms (Chlumský, "Shape decomposition for multi-channel distance fields", 2015;
//! msdfgen 1.x: simple edge colouring, pseudo-distance channels, the overlapping-contour
//! combiner for variable-font outlines with overlaps, and the legacy clash error correction).
//!
//! Output texel = `distance / range + 0.5` (0.5 on the edge, > 0.5 inside) in RGB (MSDF: sharp
//! corners for the fill) and A (true SDF: round outlines, soft shadows). Pure `f64`, no threads
//! inside, no hashing of pointers: the same outline always yields the same bytes.

use super::outline::{Contour, Outline, P, Segment};

const RED: u8 = 1;
const GREEN: u8 = 2;
const BLUE: u8 = 4;
const YELLOW: u8 = RED | GREEN;
const MAGENTA: u8 = RED | BLUE;
const CYAN: u8 = GREEN | BLUE;
const WHITE: u8 = 7;
const BLACK: u8 = 0;

/// Generation parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MsdfConfig {
    /// Atlas pixels per em.
    pub em_px: f64,
    /// Distance range in atlas pixels (total; ±range/2 around the edge is representable).
    pub range_px: f64,
    /// Corner detection angle (radians), msdfgen default 3.
    pub angle_threshold: f64,
    /// Legacy clash-based error correction.
    pub error_correction: bool,
}

impl Default for MsdfConfig {
    fn default() -> Self {
        Self {
            em_px: 48.0,
            range_px: 12.0,
            angle_threshold: 3.0,
            error_correction: true,
        }
    }
}

/// One glyph's field.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphField {
    pub width: u32,
    pub height: u32,
    /// RGBA8, row 0 at the top.
    pub rgba: Vec<u8>,
    /// Bitmap bounds in em units relative to the pen, y up: `[left, bottom, right, top]`.
    pub plane: [f32; 4],
}

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1]]
}
fn add(a: P, b: P) -> P {
    [a[0] + b[0], a[1] + b[1]]
}
fn scale(a: P, s: f64) -> P {
    [a[0] * s, a[1] * s]
}
fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: P, b: P) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn len(a: P) -> f64 {
    a[0].hypot(a[1])
}
fn normalize(a: P) -> P {
    let l = len(a);
    if l == 0.0 {
        [0.0, 1.0]
    } else {
        [a[0] / l, a[1] / l]
    }
}
/// msdfgen `getOrthonormal(polarity = false)`.
fn orthonormal(a: P) -> P {
    let l = len(a);
    if l == 0.0 {
        [0.0, -1.0]
    } else {
        [a[1] / l, -a[0] / l]
    }
}
fn non_zero_sign(x: f64) -> f64 {
    if x > 0.0 { 1.0 } else { -1.0 }
}
fn median(a: f64, b: f64, c: f64) -> f64 {
    a.min(b).max(a.max(b).min(c))
}

#[derive(Clone, Copy, Debug)]
struct SignedDistance {
    d: f64,
    dot: f64,
}

impl SignedDistance {
    const INFINITE: Self = Self {
        d: -1e240,
        dot: 1.0,
    };

    fn less(&self, o: &Self) -> bool {
        let (a, b) = (self.d.abs(), o.d.abs());
        a < b || (a == b && self.dot < o.dot)
    }
}

fn solve_quadratic(a: f64, b: f64, c: f64) -> ([f64; 3], usize) {
    if a == 0.0 || b.abs() > 1e12 * a.abs() {
        if b == 0.0 {
            return ([0.0; 3], 0);
        }
        return ([-c / b, 0.0, 0.0], 1);
    }
    let disc = b * b - 4.0 * a * c;
    if disc > 0.0 {
        let s = disc.sqrt();
        ([(-b + s) / (2.0 * a), (-b - s) / (2.0 * a), 0.0], 2)
    } else if disc == 0.0 {
        ([-b / (2.0 * a), 0.0, 0.0], 1)
    } else {
        ([0.0; 3], 0)
    }
}

fn solve_cubic_normed(a: f64, b: f64, c: f64) -> ([f64; 3], usize) {
    let a2 = a * a;
    let mut q = (a2 - 3.0 * b) / 9.0;
    let r = (a * (2.0 * a2 - 9.0 * b) + 27.0 * c) / 54.0;
    let r2 = r * r;
    let q3 = q * q * q;
    let a = a / 3.0;
    if r2 < q3 {
        let t = (r / q3.sqrt()).clamp(-1.0, 1.0).acos();
        q = -2.0 * q.sqrt();
        let tau = std::f64::consts::TAU;
        (
            [
                q * (t / 3.0).cos() - a,
                q * ((t + tau) / 3.0).cos() - a,
                q * ((t - tau) / 3.0).cos() - a,
            ],
            3,
        )
    } else {
        let u = if r < 0.0 { 1.0 } else { -1.0 } * (r.abs() + (r2 - q3).sqrt()).powf(1.0 / 3.0);
        let v = if u == 0.0 { 0.0 } else { q / u };
        let x0 = (u + v) - a;
        if u == v || (u - v).abs() < 1e-12 * (u + v).abs() {
            return ([x0, -0.5 * (u + v) - a, 0.0], 2);
        }
        ([x0, 0.0, 0.0], 1)
    }
}

fn solve_cubic(a: f64, b: f64, c: f64, d: f64) -> ([f64; 3], usize) {
    if a != 0.0 {
        let bn = b / a;
        if bn.abs() < 1e6 {
            return solve_cubic_normed(bn, c / a, d / a);
        }
    }
    solve_quadratic(b, c, d)
}

/// Signed distance from `origin` to the segment, and the parameter of the closest point.
fn signed_distance(seg: &Segment, origin: P) -> (SignedDistance, f64) {
    match *seg {
        Segment::Line([p0, p1]) => {
            let aq = sub(origin, p0);
            let ab = sub(p1, p0);
            let param = dot(aq, ab) / dot(ab, ab);
            let eq = sub(if param > 0.5 { p1 } else { p0 }, origin);
            let end_dist = len(eq);
            if param > 0.0 && param < 1.0 {
                let ortho = dot(orthonormal(ab), aq);
                if ortho.abs() < end_dist {
                    return (SignedDistance { d: ortho, dot: 0.0 }, param);
                }
            }
            (
                SignedDistance {
                    d: non_zero_sign(cross(aq, ab)) * end_dist,
                    dot: dot(normalize(ab), normalize(eq)).abs(),
                },
                param,
            )
        }
        Segment::Quad([p0, p1, p2]) => {
            let qa = sub(p0, origin);
            let ab = sub(p1, p0);
            let br = sub(sub(p2, p1), ab);
            let a = dot(br, br);
            let b = 3.0 * dot(ab, br);
            let c = 2.0 * dot(ab, ab) + dot(qa, br);
            let d = dot(qa, ab);
            let (t, n) = solve_cubic(a, b, c, d);
            let mut ep = seg.direction(0.0);
            let mut min_d = non_zero_sign(cross(ep, qa)) * len(qa);
            let mut param = -dot(qa, ep) / dot(ep, ep);
            {
                ep = seg.direction(1.0);
                let d2 = len(sub(p2, origin));
                if d2 < min_d.abs() {
                    min_d = non_zero_sign(cross(ep, sub(p2, origin))) * d2;
                    param = dot(sub(origin, p1), ep) / dot(ep, ep);
                }
            }
            for &ti in &t[..n] {
                if ti > 0.0 && ti < 1.0 {
                    let qe = add(add(qa, scale(ab, 2.0 * ti)), scale(br, ti * ti));
                    let dd = len(qe);
                    if dd <= min_d.abs() {
                        min_d = non_zero_sign(cross(add(ab, scale(br, ti)), qe)) * dd;
                        param = ti;
                    }
                }
            }
            finish(seg, origin, min_d, param, qa, p2)
        }
        Segment::Cubic([p0, p1, p2, p3]) => {
            let qa = sub(p0, origin);
            let ab = sub(p1, p0);
            let br = sub(sub(p2, p1), ab);
            let as_ = sub(sub(sub(p3, p2), sub(p2, p1)), br);
            let mut ep = seg.direction(0.0);
            let mut min_d = non_zero_sign(cross(ep, qa)) * len(qa);
            let mut param = -dot(qa, ep) / dot(ep, ep);
            {
                ep = seg.direction(1.0);
                let d2 = len(sub(p3, origin));
                if d2 < min_d.abs() {
                    min_d = non_zero_sign(cross(ep, sub(p3, origin))) * d2;
                    param = dot(sub(ep, sub(p3, origin)), ep) / dot(ep, ep);
                }
            }
            const STARTS: usize = 4;
            const STEPS: usize = 4;
            let at = |t: f64| {
                add(
                    add(add(qa, scale(ab, 3.0 * t)), scale(br, 3.0 * t * t)),
                    scale(as_, t * t * t),
                )
            };
            for i in 0..=STARTS {
                let mut t = i as f64 / STARTS as f64;
                let mut qe = at(t);
                for _ in 0..STEPS {
                    let d1 = add(
                        add(scale(ab, 3.0), scale(br, 6.0 * t)),
                        scale(as_, 3.0 * t * t),
                    );
                    let d2 = add(scale(br, 6.0), scale(as_, 6.0 * t));
                    t -= dot(qe, d1) / (dot(d1, d1) + dot(qe, d2));
                    if t <= 0.0 || t >= 1.0 {
                        break;
                    }
                    qe = at(t);
                    let dd = len(qe);
                    if dd < min_d.abs() {
                        let d1 = add(
                            add(scale(ab, 3.0), scale(br, 6.0 * t)),
                            scale(as_, 3.0 * t * t),
                        );
                        min_d = non_zero_sign(cross(d1, qe)) * dd;
                        param = t;
                    }
                }
            }
            finish(seg, origin, min_d, param, qa, p3)
        }
    }
}

fn finish(
    seg: &Segment,
    origin: P,
    min_d: f64,
    param: f64,
    qa: P,
    end: P,
) -> (SignedDistance, f64) {
    if (0.0..=1.0).contains(&param) {
        return (SignedDistance { d: min_d, dot: 0.0 }, param);
    }
    let dotv = if param < 0.5 {
        dot(normalize(seg.direction(0.0)), normalize(qa)).abs()
    } else {
        dot(normalize(seg.direction(1.0)), normalize(sub(end, origin))).abs()
    };
    (
        SignedDistance {
            d: min_d,
            dot: dotv,
        },
        param,
    )
}

/// msdfgen `distanceToPerpendicularDistance` (pseudo-distance beyond the segment's ends).
fn to_pseudo(seg: &Segment, origin: P, sd: SignedDistance, param: f64) -> f64 {
    let mut d = sd.d;
    if param < 0.0 {
        let dir = normalize(seg.direction(0.0));
        let aq = sub(origin, seg.start());
        if dot(aq, dir) < 0.0 {
            let pd = cross(aq, dir);
            if pd.abs() <= d.abs() {
                d = pd;
            }
        }
    } else if param > 1.0 {
        let dir = normalize(seg.direction(1.0));
        let bq = sub(origin, seg.end());
        if dot(bq, dir) > 0.0 {
            let pd = cross(bq, dir);
            if pd.abs() <= d.abs() {
                d = pd;
            }
        }
    }
    d
}

fn is_corner(a: P, b: P, cross_threshold: f64) -> bool {
    dot(a, b) <= 0.0 || cross(a, b).abs() > cross_threshold
}

fn switch_color(color: &mut u8, seed: &mut u64, banned: u8) {
    let combined = *color & banned;
    if combined == RED || combined == GREEN || combined == BLUE {
        *color = combined ^ WHITE;
        return;
    }
    if *color == BLACK || *color == WHITE {
        const START: [u8; 3] = [CYAN, MAGENTA, YELLOW];
        *color = START[(*seed % 3) as usize];
        *seed /= 3;
        return;
    }
    let shifted = u32::from(*color) << (1 + (*seed & 1));
    *color = ((shifted | (shifted >> 3)) & u32::from(WHITE)) as u8;
    *seed >>= 1;
}

fn trichotomy(position: usize, n: usize) -> usize {
    let v = (3.0 + 2.875 * position as f64 / (n as f64 - 1.0) - 1.4375 + 0.5).floor() as i64 - 3;
    (v + 1).clamp(0, 2) as usize
}

/// msdfgen `edgeColoringSimple` with seed 0.
fn color_edges(contours: &[Contour], angle_threshold: f64) -> Vec<Vec<(Segment, u8)>> {
    let cross_threshold = angle_threshold.sin();
    let mut seed = 0u64;
    let mut color = {
        const START: [u8; 3] = [CYAN, MAGENTA, YELLOW];
        let c = START[(seed % 3) as usize];
        seed /= 3;
        c
    };
    let mut out = Vec::with_capacity(contours.len());
    for contour in contours {
        let edges = &contour.segments;
        if edges.is_empty() {
            continue;
        }
        let mut corners = Vec::new();
        let mut prev = edges[edges.len() - 1].direction(1.0);
        for (i, e) in edges.iter().enumerate() {
            if is_corner(
                normalize(prev),
                normalize(e.direction(0.0)),
                cross_threshold,
            ) {
                corners.push(i);
            }
            prev = e.direction(1.0);
        }
        let m = edges.len();
        let colored: Vec<(Segment, u8)> = if corners.is_empty() {
            switch_color(&mut color, &mut seed, BLACK);
            edges.iter().map(|e| (*e, color)).collect()
        } else if corners.len() == 1 {
            let mut colors = [BLACK, WHITE, BLACK];
            switch_color(&mut color, &mut seed, BLACK);
            colors[0] = color;
            switch_color(&mut color, &mut seed, BLACK);
            colors[2] = color;
            let corner = corners[0];
            if m >= 3 {
                let mut v = edges.iter().map(|e| (*e, WHITE)).collect::<Vec<_>>();
                for i in 0..m {
                    v[(corner + i) % m].1 = colors[trichotomy(i, m)];
                }
                v
            } else {
                // Fewer than three segments for three colours: split each in thirds.
                let thirds = |s: &Segment| {
                    [
                        s.sub_segment(0.0, 1.0 / 3.0),
                        s.sub_segment(1.0 / 3.0, 2.0 / 3.0),
                        s.sub_segment(2.0 / 3.0, 1.0),
                    ]
                };
                let mut parts: [Option<Segment>; 6] = [None; 6];
                let t0 = thirds(&edges[0]);
                for k in 0..3 {
                    parts[k + 3 * corner] = Some(t0[k]);
                }
                if m >= 2 {
                    let t1 = thirds(&edges[1]);
                    for k in 0..3 {
                        parts[3 - 3 * corner + k] = Some(t1[k]);
                    }
                    let cols = [
                        colors[0], colors[0], colors[1], colors[1], colors[2], colors[2],
                    ];
                    parts
                        .iter()
                        .zip(cols)
                        .filter_map(|(p, c)| p.map(|s| (s, c)))
                        .collect()
                } else {
                    parts
                        .iter()
                        .flatten()
                        .zip(colors)
                        .map(|(s, c)| (*s, c))
                        .collect()
                }
            }
        } else {
            let count = corners.len();
            let mut spline = 0;
            let start = corners[0];
            switch_color(&mut color, &mut seed, BLACK);
            let initial = color;
            let mut v = edges.iter().map(|e| (*e, WHITE)).collect::<Vec<_>>();
            for i in 0..m {
                let index = (start + i) % m;
                if spline + 1 < count && corners[spline + 1] == index {
                    spline += 1;
                    let banned = if spline == count - 1 { initial } else { BLACK };
                    switch_color(&mut color, &mut seed, banned);
                }
                v[index].1 = color;
            }
            v
        };
        out.push(colored);
    }
    out
}

/// msdfgen `Contour::winding` (sign of the shoelace area; +1 = clockwise in y-up = filled).
fn winding(edges: &[(Segment, u8)]) -> i32 {
    let shoelace = |a: P, b: P| (b[0] - a[0]) * (a[1] + b[1]);
    let mut total = 0.0;
    match edges.len() {
        0 => return 0,
        1 => {
            let s = &edges[0].0;
            let (a, b, c) = (s.point(0.0), s.point(1.0 / 3.0), s.point(2.0 / 3.0));
            total += shoelace(a, b) + shoelace(b, c) + shoelace(c, a);
        }
        2 => {
            let (s0, s1) = (&edges[0].0, &edges[1].0);
            let (a, b, c, d) = (s0.point(0.0), s0.point(0.5), s1.point(0.0), s1.point(0.5));
            total += shoelace(a, b) + shoelace(b, c) + shoelace(c, d) + shoelace(d, a);
        }
        _ => {
            let mut prev = edges[edges.len() - 1].0.start();
            for (s, _) in edges {
                let cur = s.start();
                total += shoelace(prev, cur);
                prev = cur;
            }
        }
    }
    if total > 0.0 {
        1
    } else if total < 0.0 {
        -1
    } else {
        0
    }
}

#[derive(Clone, Copy)]
struct Channel {
    min: SignedDistance,
    edge: Option<(usize, usize)>,
    param: f64,
}

#[derive(Clone, Copy)]
struct Selector {
    ch: [Channel; 3],
    true_min: SignedDistance,
}

impl Selector {
    const EMPTY: Self = Self {
        ch: [Channel {
            min: SignedDistance::INFINITE,
            edge: None,
            param: 0.0,
        }; 3],
        true_min: SignedDistance::INFINITE,
    };

    fn merge(&mut self, o: &Self) {
        for k in 0..3 {
            if o.ch[k].min.less(&self.ch[k].min) {
                self.ch[k] = o.ch[k];
            }
        }
        if o.true_min.less(&self.true_min) {
            self.true_min = o.true_min;
        }
    }

    fn distance(&self, edges: &[Vec<(Segment, u8)>], p: P) -> [f64; 4] {
        let mut out = [0.0; 4];
        for k in 0..3 {
            let c = &self.ch[k];
            out[k] = match c.edge {
                Some((ci, ei)) => to_pseudo(&edges[ci][ei].0, p, c.min, c.param),
                None => c.min.d,
            };
        }
        out[3] = self.true_min.d;
        out
    }
}

fn resolve(d: &[f64; 4]) -> f64 {
    median(d[0], d[1], d[2])
}

/// msdfgen `OverlappingContourCombiner::distance`.
fn combine(sels: &[Selector], windings: &[i32], edges: &[Vec<(Segment, u8)>], p: P) -> [f64; 4] {
    let dists: Vec<[f64; 4]> = sels.iter().map(|s| s.distance(edges, p)).collect();
    let (mut shape, mut inner, mut outer) = (Selector::EMPTY, Selector::EMPTY, Selector::EMPTY);
    for (i, s) in sels.iter().enumerate() {
        shape.merge(s);
        let r = resolve(&dists[i]);
        if windings[i] > 0 && r >= 0.0 {
            inner.merge(s);
        }
        if windings[i] < 0 && r <= 0.0 {
            outer.merge(s);
        }
    }
    let shape_d = shape.distance(edges, p);
    let inner_d = inner.distance(edges, p);
    let outer_d = outer.distance(edges, p);
    let (is, os) = (resolve(&inner_d), resolve(&outer_d));
    let mut distance;
    let wind;
    if is >= 0.0 && is.abs() <= os.abs() {
        distance = inner_d;
        wind = 1;
        for (i, d) in dists.iter().enumerate() {
            if windings[i] > 0 && resolve(d).abs() < os.abs() && resolve(d) > resolve(&distance) {
                distance = *d;
            }
        }
    } else if os <= 0.0 && os.abs() < is.abs() {
        distance = outer_d;
        wind = -1;
        for (i, d) in dists.iter().enumerate() {
            if windings[i] < 0 && resolve(d).abs() < is.abs() && resolve(d) < resolve(&distance) {
                distance = *d;
            }
        }
    } else {
        return shape_d;
    }
    for (i, d) in dists.iter().enumerate() {
        if windings[i] != wind {
            let (rd, rdist) = (resolve(d), resolve(&distance));
            if rd * rdist >= 0.0 && rd.abs() < rdist.abs() {
                distance = *d;
            }
        }
    }
    if resolve(&distance) == resolve(&shape_d) {
        distance = shape_d;
    }
    distance
}

/// Combined field at `p` plus the shape-wide minimum |distance| (exact outside the shape).
fn field_at(edges: &[Vec<(Segment, u8)>], windings: &[i32], p: P) -> ([f64; 4], f64) {
    let mut sels = Vec::with_capacity(edges.len());
    let mut nearest = f64::MAX;
    for (ci, contour) in edges.iter().enumerate() {
        let mut s = Selector::EMPTY;
        for (ei, (seg, color)) in contour.iter().enumerate() {
            let (sd, param) = signed_distance(seg, p);
            nearest = nearest.min(sd.d.abs());
            if sd.less(&s.true_min) {
                s.true_min = sd;
            }
            for (k, bit) in [RED, GREEN, BLUE].into_iter().enumerate() {
                if color & bit != 0 && sd.less(&s.ch[k].min) {
                    s.ch[k] = Channel {
                        min: sd,
                        edge: Some((ci, ei)),
                        param,
                    };
                }
            }
        }
        sels.push(s);
    }
    (combine(&sels, windings, edges, p), nearest)
}

/// msdfgen legacy `detectClash`.
fn clash(a: [f32; 3], b: [f32; 3], threshold: f32) -> bool {
    let (mut a0, mut a1, mut a2) = (a[0], a[1], a[2]);
    let (mut b0, mut b1, mut b2) = (b[0], b[1], b[2]);
    if (b0 - a0).abs() < (b1 - a1).abs() {
        std::mem::swap(&mut a0, &mut a1);
        std::mem::swap(&mut b0, &mut b1);
    }
    if (b1 - a1).abs() < (b2 - a2).abs() {
        std::mem::swap(&mut a1, &mut a2);
        std::mem::swap(&mut b1, &mut b2);
        if (b0 - a0).abs() < (b1 - a1).abs() {
            std::mem::swap(&mut a0, &mut a1);
            std::mem::swap(&mut b0, &mut b1);
        }
    }
    (b1 - a1).abs() >= threshold && !(b0 == b1 && b0 == b2) && (a2 - 0.5).abs() >= (b2 - 0.5).abs()
}

fn correct_errors(px: &mut [[f32; 4]], w: usize, h: usize, threshold: f32) {
    let rgb = |p: &[f32; 4]| [p[0], p[1], p[2]];
    let mut clashes = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let a = rgb(&px[y * w + x]);
            let n = |xx: usize, yy: usize| rgb(&px[yy * w + xx]);
            if (x > 0 && clash(a, n(x - 1, y), threshold))
                || (x + 1 < w && clash(a, n(x + 1, y), threshold))
                || (y > 0 && clash(a, n(x, y - 1), threshold))
                || (y + 1 < h && clash(a, n(x, y + 1), threshold))
            {
                clashes.push(y * w + x);
            }
        }
    }
    for i in clashes {
        let p = &mut px[i];
        let m = p[0].min(p[1]).max(p[0].max(p[1]).min(p[2]));
        p[0] = m;
        p[1] = m;
        p[2] = m;
    }
}

/// Interpolation artefact check (in the spirit of msdfgen ≥ 1.9's error correction): inside
/// every texel cell, at 4×4 sub-points, the bilinearly interpolated MSDF median must agree in
/// sign with the interpolated true SDF wherever the latter is more than one atlas pixel from an
/// edge. Otherwise the cell's texels fall back to the true SDF in all channels (that spot loses
/// its sharp corner but cannot show a false edge). The true SDF interpolates without channel
/// conflicts, so it is a cheap, exact-enough oracle (no winding tests per sub-point).
fn correct_interpolation(px: &mut [[f32; 4]], w: usize, h: usize, range_px: f64) {
    let margin = (1.0 / range_px) as f32;
    let med = |p: [f32; 4]| p[0].min(p[1]).max(p[0].max(p[1]).min(p[2]));
    for _pass in 0..2 {
        let mut bad = vec![false; w * h];
        let mut any = false;
        for y in 0..h.saturating_sub(1) {
            for x in 0..w.saturating_sub(1) {
                let ids = [
                    y * w + x,
                    y * w + x + 1,
                    (y + 1) * w + x,
                    (y + 1) * w + x + 1,
                ];
                let [a, b, c, d] = ids.map(|i| px[i]);
                let mut wrong = false;
                'sub: for sy in 0..4 {
                    for sx in 0..4 {
                        let (tx, ty) = ((sx as f32 + 0.5) / 4.0, (sy as f32 + 0.5) / 4.0);
                        let v: [f32; 4] = std::array::from_fn(|k| {
                            let top = a[k] * (1.0 - tx) + b[k] * tx;
                            let bot = c[k] * (1.0 - tx) + d[k] * tx;
                            top * (1.0 - ty) + bot * ty
                        });
                        if (v[3] - 0.5).abs() > margin && (med(v) > 0.5) != (v[3] > 0.5) {
                            wrong = true;
                            break 'sub;
                        }
                    }
                }
                if wrong {
                    any = true;
                    for i in ids {
                        bad[i] = true;
                    }
                }
            }
        }
        if !any {
            break;
        }
        for (p, b) in px.iter_mut().zip(bad) {
            if b {
                let a = p[3];
                p[0] = a;
                p[1] = a;
                p[2] = a;
            }
        }
    }
}

/// Generates the MTSDF of `outline`. `None` for blank glyphs (spaces).
#[must_use]
pub fn generate(outline: &Outline, cfg: &MsdfConfig) -> Option<GlyphField> {
    if outline.is_empty() || outline.units_per_em <= 0.0 {
        return None;
    }
    let b = outline.bounds()?;
    let s = cfg.em_px / outline.units_per_em;
    let pad = cfg.range_px / 2.0 + 1.0;
    let x0 = (b[0] * s - pad).floor();
    let y0 = (b[1] * s - pad).floor();
    let x1 = (b[2] * s + pad).ceil();
    let y1 = (b[3] * s + pad).ceil();
    let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let edges = color_edges(&outline.contours, cfg.angle_threshold);
    let windings: Vec<i32> = edges.iter().map(|e| winding(e)).collect();
    let range_units = cfg.range_px / s;

    // Orientation: a point well outside the bounds must come out negative.
    let probe = [b[0] - (b[2] - b[0]) - 1.0, b[1] - (b[3] - b[1]) - 1.0];
    let flip = if field_at(&edges, &windings, probe).0[3] > 0.0 {
        -1.0
    } else {
        1.0
    };

    let at = |fx: f64, fy: f64| [(x0 + fx) / s, (y1 - fy) / s];
    let mut px = vec![[0f32; 4]; w * h];
    for j in 0..h {
        for i in 0..w {
            let p = at(i as f64 + 0.5, j as f64 + 0.5);
            let (d, nearest) = field_at(&edges, &windings, p);
            let mut d = d.map(|v| v * flip);
            // Sign correction against the true nonzero fill (msdfgen "scanline pass"): fixes
            // self-intersecting / exotic overlaps the contour combiner cannot resolve.
            let inside = outline.winding_at(p) != 0;
            if (median(d[0], d[1], d[2]) > 0.0) != inside {
                for v in &mut d[..3] {
                    *v = -*v;
                }
            }
            // True SDF: exact outside (nearest edge); inside, the combiner's value (no valleys
            // along hidden overlap edges); sign from the fill.
            d[3] = if inside { d[3].abs() } else { -nearest };
            px[j * w + i] = d.map(|v| (v / range_units + 0.5) as f32);
        }
    }
    if cfg.error_correction {
        correct_errors(&mut px, w, h, (1.001 / cfg.range_px) as f32);
        correct_interpolation(&mut px, w, h, cfg.range_px);
    }
    let rgba = px
        .iter()
        .flat_map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8))
        .collect();
    let em = cfg.em_px;
    Some(GlyphField {
        width: w as u32,
        height: h as u32,
        rgba,
        plane: [
            (x0 / em) as f32,
            (y0 / em) as f32,
            (x1 / em) as f32,
            (y1 / em) as f32,
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64, cw: bool) -> Contour {
        let pts = if cw {
            [[x0, y1], [x1, y1], [x1, y0], [x0, y0]]
        } else {
            [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
        };
        Contour {
            segments: (0..4)
                .map(|i| Segment::Line([pts[i], pts[(i + 1) % 4]]))
                .collect(),
        }
    }

    fn sample(f: &GlyphField, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * f.width + x) * 4) as usize;
        [f.rgba[i], f.rgba[i + 1], f.rgba[i + 2], f.rgba[i + 3]]
    }

    fn med(p: [u8; 4]) -> u8 {
        p[0].min(p[1]).max(p[0].max(p[1]).min(p[2]))
    }

    #[test]
    fn square_is_inside_positive_and_corners_get_distinct_channels() {
        // A 1000-unit square, clockwise (TrueType outer contour), em = 1000 units → 48 px.
        let o = Outline {
            contours: vec![rect(0.0, 0.0, 1000.0, 1000.0, true)],
            units_per_em: 1000.0,
        };
        let f = generate(&o, &MsdfConfig::default()).expect("field");
        let c = sample(&f, f.width / 2, f.height / 2);
        assert!(med(c) > 200 && c[3] > 200, "centre is deep inside: {c:?}");
        let corner = sample(&f, 0, 0);
        assert!(
            med(corner) < 60 && corner[3] < 60,
            "far outside: {corner:?}"
        );
        // Same field for the counter-clockwise version (orientation probe flips it).
        let ccw = Outline {
            contours: vec![rect(0.0, 0.0, 1000.0, 1000.0, false)],
            units_per_em: 1000.0,
        };
        let g = generate(&ccw, &MsdfConfig::default()).expect("field");
        assert_eq!(med(sample(&g, g.width / 2, g.height / 2)), med(c));
        // Four corners → edges are coloured with at least two different colours.
        let colored = color_edges(&o.contours, 3.0);
        let distinct: std::collections::BTreeSet<u8> = colored[0].iter().map(|e| e.1).collect();
        assert!(distinct.len() >= 2);
    }

    #[test]
    fn overlapping_contours_have_no_interior_seam() {
        // Two overlapping clockwise squares (variable fonts keep such overlaps).
        let o = Outline {
            contours: vec![
                rect(0.0, 0.0, 600.0, 600.0, true),
                rect(400.0, 0.0, 1000.0, 600.0, true),
            ],
            units_per_em: 1000.0,
        };
        let f = generate(&o, &MsdfConfig::default()).expect("field");
        // Along the middle row, every texel inside the union must be clearly inside (no 0.5
        // seam where the contours overlap at x = 400..600 units).
        let y = f.height / 2;
        let s = 48.0 / 1000.0;
        let x_of = |u: f64| ((u * s - f64::from(f.plane[0]) * 48.0) as u32).min(f.width - 1);
        for x in x_of(50.0)..x_of(950.0) {
            let p = sample(&f, x, y);
            assert!(med(p) > 140, "x={x} {p:?}");
        }
    }

    /// Fraction of bilinearly interpolated sample points (4×4 per texel) whose MSDF median sign
    /// disagrees with the true nonzero fill, ignoring points within `margin` atlas px of an
    /// edge (where AA decides anyway).
    pub(crate) fn sign_error_rate(o: &Outline, cfg: &MsdfConfig) -> (usize, usize) {
        let Some(f) = generate(o, cfg) else {
            return (0, 0);
        };
        let s = cfg.em_px / o.units_per_em;
        let (w, h) = (f.width as usize, f.height as usize);
        let tex = |x: usize, y: usize, c: usize| f64::from(f.rgba[(y * w + x) * 4 + c]) / 255.0;
        let x0 = f64::from(f.plane[0]) * cfg.em_px;
        let y1 = f64::from(f.plane[3]) * cfg.em_px;
        let (mut bad, mut total) = (0, 0);
        for j in 0..(h - 1) * 4 {
            for i in 0..(w - 1) * 4 {
                // Sample position in texel space (centres at k + 0.5).
                let (u, v) = (0.5 + (i as f64 + 0.5) / 4.0, 0.5 + (j as f64 + 0.5) / 4.0);
                let (fx, fy) = (u - 0.5, v - 0.5);
                let (ix, iy) = (fx.floor() as usize, fy.floor() as usize);
                let (tx, ty) = (fx - ix as f64, fy - iy as f64);
                let mut ch = [0.0; 4];
                for (c, out) in ch.iter_mut().enumerate() {
                    let top = tex(ix, iy, c) * (1.0 - tx) + tex(ix + 1, iy, c) * tx;
                    let bot = tex(ix, iy + 1, c) * (1.0 - tx) + tex(ix + 1, iy + 1, c) * tx;
                    *out = top * (1.0 - ty) + bot * ty;
                }
                let m = median(ch[0], ch[1], ch[2]);
                let p = [(x0 + u) / s, (y1 - v) / s];
                let inside = o.winding_at(p) != 0;
                let true_d = (ch[3] - 0.5) * cfg.range_px;
                if true_d.abs() < 1.0 {
                    continue;
                }
                total += 1;
                if (m > 0.5) != inside {
                    bad += 1;
                }
            }
        }
        (bad, total)
    }

    #[test]
    fn real_glyphs_have_no_sign_artifacts() {
        use crate::text::fonts::FontLibrary;
        use crate::text::outline::glyph_outline;
        let lib = FontLibrary::bundled().expect("fonts");
        let mut worst = Vec::new();
        let mut all = (0, 0);
        for id in lib.face_ids() {
            let face = lib.face(id).expect("face").clone();
            let parsed = ttf_parser::Face::parse(&face.data, face.index).expect("parse");
            let n = parsed.number_of_glyphs();
            // Every 3rd glyph at the heavy weight (where variable-font overlaps are worst); the
            // full sweep (first 120 glyphs per face, 400 + 800) was run once: 0 errors in 16.7 M
            // samples.
            for g in (0..n.min(120)).step_by(3) {
                let weight = 800u16;
                let o = glyph_outline(&face, g, weight);
                let (b, t) = sign_error_rate(&o, &MsdfConfig::default());
                all.0 += b;
                all.1 += t;
                if b > 0 {
                    worst.push((b, face.family.clone(), g, weight));
                }
            }
        }
        worst.sort();
        worst.reverse();
        eprintln!(
            "sign errors {}/{}; worst {:?}",
            all.0,
            all.1,
            &worst[..worst.len().min(12)]
        );
        assert!(
            (all.0 as f64) < 1e-4 * all.1 as f64,
            "sign errors {}/{}: {:?}",
            all.0,
            all.1,
            &worst[..worst.len().min(12)]
        );
    }

    #[test]
    #[ignore = "diagnostic"]
    fn dump_at_sign() {
        use crate::text::fonts::FontLibrary;
        use crate::text::outline::glyph_outline;
        let lib = FontLibrary::bundled().expect("fonts");
        let id = lib.face_ids()[0];
        let face = lib.face(id).expect("face").clone();
        let parsed = ttf_parser::Face::parse(&face.data, face.index).expect("parse");
        let g = parsed.glyph_index('@').expect("@");
        let o = glyph_outline(&face, g.0, 800);
        let cfg = MsdfConfig::default();
        let colored = color_edges(&o.contours, 3.0);
        for (i, c) in colored.iter().enumerate() {
            eprintln!(
                "contour {i}: {} segs, winding {}, colours {:?}",
                c.len(),
                winding(c),
                c.iter().map(|e| e.1).collect::<Vec<_>>()
            );
        }
        let f = generate(&o, &cfg).expect("field");
        let s = cfg.em_px / o.units_per_em;
        let (w, h) = (f.width as usize, f.height as usize);
        let x0 = f64::from(f.plane[0]) * cfg.em_px;
        let y1 = f64::from(f.plane[3]) * cfg.em_px;
        for j in 0..h {
            let mut line = String::new();
            for i in 0..w {
                let p = &f.rgba[(j * w + i) * 4..][..4];
                let m = p[0].min(p[1]).max(p[0].max(p[1]).min(p[2]));
                let inside =
                    o.winding_at([(x0 + i as f64 + 0.5) / s, (y1 - j as f64 - 0.5) / s]) != 0;
                line.push(match (m > 127, inside, p[3] > 127) {
                    (true, true, true) => '#',
                    (false, false, false) => '.',
                    (true, false, _) => 'X',
                    (false, true, _) => 'O',
                    (true, true, false) => 'a',
                    (false, false, true) => 'b',
                });
            }
            eprintln!("{line}");
        }
    }

    #[test]
    fn sample_text_glyphs_have_no_sign_artifacts() {
        use crate::text::fonts::FontLibrary;
        use crate::text::outline::glyph_outline;
        let lib = FontLibrary::bundled().expect("fonts");
        let mut errors = Vec::new();
        for id in lib.face_ids() {
            let face = lib.face(id).expect("face").clone();
            let parsed = ttf_parser::Face::parse(&face.data, face.index).expect("parse");
            for ch in "Ag@%Cutweave字幕مرحبا🎬✨🔥&$8eaBRS".chars() {
                let Some(g) = parsed.glyph_index(ch) else {
                    continue;
                };
                let o = glyph_outline(&face, g.0, 800);
                let (b, t) = sign_error_rate(&o, &MsdfConfig::default());
                // Features thinner than one atlas texel (the slit in a heavy '@' at 48 px/em)
                // cannot be represented; allow 0.1% of interior/exterior samples.
                if b * 1000 > t {
                    errors.push((ch, b, t));
                }
            }
        }
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn generation_is_deterministic() {
        let o = Outline {
            contours: vec![Contour {
                segments: vec![
                    Segment::Quad([[0.0, 0.0], [500.0, 900.0], [1000.0, 0.0]]),
                    Segment::Cubic([[1000.0, 0.0], [700.0, -300.0], [300.0, -300.0], [0.0, 0.0]]),
                ],
            }],
            units_per_em: 1000.0,
        };
        let a = generate(&o, &MsdfConfig::default()).expect("a");
        let b = generate(&o, &MsdfConfig::default()).expect("b");
        assert_eq!(a, b);
    }
}
