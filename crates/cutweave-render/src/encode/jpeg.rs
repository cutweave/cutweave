// SPDX-License-Identifier: GPL-3.0-or-later
//! A baseline JPEG encoder (ITU-T T.81, JFIF 1.02) for agent frames and contact sheets.
//!
//! Written for Cutweave from the standard: sequential DCT, 8-bit samples, Huffman coding with the
//! example tables of T.81 Annex K.3, the example quantisation tables of Annex K.1 scaled by a
//! 1–100 quality (the usual `5000 / q` below 50, `200 − 2q` above), 4:2:0 chroma from
//! full-range BT.601 Y'CbCr (JFIF). The forward DCT is the separable textbook definition in `f32`
//! with a constant cosine table, so the bytes depend only on the pixels and the quality: no
//! platform maths library is involved (determinism, arch §8).
//!
//! Alpha is flattened over black (`rgb · a`); use PNG to keep it.
//!
//! Why not a crate: `jpeg-encoder` declares `(MIT OR Apache-2.0) AND IJG` (its DCT is ported from
//! libjpeg), which is not on the cargo-deny allow-list (arch §21), and `image`'s encoder carries
//! the same IJG-derived DCT under an MIT/Apache declaration. This encoder contains no IJG code.

// Pixel and block maths reads best with the conventional single-letter names.
// Cb / Cr are the standard component names.
#![allow(clippy::many_single_char_names, clippy::similar_names)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss
)]

use super::EncodeError;
use crate::image::Image;

/// Example luminance quantisation table (T.81 Annex K.1), natural order.
const LUMA_Q: [u8; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, //
    12, 12, 14, 19, 26, 58, 60, 55, //
    14, 13, 16, 24, 40, 57, 69, 56, //
    14, 17, 22, 29, 51, 87, 80, 62, //
    18, 22, 37, 56, 68, 109, 103, 77, //
    24, 35, 55, 64, 81, 104, 113, 92, //
    49, 64, 78, 87, 103, 121, 120, 101, //
    72, 92, 95, 98, 112, 100, 103, 99, //
];

/// Example chrominance quantisation table (T.81 Annex K.1), natural order.
const CHROMA_Q: [u8; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, //
    18, 21, 26, 66, 99, 99, 99, 99, //
    24, 26, 56, 99, 99, 99, 99, 99, //
    47, 66, 99, 99, 99, 99, 99, 99, //
    99, 99, 99, 99, 99, 99, 99, 99, //
    99, 99, 99, 99, 99, 99, 99, 99, //
    99, 99, 99, 99, 99, 99, 99, 99, //
    99, 99, 99, 99, 99, 99, 99, 99, //
];

/// A Huffman table specification: code counts per length 1..=16, then the symbols.
struct HuffSpec {
    bits: [u8; 16],
    vals: &'static [u8],
}

const DC_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

/// T.81 Annex K.3, table K.3: luminance DC.
const DC_LUMA: HuffSpec = HuffSpec {
    bits: [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0],
    vals: &DC_VALS,
};

/// Table K.4: chrominance DC.
const DC_CHROMA: HuffSpec = HuffSpec {
    bits: [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0],
    vals: &DC_VALS,
};

/// Table K.5: luminance AC.
const AC_LUMA: HuffSpec = HuffSpec {
    bits: [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d],
    vals: &[
        0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61,
        0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52,
        0xd1, 0xf0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25,
        0x26, 0x27, 0x28, 0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45,
        0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64,
        0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83,
        0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99,
        0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6,
        0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3,
        0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8,
        0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
    ],
};

/// Table K.6: chrominance AC.
const AC_CHROMA: HuffSpec = HuffSpec {
    bits: [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77],
    vals: &[
        0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61,
        0x71, 0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33,
        0x52, 0xf0, 0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18,
        0x19, 0x1a, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44,
        0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63,
        0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a,
        0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97,
        0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4,
        0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca,
        0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7,
        0xe8, 0xe9, 0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
    ],
};

/// Natural-order index of the `k`-th coefficient in zig-zag order (T.81 figure A.6).
const ZIGZAG: [usize; 64] = zigzag();

const fn zigzag() -> [usize; 64] {
    let mut out = [0usize; 64];
    let mut k = 0;
    let mut diagonal: usize = 0;
    while diagonal < 15 {
        // Row range of this anti-diagonal (row + col = diagonal).
        let lo = diagonal.saturating_sub(7);
        let hi = if diagonal < 7 { diagonal } else { 7 };
        let mut i = 0;
        while i <= hi - lo {
            // Even diagonals run up-right (row decreasing), odd ones down-left.
            let row = if diagonal.is_multiple_of(2) {
                hi - i
            } else {
                lo + i
            };
            out[k] = row * 8 + (diagonal - row);
            k += 1;
            i += 1;
        }
        diagonal += 1;
    }
    out
}

/// `cos(k·π/16)` for `k` in `0..=8`.
const COS16: [f32; 9] = [
    1.0,
    0.980_785_3,
    0.923_879_5,
    0.831_469_6,
    0.707_106_77,
    0.555_570_24,
    0.382_683_43,
    0.195_090_32,
    0.0,
];

/// `cos(k·π/16)` for any `k ≥ 0`, from [`COS16`] by symmetry.
fn cos16(k: usize) -> f32 {
    let k = k % 32;
    let k = if k > 16 { 32 - k } else { k };
    if k <= 8 { COS16[k] } else { -COS16[16 - k] }
}

/// The 1-D DCT matrix `D[u][x] = C(u)/2 · cos((2x+1)uπ/16)`, `C(0) = 1/√2`, so that the 2-D DCT
/// is `F = D · f · Dᵀ` (T.81 A.3.3).
fn dct_matrix() -> [[f32; 8]; 8] {
    let mut d = [[0.0f32; 8]; 8];
    for (u, row) in d.iter_mut().enumerate() {
        let c = if u == 0 { COS16[4] } else { 1.0 };
        for (x, v) in row.iter_mut().enumerate() {
            *v = 0.5 * c * cos16((2 * x + 1) * u);
        }
    }
    d
}

/// The quality-scaled table (`1..=255`, baseline 8-bit precision), natural order.
fn scaled_table(base: &[u8; 64], quality: u8) -> [u16; 64] {
    let q = u32::from(quality.clamp(1, 100));
    let scale = if q < 50 { 5000 / q } else { 200 - 2 * q };
    base.map(|b| ((u32::from(b) * scale + 50) / 100).clamp(1, 255) as u16)
}

/// Codes and code lengths per symbol (T.81 Annex C).
struct Huffman {
    code: [u16; 256],
    size: [u8; 256],
}

impl Huffman {
    fn new(spec: &HuffSpec) -> Huffman {
        let mut h = Huffman {
            code: [0; 256],
            size: [0; 256],
        };
        let mut code: u16 = 0;
        let mut k = 0;
        for (len, &count) in (1u8..).zip(&spec.bits) {
            for _ in 0..count {
                let sym = usize::from(spec.vals[k]);
                h.code[sym] = code;
                h.size[sym] = len;
                code += 1;
                k += 1;
            }
            code <<= 1;
        }
        h
    }
}

/// Entropy-coded segment writer with `0xFF` byte stuffing.
struct BitWriter<'a> {
    out: &'a mut Vec<u8>,
    acc: u32,
    bits: u32,
}

impl BitWriter<'_> {
    /// Appends the low `len` (≤ 16) bits of `value`.
    fn put(&mut self, value: u32, len: u32) {
        if len == 0 {
            return;
        }
        self.acc = (self.acc << len) | (value & ((1 << len) - 1));
        self.bits += len;
        while self.bits >= 8 {
            self.bits -= 8;
            let byte = (self.acc >> self.bits) as u8;
            self.out.push(byte);
            if byte == 0xFF {
                self.out.push(0);
            }
        }
        self.acc &= (1 << self.bits) - 1;
    }

    fn code(&mut self, table: &Huffman, symbol: usize) {
        self.put(u32::from(table.code[symbol]), u32::from(table.size[symbol]));
    }

    /// Pads the last byte with 1-bits (T.81 F.1.2.3).
    fn flush(&mut self) {
        if self.bits > 0 {
            let pad = 8 - self.bits;
            self.put((1 << pad) - 1, pad);
        }
    }
}

/// Magnitude category and the additional bits of a coefficient (T.81 F.1.2.1).
fn magnitude(v: i32) -> (usize, u32) {
    if v == 0 {
        return (0, 0);
    }
    let size = 32 - v.unsigned_abs().leading_zeros();
    let bits = if v < 0 {
        (v - 1) as u32 & ((1 << size) - 1)
    } else {
        v as u32
    };
    (size as usize, bits)
}

struct Component<'a> {
    quant: [f32; 64],
    dc: &'a Huffman,
    ac: &'a Huffman,
    pred: i32,
}

/// DCT, quantisation and Huffman coding of one level-shifted 8×8 block (row-major).
fn encode_block(
    w: &mut BitWriter<'_>,
    block: &[f32; 64],
    d: &[[f32; 8]; 8],
    comp: &mut Component<'_>,
) {
    // Rows: tmp[y][u] = Σx D[u][x]·f[y][x]; columns: F[v][u] = Σy D[v][y]·tmp[y][u].
    let mut tmp = [0.0f32; 64];
    for y in 0..8 {
        for u in 0..8 {
            let mut s = 0.0;
            for x in 0..8 {
                s += d[u][x] * block[y * 8 + x];
            }
            tmp[y * 8 + u] = s;
        }
    }
    let mut zz = [0i32; 64];
    for (k, &natural) in ZIGZAG.iter().enumerate() {
        let (v, u) = (natural / 8, natural % 8);
        let mut s = 0.0;
        for y in 0..8 {
            s += d[v][y] * tmp[y * 8 + u];
        }
        let q = (s / comp.quant[natural]).round() as i32;
        zz[k] = if k == 0 { q } else { q.clamp(-1023, 1023) };
    }

    let diff = zz[0] - comp.pred;
    comp.pred = zz[0];
    let (size, bits) = magnitude(diff);
    w.code(comp.dc, size);
    w.put(bits, size as u32);
    let mut run = 0usize;
    for &v in &zz[1..] {
        if v == 0 {
            run += 1;
            continue;
        }
        while run > 15 {
            w.code(comp.ac, 0xF0);
            run -= 16;
        }
        let (size, bits) = magnitude(v);
        w.code(comp.ac, (run << 4) | size);
        w.put(bits, size as u32);
        run = 0;
    }
    if run > 0 {
        w.code(comp.ac, 0x00);
    }
}

fn segment(out: &mut Vec<u8>, marker: u8, body: &[&[u8]]) {
    let len: usize = 2 + body.iter().map(|b| b.len()).sum::<usize>();
    out.extend_from_slice(&[0xFF, marker]);
    out.extend_from_slice(&(len as u16).to_be_bytes());
    for b in body {
        out.extend_from_slice(b);
    }
}

fn huffman_segment_part(class_id: u8, spec: &HuffSpec, out: &mut Vec<u8>) {
    out.push(class_id);
    out.extend_from_slice(&spec.bits);
    out.extend_from_slice(spec.vals);
}

fn headers(out: &mut Vec<u8>, width: u16, height: u16, luma: &[u16; 64], chroma: &[u16; 64]) {
    out.extend_from_slice(&[0xFF, 0xD8]); // SOI
    // APP0 JFIF 1.02, no density, no thumbnail.
    segment(out, 0xE0, &[b"JFIF\0", &[1, 2, 0, 0, 1, 0, 1, 0, 0]]);
    let table = |id: u8, t: &[u16; 64]| {
        let mut v = Vec::with_capacity(65);
        v.push(id);
        v.extend(ZIGZAG.iter().map(|&i| t[i] as u8));
        v
    };
    segment(out, 0xDB, &[&table(0, luma), &table(1, chroma)]); // DQT
    let [hh, hl] = height.to_be_bytes();
    let [wh, wl] = width.to_be_bytes();
    // SOF0: 8-bit, 3 components; Y 2×2 sampling on table 0, Cb and Cr 1×1 on table 1.
    segment(
        out,
        0xC0,
        &[&[8, hh, hl, wh, wl, 3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]],
    );
    let mut dht = Vec::with_capacity(416);
    huffman_segment_part(0x00, &DC_LUMA, &mut dht);
    huffman_segment_part(0x10, &AC_LUMA, &mut dht);
    huffman_segment_part(0x01, &DC_CHROMA, &mut dht);
    huffman_segment_part(0x11, &AC_CHROMA, &mut dht);
    segment(out, 0xC4, &[&dht]);
    // SOS: all three components, full spectral range.
    segment(out, 0xDA, &[&[3, 1, 0x00, 2, 0x11, 3, 0x11, 0, 63, 0]]);
}

/// Encodes `image` as a baseline JFIF JPEG at `quality` (1–100, clamped) into `out` (cleared
/// first; its allocation is reused).
pub fn encode_jpeg(image: &Image, quality: u8, out: &mut Vec<u8>) -> Result<(), EncodeError> {
    let (w, h) = (image.width, image.height);
    if w == 0 || h == 0 || !image.is_valid() {
        return Err(EncodeError::InvalidImage {
            width: w,
            height: h,
        });
    }
    let (Ok(w16), Ok(h16)) = (u16::try_from(w), u16::try_from(h)) else {
        return Err(EncodeError::TooLarge {
            width: w,
            height: h,
            max: u32::from(u16::MAX),
        });
    };
    let luma_q = scaled_table(&LUMA_Q, quality);
    let chroma_q = scaled_table(&CHROMA_Q, quality);
    out.clear();
    out.reserve(w as usize * h as usize / 4);
    headers(out, w16, h16, &luma_q, &chroma_q);

    let d = dct_matrix();
    let (dc_l, ac_l) = (Huffman::new(&DC_LUMA), Huffman::new(&AC_LUMA));
    let (dc_c, ac_c) = (Huffman::new(&DC_CHROMA), Huffman::new(&AC_CHROMA));
    let quant = |t: &[u16; 64]| t.map(f32::from);
    let mut y_comp = Component {
        quant: quant(&luma_q),
        dc: &dc_l,
        ac: &ac_l,
        pred: 0,
    };
    let mut cb_comp = Component {
        quant: quant(&chroma_q),
        dc: &dc_c,
        ac: &ac_c,
        pred: 0,
    };
    let mut cr_comp = Component {
        quant: quant(&chroma_q),
        dc: &dc_c,
        ac: &ac_c,
        pred: 0,
    };

    let mut writer = BitWriter {
        out,
        acc: 0,
        bits: 0,
    };
    let (wu, hu) = (w as usize, h as usize);
    let mut luma = [[0.0f32; 64]; 4];
    let mut cb_full = [0.0f32; 256];
    let mut cr_full = [0.0f32; 256];
    let mut cb = [0.0f32; 64];
    let mut cr = [0.0f32; 64];
    for my in 0..hu.div_ceil(16) {
        for mx in 0..wu.div_ceil(16) {
            // Gather the 16×16 MCU (edges replicated), convert to level-shifted Y'CbCr.
            for dy in 0..16 {
                let y = (my * 16 + dy).min(hu - 1);
                let row = &image.data[y * wu * 4..(y + 1) * wu * 4];
                for dx in 0..16 {
                    let x = (mx * 16 + dx).min(wu - 1);
                    let p = &row[x * 4..x * 4 + 4];
                    let a = u32::from(p[3]);
                    let flat = |c: u8| ((u32::from(c) * a + 127) / 255) as f32;
                    let (r, g, b) = (flat(p[0]), flat(p[1]), flat(p[2]));
                    let block = (dy / 8) * 2 + dx / 8;
                    luma[block][(dy % 8) * 8 + dx % 8] = 0.299 * r + 0.587 * g + 0.114 * b - 128.0;
                    cb_full[dy * 16 + dx] = -0.168_736 * r - 0.331_264 * g + 0.5 * b;
                    cr_full[dy * 16 + dx] = 0.5 * r - 0.418_688 * g - 0.081_312 * b;
                }
            }
            for j in 0..8 {
                for i in 0..8 {
                    let at = |p: &[f32; 256]| {
                        let o = 2 * j * 16 + 2 * i;
                        (p[o] + p[o + 1] + p[o + 16] + p[o + 17]) * 0.25
                    };
                    cb[j * 8 + i] = at(&cb_full);
                    cr[j * 8 + i] = at(&cr_full);
                }
            }
            for block in &luma {
                encode_block(&mut writer, block, &d, &mut y_comp);
            }
            encode_block(&mut writer, &cb, &d, &mut cb_comp);
            encode_block(&mut writer, &cr, &d, &mut cr_comp);
        }
    }
    writer.flush();
    out.extend_from_slice(&[0xFF, 0xD9]); // EOI
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn zigzag_matches_figure_a6() {
        assert_eq!(
            &ZIGZAG[..16],
            &[0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5]
        );
        assert_eq!(&ZIGZAG[56..], &[53, 60, 61, 54, 47, 55, 62, 63]);
        let set: BTreeSet<usize> = ZIGZAG.iter().copied().collect();
        assert_eq!(set.len(), 64);
        assert_eq!(*set.last().unwrap(), 63);
    }

    #[test]
    fn huffman_tables_cover_every_baseline_symbol_once() {
        for spec in [&DC_LUMA, &DC_CHROMA] {
            assert_eq!(usize::from(spec.bits.iter().sum::<u8>()), spec.vals.len());
            assert_eq!(spec.vals, &DC_VALS);
        }
        // AC: EOB, ZRL and (run 0..=15, size 1..=10).
        let mut expected: BTreeSet<u8> = [0x00, 0xF0].into_iter().collect();
        for run in 0..16u8 {
            for size in 1..=10u8 {
                expected.insert((run << 4) | size);
            }
        }
        for spec in [&AC_LUMA, &AC_CHROMA] {
            assert_eq!(
                spec.bits.iter().map(|&b| usize::from(b)).sum::<usize>(),
                spec.vals.len()
            );
            let got: BTreeSet<u8> = spec.vals.iter().copied().collect();
            assert_eq!(got.len(), spec.vals.len(), "duplicate symbols");
            assert_eq!(got, expected);
        }
        // Prefix-free and no all-ones code (Kraft sum < 1).
        for spec in [&DC_LUMA, &DC_CHROMA, &AC_LUMA, &AC_CHROMA] {
            let kraft: f64 = (1..=16)
                .map(|len| f64::from(spec.bits[len - 1]) / f64::from(1u32 << len))
                .sum();
            assert!(kraft < 1.0, "{kraft}");
        }
    }

    #[test]
    fn dct_matrix_is_orthonormal_with_constant_cosines() {
        for (k, want) in COS16.iter().enumerate() {
            let exact = (k as f64 * std::f64::consts::PI / 16.0).cos();
            assert!((f64::from(*want) - exact).abs() < 1e-7, "cos({k}π/16)");
        }
        let d = dct_matrix();
        for a in 0..8 {
            for b in 0..8 {
                let dot: f32 = (0..8).map(|x| d[a][x] * d[b][x]).sum();
                let want = if a == b { 1.0 } else { 0.0 };
                assert!((dot - want).abs() < 1e-6, "{a}·{b} = {dot}");
            }
        }
    }

    #[test]
    fn quality_scales_the_tables() {
        assert_eq!(scaled_table(&LUMA_Q, 50)[0], 16);
        assert_eq!(scaled_table(&LUMA_Q, 80)[0], 6);
        assert!(scaled_table(&LUMA_Q, 100).iter().all(|&v| v == 1));
        assert!(scaled_table(&CHROMA_Q, 1).iter().all(|&v| v == 255));
        assert_eq!(scaled_table(&LUMA_Q, 0), scaled_table(&LUMA_Q, 1));
    }

    #[test]
    fn magnitude_categories() {
        assert_eq!(magnitude(0), (0, 0));
        assert_eq!(magnitude(1), (1, 1));
        assert_eq!(magnitude(-1), (1, 0));
        assert_eq!(magnitude(-3), (2, 0));
        assert_eq!(magnitude(3), (2, 3));
        assert_eq!(magnitude(1023), (10, 1023));
        assert_eq!(magnitude(-2047), (11, 0));
    }

    #[test]
    fn stuffing_and_padding() {
        let mut out = Vec::new();
        let mut w = BitWriter {
            out: &mut out,
            acc: 0,
            bits: 0,
        };
        w.put(0xFF, 8);
        w.put(0b101, 3);
        w.flush();
        assert_eq!(out, [0xFF, 0x00, 0b1011_1111]);
    }

    #[test]
    fn rejects_empty_and_oversized_images() {
        let mut out = Vec::new();
        let empty = Image {
            width: 0,
            height: 4,
            data: Vec::new(),
        };
        assert!(matches!(
            encode_jpeg(&empty, 80, &mut out),
            Err(EncodeError::InvalidImage { .. })
        ));
        let wide = Image {
            width: 70_000,
            height: 1,
            data: vec![0; 280_000],
        };
        assert!(matches!(
            encode_jpeg(&wide, 80, &mut out),
            Err(EncodeError::TooLarge { .. })
        ));
    }

    /// A smooth colour field with some detail, `w × h`.
    fn field(w: u32, h: u32) -> Image {
        let mut img = Image::filled(w, h, [0, 0, 0, 255]);
        for y in 0..h {
            for x in 0..w {
                let r = (x * 255 / w.max(1)) as u8;
                let g = (y * 255 / h.max(1)) as u8;
                let b = (((x + y) % 32) * 4 + 64) as u8;
                img.set_pixel(x, y, [r, g, b, 255]);
            }
        }
        img
    }

    #[test]
    fn an_independent_decoder_reads_it_back_at_every_size() {
        use crate::metrics::psnr;
        use crate::testkit::decode_jpeg;
        for (w, h) in [(1, 1), (8, 8), (16, 16), (17, 9), (101, 57), (768, 432)] {
            let img = field(w, h);
            let mut out = Vec::new();
            encode_jpeg(&img, 90, &mut out).unwrap();
            let back = decode_jpeg(&out);
            assert_eq!((back.width, back.height), (w, h));
            if w >= 16 {
                let db = psnr(&img, &back);
                assert!(db > 30.0, "{w}x{h}: PSNR {db:.1} dB");
            }
        }
        // Higher quality is closer to the source.
        let img = field(128, 72);
        let mut lo = Vec::new();
        let mut hi = Vec::new();
        encode_jpeg(&img, 30, &mut lo).unwrap();
        encode_jpeg(&img, 95, &mut hi).unwrap();
        assert!(lo.len() < hi.len());
        assert!(psnr(&img, &decode_jpeg(&lo)) < psnr(&img, &decode_jpeg(&hi)));
    }

    #[test]
    fn flat_colours_survive_and_alpha_is_flattened_over_black() {
        use crate::testkit::{decode_jpeg, delta_e};
        for rgb in [
            [200, 30, 90],
            [16, 16, 16],
            [255, 255, 255],
            [0, 0, 0],
            [30, 160, 230],
        ] {
            let img = Image::filled(48, 32, [rgb[0], rgb[1], rgb[2], 255]);
            let mut out = Vec::new();
            encode_jpeg(&img, 80, &mut out).unwrap();
            let p = decode_jpeg(&out).pixel(20, 15);
            let de = delta_e([p[0], p[1], p[2]], rgb);
            assert!(de < 1.0, "{rgb:?} → {p:?} (ΔE {de:.2})");
        }
        let half = Image::filled(16, 16, [200, 100, 50, 128]);
        let mut out = Vec::new();
        encode_jpeg(&half, 100, &mut out).unwrap();
        let p = decode_jpeg(&out).pixel(8, 8);
        for (got, want) in p.iter().zip([100u8, 50, 25]) {
            assert!(got.abs_diff(want) <= 2, "{p:?}");
        }
    }

    #[test]
    fn output_is_a_framed_deterministic_jfif_stream() {
        let mut img = Image::filled(37, 21, [200, 30, 90, 255]);
        img.set_pixel(5, 5, [0, 0, 0, 255]);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        encode_jpeg(&img, 80, &mut a).unwrap();
        encode_jpeg(&img, 80, &mut b).unwrap();
        assert_eq!(a, b);
        assert_eq!(&a[..2], &[0xFF, 0xD8]);
        assert_eq!(&a[6..11], b"JFIF\0");
        assert_eq!(&a[a.len() - 2..], &[0xFF, 0xD9]);
        // Another quality changes the bytes.
        encode_jpeg(&img, 40, &mut b).unwrap();
        assert_ne!(a, b);
    }
}
