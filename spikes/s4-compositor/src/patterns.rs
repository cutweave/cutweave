//! Synthetic test patterns and colour charts, encoded exactly like decoder output would be.

use crate::color::{self, kr_kb, range_params};
use crate::scene::{ColorTags, Matrix, Primaries, Range, Transfer};
use crate::source::PixelData;

/// Encodes an R'G'B' signal function into 4:2:0 Y'CbCr planes (MPEG-2 chroma siting: each
/// chroma sample averages the two vertically adjacent luma sites in its even column).
fn encode_yuv420(
    w: u32,
    h: u32,
    bits: u8,
    tags: ColorTags,
    rgb: &dyn Fn(u32, u32) -> [f64; 3],
) -> PixelData {
    let (kr, kb) = kr_kb(tags.matrix).unwrap_or((0.2126, 0.0722));
    let rp = range_params(tags.range, bits);
    let max = f64::from((1u32 << bits) - 1);
    let q = |v: f64| v.round().clamp(0.0, max) as u16;
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut y_plane = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let ycc = color::rgb_to_ycbcr(rgb(x, y), kr, kb);
            y_plane.push(q(rp.y_off + rp.y_scale * ycc[0]));
        }
    }
    let mut cb_plane = Vec::with_capacity((cw * ch) as usize);
    let mut cr_plane = Vec::with_capacity((cw * ch) as usize);
    for cy in 0..ch {
        for cx in 0..cw {
            let x = (2 * cx).min(w - 1);
            let a = color::rgb_to_ycbcr(rgb(x, (2 * cy).min(h - 1)), kr, kb);
            let b = color::rgb_to_ycbcr(rgb(x, (2 * cy + 1).min(h - 1)), kr, kb);
            cb_plane.push(q(rp.c_off + rp.c_scale * 0.5 * (a[1] + b[1])));
            cr_plane.push(q(rp.c_off + rp.c_scale * 0.5 * (a[2] + b[2])));
        }
    }
    PixelData::Yuv420 {
        bit_depth: bits,
        y: y_plane,
        cb: cb_plane,
        cr: cr_plane,
    }
}

fn uv(x: u32, y: u32, w: u32, h: u32) -> (f64, f64) {
    (
        (f64::from(x) + 0.5) / f64::from(w),
        (f64::from(y) + 0.5) / f64::from(h),
    )
}

/// "Camera" clip: BT.709, 8-bit limited. 75% bars, a luma ramp, 11 grey steps, and a white bar
/// sweeping across the frame (one full sweep per 100 frames) so consecutive frames differ.
#[must_use]
pub fn camera709(w: u32, h: u32, frame: i64) -> (PixelData, ColorTags) {
    let tags = ColorTags {
        matrix: Matrix::Bt709,
        range: Range::Limited,
        transfer: Transfer::Bt709,
        primaries: Primaries::Bt709,
    };
    const BARS: [[f64; 3]; 7] = [
        [0.75, 0.75, 0.75],
        [0.75, 0.75, 0.0],
        [0.0, 0.75, 0.75],
        [0.0, 0.75, 0.0],
        [0.75, 0.0, 0.75],
        [0.75, 0.0, 0.0],
        [0.0, 0.0, 0.75],
    ];
    let sweep = (frame.rem_euclid(100) as f64 + 0.5) / 100.0;
    let f = move |x: u32, y: u32| -> [f64; 3] {
        let (u, v) = uv(x, y, w, h);
        if (u - sweep).abs() < 0.0125 {
            return [1.0, 1.0, 1.0];
        }
        if v < 0.6 {
            BARS[((u * 7.0) as usize).min(6)]
        } else if v < 0.75 {
            [u, u, u]
        } else {
            let s = ((u * 11.0).floor() / 10.0).min(1.0);
            [s, s, s]
        }
    };
    (encode_yuv420(w, h, 8, tags, &f), tags)
}

/// HDR10 clip: BT.2020 NCL, 10-bit limited, PQ. Top: neutral ramp 0–4000 cd/m² (uniform in
/// PQ); middle: BT.2020 R, G, B, C, M, Y at 1000 cd/m² per component (mostly outside BT.709);
/// bottom: grey steps 0.1 … 4000 cd/m².
#[must_use]
pub fn hdr10(w: u32, h: u32) -> (PixelData, ColorTags) {
    let tags = ColorTags {
        matrix: Matrix::Bt2020Ncl,
        range: Range::Limited,
        transfer: Transfer::Pq,
        primaries: Primaries::Bt2020,
    };
    const PATCHES: [[f64; 3]; 6] = [
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 1.0, 1.0],
        [1.0, 0.0, 1.0],
        [1.0, 1.0, 0.0],
    ];
    const STEPS: [f64; 10] = [
        0.1, 1.0, 10.0, 50.0, 100.0, 203.0, 400.0, 1000.0, 2000.0, 4000.0,
    ];
    let ramp_top = color::pq_inv_eotf(0.4);
    let f = move |x: u32, y: u32| -> [f64; 3] {
        let (u, v) = uv(x, y, w, h);
        let nits = if v < 0.4 {
            let n = color::pq_eotf(u * ramp_top) * 10_000.0;
            [n, n, n]
        } else if v < 0.7 {
            PATCHES[((u * 6.0) as usize).min(5)].map(|c| c * 1000.0)
        } else {
            let n = STEPS[((u * 10.0) as usize).min(9)];
            [n, n, n]
        };
        nits.map(|n| color::pq_inv_eotf(n / 10_000.0))
    };
    (encode_yuv420(w, h, 10, tags, &f), tags)
}

fn hsv_to_rgb(h: f64, s: f64, v: f64) -> [f64; 3] {
    let h6 = (h.rem_euclid(1.0)) * 6.0;
    let i = h6.floor();
    let f = h6 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as u32 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// "Phone" clip: BT.601 matrix, 8-bit **full** range, sRGB transfer (JFIF-style tags).
#[must_use]
pub fn phone601(w: u32, h: u32) -> (PixelData, ColorTags) {
    let tags = ColorTags {
        matrix: Matrix::Bt601,
        range: Range::Full,
        transfer: Transfer::Srgb,
        primaries: Primaries::Bt709,
    };
    let aspect = f64::from(w) / f64::from(h);
    let f = move |x: u32, y: u32| -> [f64; 3] {
        let (u, v) = uv(x, y, w, h);
        let base = hsv_to_rgb(u, 0.75, 0.3 + 0.65 * (1.0 - v));
        let (dx, dy) = ((u - 0.5) * aspect, v - 0.5);
        let r2 = dx * dx + dy * dy;
        if r2 < 0.09 {
            let ring = 0.5 + 0.5 * (std::f64::consts::TAU * 30.0 * r2).cos();
            base.map(|c| c * 0.4 + 0.6 * ring)
        } else {
            base
        }
    };
    (encode_yuv420(w, h, 8, tags, &f), tags)
}

/// sRGB values of the 24-patch ColorChecker Classic (common published approximation).
pub const CHECKER_SRGB: [[u8; 3]; 24] = [
    [115, 82, 68],
    [194, 150, 130],
    [98, 122, 157],
    [87, 108, 67],
    [133, 128, 177],
    [103, 189, 170],
    [214, 126, 44],
    [80, 91, 166],
    [193, 90, 99],
    [94, 60, 108],
    [157, 188, 64],
    [224, 163, 46],
    [56, 61, 150],
    [70, 148, 73],
    [175, 54, 60],
    [231, 199, 31],
    [187, 86, 149],
    [8, 133, 161],
    [243, 243, 242],
    [200, 200, 200],
    [160, 160, 160],
    [122, 122, 121],
    [85, 85, 85],
    [52, 52, 52],
];

/// Rounded-box signed distance (negative inside), box centred at the origin.
#[must_use]
pub fn rounded_box_sdf(p: [f64; 2], half: [f64; 2], r: f64) -> f64 {
    let q = [p[0].abs() - (half[0] - r), p[1].abs() - (half[1] - r)];
    let outside = (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2)).sqrt();
    outside + q[0].max(q[1]).min(0.0) - r
}

/// Colour checker card: RGBA8, sRGB, straight alpha, anti-aliased rounded corners.
#[must_use]
pub fn chart(w: u32, h: u32) -> (PixelData, ColorTags) {
    let tags = ColorTags {
        matrix: Matrix::Rgb,
        range: Range::Full,
        transfer: Transfer::Srgb,
        primaries: Primaries::Bt709,
    };
    let (wf, hf) = (f64::from(w), f64::from(h));
    let margin = wf * 0.035;
    let gap = wf * 0.012;
    let pw = (wf - 2.0 * margin - 5.0 * gap) / 6.0;
    let ph = (hf - 2.0 * margin - 3.0 * gap) / 4.0;
    let radius = wf * 0.04;
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            let d = rounded_box_sdf([px - wf / 2.0, py - hf / 2.0], [wf / 2.0, hf / 2.0], radius);
            let alpha = (0.5 - d).clamp(0.0, 1.0);
            let (gx, gy) = ((px - margin) / (pw + gap), (py - margin) / (ph + gap));
            let mut rgb = [28u8, 28, 28];
            if gx >= 0.0 && gy >= 0.0 && gx < 6.0 && gy < 4.0 {
                let (fx, fy) = (gx.fract() * (pw + gap), gy.fract() * (ph + gap));
                if fx < pw && fy < ph {
                    rgb = CHECKER_SRGB[gy as usize * 6 + gx as usize];
                }
            }
            data.extend_from_slice(&[rgb[0], rgb[1], rgb[2], (alpha * 255.0).round() as u8]);
        }
    }
    (PixelData::Rgba8(data), tags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_codes_are_legal_limited_range() {
        let (px, _) = camera709(64, 36, 0);
        let PixelData::Yuv420 {
            y,
            cb,
            cr,
            bit_depth,
        } = px
        else {
            panic!("yuv")
        };
        assert_eq!(bit_depth, 8);
        assert_eq!(y.len(), 64 * 36);
        assert_eq!(cb.len(), 32 * 18);
        assert!(y.iter().all(|&v| (16..=235).contains(&v)));
        assert!(cb.iter().chain(&cr).all(|&v| (16..=240).contains(&v)));
    }

    #[test]
    fn camera_changes_with_frame() {
        assert_ne!(camera709(64, 36, 0).0, camera709(64, 36, 30).0);
        assert_eq!(
            camera709(64, 36, 7).0,
            camera709(64, 36, 107).0,
            "100-frame period"
        );
    }

    #[test]
    fn hdr_is_ten_bit() {
        let (px, tags) = hdr10(32, 16);
        assert_eq!(tags.transfer, Transfer::Pq);
        let PixelData::Yuv420 { y, bit_depth, .. } = px else {
            panic!("yuv")
        };
        assert_eq!(bit_depth, 10);
        assert!(y.iter().any(|&v| v > 255), "uses the 10-bit range");
        assert!(y.iter().all(|&v| (64..=940).contains(&v)));
    }

    #[test]
    fn chart_has_transparent_corners() {
        let (px, _) = chart(120, 80);
        let PixelData::Rgba8(d) = px else {
            panic!("rgba")
        };
        assert_eq!(d[3], 0, "corner pixel transparent");
        assert_eq!(d[(40 * 120 + 60) * 4 + 3], 255, "centre opaque");
    }
}
