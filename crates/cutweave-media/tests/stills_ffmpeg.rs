// SPDX-License-Identifier: GPL-3.0-or-later
//! Still images on the FFmpeg backend (WP-2F.05a): PNG, JPEG, WebP, TIFF and BMP files probe as
//! one picture without a duration and decode to their colours; the EXIF orientation becomes the
//! probe's display rotation (frames stay in coded orientation); 16-bit, gray, palette and
//! transparent PNGs come out as straight-alpha RGBA.
//!
//! The files are written by `cutweave_media::testing::stills` into a temporary directory: no
//! corpus, no network. Needs the FFmpeg bundle (`just test-ffmpeg`, docs/dev/ffmpeg.md).
#![cfg(feature = "ffmpeg")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use cutweave_media::testing::contract::TempDir;
use cutweave_media::testing::stills::{self, exif_orientation, png_color};
use cutweave_media::{DecodeOpts, FfmpegBackend, MediaBackend, PixelFormat, VideoFrame};
use cutweave_model::{ColorRange, StreamKind, Time};

/// The test card: 3 × 2 cells of 8 × 8 pixels, each one colour (R'G'B').
const CELLS: [[u8; 3]; 6] = [
    [200, 30, 30],
    [30, 200, 30],
    [30, 30, 200],
    [240, 240, 240],
    [20, 20, 20],
    [220, 220, 40],
];
const W: u32 = 24;
const H: u32 = 16;

fn backend() -> FfmpegBackend {
    FfmpegBackend::new().expect("the loaded FFmpeg matches the build")
}

fn write(dir: &TempDir, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn cell_of(x: u32, y: u32) -> [u8; 3] {
    CELLS[(y / 8 * 3 + x / 8) as usize]
}

/// The card as interleaved samples: `channels` 3 or 4 (alpha 255), `bytes` 1 or 2 per sample
/// (2: the 8-bit value in both bytes, so either byte order reads the same 16-bit value).
fn card(channels: usize, bytes: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for y in 0..H {
        for x in 0..W {
            let [r, g, b] = cell_of(x, y);
            for v in [r, g, b, 255].into_iter().take(channels) {
                out.extend(std::iter::repeat_n(v, bytes));
            }
        }
    }
    out
}

/// JFIF's full-range BT.601 Y'CbCr of an R'G'B' colour.
fn ycbcr([r, g, b]: [u8; 3]) -> [u8; 3] {
    let (r, g, b) = (f64::from(r), f64::from(g), f64::from(b));
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    let cb = 128.0 - 0.168_736 * r - 0.331_264 * g + 0.5 * b;
    let cr = 128.0 + 0.5 * r - 0.418_688 * g - 0.081_312 * b;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    [y, cb, cr].map(|v| v.round().clamp(0.0, 255.0) as u8)
}

fn card_jpeg(exif: Option<&[u8]>) -> Vec<u8> {
    stills::jpeg(3, 2, &CELLS.map(ycbcr), exif)
}

/// The decoded picture as straight-alpha RGBA8, in coded orientation.
fn rgba(path: &Path) -> VideoFrame {
    let opts = DecodeOpts::default().with_target(PixelFormat::Rgba8);
    let mut d = backend().open_decoder(path, &opts).unwrap();
    let f = d.seek_exact(Time::ZERO).unwrap();
    f.validate().unwrap();
    assert_eq!(f.format, PixelFormat::Rgba8);
    f
}

fn pixel(f: &VideoFrame, x: u32, y: u32) -> [u8; 4] {
    let plane = &f.planes[0];
    let at = y as usize * plane.stride as usize + x as usize * 4;
    let d = &plane.data;
    [d[at], d[at + 1], d[at + 2], d[at + 3]]
}

fn close(a: [u8; 4], b: [u8; 4], tolerance: u8) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= tolerance)
}

/// Checks the middle of every cell of the card against its colour.
fn assert_card(name: &str, f: &VideoFrame, tolerance: u8) {
    assert_eq!((f.width, f.height), (W, H), "{name}");
    for cy in 0..2 {
        for cx in 0..3 {
            let [r, g, b] = CELLS[(cy * 3 + cx) as usize];
            let got = pixel(f, cx * 8 + 4, cy * 8 + 4);
            assert!(
                close(got, [r, g, b, 255], tolerance),
                "{name}: cell ({cx}, {cy}) is {got:?}, expected {:?}",
                [r, g, b, 255]
            );
        }
    }
}

#[test]
fn png_jpeg_tiff_and_bmp_probe_as_one_picture_and_decode_to_their_colours() {
    let dir = TempDir::new("stills-formats");
    let files = [
        (
            "card.png",
            stills::png(W, H, 8, png_color::RGBA, &card(4, 1), &[]),
            0,
        ),
        (
            "card16.png",
            stills::png(W, H, 16, png_color::RGB, &card(3, 2), &[]),
            // 16 bits to 8: swscale's rounding is within one step.
            1,
        ),
        // 4:4:4 Y'CbCr goes through 4:2:2 on the way: a rounding step or two per channel.
        ("card.jpg", card_jpeg(None), 4),
        ("card.tif", stills::tiff(W, H, 3, 8, &card(3, 1), None), 0),
        (
            "card16.tiff",
            stills::tiff(W, H, 4, 16, &card(4, 2), None),
            1,
        ),
        ("card.bmp", stills::bmp(W, H, &card(3, 1)), 0),
    ];
    let backend = backend();
    for (name, bytes, tolerance) in files {
        let path = write(&dir, name, &bytes);
        let info = backend.probe(&path).unwrap();
        assert_eq!(info.duration, None, "{name}");
        assert_eq!(info.fps, None, "{name}");
        assert_eq!(info.audio, None, "{name}");
        assert_eq!(info.rotation, 0, "{name}");
        assert_eq!(info.streams.len(), 1, "{name}");
        let s = &info.streams[0];
        assert_eq!(
            (s.kind, s.width, s.height),
            (StreamKind::Video, Some(W), Some(H)),
            "{name}"
        );
        assert_card(name, &rgba(&path), tolerance);
        // The picture never ends: any time shows it.
        let mut d = backend.open_decoder(&path, &DecodeOpts::default()).unwrap();
        assert_eq!(d.index().frame_count(), 1, "{name}");
        for t in [Time::ZERO, Time::from_secs(3600).unwrap()] {
            let f = d.seek_exact(t).unwrap();
            f.validate().unwrap();
            assert_eq!((f.width, f.height), (W, H), "{name}");
        }
    }
}

#[test]
fn a_lossless_webp_decodes_to_its_two_colours() {
    let dir = TempDir::new("stills-webp");
    // The left half is the first colour, the right half the second.
    let second: Vec<bool> = (0..H).flat_map(|_| (0..W).map(|x| x >= W / 2)).collect();
    let bytes = stills::webp(W, H, &second, [200, 40, 255], [10, 220], None);
    let path = write(&dir, "two.webp", &bytes);
    let info = backend().probe(&path).unwrap();
    assert_eq!(info.duration, None);
    assert_eq!(
        (info.streams[0].width, info.streams[0].height),
        (Some(W), Some(H))
    );
    let f = rgba(&path);
    assert_eq!(pixel(&f, 2, 2), [200, 10, 40, 255]);
    assert_eq!(pixel(&f, W - 2, H - 2), [200, 220, 40, 255]);
}

#[test]
fn the_exif_orientation_is_the_probe_rotation() {
    let dir = TempDir::new("stills-exif");
    let backend = backend();
    let rotation = |name: &str, bytes: &[u8]| backend.probe(&write(&dir, name, bytes)).unwrap();
    // A photo taken with the camera turned (EXIF 6): shown turned 90° clockwise.
    for (value, degrees) in [(1, 0), (3, 180), (6, 90), (8, 270)] {
        for big in [false, true] {
            let info = rotation(
                &format!("o{value}-{big}.jpg"),
                &card_jpeg(Some(&exif_orientation(value, big))),
            );
            assert_eq!(info.rotation, degrees, "JPEG, EXIF {value}");
            // Still one picture of the coded size.
            assert_eq!(info.duration, None);
            assert_eq!(
                (info.streams[0].width, info.streams[0].height),
                (Some(W), Some(H))
            );
        }
    }
    let exif = exif_orientation(6, false);
    let info = rotation(
        "o6.png",
        &stills::png(W, H, 8, png_color::RGBA, &card(4, 1), &[(*b"eXIf", &exif)]),
    );
    assert_eq!(info.rotation, 90, "PNG eXIf");
    let info = rotation("o8.tif", &stills::tiff(W, H, 3, 8, &card(3, 1), Some(8)));
    assert_eq!(info.rotation, 270, "TIFF tag 274");
    let second = vec![false; (W * H) as usize];
    let info = rotation(
        "o6.webp",
        &stills::webp(W, H, &second, [1, 2, 255], [3, 4], Some(&exif)),
    );
    assert_eq!(info.rotation, 90, "WebP EXIF");
    // A mirrored orientation reports its turn; the decoder applies the mirror.
    for (value, degrees) in [(2, 0), (4, 180), (5, 90), (7, 270)] {
        let info = rotation(
            &format!("m{value}.jpg"),
            &card_jpeg(Some(&exif_orientation(value, false))),
        );
        assert_eq!(info.rotation, degrees, "JPEG, EXIF {value}");
    }
    // No EXIF: as coded.
    assert_eq!(rotation("plain.jpg", &card_jpeg(None)).rotation, 0);
}

#[test]
fn a_rotated_still_decodes_in_coded_orientation() {
    // The compositor turns the picture from the probe's rotation (as it does for phone video):
    // the decoder must not turn it too.
    let dir = TempDir::new("stills-coded");
    let files = [
        ("o6.jpg", card_jpeg(Some(&exif_orientation(6, false))), 4),
        ("o6.tif", stills::tiff(W, H, 3, 8, &card(3, 1), Some(6)), 0),
        (
            "o6.png",
            stills::png(
                W,
                H,
                8,
                png_color::RGBA,
                &card(4, 1),
                &[(*b"eXIf", &exif_orientation(6, true))],
            ),
            0,
        ),
    ];
    for (name, bytes, tolerance) in files {
        assert_card(name, &rgba(&write(&dir, name, &bytes)), tolerance);
    }
}

#[test]
fn a_16_bit_png_with_alpha_decodes_to_straight_rgba() {
    let dir = TempDir::new("stills-rgba16");
    // 2 × 1: an opaque colour, and a half-transparent one whose colour is NOT multiplied by its
    // alpha in the file (PNG stores straight alpha).
    let px: [[u16; 4]; 2] = [
        [0xFFFF, 0x8000, 0x0000, 0xFFFF],
        [0xC0C0, 0x4040, 0x2020, 0x8080],
    ];
    let samples: Vec<u8> = px.iter().flatten().flat_map(|v| v.to_be_bytes()).collect();
    let path = write(
        &dir,
        "deep.png",
        &stills::png(2, 1, 16, png_color::RGBA, &samples, &[]),
    );
    let info = backend().probe(&path).unwrap();
    assert_eq!(info.duration, None);
    let f = rgba(&path);
    assert!(close(pixel(&f, 0, 0), [255, 128, 0, 255], 1), "{f:?}");
    assert!(
        close(pixel(&f, 1, 0), [0xC0, 0x40, 0x20, 0x80], 1),
        "{:?}",
        pixel(&f, 1, 0)
    );
    // The decoder's own layout for a picture with alpha is RGBA too: nothing downstream has to
    // ask for it.
    let mut d = backend()
        .open_decoder(&path, &DecodeOpts::default())
        .unwrap();
    let native = d.seek_exact(Time::ZERO).unwrap();
    assert_eq!(native.format, PixelFormat::Rgba8);
    assert!(close(pixel(&native, 1, 0), [0xC0, 0x40, 0x20, 0x80], 1));
}

#[test]
fn gray_gray_alpha_and_palette_pngs_decode_to_their_colours() {
    let dir = TempDir::new("stills-kinds");
    // Gray: black, mid gray, white. Full range: 0 is black and 255 is white.
    let gray = write(
        &dir,
        "gray.png",
        &stills::png(
            4,
            2,
            8,
            png_color::GRAY,
            &[0, 128, 255, 64, 0, 128, 255, 64],
            &[],
        ),
    );
    let f = rgba(&gray);
    for (x, v) in [(0, 0), (1, 128), (2, 255), (3, 64)] {
        assert!(
            close(pixel(&f, x, 0), [v, v, v, 255], 1),
            "gray {x}: {:?}",
            pixel(&f, x, 0)
        );
    }
    // In the decoder's own layout a gray picture is full-range luma (or already RGBA): a
    // limited-range reading would show it washed out.
    let mut d = backend()
        .open_decoder(&gray, &DecodeOpts::default())
        .unwrap();
    let native = d.seek_exact(Time::ZERO).unwrap();
    assert!(
        native.format == PixelFormat::Rgba8 || native.color.range == Some(ColorRange::Full),
        "{:?} {:?}",
        native.format,
        native.color
    );

    let gray_alpha = write(
        &dir,
        "ga.png",
        &stills::png(2, 1, 8, png_color::GRAY_ALPHA, &[200, 255, 100, 64], &[]),
    );
    let f = rgba(&gray_alpha);
    assert_eq!(pixel(&f, 0, 0), [200, 200, 200, 255]);
    assert_eq!(pixel(&f, 1, 0), [100, 100, 100, 64]);

    // Palette with a transparency chunk: index 1 is half transparent.
    let palette = [255, 0, 0, 0, 255, 0];
    let indexed = write(
        &dir,
        "indexed.png",
        &stills::png(
            2,
            1,
            8,
            png_color::PALETTE,
            &[0, 1],
            &[(*b"PLTE", &palette), (*b"tRNS", &[255, 128])],
        ),
    );
    let f = rgba(&indexed);
    assert_eq!(pixel(&f, 0, 0), [255, 0, 0, 255]);
    assert_eq!(pixel(&f, 1, 0), [0, 255, 0, 128]);
}

#[test]
fn a_jpeg_is_full_range_bt601_in_the_decoders_own_layout() {
    // JFIF is full-range BT.601 whatever the picture size; a 720-line-or-taller JPEG read as
    // BT.709 or as limited range shows wrong colours. The compositor converts from these tags.
    let dir = TempDir::new("stills-jfif");
    let blocks = vec![ycbcr([200, 30, 30]); 160 * 90];
    let path = write(&dir, "hd.jpg", &stills::jpeg(160, 90, &blocks, None));
    let info = backend().probe(&path).unwrap();
    assert_eq!(
        (info.streams[0].width, info.streams[0].height),
        (Some(1280), Some(720))
    );
    let mut d = backend()
        .open_decoder(&path, &DecodeOpts::default())
        .unwrap();
    let native = d.seek_exact(Time::ZERO).unwrap();
    let color = if native.color.matrix.is_some() {
        native.color.clone()
    } else {
        info.color.clone()
    };
    assert_eq!(
        native.color.range.or(info.color.range),
        Some(ColorRange::Full)
    );
    assert!(
        matches!(
            color.matrix,
            Some(
                cutweave_model::MatrixCoefficients::Bt470Bg
                    | cutweave_model::MatrixCoefficients::Smpte170M
            )
        ),
        "{color:?} (probe {:?})",
        info.color
    );
    // And the RGBA conversion (thumbnails) agrees.
    let f = rgba(&path);
    assert!(
        close(pixel(&f, 640, 360), [200, 30, 30, 255], 4),
        "{:?}",
        pixel(&f, 640, 360)
    );
}

/// The card turned clockwise by `degrees`: its size and the colour at each pixel.
fn turned(f: &VideoFrame, degrees: u16, x: u32, y: u32) -> [u8; 4] {
    // Display (x, y) of a picture turned clockwise comes from this coded pixel.
    let (cx, cy) = match degrees {
        0 => (x, y),
        90 => (y, f.height - 1 - x),
        180 => (f.width - 1 - x, f.height - 1 - y),
        _ => (f.width - 1 - y, x),
    };
    pixel(f, cx, cy)
}

#[test]
fn a_mirrored_exif_orientation_is_applied_by_the_decoder() {
    // What each EXIF orientation shows at display pixel (x, y), from the card as stored
    // (`s` is the stored picture, W × H): the definitions of the EXIF specification.
    type Shown = fn(u32, u32) -> (u32, u32);
    let cases: [(u16, (u32, u32), Shown); 4] = [
        // 2: mirrored left to right.
        (2, (W, H), |x, y| (W - 1 - x, y)),
        // 4: mirrored top to bottom.
        (4, (W, H), |x, y| (x, H - 1 - y)),
        // 5: transposed (rows become columns).
        (5, (H, W), |x, y| (y, x)),
        // 7: transposed along the other diagonal.
        (7, (H, W), |x, y| (W - 1 - y, H - 1 - x)),
    ];
    let dir = TempDir::new("stills-mirror");
    let backend = backend();
    for (value, (dw, dh), stored) in cases {
        let exif = exif_orientation(value, false);
        let files = [
            (format!("m{value}.jpg"), card_jpeg(Some(&exif)), 4),
            (
                format!("m{value}.png"),
                stills::png(W, H, 8, png_color::RGBA, &card(4, 1), &[(*b"eXIf", &exif)]),
                0,
            ),
        ];
        for (name, bytes, tolerance) in files {
            let path = write(&dir, &name, &bytes);
            let degrees = backend.probe(&path).unwrap().rotation;
            // Both the RGBA conversion and the decoder's own layout (JPEG: Y'CbCr planes).
            let f = rgba(&path);
            for (x, y) in [
                (2, 2),
                (dw - 3, 2),
                (2, dh - 3),
                (dw - 3, dh - 3),
                (dw / 2, 2),
            ] {
                let (sx, sy) = stored(x, y);
                let [r, g, b] = cell_of(sx, sy);
                let got = turned(&f, degrees, x, y);
                assert!(
                    close(got, [r, g, b, 255], tolerance),
                    "{name}: display ({x}, {y}) is {got:?}, expected {:?}",
                    [r, g, b]
                );
            }
            let mut d = backend.open_decoder(&path, &DecodeOpts::default()).unwrap();
            let native = d.seek_exact(Time::ZERO).unwrap();
            native.validate().unwrap();
            assert_eq!((native.width, native.height), (W, H), "{name}");
        }
    }
}

#[test]
fn an_untagged_still_is_probed_as_srgb() {
    use cutweave_model::{ColorPrimaries, MatrixCoefficients, TransferCharacteristics};
    let dir = TempDir::new("stills-srgb");
    let backend = backend();
    let files = [
        (
            "u.png",
            stills::png(W, H, 8, png_color::RGBA, &card(4, 1), &[]),
        ),
        ("u.jpg", card_jpeg(None)),
        ("u.tif", stills::tiff(W, H, 3, 8, &card(3, 1), None)),
        ("u.bmp", stills::bmp(W, H, &card(3, 1))),
        (
            "gray.png",
            stills::png(2, 2, 8, png_color::GRAY, &[0, 128, 255, 64], &[]),
        ),
    ];
    for (name, bytes) in files {
        let color = backend.probe(&write(&dir, name, &bytes)).unwrap().color;
        // Not video's defaults (the BT.709 curve; SD primaries for a picture this small).
        assert_eq!(
            color.transfer,
            Some(TransferCharacteristics::Srgb),
            "{name}"
        );
        assert_eq!(color.primaries, Some(ColorPrimaries::Bt709), "{name}");
        // Y'CbCr stills are BT.601 (JFIF) at any size; the matrix is not used for R'G'B'.
        assert!(
            matches!(
                color.matrix,
                Some(MatrixCoefficients::Bt470Bg | MatrixCoefficients::Smpte170M)
            ) || name != "u.jpg",
            "{name}: {color:?}"
        );
    }
    // A file that says what it is keeps its own tags: a PNG with a gamma of 1.0 is linear, on
    // the frame (which wins over the probe) or on the probe itself.
    let linear = write(
        &dir,
        "linear.png",
        &stills::png(
            W,
            H,
            8,
            png_color::RGBA,
            &card(4, 1),
            &[(*b"gAMA", &100_000_u32.to_be_bytes())],
        ),
    );
    let probed = backend.probe(&linear).unwrap().color;
    let mut d = backend
        .open_decoder(&linear, &DecodeOpts::default())
        .unwrap();
    let frame = d.seek_exact(Time::ZERO).unwrap().color;
    assert_eq!(
        frame.transfer.clone().or_else(|| probed.transfer.clone()),
        Some(TransferCharacteristics::Linear),
        "frame {frame:?}, probe {probed:?}"
    );
}
