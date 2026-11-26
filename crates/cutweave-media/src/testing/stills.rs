// SPDX-License-Identifier: GPL-3.0-or-later
//! Still-image files written in memory for tests (WP-2F.05a): no media is committed, no encoder
//! library is needed, and every byte is known.
//!
//! - [`png`]: any PNG colour type and bit depth, stored (uncompressed) deflate, with extra chunks
//!   (`eXIf`, `PLTE`, `tRNS`, ...) before the picture data.
//! - [`jpeg`]: baseline Y'CbCr 4:4:4 JFIF made of flat 8×8 blocks (only DC coefficients, unit
//!   quantisation), so every block decodes to exactly the colour it was given; optional EXIF.
//! - [`tiff`]: uncompressed little-endian RGB or RGBA, 8 or 16 bits, optional orientation tag.
//! - [`bmp`]: 24-bit bottom-up.
//! - [`webp`]: lossless (VP8L) with two colours, optionally in the extended container with an
//!   `EXIF` chunk.
//! - [`exif_orientation`]: the TIFF structure of an EXIF block holding one orientation tag.

/// CRC-32 (IEEE 802.3, as PNG chunks use it).
fn crc32(parts: &[&[u8]]) -> u32 {
    let mut crc = u32::MAX;
    for part in parts {
        for &byte in *part {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1_u32, 0_u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn be32(n: usize) -> [u8; 4] {
    u32::try_from(n).unwrap_or(u32::MAX).to_be_bytes()
}

fn le32(n: usize) -> [u8; 4] {
    u32::try_from(n).unwrap_or(u32::MAX).to_le_bytes()
}

fn le16(n: usize) -> [u8; 2] {
    u16::try_from(n).unwrap_or(u16::MAX).to_le_bytes()
}

fn be16(n: usize) -> [u8; 2] {
    u16::try_from(n).unwrap_or(u16::MAX).to_be_bytes()
}

fn png_chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
    out.extend_from_slice(&be32(data.len()));
    out.extend_from_slice(&kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[&kind, data]).to_be_bytes());
}

/// PNG colour types (the IHDR byte).
pub mod png_color {
    /// One gray sample per pixel.
    pub const GRAY: u8 = 0;
    /// Red, green, blue.
    pub const RGB: u8 = 2;
    /// One palette index per pixel (needs a `PLTE` chunk).
    pub const PALETTE: u8 = 3;
    /// Gray and alpha.
    pub const GRAY_ALPHA: u8 = 4;
    /// Red, green, blue, alpha.
    pub const RGBA: u8 = 6;
}

/// A PNG file of `width × height` pixels with `depth` bits per sample (8 or 16) and colour type
/// `color` ([`png_color`]). `samples` holds the rows top to bottom, 16-bit samples big-endian
/// (as PNG stores them). `chunks` go between the header and the picture data.
#[must_use]
pub fn png(
    width: u32,
    height: u32,
    depth: u8,
    color: u8,
    samples: &[u8],
    chunks: &[([u8; 4], &[u8])],
) -> Vec<u8> {
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[depth, color, 0, 0, 0]);
    png_chunk(&mut out, *b"IHDR", &header);
    for (kind, data) in chunks {
        png_chunk(&mut out, *kind, data);
    }
    // Every row starts with filter type 0 (none).
    let row = samples.len() / (height.max(1) as usize);
    let mut raw = Vec::with_capacity(samples.len() + height as usize);
    for line in samples.chunks(row.max(1)) {
        raw.push(0);
        raw.extend_from_slice(line);
    }
    // zlib with stored blocks.
    let mut z = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65_535).peekable();
    while let Some(block) = blocks.next() {
        z.push(u8::from(blocks.peek().is_none()));
        z.extend_from_slice(&le16(block.len()));
        z.extend_from_slice(&(!u16::try_from(block.len()).unwrap_or(u16::MAX)).to_le_bytes());
        z.extend_from_slice(block);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    png_chunk(&mut out, *b"IDAT", &z);
    png_chunk(&mut out, *b"IEND", &[]);
    out
}

/// The TIFF structure of an EXIF block with one tag: orientation `value` (1 to 8), in the byte
/// order asked for. JPEG puts it in APP1 after `Exif\0\0`; PNG in `eXIf`; WebP in `EXIF`.
#[must_use]
pub fn exif_orientation(value: u16, big_endian: bool) -> Vec<u8> {
    let u16b = |n: u16| {
        if big_endian {
            n.to_be_bytes()
        } else {
            n.to_le_bytes()
        }
    };
    let u32b = |n: u32| {
        if big_endian {
            n.to_be_bytes()
        } else {
            n.to_le_bytes()
        }
    };
    let mut t = Vec::new();
    t.extend_from_slice(if big_endian { b"MM" } else { b"II" });
    t.extend_from_slice(&u16b(42));
    t.extend_from_slice(&u32b(8));
    t.extend_from_slice(&u16b(1));
    t.extend_from_slice(&u16b(0x0112));
    t.extend_from_slice(&u16b(3));
    t.extend_from_slice(&u32b(1));
    t.extend_from_slice(&u16b(value));
    t.extend_from_slice(&[0, 0]);
    t.extend_from_slice(&u32b(0));
    t
}

/// Bits written most significant first, with JPEG's `FF 00` stuffing.
struct JpegBits {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl JpegBits {
    fn put(&mut self, code: u32, len: u32) {
        for i in (0..len).rev() {
            self.acc = (self.acc << 1) | ((code >> i) & 1);
            self.n += 1;
            if self.n == 8 {
                #[allow(clippy::cast_possible_truncation)]
                let byte = self.acc as u8;
                self.out.push(byte);
                if byte == 0xFF {
                    self.out.push(0);
                }
                self.acc = 0;
                self.n = 0;
            }
        }
    }

    fn finish(mut self) -> Vec<u8> {
        while self.n != 0 {
            self.put(1, 1);
        }
        self.out
    }
}

fn jpeg_segment(out: &mut Vec<u8>, marker: u8, data: &[u8]) {
    out.extend_from_slice(&[0xFF, marker]);
    out.extend_from_slice(&be16(data.len() + 2));
    out.extend_from_slice(data);
}

/// A baseline JFIF file of `blocks_w × blocks_h` flat 8×8 blocks (so `8·blocks_w × 8·blocks_h`
/// pixels), Y'CbCr 4:4:4. `blocks` gives each block's `[Y', Cb, Cr]` (full range, as JFIF
/// defines), row by row. `exif`, if given, is the TIFF structure of an APP1 EXIF segment
/// ([`exif_orientation`]).
#[must_use]
pub fn jpeg(blocks_w: u16, blocks_h: u16, blocks: &[[u8; 3]], exif: Option<&[u8]>) -> Vec<u8> {
    let mut out = vec![0xFF, 0xD8];
    jpeg_segment(
        &mut out,
        0xE0,
        &[b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0],
    );
    if let Some(tiff) = exif {
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(tiff);
        jpeg_segment(&mut out, 0xE1, &app1);
    }
    // One quantisation table of ones: a coefficient is its value.
    let mut dqt = vec![0];
    dqt.extend_from_slice(&[1; 64]);
    jpeg_segment(&mut out, 0xDB, &dqt);
    let mut sof = vec![8];
    sof.extend_from_slice(&(blocks_h * 8).to_be_bytes());
    sof.extend_from_slice(&(blocks_w * 8).to_be_bytes());
    sof.extend_from_slice(&[3, 1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0]);
    jpeg_segment(&mut out, 0xC0, &sof);
    // DC table 0: the standard luminance table (categories 0 to 11).
    let mut dc = vec![0x00, 0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
    dc.extend_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
    jpeg_segment(&mut out, 0xC4, &dc);
    // AC table 0: two 2-bit codes; `00` is end-of-block, the only one used.
    let mut ac = vec![0x10, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    ac.extend_from_slice(&[0x00, 0x01]);
    jpeg_segment(&mut out, 0xC4, &ac);
    jpeg_segment(&mut out, 0xDA, &[3, 1, 0x00, 2, 0x00, 3, 0x00, 0, 63, 0]);

    let mut bits = JpegBits {
        out: Vec::new(),
        acc: 0,
        n: 0,
    };
    let mut pred = [0_i32; 3];
    for block in blocks
        .iter()
        .take(usize::from(blocks_w) * usize::from(blocks_h))
    {
        for (c, &sample) in block.iter().enumerate() {
            // The DC coefficient of a flat block of `sample` (level-shifted by 128) is 8 times it.
            let dc = (i32::from(sample) - 128) * 8;
            let diff = dc - pred[c];
            pred[c] = dc;
            let category = 32 - diff.unsigned_abs().leading_zeros();
            let (code, len) = match category {
                0 => (0b00, 2),
                1..=5 => (0b010 + category - 1, 3),
                _ => ((1 << (category - 2)) - 2, category - 2),
            };
            bits.put(code, len);
            if category > 0 {
                let value = if diff >= 0 {
                    diff
                } else {
                    diff + (1 << category) - 1
                };
                bits.put(value.unsigned_abs(), category);
            }
            bits.put(0b00, 2);
        }
    }
    out.extend_from_slice(&bits.finish());
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}

/// An uncompressed little-endian TIFF: `channels` 3 (RGB) or 4 (RGB and unassociated alpha),
/// `depth` 8 or 16 bits per sample (`samples` little-endian for 16), one strip. `orientation`
/// writes tag 274.
#[must_use]
pub fn tiff(
    width: u32,
    height: u32,
    channels: u16,
    depth: u16,
    samples: &[u8],
    orientation: Option<u16>,
) -> Vec<u8> {
    let mut entries: Vec<(u16, u16, u32, u32)> = Vec::new();
    let count = 9 + usize::from(orientation.is_some()) + usize::from(channels == 4);
    let bits_at = 8 + 2 + 12 * count + 4;
    let data_at = bits_at + 2 * usize::from(channels);
    let as_u32 = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    entries.push((256, 4, 1, width));
    entries.push((257, 4, 1, height));
    entries.push((258, 3, u32::from(channels), as_u32(bits_at)));
    entries.push((259, 3, 1, 1));
    entries.push((262, 3, 1, 2));
    entries.push((273, 4, 1, as_u32(data_at)));
    if let Some(o) = orientation {
        entries.push((274, 3, 1, u32::from(o)));
    }
    entries.push((277, 3, 1, u32::from(channels)));
    entries.push((278, 4, 1, height));
    entries.push((279, 4, 1, as_u32(samples.len())));
    if channels == 4 {
        entries.push((338, 3, 1, 2));
    }
    let mut out = vec![b'I', b'I', 42, 0, 8, 0, 0, 0];
    out.extend_from_slice(&le16(entries.len()));
    for (tag, kind, n, value) in entries {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&[0; 4]);
    for _ in 0..channels {
        out.extend_from_slice(&depth.to_le_bytes());
    }
    out.extend_from_slice(samples);
    out
}

/// A 24-bit BMP from `rgb` rows given top to bottom.
#[must_use]
pub fn bmp(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
    let stride = (width as usize * 3).div_ceil(4) * 4;
    let size = 54 + stride * height as usize;
    let mut out = vec![b'B', b'M'];
    out.extend_from_slice(&le32(size));
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&le32(54));
    out.extend_from_slice(&le32(40));
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&[1, 0, 24, 0]);
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&le32(stride * height as usize));
    out.extend_from_slice(&[0; 16]);
    for row in rgb.chunks(width as usize * 3).rev() {
        for px in row.chunks(3) {
            out.extend_from_slice(&[px[2], px[1], px[0]]);
        }
        out.resize(out.len() + stride - row.len(), 0);
    }
    out
}

/// Bits written least significant first (VP8L).
struct LsbBits {
    out: Vec<u8>,
    n: u32,
}

impl LsbBits {
    fn put(&mut self, value: u32, len: u32) {
        for i in 0..len {
            if self.n.is_multiple_of(8) {
                self.out.push(0);
            }
            if (value >> i) & 1 == 1
                && let Some(last) = self.out.last_mut()
            {
                *last |= 1 << (self.n % 8);
            }
            self.n += 1;
        }
    }
}

fn riff_chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
    out.extend_from_slice(&kind);
    out.extend_from_slice(&le32(data.len()));
    out.extend_from_slice(data);
    if !data.len().is_multiple_of(2) {
        out.push(0);
    }
}

/// A lossless WebP of two colours: pixel `i` (row by row) is `[red, greens[1], blue, alpha]`
/// where `second[i]`, else `[red, greens[0], blue, alpha]`; `greens[0] < greens[1]`. With
/// `exif` (the TIFF structure, [`exif_orientation`]) the file uses the extended container
/// (`VP8X`) and carries an `EXIF` chunk.
#[must_use]
pub fn webp(
    width: u32,
    height: u32,
    second: &[bool],
    [red, blue, alpha]: [u8; 3],
    greens: [u8; 2],
    exif: Option<&[u8]>,
) -> Vec<u8> {
    let mut bits = LsbBits {
        out: vec![0x2F],
        n: 0,
    };
    bits.put(width - 1, 14);
    bits.put(height - 1, 14);
    bits.put(u32::from(alpha != 255), 1);
    bits.put(0, 3);
    // No transform, no colour cache, no meta prefix codes.
    bits.put(0, 3);
    // Green: a simple code of two 8-bit symbols (one bit per pixel).
    bits.put(0b111, 3);
    bits.put(u32::from(greens[0]), 8);
    bits.put(u32::from(greens[1]), 8);
    // Red, blue, alpha: simple codes of one 8-bit symbol (no bits per pixel).
    for value in [red, blue, alpha] {
        bits.put(0b101, 3);
        bits.put(u32::from(value), 8);
    }
    // Distance: one 1-bit symbol, never used.
    bits.put(0b001, 3);
    bits.put(0, 1);
    for &is_second in second.iter().take(width as usize * height as usize) {
        bits.put(u32::from(is_second), 1);
    }
    let lossless = bits.out;

    let mut body = b"WEBP".to_vec();
    if let Some(tiff) = exif {
        let mut extended = vec![0x08 | if alpha == 255 { 0 } else { 0x10 }, 0, 0, 0];
        extended.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
        extended.extend_from_slice(&(height - 1).to_le_bytes()[..3]);
        riff_chunk(&mut body, *b"VP8X", &extended);
        riff_chunk(&mut body, *b"VP8L", &lossless);
        riff_chunk(&mut body, *b"EXIF", tiff);
    } else {
        riff_chunk(&mut body, *b"VP8L", &lossless);
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&le32(body.len()));
    out.extend_from_slice(&body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_their_reference_values() {
        assert_eq!(crc32(&[b"123456789"]), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn files_start_with_their_signatures_and_have_the_expected_sizes() {
        let png_file = png(2, 2, 8, png_color::RGBA, &[0; 16], &[]);
        assert_eq!(&png_file[..4], &[0x89, b'P', b'N', b'G']);
        assert_eq!(&png_file[png_file.len() - 8..png_file.len() - 4], b"IEND");
        let jpeg_file = jpeg(2, 1, &[[128, 128, 128]; 2], None);
        assert_eq!(&jpeg_file[..2], &[0xFF, 0xD8]);
        assert_eq!(&jpeg_file[jpeg_file.len() - 2..], &[0xFF, 0xD9]);
        let tiff_file = tiff(2, 2, 3, 8, &[0; 12], Some(6));
        assert_eq!(&tiff_file[..4], b"II*\0");
        let bmp_file = bmp(3, 2, &[0; 18]);
        assert_eq!(bmp_file.len(), 54 + 12 * 2);
        let webp_file = webp(
            2,
            2,
            &[false, true, true, false],
            [0, 0, 255],
            [10, 200],
            None,
        );
        assert_eq!(&webp_file[..4], b"RIFF");
        assert_eq!(&webp_file[8..16], b"WEBPVP8L");
    }
}
