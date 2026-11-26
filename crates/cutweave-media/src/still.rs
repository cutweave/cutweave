// SPDX-License-Identifier: GPL-3.0-or-later
//! The orientation of a still image file (WP-2F.05a): the EXIF orientation tag of a JPEG (APP1),
//! PNG (`eXIf`), WebP (`EXIF`) or TIFF (tag 274 of the first directory), read from the
//! container without decoding the picture.
//!
//! Decoders return stills in coded orientation, like video ([`VideoFrame`](crate::VideoFrame));
//! a backend's probe reports the turn as [`ProbeInfo::rotation`](cutweave_model::ProbeInfo) and
//! the compositor applies it. A photo taken with the camera on its side (orientation 6 or 8)
//! then shows upright.
//!
//! The reader is bounded: it walks at most [`MAX_CHUNKS`] container chunks, reads at most
//! [`MAX_EXIF_BYTES`] of an EXIF block and never allocates from a length it has not capped, so a
//! damaged or hostile file costs a few seeks and yields `None`.

use std::fs::File;
use std::io::{self, BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::Path;

/// Most container chunks or segments walked while looking for EXIF data.
const MAX_CHUNKS: usize = 4096;
/// Most bytes of an EXIF block read (the orientation tag is in its first directory).
const MAX_EXIF_BYTES: u64 = 1 << 20;
/// Most entries of a TIFF directory looked at.
const MAX_IFD_ENTRIES: u16 = 512;
/// TIFF tag 274 (0x0112): orientation.
const TAG_ORIENTATION: u16 = 0x0112;

/// How a still is shown: turned clockwise by `rotation`, then mirrored left to right if
/// `mirrored`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StillOrientation {
    /// Clockwise quarter turns in degrees: 0, 90, 180 or 270.
    pub rotation: u16,
    /// Mirrored left to right after the turn (EXIF orientations 2, 4, 5 and 7).
    pub mirrored: bool,
}

impl StillOrientation {
    /// The orientation an EXIF tag value (1 to 8) asks for; `None` for any other value.
    #[must_use]
    pub const fn from_exif(value: u16) -> Option<StillOrientation> {
        let (rotation, mirrored) = match value {
            1 => (0, false),
            2 => (0, true),
            3 => (180, false),
            4 => (180, true),
            5 => (90, true),
            6 => (90, false),
            7 => (270, true),
            8 => (270, false),
            _ => return None,
        };
        Some(StillOrientation { rotation, mirrored })
    }
}

/// The orientation the file at `path` carries; `None` when it has none, is not a format this
/// reads, or cannot be read (the picture is then shown as coded).
#[must_use]
pub fn still_orientation(path: &Path) -> Option<StillOrientation> {
    let file = File::open(path).ok()?;
    read_orientation(&mut BufReader::new(file))
}

/// [`still_orientation`] of an open file or buffer.
pub fn read_orientation<R: Read + Seek>(r: &mut R) -> Option<StillOrientation> {
    let value = exif_value(r).ok().flatten()?;
    StillOrientation::from_exif(value)
}

fn exif_value<R: Read + Seek>(r: &mut R) -> io::Result<Option<u16>> {
    let mut magic = [0_u8; 12];
    let got = read_up_to(r, &mut magic)?;
    let magic = &magic[..got];
    if magic.starts_with(&[0xFF, 0xD8]) {
        r.seek(SeekFrom::Start(2))?;
        jpeg(r)
    } else if magic.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        r.seek(SeekFrom::Start(8))?;
        png(r)
    } else if magic.len() == 12 && &magic[..4] == b"RIFF" && &magic[8..] == b"WEBP" {
        webp(r)
    } else if magic.starts_with(b"II*\0") || magic.starts_with(b"MM\0*") {
        tiff(r, 0)
    } else {
        Ok(None)
    }
}

/// Reads until `buf` is full or the input ends; the bytes read.
fn read_up_to<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

fn array<R: Read, const N: usize>(r: &mut R) -> io::Result<[u8; N]> {
    let mut b = [0_u8; N];
    r.read_exact(&mut b)?;
    Ok(b)
}

/// The next `len` bytes (at most [`MAX_EXIF_BYTES`]) as an EXIF block.
fn exif_block<R: Read + Seek>(r: &mut R, len: u64) -> io::Result<Option<u16>> {
    let mut block = Vec::new();
    r.take(len.min(MAX_EXIF_BYTES)).read_to_end(&mut block)?;
    // JPEG's APP1 prefix; some WebP and PNG writers keep it too.
    let tiff_bytes = block.strip_prefix(b"Exif\0\0").unwrap_or(&block);
    tiff(&mut Cursor::new(tiff_bytes), 0)
}

fn jpeg<R: Read + Seek>(r: &mut R) -> io::Result<Option<u16>> {
    for _ in 0..MAX_CHUNKS {
        let mut marker = array::<_, 2>(r)?;
        if marker[0] != 0xFF {
            return Ok(None);
        }
        // Fill bytes before a marker.
        while marker[1] == 0xFF {
            marker[1] = array::<_, 1>(r)?[0];
        }
        match marker[1] {
            // Start of scan, end of image: the headers are over.
            0xDA | 0xD9 => return Ok(None),
            // Markers without a length.
            0x01 | 0xD0..=0xD8 => continue,
            _ => {}
        }
        let len = u64::from(u16::from_be_bytes(array(r)?));
        let Some(body) = len.checked_sub(2) else {
            return Ok(None);
        };
        if marker[1] == 0xE1 && body >= 6 {
            let at = r.stream_position()?;
            let head = array::<_, 6>(r)?;
            if &head == b"Exif\0\0" {
                return exif_block(r, body - 6);
            }
            // Another APP1 (XMP): skip it.
            r.seek(SeekFrom::Start(at + body))?;
        } else {
            r.seek(SeekFrom::Current(i64::try_from(body).unwrap_or(0)))?;
        }
    }
    Ok(None)
}

fn png<R: Read + Seek>(r: &mut R) -> io::Result<Option<u16>> {
    for _ in 0..MAX_CHUNKS {
        let len = u64::from(u32::from_be_bytes(array(r)?));
        let kind = array::<_, 4>(r)?;
        match &kind {
            b"eXIf" => return exif_block(r, len),
            b"IEND" => return Ok(None),
            // The chunk and its CRC.
            _ => r.seek(SeekFrom::Current(i64::try_from(len + 4).unwrap_or(0)))?,
        };
    }
    Ok(None)
}

fn webp<R: Read + Seek>(r: &mut R) -> io::Result<Option<u16>> {
    r.seek(SeekFrom::Start(12))?;
    for i in 0..MAX_CHUNKS {
        let kind = array::<_, 4>(r)?;
        let len = u64::from(u32::from_le_bytes(array(r)?));
        // Only the extended container (it starts with `VP8X`) has metadata chunks.
        if i == 0 && &kind != b"VP8X" {
            return Ok(None);
        }
        if &kind == b"EXIF" {
            return exif_block(r, len);
        }
        // Chunks are padded to an even length.
        r.seek(SeekFrom::Current(
            i64::try_from(len + (len & 1)).unwrap_or(0),
        ))?;
    }
    Ok(None)
}

/// The orientation tag of the first directory of the TIFF structure that starts at `base`.
fn tiff<R: Read + Seek>(r: &mut R, base: u64) -> io::Result<Option<u16>> {
    r.seek(SeekFrom::Start(base))?;
    let head = array::<_, 4>(r)?;
    let big = match &head {
        b"II*\0" => false,
        b"MM\0*" => true,
        _ => return Ok(None),
    };
    let u16_of = |b: [u8; 2]| {
        if big {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        }
    };
    let u32_of = |b: [u8; 4]| {
        if big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        }
    };
    let ifd = u64::from(u32_of(array(r)?));
    r.seek(SeekFrom::Start(base + ifd))?;
    let entries = u16_of(array(r)?).min(MAX_IFD_ENTRIES);
    for _ in 0..entries {
        let entry = array::<_, 12>(r)?;
        let tag = u16_of([entry[0], entry[1]]);
        if tag != TAG_ORIENTATION {
            continue;
        }
        let kind = u16_of([entry[2], entry[3]]);
        let value = match kind {
            // BYTE, SHORT, LONG: the value sits in the entry, left-justified.
            1 => u16::from(entry[8]),
            3 => u16_of([entry[8], entry[9]]),
            4 => u16::try_from(u32_of([entry[8], entry[9], entry[10], entry[11]])).unwrap_or(0),
            _ => return Ok(None),
        };
        return Ok(Some(value));
    }
    Ok(None)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::unnecessary_wraps)]
mod tests {
    use super::*;
    use crate::testing::stills::{self, exif_orientation, png_color};

    fn of(bytes: &[u8]) -> Option<StillOrientation> {
        read_orientation(&mut Cursor::new(bytes))
    }

    fn turned(rotation: u16) -> Option<StillOrientation> {
        Some(StillOrientation {
            rotation,
            mirrored: false,
        })
    }

    #[test]
    fn the_eight_exif_values_map_to_a_turn_and_a_mirror() {
        let all: Vec<_> = (1..=8)
            .map(|v| StillOrientation::from_exif(v).unwrap())
            .map(|o| (o.rotation, o.mirrored))
            .collect();
        assert_eq!(
            all,
            [
                (0, false),
                (0, true),
                (180, false),
                (180, true),
                (90, true),
                (90, false),
                (270, true),
                (270, false)
            ]
        );
        assert_eq!(StillOrientation::from_exif(0), None);
        assert_eq!(StillOrientation::from_exif(9), None);
    }

    #[test]
    fn a_jpeg_reports_its_exif_orientation_in_either_byte_order() {
        let blocks = [[128, 128, 128]; 6];
        for big in [false, true] {
            let file = stills::jpeg(3, 2, &blocks, Some(&exif_orientation(6, big)));
            assert_eq!(of(&file), turned(90), "big endian: {big}");
        }
        let file = stills::jpeg(3, 2, &blocks, Some(&exif_orientation(8, false)));
        assert_eq!(of(&file), turned(270));
        let file = stills::jpeg(3, 2, &blocks, Some(&exif_orientation(3, false)));
        assert_eq!(of(&file), turned(180));
        assert_eq!(of(&stills::jpeg(3, 2, &blocks, None)), None);
    }

    #[test]
    fn a_mirrored_orientation_is_reported_as_mirrored() {
        let file = stills::jpeg(1, 1, &[[1, 2, 3]], Some(&exif_orientation(5, false)));
        assert_eq!(
            of(&file),
            Some(StillOrientation {
                rotation: 90,
                mirrored: true
            })
        );
    }

    #[test]
    fn a_png_reports_its_exif_chunk() {
        let exif = exif_orientation(6, true);
        let file = stills::png(1, 1, 8, png_color::RGBA, &[0; 4], &[(*b"eXIf", &exif)]);
        assert_eq!(of(&file), turned(90));
        assert_eq!(
            of(&stills::png(1, 1, 8, png_color::RGBA, &[0; 4], &[])),
            None
        );
    }

    #[test]
    fn a_webp_reports_its_exif_chunk_with_or_without_the_jpeg_prefix() {
        let exif = exif_orientation(8, false);
        let px = [false, true];
        let file = stills::webp(2, 1, &px, [0, 0, 255], [0, 255], Some(&exif));
        assert_eq!(of(&file), turned(270));
        let mut prefixed = b"Exif\0\0".to_vec();
        prefixed.extend_from_slice(&exif);
        let file = stills::webp(2, 1, &px, [0, 0, 255], [0, 255], Some(&prefixed));
        assert_eq!(of(&file), turned(270));
        assert_eq!(
            of(&stills::webp(2, 1, &px, [0, 0, 255], [0, 255], None)),
            None
        );
    }

    #[test]
    fn a_tiff_reports_its_own_orientation_tag() {
        assert_eq!(of(&stills::tiff(1, 1, 3, 8, &[0; 3], Some(6))), turned(90));
        assert_eq!(of(&stills::tiff(1, 1, 3, 8, &[0; 3], None)), None);
    }

    #[test]
    fn other_files_and_damaged_ones_have_no_orientation() {
        assert_eq!(of(b""), None);
        assert_eq!(of(b"not an image at all"), None);
        assert_eq!(of(&stills::bmp(1, 1, &[0; 3])), None);
        // Every truncation of a rotated JPEG, PNG, WebP and TIFF: never a panic, and never a
        // value other than the file's own.
        let exif = exif_orientation(6, false);
        let files = [
            stills::jpeg(1, 1, &[[1, 2, 3]], Some(&exif)),
            stills::png(1, 1, 8, png_color::RGBA, &[0; 4], &[(*b"eXIf", &exif)]),
            stills::webp(1, 1, &[true], [0, 0, 255], [0, 255], Some(&exif)),
            stills::tiff(1, 1, 3, 8, &[0; 3], Some(6)),
        ];
        for file in files {
            for len in 0..file.len() {
                let got = of(&file[..len]);
                assert!(got.is_none() || got == turned(90), "{len}: {got:?}");
            }
        }
        // An EXIF value out of range.
        let file = stills::jpeg(1, 1, &[[1, 2, 3]], Some(&exif_orientation(42, false)));
        assert_eq!(of(&file), None);
        // A directory that points past the end, and a chunk length that would overflow a seek.
        let mut bad = exif_orientation(6, false);
        bad[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(of(&stills::jpeg(1, 1, &[[1, 2, 3]], Some(&bad))), None);
        let mut png = stills::png(1, 1, 8, png_color::RGBA, &[0; 4], &[(*b"tEXt", b"x")]);
        png[33..37].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(of(&png), None);
    }
}
