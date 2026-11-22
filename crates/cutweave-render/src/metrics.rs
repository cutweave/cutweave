// SPDX-License-Identifier: GPL-3.0-or-later
//! Image comparison metrics and the golden-frame tolerances (arch §20, single source).
//!
//! * PSNR over the R, G, B samples (8-bit peak 255).
//! * SSIM (Wang et al. 2004): 11×11 Gaussian window, σ = 1.5, K1 = 0.01, K2 = 0.03, on Rec.709
//!   luma of the encoded values, "valid" region.
//! * CIEDE2000 ΔE between the colours the encoded values display as (decoded through the display
//!   EOTF of the output transfer, BT.709 primaries, D65).
//!
//! Ported from Spike S4 (`spikes/s4-compositor/src/metrics.rs`), where the ΔE implementation was
//! validated against the Sharma et al. test pairs (kept below).

// Pixel and matrix math reads best with the conventional single-letter names.
#![allow(clippy::many_single_char_names)]
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use crate::color::{self, OutputTransfer, Primaries, Transfer, mat_vec, rgb_to_xyz, xyz_to_lab};
use crate::image::Image;

/// §20 regression tolerance vs a stored (per-adapter) reference: minimum SSIM.
pub const GOLDEN_MIN_SSIM: f64 = 0.995;
/// §20 regression tolerance vs a stored (per-adapter) reference: minimum PSNR in dB.
pub const GOLDEN_MIN_PSNR_DB: f64 = 40.0;
/// GPU vs the independent CPU float reference (§20 spike quality, WP-1F.02): minimum PSNR.
pub const REFERENCE_MIN_PSNR_DB: f64 = 45.0;
/// GPU vs the independent CPU float reference (WP-1F.02): maximum mean CIEDE2000.
pub const REFERENCE_MAX_DELTA_E: f64 = 2.0;
/// Identity transform at opacity 1 vs the decoded source (WP-1F.02): minimum PSNR.
pub const IDENTITY_MIN_PSNR_DB: f64 = 50.0;

fn same_shape(a: &Image, b: &Image) -> bool {
    a.width == b.width && a.height == b.height && a.is_valid() && b.is_valid()
}

/// PSNR over the R, G and B samples in dB: `f64::INFINITY` for identical colour, `NaN` for
/// images of different sizes.
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
        .zip(b.data.as_chunks::<4>().0.iter())
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
    10.0 * (255.0 * 255.0 / mse).log10()
}

fn luma_plane(img: &Image) -> Vec<f64> {
    img.data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            (0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])) / 255.0
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

/// Mean SSIM; exactly 1.0 for identical images, `NaN` for images of different sizes.
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
        let n = x.len().max(1) as f64;
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
#[allow(clippy::similar_names, clippy::many_single_char_names)]
pub fn delta_e_2000(lab1: [f64; 3], lab2: [f64; 3]) -> f64 {
    use std::f64::consts::PI;
    let [l1, a1, b1] = lab1;
    let [l2, a2, b2] = lab2;
    let c1 = a1.hypot(b1);
    let c2 = a2.hypot(b2);
    let cbar = f64::midpoint(c1, c2);
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
    let lbarp = f64::midpoint(l1, l2);
    let cbarp = f64::midpoint(c1p, c2p);
    let hbarp = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        f64::midpoint(h1p, h2p)
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
    /// Mean over all pixels.
    pub mean: f64,
    /// 95th percentile.
    pub p95: f64,
    /// Maximum.
    pub max: f64,
}

/// The display EOTF that turns an encoded output back into light for colorimetry: sRGB for
/// sRGB output, BT.1886 (gamma 2.4) for BT.709-OETF video.
#[must_use]
pub fn display_eotf(v: f64, tf: OutputTransfer) -> f64 {
    match tf {
        OutputTransfer::Srgb => color::to_linear(v, Transfer::Srgb),
        OutputTransfer::Bt709 => v.max(0.0).powf(2.4),
    }
}

/// Per-pixel ΔE2000 between two images encoded with `tf` (alpha ignored).
#[must_use]
pub fn delta_e_stats(a: &Image, b: &Image, tf: OutputTransfer) -> DeltaEStats {
    if !same_shape(a, b) {
        return DeltaEStats {
            mean: f64::NAN,
            p95: f64::NAN,
            max: f64::NAN,
        };
    }
    let m = rgb_to_xyz(Primaries::Bt709);
    let white = color::d65_xyz();
    let lut: Vec<f64> = (0..=255u8)
        .map(|v| display_eotf(f64::from(v) / 255.0, tf))
        .collect();
    let lab = |p: &[u8]| {
        let rgb = [
            lut[usize::from(p[0])],
            lut[usize::from(p[1])],
            lut[usize::from(p[2])],
        ];
        xyz_to_lab(mat_vec(&m, rgb), white)
    };
    let mut d: Vec<f64> = a
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.data.as_chunks::<4>().0.iter())
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

/// Every metric for one image pair.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Comparison {
    /// PSNR in dB (∞ when the colour is identical).
    pub psnr_db: f64,
    /// Mean SSIM.
    pub ssim: f64,
    /// ΔE2000 statistics.
    pub delta_e: DeltaEStats,
    /// Largest absolute difference of any sample, alpha included.
    pub max_abs_diff: u8,
    /// Whether every byte is equal.
    pub identical: bool,
}

impl Comparison {
    /// The §20 regression rule against a stored reference: SSIM ≥ 0.995 and PSNR ≥ 40 dB.
    #[must_use]
    pub fn passes_golden(&self) -> bool {
        self.identical || (self.ssim >= GOLDEN_MIN_SSIM && self.psnr_db >= GOLDEN_MIN_PSNR_DB)
    }

    /// The WP-1F.02 rule against the independent CPU float reference: PSNR > 45 dB and mean
    /// ΔE2000 < 2.
    #[must_use]
    pub fn passes_reference(&self) -> bool {
        self.identical
            || (self.psnr_db > REFERENCE_MIN_PSNR_DB && self.delta_e.mean < REFERENCE_MAX_DELTA_E)
    }
}

impl std::fmt::Display for Comparison {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.identical {
            return write!(f, "bit-identical");
        }
        write!(
            f,
            "PSNR {:.2} dB, SSIM {:.5}, ΔE2000 mean {:.3} / p95 {:.3} / max {:.3}, max |Δ| {}",
            self.psnr_db,
            self.ssim,
            self.delta_e.mean,
            self.delta_e.p95,
            self.delta_e.max,
            self.max_abs_diff
        )
    }
}

/// Compares `a` with `b` (both encoded with `tf`).
#[must_use]
pub fn compare(a: &Image, b: &Image, tf: OutputTransfer) -> Comparison {
    let identical = same_shape(a, b) && a.data == b.data;
    let max_abs_diff = if same_shape(a, b) {
        a.data
            .iter()
            .zip(&b.data)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0)
    } else {
        u8::MAX
    };
    Comparison {
        psnr_db: psnr(a, b),
        ssim: ssim(a, b),
        delta_e: delta_e_stats(a, b, tf),
        max_abs_diff,
        identical,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(w: u32, h: u32, seed: u32) -> Image {
        let mut s = seed.wrapping_mul(2_654_435_761).max(1);
        let mut img = Image::filled(w, h, [0, 0, 0, 255]);
        for px in img.data.as_chunks_mut::<4>().0.iter_mut() {
            for c in px.iter_mut().take(3) {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                *c = (s >> 24) as u8;
            }
        }
        img
    }

    #[test]
    fn identical_images_are_perfect() {
        let a = noise(32, 24, 1);
        let c = compare(&a, &a, OutputTransfer::Srgb);
        assert!(c.identical && c.psnr_db.is_infinite());
        assert!((c.ssim - 1.0).abs() < 1e-12);
        assert_eq!(c.delta_e.max, 0.0);
        assert!(c.passes_golden() && c.passes_reference());
    }

    #[test]
    fn psnr_closed_form() {
        // Every R/G/B sample off by 1 → MSE 1 → 20·log10(255) ≈ 48.13 dB.
        let a = Image::filled(16, 16, [100, 100, 100, 255]);
        let b = Image::filled(16, 16, [101, 101, 101, 255]);
        assert!((psnr(&a, &b) - 20.0 * 255f64.log10()).abs() < 1e-9);
        assert_eq!(compare(&a, &b, OutputTransfer::Srgb).max_abs_diff, 1);
        assert!(psnr(&a, &Image::filled(8, 8, [0; 4])).is_nan());
    }

    #[test]
    fn ssim_drops_with_structure_loss() {
        let a = noise(48, 48, 3);
        let b = noise(48, 48, 4);
        assert!(ssim(&a, &b) < 0.2);
    }

    #[test]
    fn ciede2000_matches_sharma_test_pairs() {
        // Sharma, Wu & Dalal (2005), table 1 (pairs 1, 7, 15, 17, 25, 34).
        let pairs: [([f64; 3], [f64; 3], f64); 6] = [
            ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
            ([50.0, 0.0, 0.0], [50.0, -1.0, 2.0], 2.3669),
            ([50.0, 2.49, -0.001], [50.0, -2.49, 0.0011], 7.2195),
            ([50.0, 2.5, 0.0], [73.0, 25.0, -18.0], 27.1492),
            (
                [60.2574, -34.0099, 36.2677],
                [60.4626, -34.1751, 39.4387],
                1.2644,
            ),
            (
                [2.0776, 0.0795, -1.1350],
                [0.9033, -0.0636, -0.5514],
                0.9082,
            ),
        ];
        for (a, b, want) in pairs {
            let got = delta_e_2000(a, b);
            assert!((got - want).abs() < 1e-4, "{a:?} {b:?}: {got} vs {want}");
        }
    }

    #[test]
    fn tolerances_are_the_section_20_values() {
        assert!((GOLDEN_MIN_SSIM - 0.995).abs() < f64::EPSILON);
        assert!((GOLDEN_MIN_PSNR_DB - 40.0).abs() < f64::EPSILON);
        let a = Image::filled(16, 16, [100, 100, 100, 255]);
        let far = Image::filled(16, 16, [140, 100, 100, 255]);
        assert!(!compare(&a, &far, OutputTransfer::Srgb).passes_golden());
    }
}
