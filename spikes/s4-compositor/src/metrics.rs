//! Image comparison metrics for the golden/reference policy (§20 and the Phase-0 tolerance
//! table): PSNR, SSIM and CIEDE2000 ΔE.

use crate::color::{self, display_eotf, mat_vec, rgb_to_xyz, xyz_to_lab};
use crate::image::Image;
use crate::scene::{Primaries, Transfer};

/// §20 regression tolerance vs a stored per-adapter reference.
pub const GOLDEN_MIN_SSIM: f64 = 0.995;
/// §20 regression tolerance vs a stored per-adapter reference.
pub const GOLDEN_MIN_PSNR_DB: f64 = 40.0;
/// Phase-0 tolerance: implementation vs an independent (CPU float) reference.
pub const REFERENCE_MIN_PSNR_DB: f64 = 45.0;
/// Phase-0 tolerance for colour vs the independent reference.
pub const REFERENCE_MAX_DELTA_E: f64 = 2.0;

fn same_shape(a: &Image, b: &Image) -> bool {
    a.width == b.width && a.height == b.height && a.bit_depth == b.bit_depth
}

/// PSNR over the R, G and B samples in dB; `f64::INFINITY` for identical images and `NaN` for
/// mismatched shapes.
#[must_use]
pub fn psnr(a: &Image, b: &Image) -> f64 {
    if !same_shape(a, b) {
        return f64::NAN;
    }
    let mut sse = 0.0f64;
    let mut n = 0u64;
    for (pa, pb) in a
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.data.as_chunks::<4>().0)
    {
        for c in 0..3 {
            let d = f64::from(pa[c]) - f64::from(pb[c]);
            sse += d * d;
        }
        n += 3;
    }
    if sse == 0.0 {
        return f64::INFINITY;
    }
    let mse = sse / n as f64;
    let max = f64::from(a.max_value());
    10.0 * (max * max / mse).log10()
}

fn luma_plane(img: &Image) -> Vec<f64> {
    let max = f64::from(img.max_value());
    img.data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            (0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])) / max
        })
        .collect()
}

fn gaussian_11() -> [f64; 11] {
    let mut w = [0.0; 11];
    for (i, v) in w.iter_mut().enumerate() {
        let x = i as f64 - 5.0;
        *v = (-(x * x) / (2.0 * 1.5 * 1.5)).exp();
    }
    let s: f64 = w.iter().sum();
    w.map(|v| v / s)
}

/// Separable 11-tap filter, "valid" region only.
fn filter_valid(src: &[f64], w: usize, h: usize, k: &[f64; 11]) -> (Vec<f64>, usize, usize) {
    let (ow, oh) = (w - 10, h - 10);
    let mut tmp = vec![0.0; ow * h];
    for y in 0..h {
        for x in 0..ow {
            tmp[y * ow + x] = (0..11).map(|i| k[i] * src[y * w + x + i]).sum();
        }
    }
    let mut out = vec![0.0; ow * oh];
    for y in 0..oh {
        for x in 0..ow {
            out[y * ow + x] = (0..11).map(|i| k[i] * tmp[(y + i) * ow + x]).sum();
        }
    }
    (out, ow, oh)
}

/// Mean SSIM (Wang et al. 2004) on Rec.709 luma of the encoded values: 11×11 Gaussian window,
/// σ = 1.5, K1 = 0.01, K2 = 0.03, valid region. Exactly 1.0 for identical images.
#[must_use]
pub fn ssim(a: &Image, b: &Image) -> f64 {
    if !same_shape(a, b) {
        return f64::NAN;
    }
    let (w, h) = (a.width as usize, a.height as usize);
    let (x, y) = (luma_plane(a), luma_plane(b));
    let (c1, c2) = (0.01f64.powi(2), 0.03f64.powi(2));
    if w < 11 || h < 11 {
        // Global statistics for tiny images.
        let n = x.len() as f64;
        let (mx, my) = (x.iter().sum::<f64>() / n, y.iter().sum::<f64>() / n);
        let sxx = x.iter().map(|v| (v - mx).powi(2)).sum::<f64>() / n;
        let syy = y.iter().map(|v| (v - my).powi(2)).sum::<f64>() / n;
        let sxy = x
            .iter()
            .zip(&y)
            .map(|(p, q)| (p - mx) * (q - my))
            .sum::<f64>()
            / n;
        return ((2.0 * mx * my + c1) * (2.0 * sxy + c2))
            / ((mx * mx + my * my + c1) * (sxx + syy + c2));
    }
    let k = gaussian_11();
    let xx: Vec<f64> = x.iter().map(|v| v * v).collect();
    let yy: Vec<f64> = y.iter().map(|v| v * v).collect();
    let xy: Vec<f64> = x.iter().zip(&y).map(|(p, q)| p * q).collect();
    let (mx, ow, oh) = filter_valid(&x, w, h, &k);
    let (my, _, _) = filter_valid(&y, w, h, &k);
    let (exx, _, _) = filter_valid(&xx, w, h, &k);
    let (eyy, _, _) = filter_valid(&yy, w, h, &k);
    let (exy, _, _) = filter_valid(&xy, w, h, &k);
    let mut total = 0.0;
    for i in 0..ow * oh {
        let (ux, uy) = (mx[i], my[i]);
        let sxx = exx[i] - ux * ux;
        let syy = eyy[i] - uy * uy;
        let sxy = exy[i] - ux * uy;
        total += ((2.0 * ux * uy + c1) * (2.0 * sxy + c2))
            / ((ux * ux + uy * uy + c1) * (sxx + syy + c2));
    }
    total / (ow * oh) as f64
}

/// CIEDE2000 colour difference (Sharma, Wu & Dalal 2005), kL = kC = kH = 1.
#[must_use]
pub fn delta_e_2000(lab1: [f64; 3], lab2: [f64; 3]) -> f64 {
    use std::f64::consts::PI;
    let [l1, a1, b1] = lab1;
    let [l2, a2, b2] = lab2;
    let c1 = a1.hypot(b1);
    let c2 = a2.hypot(b2);
    let cbar = (c1 + c2) / 2.0;
    let cbar7 = cbar.powi(7);
    let g = 0.5 * (1.0 - (cbar7 / (cbar7 + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let hue = |b: f64, a: f64| {
        if a == 0.0 && b == 0.0 {
            0.0
        } else {
            let h = b.atan2(a).to_degrees();
            if h < 0.0 { h + 360.0 } else { h }
        }
    };
    let (h1p, h2p) = (hue(b1, a1p), hue(b2, a2p));
    let dlp = l2 - l1;
    let dcp = c2p - c1p;
    let dhp = if c1p * c2p == 0.0 {
        0.0
    } else if (h2p - h1p).abs() <= 180.0 {
        h2p - h1p
    } else if h2p - h1p > 180.0 {
        h2p - h1p - 360.0
    } else {
        h2p - h1p + 360.0
    };
    let dhp_big = 2.0 * (c1p * c2p).sqrt() * (dhp.to_radians() / 2.0).sin();
    let lbarp = (l1 + l2) / 2.0;
    let cbarp = (c1p + c2p) / 2.0;
    let hbarp = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * (hbarp - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hbarp).to_radians().cos()
        + 0.32 * (3.0 * hbarp + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hbarp - 63.0).to_radians().cos();
    let dtheta = 30.0 * (-((hbarp - 275.0) / 25.0).powi(2)).exp();
    let cbarp7 = cbarp.powi(7);
    let rc = 2.0 * (cbarp7 / (cbarp7 + 25f64.powi(7))).sqrt();
    let lterm = (lbarp - 50.0).powi(2);
    let sl = 1.0 + 0.015 * lterm / (20.0 + lterm).sqrt();
    let sc = 1.0 + 0.045 * cbarp;
    let sh = 1.0 + 0.015 * cbarp * t;
    let rt = -(2.0 * dtheta * PI / 180.0).sin() * rc;
    let (tl, tc, th) = (dlp / sl, dcp / sc, dhp_big / sh);
    (tl * tl + tc * tc + th * th + rt * tc * th).sqrt()
}

/// ΔE2000 statistics over an image pair.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeltaEStats {
    pub mean: f64,
    pub p95: f64,
    pub max: f64,
}

/// Per-pixel ΔE2000 between two images encoded with `transfer` (decoded through the display
/// EOTF, BT.709 primaries, D65, alpha ignored).
#[must_use]
pub fn delta_e_stats(a: &Image, b: &Image, transfer: Transfer) -> DeltaEStats {
    if !same_shape(a, b) {
        return DeltaEStats {
            mean: f64::NAN,
            p95: f64::NAN,
            max: f64::NAN,
        };
    }
    let m = rgb_to_xyz(Primaries::Bt709);
    let white = color::d65_xyz();
    let max = f64::from(a.max_value());
    // Encoded → linear lookup per code value.
    let lut: Vec<f64> = (0..=a.max_value())
        .map(|v| display_eotf(f64::from(v) / max, transfer))
        .collect();
    let lab = |p: &[u16]| {
        let rgb = [lut[p[0] as usize], lut[p[1] as usize], lut[p[2] as usize]];
        xyz_to_lab(mat_vec(&m, rgb), white)
    };
    let mut d: Vec<f64> = a
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.data.as_chunks::<4>().0)
        .map(|(pa, pb)| {
            if pa[..3] == pb[..3] {
                0.0
            } else {
                delta_e_2000(lab(pa), lab(pb))
            }
        })
        .collect();
    let n = d.len().max(1);
    let mean = d.iter().sum::<f64>() / n as f64;
    d.sort_by(f64::total_cmp);
    let p95 = d
        .get(((n as f64 * 0.95) as usize).min(n - 1))
        .copied()
        .unwrap_or(0.0);
    DeltaEStats {
        mean,
        p95,
        max: d.last().copied().unwrap_or(0.0),
    }
}

/// All metrics for one comparison.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Comparison {
    pub psnr_db: f64,
    pub ssim: f64,
    pub delta_e: DeltaEStats,
    pub bit_identical: bool,
    pub max_abs_diff: u16,
}

impl Comparison {
    /// §20 golden regression check.
    #[must_use]
    pub fn passes_golden(&self) -> bool {
        self.bit_identical || (self.ssim >= GOLDEN_MIN_SSIM && self.psnr_db >= GOLDEN_MIN_PSNR_DB)
    }

    /// Phase-0 independent-reference check (PSNR > 45 dB and mean ΔE2000 < 2).
    #[must_use]
    pub fn passes_reference(&self) -> bool {
        self.psnr_db > REFERENCE_MIN_PSNR_DB && self.delta_e.mean < REFERENCE_MAX_DELTA_E
    }
}

/// Computes every metric.
#[must_use]
pub fn compare(a: &Image, b: &Image, transfer: Transfer) -> Comparison {
    Comparison {
        psnr_db: psnr(a, b),
        ssim: ssim(a, b),
        delta_e: delta_e_stats(a, b, transfer),
        bit_identical: a.diff_count(b) == 0,
        max_abs_diff: a.max_abs_diff(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32, f: impl Fn(u32, u32, usize) -> u16) -> Image {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                for c in 0..4 {
                    data.push(if c == 3 { 255 } else { f(x, y, c) });
                }
            }
        }
        Image {
            width: w,
            height: h,
            bit_depth: 8,
            data,
        }
    }

    fn pattern(x: u32, y: u32, c: usize) -> u16 {
        ((x * 7 + y * 13 + c as u32 * 50) % 256) as u16
    }

    #[test]
    fn identical_images_are_perfect() {
        let a = img(40, 30, pattern);
        let c = compare(&a, &a.clone(), Transfer::Bt709);
        assert_eq!(c.psnr_db, f64::INFINITY);
        assert_eq!(c.ssim, 1.0);
        assert_eq!(
            c.delta_e,
            DeltaEStats {
                mean: 0.0,
                p95: 0.0,
                max: 0.0
            }
        );
        assert!(c.bit_identical && c.passes_golden() && c.passes_reference());
    }

    #[test]
    fn psnr_known_value() {
        let a = img(10, 10, |_, _, _| 100);
        // One sample of 300 differs by 3 → MSE = 9/300 = 0.03.
        let mut b = a.clone();
        b.data[0] = 103;
        let expected = 10.0 * (255.0f64 * 255.0 / 0.03).log10();
        assert!((psnr(&a, &b) - expected).abs() < 1e-9);
        // Alpha differences are ignored.
        let mut c = a.clone();
        c.data[3] = 0;
        assert_eq!(psnr(&a, &c), f64::INFINITY);
    }

    #[test]
    fn ssim_drops_with_noise_and_is_symmetric() {
        let a = img(64, 48, pattern);
        let b = img(64, 48, |x, y, c| {
            let v = i32::from(pattern(x, y, c)) + if (x + y) % 2 == 0 { 20 } else { -20 };
            v.clamp(0, 255) as u16
        });
        let s = ssim(&a, &b);
        assert!(s < 0.99 && s > 0.0, "{s}");
        assert!((s - ssim(&b, &a)).abs() < 1e-12);
    }

    #[test]
    fn ciede2000_matches_sharma_test_data() {
        // Pairs 1, 7, 17 and 25 of Sharma, Wu & Dalal (2005), Table 1.
        let cases = [
            ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
            ([50.0, 0.0, 0.0], [50.0, -1.0, 2.0], 2.3669),
            ([50.0, 2.5, 0.0], [73.0, 25.0, -18.0], 27.1492),
            (
                [60.2574, -34.0099, 36.2677],
                [60.4626, -34.1751, 39.4387],
                1.2644,
            ),
        ];
        for (a, b, expected) in cases {
            let d = delta_e_2000(a, b);
            assert!(
                (d - expected).abs() < 1e-4,
                "{a:?} {b:?}: {d} vs {expected}"
            );
            assert!((delta_e_2000(b, a) - d).abs() < 1e-9, "symmetric");
        }
        assert_eq!(delta_e_2000([42.0, 10.0, -5.0], [42.0, 10.0, -5.0]), 0.0);
    }

    #[test]
    fn one_code_value_is_a_small_delta_e() {
        let a = img(16, 16, |_, _, _| 128);
        let b = img(16, 16, |_, _, c| if c == 0 { 129 } else { 128 });
        let d = delta_e_stats(&a, &b, Transfer::Bt709);
        assert!(d.mean > 0.0 && d.mean < 1.0, "{d:?}");
        let c = compare(&a, &b, Transfer::Bt709);
        assert!(c.passes_golden(), "1 LSB on one channel passes §20");
    }
}
