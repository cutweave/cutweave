//! CPU colour math in `f64`: transfer functions, Y'CbCr matrices and ranges, primaries
//! conversion, the BT.2390 EETF and CIE Lab. Used by the synthetic sources, the CPU reference
//! renderer and the metrics. The GPU path has its own independent `f32` WGSL implementation
//! (`src/shaders/common.wgsl`); only derived constants (matrices, EETF knee) are shared.

use crate::scene::{Matrix, Primaries, Range, ToneMap, Transfer};

/// BT.709 / BT.2020 OETF constants in their continuous ("12-bit precise") form.
pub const BT709_ALPHA: f64 = 1.099_296_826_809_44;
pub const BT709_BETA: f64 = 0.018_053_968_510_807;

const PQ_M1: f64 = 2610.0 / 16384.0;
const PQ_M2: f64 = 2523.0 / 4096.0 * 128.0;
const PQ_C1: f64 = 3424.0 / 4096.0;
const PQ_C2: f64 = 2413.0 / 4096.0 * 32.0;
const PQ_C3: f64 = 2392.0 / 4096.0 * 32.0;

fn safe_pow(x: f64, y: f64) -> f64 {
    if x <= 0.0 { 0.0 } else { x.powf(y) }
}

/// SMPTE ST 2084 EOTF: PQ signal → luminance normalised to 10,000 cd/m².
#[must_use]
pub fn pq_eotf(e: f64) -> f64 {
    let ep = safe_pow(e, 1.0 / PQ_M2);
    safe_pow((ep - PQ_C1).max(0.0) / (PQ_C2 - PQ_C3 * ep), 1.0 / PQ_M1)
}

/// Inverse ST 2084 EOTF: luminance normalised to 10,000 cd/m² → PQ signal.
#[must_use]
pub fn pq_inv_eotf(y: f64) -> f64 {
    let yp = safe_pow(y, PQ_M1);
    safe_pow((PQ_C1 + PQ_C2 * yp) / (1.0 + PQ_C3 * yp), PQ_M2)
}

/// Signal → linear. For `Pq` the result is normalised to 10,000 cd/m².
#[must_use]
pub fn to_linear(v: f64, tf: Transfer) -> f64 {
    match tf {
        Transfer::Linear => v,
        Transfer::Srgb => {
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        Transfer::Bt709 => {
            if v < 4.5 * BT709_BETA {
                v / 4.5
            } else {
                ((v + (BT709_ALPHA - 1.0)) / BT709_ALPHA).powf(1.0 / 0.45)
            }
        }
        Transfer::Bt1886 => safe_pow(v, 2.4),
        Transfer::Pq => pq_eotf(v),
    }
}

/// Linear → signal (inverse of [`to_linear`]).
#[must_use]
pub fn from_linear(l: f64, tf: Transfer) -> f64 {
    match tf {
        Transfer::Linear => l,
        Transfer::Srgb => {
            if l <= 0.003_130_8 {
                l * 12.92
            } else {
                1.055 * l.powf(1.0 / 2.4) - 0.055
            }
        }
        Transfer::Bt709 => {
            if l < BT709_BETA {
                l * 4.5
            } else {
                BT709_ALPHA * l.powf(0.45) - (BT709_ALPHA - 1.0)
            }
        }
        Transfer::Bt1886 => safe_pow(l, 1.0 / 2.4),
        Transfer::Pq => pq_inv_eotf(l),
    }
}

/// The display EOTF used to interpret an *encoded output* for colorimetry (ΔE). BT.709-OETF
/// encoded video is displayed through BT.1886 (BT.2087 / common practice).
#[must_use]
pub fn display_eotf(v: f64, tf: Transfer) -> f64 {
    match tf {
        Transfer::Bt709 | Transfer::Bt1886 => to_linear(v, Transfer::Bt1886),
        other => to_linear(v, other),
    }
}

/// (Kr, Kb) luma coefficients; `None` for RGB.
#[must_use]
pub fn kr_kb(m: Matrix) -> Option<(f64, f64)> {
    match m {
        Matrix::Rgb => None,
        Matrix::Bt601 => Some((0.299, 0.114)),
        Matrix::Bt709 => Some((0.2126, 0.0722)),
        Matrix::Bt2020Ncl => Some((0.2627, 0.0593)),
    }
}

/// Code value normalisation: `Y' = (D − y_off) / y_scale`, `C = (D − c_off) / c_scale`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RangeParams {
    pub y_off: f64,
    pub y_scale: f64,
    pub c_off: f64,
    pub c_scale: f64,
}

/// Range parameters per ITU-T H.273 for `bits`-bit samples.
#[must_use]
pub fn range_params(range: Range, bits: u8) -> RangeParams {
    let k = f64::from(1u32 << (bits - 8));
    let max = f64::from((1u32 << bits) - 1);
    match range {
        Range::Limited => RangeParams {
            y_off: 16.0 * k,
            y_scale: 219.0 * k,
            c_off: 128.0 * k,
            c_scale: 224.0 * k,
        },
        Range::Full => RangeParams {
            y_off: 0.0,
            y_scale: max,
            c_off: f64::from(1u32 << (bits - 1)),
            c_scale: max,
        },
    }
}

/// Normalised Y'CbCr → R'G'B'.
#[must_use]
pub fn ycbcr_to_rgb(y: f64, cb: f64, cr: f64, kr: f64, kb: f64) -> [f64; 3] {
    let kg = 1.0 - kr - kb;
    let r = y + 2.0 * (1.0 - kr) * cr;
    let b = y + 2.0 * (1.0 - kb) * cb;
    let g = (y - kr * r - kb * b) / kg;
    [r, g, b]
}

/// R'G'B' → normalised Y'CbCr.
#[must_use]
pub fn rgb_to_ycbcr(c: [f64; 3], kr: f64, kb: f64) -> [f64; 3] {
    let kg = 1.0 - kr - kb;
    let y = kr * c[0] + kg * c[1] + kb * c[2];
    [
        y,
        (c[2] - y) / (2.0 * (1.0 - kb)),
        (c[0] - y) / (2.0 * (1.0 - kr)),
    ]
}

/// 3×3 matrix, row-major.
pub type Mat3 = [[f64; 3]; 3];

/// Identity.
pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

#[must_use]
pub fn mat_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut m = [[0.0; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}

#[must_use]
pub fn mat_vec(m: &Mat3, v: [f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// Inverse of a 3×3 matrix (assumed non-singular: primaries matrices always are).
#[must_use]
pub fn mat_inv(m: &Mat3) -> Mat3 {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    [
        [
            (e * i - f * h) / det,
            (c * h - b * i) / det,
            (b * f - c * e) / det,
        ],
        [
            (f * g - d * i) / det,
            (a * i - c * g) / det,
            (c * d - a * f) / det,
        ],
        [
            (d * h - e * g) / det,
            (b * g - a * h) / det,
            (a * e - b * d) / det,
        ],
    ]
}

const D65: [f64; 2] = [0.3127, 0.3290];

fn xy_to_xyz(xy: [f64; 2]) -> [f64; 3] {
    [xy[0] / xy[1], 1.0, (1.0 - xy[0] - xy[1]) / xy[1]]
}

/// D65 white point XYZ (Y = 1).
#[must_use]
pub fn d65_xyz() -> [f64; 3] {
    xy_to_xyz(D65)
}

/// Linear RGB → CIE XYZ for the given primaries (D65 white).
#[must_use]
pub fn rgb_to_xyz(p: Primaries) -> Mat3 {
    let prim = match p {
        Primaries::Bt709 => [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]],
        Primaries::Bt2020 => [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]],
    };
    let cols = prim.map(xy_to_xyz);
    let m = [
        [cols[0][0], cols[1][0], cols[2][0]],
        [cols[0][1], cols[1][1], cols[2][1]],
        [cols[0][2], cols[1][2], cols[2][2]],
    ];
    let s = mat_vec(&mat_inv(&m), d65_xyz());
    [
        [m[0][0] * s[0], m[0][1] * s[1], m[0][2] * s[2]],
        [m[1][0] * s[0], m[1][1] * s[1], m[1][2] * s[2]],
        [m[2][0] * s[0], m[2][1] * s[1], m[2][2] * s[2]],
    ]
}

/// Linear RGB(from) → linear RGB(to).
#[must_use]
pub fn primaries_conversion(from: Primaries, to: Primaries) -> Mat3 {
    if from == to {
        return IDENTITY;
    }
    mat_mul(&mat_inv(&rgb_to_xyz(to)), &rgb_to_xyz(from))
}

/// ITU-R BT.2390 EETF constants for one tone-map setting (source black = target black = 0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Eetf {
    pub src_min_pq: f64,
    pub src_max_pq: f64,
    pub min_lum: f64,
    pub max_lum: f64,
    pub ks: f64,
    pub sdr_white_nits: f64,
}

impl Eetf {
    #[must_use]
    pub fn new(tm: &ToneMap) -> Self {
        let src_min_pq = pq_inv_eotf(0.0);
        let src_max_pq = pq_inv_eotf(f64::from(tm.source_peak_nits) / 10_000.0);
        let range = src_max_pq - src_min_pq;
        let max_lum = (pq_inv_eotf(f64::from(tm.target_peak_nits) / 10_000.0) - src_min_pq) / range;
        let min_lum = (pq_inv_eotf(0.0) - src_min_pq) / range;
        Self {
            src_min_pq,
            src_max_pq,
            min_lum,
            max_lum,
            ks: 1.5 * max_lum - 0.5,
            sdr_white_nits: f64::from(tm.sdr_white_nits),
        }
    }

    /// The EETF on an absolute PQ signal (BT.2390-x §5.4, Hermite knee).
    #[must_use]
    pub fn eetf(&self, e1: f64) -> f64 {
        let e1n = ((e1 - self.src_min_pq) / (self.src_max_pq - self.src_min_pq)).clamp(0.0, 1.0);
        let mut e2 = e1n;
        if e1n > self.ks && self.ks < 1.0 {
            let t = (e1n - self.ks) / (1.0 - self.ks);
            let (t2, t3) = (t * t, t * t * t);
            e2 = (2.0 * t3 - 3.0 * t2 + 1.0) * self.ks
                + (t3 - 2.0 * t2 + t) * (1.0 - self.ks)
                + (-2.0 * t3 + 3.0 * t2) * self.max_lum;
        }
        let k = 1.0 - e2;
        let e3 = e2 + self.min_lum * (k * k * k * k);
        e3 * (self.src_max_pq - self.src_min_pq) + self.src_min_pq
    }

    /// Hue-preserving tone map of linear RGB in cd/m² → SDR scene-linear (1.0 = SDR white).
    #[must_use]
    pub fn tone_map(&self, rgb_nits: [f64; 3]) -> [f64; 3] {
        let m = rgb_nits[0].max(rgb_nits[1]).max(rgb_nits[2]);
        if m <= 0.0 {
            return [0.0; 3];
        }
        let m_out = pq_eotf(self.eetf(pq_inv_eotf(m / 10_000.0))) * 10_000.0;
        let k = m_out / m / self.sdr_white_nits;
        rgb_nits.map(|c| c * k)
    }
}

/// sRGB-encoded straight-alpha colour → premultiplied linear (shape / text fills).
#[must_use]
pub fn srgb_fill_to_linear_premul(c: [f32; 4]) -> [f32; 4] {
    let a = f64::from(c[3].clamp(0.0, 1.0));
    let l = |v: f32| (to_linear(f64::from(v.clamp(0.0, 1.0)), Transfer::Srgb) * a) as f32;
    [l(c[0]), l(c[1]), l(c[2]), a as f32]
}

/// CIE XYZ → CIE L*a*b* relative to `white`.
#[must_use]
pub fn xyz_to_lab(xyz: [f64; 3], white: [f64; 3]) -> [f64; 3] {
    let f = |t: f64| {
        const D: f64 = 6.0 / 29.0;
        if t > D * D * D {
            t.cbrt()
        } else {
            t / (3.0 * D * D) + 4.0 / 29.0
        }
    };
    let (fx, fy, fz) = (
        f(xyz[0] / white[0]),
        f(xyz[1] / white[1]),
        f(xyz[2] / white[2]),
    );
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfers_round_trip() {
        for tf in [
            Transfer::Srgb,
            Transfer::Bt709,
            Transfer::Bt1886,
            Transfer::Linear,
            Transfer::Pq,
        ] {
            for i in 0..=100 {
                let v = f64::from(i) / 100.0;
                let back = from_linear(to_linear(v, tf), tf);
                // PQ(0 cd/m²) is c1^m2 ≈ 7.3e-7, not 0: signals below it decode to black.
                let tol = if tf == Transfer::Pq { 1e-6 } else { 1e-9 };
                assert!((back - v).abs() < tol, "{tf:?} {v} -> {back}");
            }
        }
    }

    #[test]
    fn bt709_oetf_is_continuous() {
        let below = from_linear(BT709_BETA - 1e-12, Transfer::Bt709);
        let above = from_linear(BT709_BETA, Transfer::Bt709);
        assert!((below - above).abs() < 1e-9);
    }

    #[test]
    fn pq_reference_points() {
        // Published ST 2084 values: 100 cd/m² ≈ 0.5081, 1000 cd/m² ≈ 0.7518, 10000 → 1.0.
        assert!((pq_inv_eotf(0.01) - 0.508_078).abs() < 1e-5);
        assert!((pq_inv_eotf(0.1) - 0.751_827).abs() < 1e-5);
        assert!((pq_inv_eotf(1.0) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn bt2020_to_bt709_matrix_matches_bt2087() {
        let m = primaries_conversion(Primaries::Bt2020, Primaries::Bt709);
        let expected = [
            [1.6605, -0.5876, -0.0728],
            [-0.1246, 1.1329, -0.0083],
            [-0.0182, -0.1006, 1.1187],
        ];
        for i in 0..3 {
            let row_sum: f64 = m[i].iter().sum();
            assert!((row_sum - 1.0).abs() < 1e-9, "white must map to white");
            for j in 0..3 {
                assert!(
                    (m[i][j] - expected[i][j]).abs() < 2e-4,
                    "{i},{j}: {}",
                    m[i][j]
                );
            }
        }
    }

    #[test]
    fn ycbcr_round_trip() {
        let (kr, kb) = kr_kb(Matrix::Bt709).expect("709");
        let c = [0.2, 0.7, 0.4];
        let ycc = rgb_to_ycbcr(c, kr, kb);
        let back = ycbcr_to_rgb(ycc[0], ycc[1], ycc[2], kr, kb);
        for i in 0..3 {
            assert!((back[i] - c[i]).abs() < 1e-12);
        }
    }

    #[test]
    fn eetf_maps_peak_to_target_and_is_monotonic() {
        let e = Eetf::new(&ToneMap::default());
        let peak = e.tone_map([1000.0, 1000.0, 1000.0]);
        assert!(
            (peak[0] - 1.0).abs() < 1e-6,
            "source peak → target peak (1.0): {peak:?}"
        );
        // Below the knee the curve is the identity in absolute luminance.
        let low = e.tone_map([5.0, 5.0, 5.0]);
        assert!((low[0] - 0.05).abs() < 1e-9);
        let mut prev = 0.0;
        for i in 1..=200 {
            let v = e.tone_map([f64::from(i) * 20.0; 3])[0];
            assert!(
                v >= prev - 1e-12,
                "monotonic at {} cd/m²: {v} < {prev}",
                i * 20
            );
            prev = v;
        }
        // Above the source peak everything clips to the target peak.
        assert!((e.tone_map([4000.0; 3])[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn eetf_is_identity_when_source_fits_target() {
        let e = Eetf::new(&ToneMap {
            source_peak_nits: 100.0,
            target_peak_nits: 100.0,
            sdr_white_nits: 100.0,
        });
        let v = e.tone_map([50.0, 25.0, 10.0]);
        assert!((v[0] - 0.5).abs() < 1e-9 && (v[2] - 0.1).abs() < 1e-9);
    }
}
