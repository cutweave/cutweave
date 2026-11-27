// SPDX-License-Identifier: GPL-3.0-or-later
//! Colour management in `f64` (arch §8 "Color"): transfer functions, Y'CbCr matrices and ranges,
//! primaries conversion, the HDR → SDR tone map, and the resolution of probe [`ColorTags`] into a
//! concrete input transform.
//!
//! The compositor works in **scene-linear light, Rec.709 primaries, premultiplied alpha**. Every
//! source is converted on input:
//!
//! ```text
//! codes ─range─► Y'CbCr ─matrix─► R'G'B' ─clamp [0,1]─► transfer⁻¹ ─(HLG: OOTF)─► primaries → 709
//!       ─(PQ/HLG: BT.2390 EETF on max(R,G,B))─► clip negatives ─► premultiply
//! ```
//!
//! This module is the CPU side: the WGSL shaders (`gpu/shaders/*.wgsl`) implement the same
//! formulas independently in `f32` and only take the *derived constants* from here
//! ([`InputTransform`]), so the CPU reference ([`crate::reference`]) is a genuine cross-check.
//!
//! Decisions carried over from Spike S4 (spikes/s4-compositor, "Findings for Phase 1"):
//! * BT.709-family sources are linearised with the inverse **OETF** (scene-linear), PQ with the
//!   ST 2084 EOTF; R'G'B' is clamped to `[0, 1]` before linearisation (super-whites are clipped).
//! * PQ is tone-mapped with the BT.2390 EETF on `max(R, G, B)` in the PQ domain (hue-preserving),
//!   source peak 1000 → target 100 cd/m², `1.0` = 100 cd/m² ([`ToneMap::default`]).
//! * Out-of-gamut colours (BT.2020 → 709) are clipped, not compressed.
//!
//! New here: **HLG** is decoded with the BT.2100 inverse OETF plus the OOTF for a nominal
//! `source_peak_nits` display (system gamma `1.2 + 0.42·log10(Lw/1000)`), then takes the same
//! EETF path as PQ. S4 left HLG open.

// Pixel and matrix math reads best with the conventional single-letter names.
#![allow(clippy::many_single_char_names)]

use cutweave_model::{
    ColorPrimaries, ColorRange, ColorTags, MatrixCoefficients, TransferCharacteristics,
};

/// BT.709 / BT.2020 OETF constants in their continuous ("12-bit precise") form.
pub const BT709_ALPHA: f64 = 1.099_296_826_809_44;
/// See [`BT709_ALPHA`].
pub const BT709_BETA: f64 = 0.018_053_968_510_807;

const PQ_M1: f64 = 2610.0 / 16384.0;
const PQ_M2: f64 = 2523.0 / 4096.0 * 128.0;
const PQ_C1: f64 = 3424.0 / 4096.0;
const PQ_C2: f64 = 2413.0 / 4096.0 * 32.0;
const PQ_C3: f64 = 2392.0 / 4096.0 * 32.0;

const HLG_A: f64 = 0.178_832_77;
const HLG_B: f64 = 0.284_668_92;
const HLG_C: f64 = 0.559_910_73;

const SMPTE240_ALPHA: f64 = 1.1115;
const SMPTE240_BETA: f64 = 0.0228;

/// A transfer characteristic (H.273), as far as the compositor distinguishes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Transfer {
    /// Linear light.
    Linear,
    /// IEC 61966-2-1 (sRGB).
    Srgb,
    /// The BT.709 OETF (also BT.601 / SMPTE 170M and BT.2020 10/12-bit).
    Bt709,
    /// Pure power 2.2.
    Gamma22,
    /// Pure power 2.8.
    Gamma28,
    /// SMPTE 240M.
    Smpte240M,
    /// SMPTE ST 2084 (PQ). Linear values are normalised to 10,000 cd/m².
    Pq,
    /// ARIB STD-B67 (HLG). Linear values are scene light in `[0, 1]` (before the OOTF).
    Hlg,
}

impl Transfer {
    /// The id the WGSL `to_linear` / `from_linear` functions switch on.
    #[must_use]
    pub const fn shader_id(self) -> u32 {
        match self {
            Transfer::Linear => 0,
            Transfer::Srgb => 1,
            Transfer::Bt709 => 2,
            Transfer::Gamma22 => 3,
            Transfer::Gamma28 => 4,
            Transfer::Smpte240M => 5,
            Transfer::Pq => 6,
            Transfer::Hlg => 7,
        }
    }

    /// The transfer of a probe tag, `None` for unspecified or unsupported names.
    #[must_use]
    pub fn from_tag(tag: &TransferCharacteristics) -> Option<Transfer> {
        use TransferCharacteristics as T;
        Some(match tag {
            T::Bt709 | T::Smpte170M | T::Bt2020_10 | T::Bt2020_12 => Transfer::Bt709,
            T::Srgb => Transfer::Srgb,
            T::Linear => Transfer::Linear,
            T::Gamma22 => Transfer::Gamma22,
            T::Gamma28 => Transfer::Gamma28,
            T::Smpte240M => Transfer::Smpte240M,
            T::Pq => Transfer::Pq,
            T::Hlg => Transfer::Hlg,
            T::Other(_) => return None,
        })
    }

    /// Whether sources with this transfer are HDR (tone-mapped to SDR in v1).
    #[must_use]
    pub const fn is_hdr(self) -> bool {
        matches!(self, Transfer::Pq | Transfer::Hlg)
    }
}

/// The transfer of an encoded compositor output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OutputTransfer {
    /// sRGB: preview surfaces, PNG/JPEG frames for agents.
    #[default]
    Srgb,
    /// The BT.709 OETF: video export (players display it through BT.1886).
    Bt709,
}

impl OutputTransfer {
    /// The equivalent [`Transfer`].
    #[must_use]
    pub const fn transfer(self) -> Transfer {
        match self {
            OutputTransfer::Srgb => Transfer::Srgb,
            OutputTransfer::Bt709 => Transfer::Bt709,
        }
    }
}

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

/// BT.2100 HLG inverse OETF: signal → normalised scene light `[0, 1]`.
#[must_use]
pub fn hlg_inv_oetf(e: f64) -> f64 {
    if e <= 0.5 {
        e * e / 3.0
    } else {
        (((e - HLG_C) / HLG_A).exp() + HLG_B) / 12.0
    }
}

/// BT.2100 HLG OETF: normalised scene light → signal.
#[must_use]
pub fn hlg_oetf(l: f64) -> f64 {
    if l <= 1.0 / 12.0 {
        (3.0 * l.max(0.0)).sqrt()
    } else {
        HLG_A * (12.0 * l - HLG_B).ln() + HLG_C
    }
}

/// The HLG system gamma for a display of nominal peak `peak_nits` (BT.2100 note 5f).
#[must_use]
pub fn hlg_system_gamma(peak_nits: f64) -> f64 {
    1.2 + 0.42 * (peak_nits / 1000.0).log10()
}

/// Signal → linear (see [`Transfer`] for the normalisation of PQ and HLG).
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
        Transfer::Gamma22 => safe_pow(v, 2.2),
        Transfer::Gamma28 => safe_pow(v, 2.8),
        Transfer::Smpte240M => {
            if v < 4.0 * SMPTE240_BETA {
                v / 4.0
            } else {
                ((v + (SMPTE240_ALPHA - 1.0)) / SMPTE240_ALPHA).powf(1.0 / 0.45)
            }
        }
        Transfer::Pq => pq_eotf(v),
        Transfer::Hlg => hlg_inv_oetf(v),
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
        Transfer::Gamma22 => safe_pow(l, 1.0 / 2.2),
        Transfer::Gamma28 => safe_pow(l, 1.0 / 2.8),
        Transfer::Smpte240M => {
            if l < SMPTE240_BETA {
                l * 4.0
            } else {
                SMPTE240_ALPHA * l.powf(0.45) - (SMPTE240_ALPHA - 1.0)
            }
        }
        Transfer::Pq => pq_inv_eotf(l),
        Transfer::Hlg => hlg_oetf(l),
    }
}

/// Y'CbCr matrix coefficients the compositor supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum YcbcrMatrix {
    /// BT.709.
    Bt709,
    /// BT.601 (both the 625-line `bt470bg` and 525-line `smpte170m` tags).
    Bt601,
    /// BT.2020 non-constant luminance.
    Bt2020Ncl,
    /// SMPTE 240M.
    Smpte240M,
    /// FCC (BT.470 System M).
    Fcc,
}

impl YcbcrMatrix {
    /// `(Kr, Kb)` luma coefficients.
    #[must_use]
    pub const fn kr_kb(self) -> (f64, f64) {
        match self {
            YcbcrMatrix::Bt709 => (0.2126, 0.0722),
            YcbcrMatrix::Bt601 => (0.299, 0.114),
            YcbcrMatrix::Bt2020Ncl => (0.2627, 0.0593),
            YcbcrMatrix::Smpte240M => (0.212, 0.087),
            YcbcrMatrix::Fcc => (0.30, 0.11),
        }
    }

    /// The matrix of a probe tag; `None` for RGB, unspecified or unsupported (YCgCo, BT.2020
    /// constant luminance, ICtCp) names.
    #[must_use]
    pub fn from_tag(tag: &MatrixCoefficients) -> Option<YcbcrMatrix> {
        use MatrixCoefficients as M;
        Some(match tag {
            M::Bt709 => YcbcrMatrix::Bt709,
            M::Bt470Bg | M::Smpte170M => YcbcrMatrix::Bt601,
            M::Bt2020Ncl => YcbcrMatrix::Bt2020Ncl,
            M::Smpte240M => YcbcrMatrix::Smpte240M,
            M::Fcc => YcbcrMatrix::Fcc,
            M::Rgb | M::YCgCo | M::Bt2020Cl | M::ICtCp | M::Other(_) => return None,
        })
    }
}

/// Sample range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Range {
    /// Limited ("TV"): 16–235 luma, 16–240 chroma at 8 bits.
    Limited,
    /// Full ("PC").
    Full,
}

/// Colour primaries the compositor converts from (all with a D65 white point).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Primaries {
    /// BT.709 / sRGB (the working space).
    Bt709,
    /// BT.601 625-line (EBU, `bt470bg`).
    Bt601_625,
    /// BT.601 525-line (SMPTE C, `smpte170m` / `smpte240m`).
    Bt601_525,
    /// BT.2020 / BT.2100.
    Bt2020,
    /// Display P3 (`smpte432`).
    DisplayP3,
}

impl Primaries {
    /// The primaries of a probe tag; `None` for unspecified or unsupported names.
    #[must_use]
    pub fn from_tag(tag: &ColorPrimaries) -> Option<Primaries> {
        use ColorPrimaries as P;
        Some(match tag {
            P::Bt709 => Primaries::Bt709,
            P::Bt470Bg => Primaries::Bt601_625,
            P::Smpte170M | P::Smpte240M => Primaries::Bt601_525,
            P::Bt2020 => Primaries::Bt2020,
            P::Smpte432 => Primaries::DisplayP3,
            P::Bt470M | P::Film | P::Smpte428 | P::Smpte431 | P::Ebu3213 | P::Other(_) => {
                return None;
            }
        })
    }

    /// CIE xy chromaticities of R, G and B.
    #[must_use]
    pub const fn xy(self) -> [[f64; 2]; 3] {
        match self {
            Primaries::Bt709 => [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]],
            Primaries::Bt601_625 => [[0.64, 0.33], [0.29, 0.60], [0.15, 0.06]],
            Primaries::Bt601_525 => [[0.630, 0.340], [0.310, 0.595], [0.155, 0.070]],
            Primaries::Bt2020 => [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]],
            Primaries::DisplayP3 => [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]],
        }
    }
}

/// A fully resolved description of how a source's samples encode colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InputColor {
    /// Y'CbCr matrix; `None` for R'G'B' samples.
    pub matrix: Option<YcbcrMatrix>,
    /// Sample range.
    pub range: Range,
    /// Transfer characteristic.
    pub transfer: Transfer,
    /// Colour primaries.
    pub primaries: Primaries,
}

impl InputColor {
    /// Resolves probe tags into a concrete input colour, filling unspecified (or unsupported)
    /// fields with the usual player conventions:
    ///
    /// * Y'CbCr: matrix BT.601 up to 576 lines, else BT.709; limited range; BT.709 transfer;
    ///   primaries BT.601-525 up to 486 lines, BT.601-625 up to 576, else BT.709.
    /// * R'G'B' (`yuv == false`): the matrix is ignored; full range; sRGB transfer; BT.709
    ///   primaries.
    #[must_use]
    pub fn resolve(tags: &ColorTags, yuv: bool, height: u32) -> InputColor {
        let sd = height <= 576;
        let matrix = if yuv {
            Some(
                tags.matrix
                    .as_ref()
                    .and_then(YcbcrMatrix::from_tag)
                    .unwrap_or(if sd {
                        YcbcrMatrix::Bt601
                    } else {
                        YcbcrMatrix::Bt709
                    }),
            )
        } else {
            None
        };
        let range = match tags.range {
            Some(ColorRange::Limited) => Range::Limited,
            Some(ColorRange::Full) => Range::Full,
            None => {
                if yuv {
                    Range::Limited
                } else {
                    Range::Full
                }
            }
        };
        let transfer = tags
            .transfer
            .as_ref()
            .and_then(Transfer::from_tag)
            .unwrap_or(if yuv { Transfer::Bt709 } else { Transfer::Srgb });
        let primaries = tags
            .primaries
            .as_ref()
            .and_then(Primaries::from_tag)
            .unwrap_or(if !yuv || height > 576 {
                Primaries::Bt709
            } else if height <= 486 {
                Primaries::Bt601_525
            } else {
                Primaries::Bt601_625
            });
        InputColor {
            matrix,
            range,
            transfer,
            primaries,
        }
    }
}

/// Field-wise merge: the decoded frame's own tags win, the probe's fill the gaps.
#[must_use]
pub fn merge_tags(frame: &ColorTags, probe: &ColorTags) -> ColorTags {
    ColorTags {
        primaries: frame.primaries.clone().or_else(|| probe.primaries.clone()),
        transfer: frame.transfer.clone().or_else(|| probe.transfer.clone()),
        matrix: frame.matrix.clone().or_else(|| probe.matrix.clone()),
        range: frame.range.or(probe.range),
    }
}

/// Code value normalisation: `Y' = (D − y_off) / y_scale`, `C = (D − c_off) / c_scale`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RangeParams {
    /// Luma (or R'G'B') black code.
    pub y_off: f64,
    /// Luma (or R'G'B') black-to-white code span.
    pub y_scale: f64,
    /// Chroma zero code.
    pub c_off: f64,
    /// Chroma code span of `[-0.5, 0.5]`.
    pub c_scale: f64,
}

/// Range parameters per ITU-T H.273 for `bits`-bit samples (8..=16).
#[must_use]
pub fn range_params(range: Range, bits: u8) -> RangeParams {
    let bits = bits.clamp(8, 16);
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

/// Normalised Y'CbCr (Y' in `[0, 1]`, Cb/Cr in `[-0.5, 0.5]`) → R'G'B'.
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

/// The identity matrix.
pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// `a · b`.
#[must_use]
pub fn mat_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut m = [[0.0; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    m
}

/// `m · v`.
#[must_use]
pub fn mat_vec(m: &Mat3, v: [f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// Inverse of a non-singular 3×3 matrix (primaries matrices always are).
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
    let cols = p.xy().map(xy_to_xyz);
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

/// Linear RGB(`from`) → linear RGB(`to`).
#[must_use]
pub fn primaries_conversion(from: Primaries, to: Primaries) -> Mat3 {
    if from == to {
        return IDENTITY;
    }
    mat_mul(&mat_inv(&rgb_to_xyz(to)), &rgb_to_xyz(from))
}

/// HDR → SDR tone-map setting (arch §8: HDR sources are tone-mapped to SDR in v1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToneMap {
    /// Source peak in cd/m² (PQ mastering peak; HLG nominal display peak).
    pub source_peak_nits: f64,
    /// Target (SDR) peak in cd/m².
    pub target_peak_nits: f64,
    /// Luminance that maps to linear `1.0` (SDR reference white) in cd/m².
    pub sdr_white_nits: f64,
}

/// The Spike S4 setting: 1000 → 100 cd/m², 1.0 = 100 cd/m².
impl Default for ToneMap {
    fn default() -> ToneMap {
        ToneMap {
            source_peak_nits: 1000.0,
            target_peak_nits: 100.0,
            sdr_white_nits: 100.0,
        }
    }
}

/// ITU-R BT.2390 EETF constants for one [`ToneMap`] (source and target black = 0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Eetf {
    /// PQ signal of the source black.
    pub src_min_pq: f64,
    /// PQ signal of the source peak.
    pub src_max_pq: f64,
    /// Normalised target black.
    pub min_lum: f64,
    /// Normalised target peak.
    pub max_lum: f64,
    /// Knee start.
    pub ks: f64,
    /// Luminance of linear `1.0` in the output.
    pub sdr_white_nits: f64,
}

impl Eetf {
    /// Derives the EETF constants for `tm`.
    #[must_use]
    pub fn new(tm: &ToneMap) -> Eetf {
        let src_min_pq = pq_inv_eotf(0.0);
        let src_max_pq = pq_inv_eotf(tm.source_peak_nits.max(1e-3) / 10_000.0);
        let range = src_max_pq - src_min_pq;
        let max_lum = (pq_inv_eotf(tm.target_peak_nits.max(1e-3) / 10_000.0) - src_min_pq) / range;
        let min_lum = (pq_inv_eotf(0.0) - src_min_pq) / range;
        Eetf {
            src_min_pq,
            src_max_pq,
            min_lum,
            max_lum,
            ks: 1.5 * max_lum - 0.5,
            sdr_white_nits: tm.sdr_white_nits.max(1e-3),
        }
    }

    /// The EETF on an absolute PQ signal (BT.2390 §5.4, Hermite knee).
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

/// The numeric input transform of one source: every constant the decode shader and the CPU
/// reference need, derived once per (colour, bit depth, tone map).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InputTransform {
    /// `(Kr, Kb)` for Y'CbCr sources, `None` for R'G'B'.
    pub kr_kb: Option<(f64, f64)>,
    /// Code normalisation.
    pub range: RangeParams,
    /// Transfer to undo.
    pub transfer: Transfer,
    /// Linear multiplier after the transfer: 10,000 for PQ (→ cd/m²), the nominal peak for HLG,
    /// 1 for SDR.
    pub linear_scale: f64,
    /// HLG OOTF: luma coefficients of the source primaries and `γ − 1`; `None` otherwise.
    pub hlg_ootf: Option<([f64; 3], f64)>,
    /// Source primaries → BT.709.
    pub primaries: Mat3,
    /// The tone map for HDR sources.
    pub tone_map: Option<Eetf>,
}

impl InputTransform {
    /// The transform for `color` at `bits` bits per sample.
    #[must_use]
    pub fn new(color: &InputColor, bits: u8, tone_map: &ToneMap) -> InputTransform {
        let peak = tone_map.source_peak_nits.max(1e-3);
        let (linear_scale, hlg_ootf) = match color.transfer {
            Transfer::Pq => (10_000.0, None),
            Transfer::Hlg => (
                peak,
                Some((rgb_to_xyz(color.primaries)[1], hlg_system_gamma(peak) - 1.0)),
            ),
            _ => (1.0, None),
        };
        InputTransform {
            kr_kb: color.matrix.map(YcbcrMatrix::kr_kb),
            range: range_params(color.range, bits),
            transfer: color.transfer,
            linear_scale,
            hlg_ootf,
            primaries: primaries_conversion(color.primaries, Primaries::Bt709),
            tone_map: color.transfer.is_hdr().then(|| Eetf::new(tone_map)),
        }
    }

    /// Code values → R'G'B' signal (unclamped). For R'G'B' sources `c0..c2` are R, G, B codes.
    #[must_use]
    pub fn codes_to_signal(&self, c0: f64, c1: f64, c2: f64) -> [f64; 3] {
        let r = &self.range;
        match self.kr_kb {
            Some((kr, kb)) => ycbcr_to_rgb(
                (c0 - r.y_off) / r.y_scale,
                (c1 - r.c_off) / r.c_scale,
                (c2 - r.c_off) / r.c_scale,
                kr,
                kb,
            ),
            None => [c0, c1, c2].map(|c| (c - r.y_off) / r.y_scale),
        }
    }

    /// R'G'B' signal → linear Rec.709 (clamp, transfer⁻¹, OOTF, primaries, tone map, gamut clip).
    #[must_use]
    pub fn signal_to_linear(&self, signal: [f64; 3]) -> [f64; 3] {
        let mut lin = signal.map(|s| to_linear(s.clamp(0.0, 1.0), self.transfer));
        let scale = match self.hlg_ootf {
            Some((luma, gamma_minus_1)) => {
                let ys = luma[0] * lin[0] + luma[1] * lin[1] + luma[2] * lin[2];
                self.linear_scale * safe_pow(ys, gamma_minus_1)
            }
            None => self.linear_scale,
        };
        lin = lin.map(|c| c * scale);
        let mut out = mat_vec(&self.primaries, lin);
        if let Some(eetf) = &self.tone_map {
            out = eetf.tone_map(out);
        }
        out.map(|c| c.max(0.0))
    }
}

/// An sRGB-encoded 8-bit straight-alpha colour (the model's `Color`) → premultiplied linear.
#[must_use]
pub fn srgb8_to_linear_premul(c: cutweave_model::Color) -> [f64; 4] {
    let a = f64::from(c.a) / 255.0;
    let l = |v: u8| to_linear(f64::from(v) / 255.0, Transfer::Srgb) * a;
    [l(c.r), l(c.g), l(c.b), a]
}

/// Quantises an encoded value in `[0, 1]` to 8 bits, rounding half up (the shader's rule).
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn quantize8(v: f64) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor().min(255.0) as u8
}

/// The output transform: premultiplied linear RGBA → straight-alpha encoded RGBA8 (clamp,
/// un-premultiply, encode, round half up).
#[must_use]
pub fn encode_output(c: [f64; 4], tf: OutputTransfer) -> [u8; 4] {
    let a = c[3].clamp(0.0, 1.0);
    let rgb = if a >= 1.0 {
        [c[0], c[1], c[2]]
    } else if a > 0.0 {
        [c[0] / a, c[1] / a, c[2] / a]
    } else {
        [0.0; 3]
    };
    let e = rgb.map(|v| quantize8(from_linear(v.clamp(0.0, 1.0), tf.transfer())));
    [e[0], e[1], e[2], quantize8(a)]
}

/// [`encode_output`] with an ordered dither on the colour: each encoded channel quantises to
/// `⌊255·v + threshold⌋` (`threshold` in `(0, 1)`, [`crate::backdrop::dither_threshold`]); alpha
/// still rounds half up.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn encode_output_dithered(c: [f64; 4], tf: OutputTransfer, threshold: f64) -> [u8; 4] {
    let a = c[3].clamp(0.0, 1.0);
    let rgb = if a >= 1.0 {
        [c[0], c[1], c[2]]
    } else if a > 0.0 {
        [c[0] / a, c[1] / a, c[2] / a]
    } else {
        [0.0; 3]
    };
    let e = rgb.map(|v| {
        let encoded = from_linear(v.clamp(0.0, 1.0), tf.transfer()).clamp(0.0, 1.0);
        (encoded * 255.0 + threshold).floor().min(255.0) as u8
    });
    [e[0], e[1], e[2], quantize8(a)]
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

    const ALL_TF: [Transfer; 8] = [
        Transfer::Linear,
        Transfer::Srgb,
        Transfer::Bt709,
        Transfer::Gamma22,
        Transfer::Gamma28,
        Transfer::Smpte240M,
        Transfer::Pq,
        Transfer::Hlg,
    ];

    #[test]
    fn transfers_round_trip() {
        for tf in ALL_TF {
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
    fn piecewise_transfers_are_continuous() {
        for (tf, knee) in [
            (Transfer::Bt709, BT709_BETA),
            (Transfer::Smpte240M, SMPTE240_BETA),
            (Transfer::Srgb, 0.003_130_8),
            (Transfer::Hlg, 1.0 / 12.0),
        ] {
            let below = from_linear(knee - 1e-12, tf);
            let above = from_linear(knee + 1e-12, tf);
            // SMPTE 240M's published (rounded) constants leave a 6e-5 step at the knee.
            let tol = if tf == Transfer::Smpte240M {
                1e-4
            } else {
                1e-6
            };
            assert!((below - above).abs() < tol, "{tf:?}: {below} vs {above}");
        }
    }

    #[test]
    fn shader_ids_are_distinct() {
        let mut ids: Vec<u32> = ALL_TF.iter().map(|t| t.shader_id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), ALL_TF.len());
    }

    #[test]
    fn pq_and_hlg_reference_points() {
        // ST 2084: 100 cd/m² ≈ 0.5081, 1000 cd/m² ≈ 0.7518, 10000 → 1.0.
        assert!((pq_inv_eotf(0.01) - 0.508_078).abs() < 1e-5);
        assert!((pq_inv_eotf(0.1) - 0.751_827).abs() < 1e-5);
        assert!((pq_inv_eotf(1.0) - 1.0).abs() < 1e-12);
        // BT.2100 HLG: E = 1/12 ↔ E' = 0.5, E = 1 ↔ E' = 1.
        assert!((hlg_oetf(1.0 / 12.0) - 0.5).abs() < 1e-9);
        assert!((hlg_oetf(1.0) - 1.0).abs() < 1e-6);
        assert!((hlg_system_gamma(1000.0) - 1.2).abs() < 1e-12);
    }

    #[test]
    fn ycbcr_matrices_round_trip_and_hit_reference_values() {
        for m in [
            YcbcrMatrix::Bt709,
            YcbcrMatrix::Bt601,
            YcbcrMatrix::Bt2020Ncl,
            YcbcrMatrix::Smpte240M,
            YcbcrMatrix::Fcc,
        ] {
            let (kr, kb) = m.kr_kb();
            let c = [0.2, 0.7, 0.4];
            let ycc = rgb_to_ycbcr(c, kr, kb);
            let back = ycbcr_to_rgb(ycc[0], ycc[1], ycc[2], kr, kb);
            for i in 0..3 {
                assert!((back[i] - c[i]).abs() < 1e-12, "{m:?}");
            }
            // White has no chroma; primaries hit ±0.5 chroma.
            let w = rgb_to_ycbcr([1.0; 3], kr, kb);
            assert!((w[0] - 1.0).abs() < 1e-12 && w[1].abs() < 1e-12 && w[2].abs() < 1e-12);
            assert!((rgb_to_ycbcr([0.0, 0.0, 1.0], kr, kb)[1] - 0.5).abs() < 1e-12);
            assert!((rgb_to_ycbcr([1.0, 0.0, 0.0], kr, kb)[2] - 0.5).abs() < 1e-12);
        }
        // BT.709 R'G'B' → Y'CbCr matrix rows as published (BT.709-6 table 3.2).
        let (kr, kb) = YcbcrMatrix::Bt709.kr_kb();
        let red = rgb_to_ycbcr([1.0, 0.0, 0.0], kr, kb);
        assert!((red[0] - 0.2126).abs() < 1e-12);
        assert!((red[1] + 0.114_572).abs() < 1e-6);
        // BT.601 green → (0.587, -0.331264, -0.418688).
        let (kr, kb) = YcbcrMatrix::Bt601.kr_kb();
        let g = rgb_to_ycbcr([0.0, 1.0, 0.0], kr, kb);
        assert!((g[0] - 0.587).abs() < 1e-12);
        assert!((g[1] + 0.331_264).abs() < 1e-6);
        assert!((g[2] + 0.418_688).abs() < 1e-6);
    }

    #[test]
    fn range_params_match_h273() {
        let l8 = range_params(Range::Limited, 8);
        assert_eq!(
            (l8.y_off, l8.y_scale, l8.c_off, l8.c_scale),
            (16.0, 219.0, 128.0, 224.0)
        );
        let l10 = range_params(Range::Limited, 10);
        assert_eq!(
            (l10.y_off, l10.y_scale, l10.c_off, l10.c_scale),
            (64.0, 876.0, 512.0, 896.0)
        );
        let f8 = range_params(Range::Full, 8);
        assert_eq!(
            (f8.y_off, f8.y_scale, f8.c_off, f8.c_scale),
            (0.0, 255.0, 128.0, 255.0)
        );
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
        for p in [
            Primaries::Bt601_625,
            Primaries::Bt601_525,
            Primaries::DisplayP3,
        ] {
            let m = primaries_conversion(p, Primaries::Bt709);
            for row in m {
                assert!((row.iter().sum::<f64>() - 1.0).abs() < 1e-9, "{p:?}");
            }
        }
        // The Y row of BT.709 RGB → XYZ is the BT.709 luma.
        let y = rgb_to_xyz(Primaries::Bt709)[1];
        assert!((y[0] - 0.2126).abs() < 1e-4 && (y[2] - 0.0722).abs() < 1e-4);
    }

    #[test]
    fn tags_resolve_with_player_defaults() {
        let none = ColorTags::default();
        let hd = InputColor::resolve(&none, true, 1080);
        assert_eq!(hd.matrix, Some(YcbcrMatrix::Bt709));
        assert_eq!(hd.range, Range::Limited);
        assert_eq!(hd.transfer, Transfer::Bt709);
        assert_eq!(hd.primaries, Primaries::Bt709);
        let ntsc = InputColor::resolve(&none, true, 480);
        assert_eq!(ntsc.matrix, Some(YcbcrMatrix::Bt601));
        assert_eq!(ntsc.primaries, Primaries::Bt601_525);
        assert_eq!(
            InputColor::resolve(&none, true, 576).primaries,
            Primaries::Bt601_625
        );
        let rgb = InputColor::resolve(&none, false, 480);
        assert_eq!(
            (rgb.matrix, rgb.range, rgb.transfer, rgb.primaries),
            (None, Range::Full, Transfer::Srgb, Primaries::Bt709)
        );
        let tagged = ColorTags {
            primaries: Some(ColorPrimaries::Bt2020),
            transfer: Some(TransferCharacteristics::Pq),
            matrix: Some(MatrixCoefficients::Bt2020Ncl),
            range: Some(ColorRange::Full),
        };
        let hdr = InputColor::resolve(&tagged, true, 2160);
        assert_eq!(
            (hdr.matrix, hdr.range, hdr.transfer, hdr.primaries),
            (
                Some(YcbcrMatrix::Bt2020Ncl),
                Range::Full,
                Transfer::Pq,
                Primaries::Bt2020
            )
        );
        // Unknown names fall back like unspecified ones.
        let odd = ColorTags {
            matrix: Some(MatrixCoefficients::Other("x".into())),
            ..ColorTags::default()
        };
        assert_eq!(
            InputColor::resolve(&odd, true, 720).matrix,
            Some(YcbcrMatrix::Bt709)
        );
    }

    #[test]
    fn frame_tags_win_over_probe_tags() {
        let frame = ColorTags {
            range: Some(ColorRange::Full),
            ..ColorTags::default()
        };
        let probe = ColorTags {
            range: Some(ColorRange::Limited),
            matrix: Some(MatrixCoefficients::Bt470Bg),
            ..ColorTags::default()
        };
        let m = merge_tags(&frame, &probe);
        assert_eq!(m.range, Some(ColorRange::Full));
        assert_eq!(m.matrix, Some(MatrixCoefficients::Bt470Bg));
    }

    #[test]
    fn eetf_maps_peak_to_target_and_is_monotonic() {
        let e = Eetf::new(&ToneMap::default());
        let peak = e.tone_map([1000.0; 3]);
        assert!((peak[0] - 1.0).abs() < 1e-6, "{peak:?}");
        // Below the knee the curve is the identity in absolute luminance.
        assert!((e.tone_map([5.0; 3])[0] - 0.05).abs() < 1e-9);
        let mut prev = 0.0;
        for i in 1..=200 {
            let v = e.tone_map([f64::from(i) * 20.0; 3])[0];
            assert!(v >= prev - 1e-12, "monotonic at {} cd/m²", i * 20);
            prev = v;
        }
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

    #[test]
    fn sdr_input_transform_inverts_the_encoding() {
        let color = InputColor::resolve(&ColorTags::default(), true, 1080);
        let it = InputTransform::new(&color, 8, &ToneMap::default());
        assert!(it.tone_map.is_none() && it.hlg_ootf.is_none());
        // Limited-range black, white and a 75% grey.
        let black = it.signal_to_linear(it.codes_to_signal(16.0, 128.0, 128.0));
        let white = it.signal_to_linear(it.codes_to_signal(235.0, 128.0, 128.0));
        assert_eq!(black, [0.0; 3]);
        for c in white {
            assert!((c - 1.0).abs() < 1e-12);
        }
        let y75 = 16.0 + 219.0 * 0.75;
        let grey = it.signal_to_linear(it.codes_to_signal(y75, 128.0, 128.0));
        assert!((grey[0] - to_linear(0.75, Transfer::Bt709)).abs() < 1e-12);
        // Super-white is clipped at decode (S4 finding 12).
        assert_eq!(
            it.signal_to_linear(it.codes_to_signal(255.0, 128.0, 128.0)),
            white
        );
    }

    #[test]
    fn hdr_input_transforms_tone_map_to_sdr() {
        let tm = ToneMap::default();
        let pq = InputColor {
            matrix: Some(YcbcrMatrix::Bt2020Ncl),
            range: Range::Full,
            transfer: Transfer::Pq,
            primaries: Primaries::Bt2020,
        };
        let it = InputTransform::new(&pq, 10, &tm);
        // 5 cd/m² neutral grey stays at 0.05 (below the knee), 1000 cd/m² reaches SDR white.
        let low = it.signal_to_linear([pq_inv_eotf(5.0 / 10_000.0); 3]);
        assert!((low[1] - 0.05).abs() < 1e-6, "{low:?}");
        let peak = it.signal_to_linear([pq_inv_eotf(0.1); 3]);
        assert!((peak[1] - 1.0).abs() < 1e-6, "{peak:?}");
        // HLG: nominal white (75% signal) lands near 203 cd/m² before the EETF and below 1.0
        // after it; peak signal maps to SDR white.
        let hlg = InputColor {
            transfer: Transfer::Hlg,
            ..pq
        };
        let it = InputTransform::new(&hlg, 10, &tm);
        let white = it.signal_to_linear([1.0; 3]);
        assert!((white[1] - 1.0).abs() < 1e-6, "{white:?}");
        let ref_white = it.signal_to_linear([0.75; 3]);
        assert!(ref_white[1] > 0.7 && ref_white[1] < 1.0, "{ref_white:?}");
    }

    #[test]
    fn output_encoding_rounds_half_up_and_unpremultiplies() {
        assert_eq!(quantize8(0.5 / 255.0), 1);
        assert_eq!(quantize8(0.499 / 255.0), 0);
        assert_eq!(quantize8(2.0), 255);
        assert_eq!(quantize8(-1.0), 0);
        let opaque = encode_output([1.0, 0.0, 0.214_041_140_5, 1.0], OutputTransfer::Srgb);
        assert_eq!(opaque, [255, 0, 128, 255]);
        // Half-transparent premultiplied white → straight white at alpha 128.
        assert_eq!(
            encode_output([0.5, 0.5, 0.5, 0.5], OutputTransfer::Srgb),
            [255, 255, 255, 128]
        );
        assert_eq!(encode_output([0.0; 4], OutputTransfer::Bt709), [0; 4]);
    }

    #[test]
    fn background_color_converts_to_premultiplied_linear() {
        let c = srgb8_to_linear_premul(cutweave_model::Color {
            r: 255,
            g: 128,
            b: 0,
            a: 255,
        });
        assert!((c[0] - 1.0).abs() < 1e-12 && (c[1] - 0.215_860_5).abs() < 1e-6);
        let half = srgb8_to_linear_premul(cutweave_model::Color {
            r: 255,
            g: 255,
            b: 255,
            a: 51,
        });
        assert!((half[0] - 0.2).abs() < 1e-12 && (half[3] - 0.2).abs() < 1e-12);
    }
}
