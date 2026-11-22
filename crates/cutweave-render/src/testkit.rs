// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared fixtures of the WP-1F.03 tests: a 3-clip project over synthetic sources, analytic
//! expected colours, and decoders for the queries' base64 PNG / JPEG output.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::sync::Arc;

use cutweave_model::{
    Animatable, Clip, Color, ColorPrimaries, ColorRange, ColorTags, Duration, Extra, FrameRate,
    ItemId, MatrixCoefficients, MediaAsset, MediaId, MediaKind, ProbeInfo, Project, SampleRate,
    Sequence, SequenceFormat, SequenceId, StreamInfo, StreamKind, Time, Track, TrackId, TrackKind,
    TransferCharacteristics, Vec2,
};

use crate::color::{OutputTransfer, Transfer, from_linear, quantize8, to_linear};
use crate::frame::PixelFormat;
use crate::image::Image;
use crate::metrics::delta_e_stats;
use crate::testcard::{Pattern, SyntheticSource, TestCard};

/// Clip A (bottom track, 0–4 s).
pub const RED: [f64; 3] = [0.8, 0.1, 0.1];
/// Clip B (bottom track, 4–8 s).
pub const GREEN: [f64; 3] = [0.1, 0.7, 0.2];
/// Clip C (top track, 2–6 s): a half-size picture-in-picture in the top-right quadrant.
pub const BLUE: [f64; 3] = [0.15, 0.2, 0.85];
/// The sequence background.
pub const BACKGROUND: Color = Color::rgb(16, 16, 16);

/// Seconds as a time.
pub fn sec(s: f64) -> Time {
    Time::from_flicks((s * 705_600_000.0).round() as i64).unwrap()
}

/// BT.709 limited-range tags (the cards' encoding).
pub fn bt709() -> ColorTags {
    ColorTags {
        primaries: Some(ColorPrimaries::Bt709),
        transfer: Some(TransferCharacteristics::Bt709),
        matrix: Some(MatrixCoefficients::Bt709),
        range: Some(ColorRange::Limited),
    }
}

/// The 3-clip fixture.
pub struct ThreeClips {
    /// The project.
    pub project: Project,
    /// Its only sequence (`width × height` @ 30 fps).
    pub seq: SequenceId,
    /// Red, bottom track, 0–4 s.
    pub a: ItemId,
    /// Green, bottom track, 4–8 s.
    pub b: ItemId,
    /// Blue picture-in-picture, top track, 2–6 s.
    pub c: ItemId,
    /// The top track.
    pub v2: TrackId,
    /// Solid NV12 cards of the sequence size for the three media.
    pub source: Arc<SyntheticSource>,
}

fn media(project: &mut Project, n: u128, size: (u32, u32)) -> MediaId {
    let id = MediaId::from_u128(n);
    let mut probe = ProbeInfo::default();
    probe.streams.push(StreamInfo {
        index: 0,
        kind: StreamKind::Video,
        codec: None,
        profile: None,
        width: Some(size.0),
        height: Some(size.1),
        pixel_format: None,
        sample_rate: None,
        channels: None,
        language: None,
        extra: Extra::new(),
    });
    probe.duration = Some(Duration::from_secs(60).unwrap());
    project.media.insert(
        id,
        MediaAsset::file(id, MediaKind::Video, format!("/m/{n}"), probe),
    );
    id
}

/// A `width × height` @ 30 fps project: A (red) 0–4 s and B (green) 4–8 s on the bottom track,
/// C (blue) 2–6 s on the top track at half size anchored to the top-right corner.
pub fn three_clips(width: u32, height: u32) -> ThreeClips {
    let seq = SequenceId::from_u128(1);
    let mut format = SequenceFormat::new(width, height, FrameRate::FPS_30, SampleRate::HZ_48_000);
    format.background = BACKGROUND;
    let mut project = Project::new("three clips");
    let mut sequence = Sequence::new(seq, "Main", format);
    let (v1, v2) = (TrackId::from_u128(11), TrackId::from_u128(12));
    sequence.insert_track(0, Track::new(v1, TrackKind::Video, "V1"));
    sequence.insert_track(0, Track::new(v2, TrackKind::Video, "V2"));
    let (ma, mb, mc) = (
        media(&mut project, 21, (width, height)),
        media(&mut project, 22, (width, height)),
        media(&mut project, 23, (width, height)),
    );
    let (a, b, c) = (
        ItemId::from_u128(31),
        ItemId::from_u128(32),
        ItemId::from_u128(33),
    );
    let four = Duration::from_secs(4).unwrap();
    let clip_c = {
        let mut clip = Clip::new(c, mc, sec(2.0), four);
        clip.visual.transform.scale = Animatable::Static(Vec2::new(0.5, 0.5));
        clip.visual.transform.anchor = Animatable::Static(Vec2::new(1.0, 0.0));
        clip.visual.transform.position = Animatable::Static(Vec2::new(0.5, -0.5));
        clip
    };
    {
        let track = sequence.track_mut(&v1).unwrap();
        track.items.insert(Clip::new(a, ma, sec(0.0), four).into());
        track.items.insert(Clip::new(b, mb, sec(4.0), four).into());
        let top = sequence.track_mut(&v2).unwrap();
        top.items.insert(clip_c.into());
    }
    project.sequences.insert(seq, sequence);
    let card = |signal| {
        TestCard::new(
            width,
            height,
            Pattern::Solid(signal),
            PixelFormat::Nv12,
            bt709(),
        )
    };
    let source = SyntheticSource::new()
        .with(ma, card(RED))
        .with(mb, card(GREEN))
        .with(mc, card(BLUE));
    ThreeClips {
        project,
        seq,
        a,
        b,
        c,
        v2,
        source: Arc::new(source),
    }
}

/// The sRGB code a BT.709-encoded R'G'B' `signal` should come out as (analytic: inverse
/// BT.709 OETF to linear, then the sRGB encoding).
pub fn expected_srgb(signal: [f64; 3]) -> [u8; 3] {
    signal.map(|v| quantize8(from_linear(to_linear(v, Transfer::Bt709), Transfer::Srgb)))
}

/// The sequence background as sRGB.
pub fn background_srgb() -> [u8; 3] {
    [BACKGROUND.r, BACKGROUND.g, BACKGROUND.b]
}

/// ΔE2000 between two sRGB colours.
pub fn delta_e(a: [u8; 3], b: [u8; 3]) -> f64 {
    let px = |c: [u8; 3]| Image::filled(1, 1, [c[0], c[1], c[2], 255]);
    delta_e_stats(&px(a), &px(b), OutputTransfer::Srgb).max
}

/// The mean colour of the `w × h` box centred at `(cx, cy)`.
pub fn mean_rgb(img: &Image, cx: u32, cy: u32, w: u32, h: u32) -> [u8; 3] {
    let mut sum = [0u64; 3];
    let mut n = 0u64;
    for y in cy - h / 2..cy + h / 2 {
        for x in cx - w / 2..cx + w / 2 {
            let p = img.pixel(x, y);
            for c in 0..3 {
                sum[c] += u64::from(p[c]);
            }
            n += 1;
        }
    }
    sum.map(|s| ((s + n / 2) / n) as u8)
}

/// Decodes standard padded base64.
pub fn unbase64(s: &str) -> Vec<u8> {
    let val = |c: u8| -> u32 {
        match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => panic!("not base64: {c}"),
        }
    };
    let bytes = s.as_bytes();
    assert_eq!(bytes.len() % 4, 0);
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let pad = chunk.iter().rev().take_while(|&&c| c == b'=').count();
        let mut n = 0u32;
        for &c in &chunk[..4 - pad] {
            n = (n << 6) | val(c);
        }
        n <<= 6 * pad as u32;
        let three = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&three[..3 - pad]);
    }
    out
}

/// Decodes a PNG (RGB or RGBA) to RGBA8.
pub fn decode_png(bytes: &[u8]) -> Image {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    let data = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => buf[..info.buffer_size()]
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect(),
        other => panic!("unexpected PNG colour type {other:?}"),
    };
    Image {
        width: info.width,
        height: info.height,
        data,
    }
}

/// Decodes a JPEG to RGBA8 (with an independent decoder).
pub fn decode_jpeg(bytes: &[u8]) -> Image {
    use zune_jpeg::JpegDecoder;
    use zune_jpeg::zune_core::bytestream::ZCursor;
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGBA)
        .set_strict_mode(true);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    let data = decoder.decode().expect("valid JPEG");
    let info = decoder.info().unwrap();
    Image {
        width: u32::from(info.width),
        height: u32::from(info.height),
        data,
    }
}
