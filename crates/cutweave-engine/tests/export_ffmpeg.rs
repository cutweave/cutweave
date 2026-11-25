// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1E.03 acceptance with real output: a 3-clip 20 s 1080p30 timeline (`FakeBackend` clips:
//! 1080p30 4:2:0, 720p30 NV12 scaled up, 1080p25 with 44.1 kHz audio; their pictures repainted
//! with smooth moving content, see [`Canvas`]) exported through the engine (`export.start`) on a
//! headless wgpu device with the FFmpeg encoder (`hw: "auto"`, so a hardware encoder where the
//! machine has one):
//!
//! - the MP4 is clean for `ffprobe` / `ffmpeg`, holds exactly 600 frames at 30/1 fps, lasts the
//!   timeline ±1 frame, and has its AAC track;
//! - **preview matches export**: at 5 timestamps the frame the pipeline hands to the encoder is
//!   bit-identical to `render.frame` (registry query, PNG) on the same adapter;
//! - the frame at t = 5 s decoded from the MP4 is within PSNR ≥ 38 dB of `render.frame`;
//! - cancelling at 50 % ends the job `Cancelled` within a second and leaves no file;
//! - two exports of one snapshot in software (`hw: "off"`) with one x264 thread decode to
//!   identical frames (`ffmpeg -f framemd5`);
//! - a hardware encoder failing mid-export (a pretend one, so on any machine) makes the export
//!   start over with x264 and leaves one clean file;
//! - (`#[ignore]`d, `CUTWEAVE_HW_TESTS=1`) each hardware encoder of the machine exports the
//!   timeline at ≥ 2× realtime (arch §19) within the same PSNR allowance.
//!
//! Needs the FFmpeg bundle (`. scripts/ffmpeg/env.sh`; `ffmpeg`/`ffprobe` from
//! `$FFMPEG_DIR/bin`) and a wgpu adapter (`CUTWEAVE_GPU` picks one; default: the best). Without
//! an adapter the GPU tests skip with a message unless `CUTWEAVE_GPU_REQUIRED=1`.
#![cfg(feature = "ffmpeg")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod export_support;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, Instant};

use cutweave_commands::{Access, QueryCtx, Registry};
use cutweave_engine::export::{DecoderSources, ExportServices};
use cutweave_engine::{Connection, Engine};
use cutweave_jobs::JobState;
use cutweave_media::testing::{FakeBackend, fake_frame_number};
use cutweave_media::{
    AudioDecoder, Capabilities, DecodeOpts, EncodeSpec, Encoder, FfmpegBackend, FrameIndex,
    MediaBackend, MediaError, PixelFormat, VideoDecoder,
};
use cutweave_model::{FrameRate, ProbeInfo, Project, SampleRate};
use cutweave_render::Image;
use cutweave_render::gpu::{
    AdapterChoice, Compositor, CompositorOptions, GpuContext, GpuFrameBackend,
};
use cutweave_render::query::SharedBackend;
use export_support::{
    LateSource, PSEUDO_HW, ProbeRenderer, PseudoHw, SEC, TempDir, agent, assert_succeeded, clip,
    engine, insert, job, must, new_project, register, wait, wait_until,
};
use serde_json::{Value, json};

const WAIT: StdDuration = StdDuration::from_secs(600);
/// Frames (at 30 fps) whose pixels are compared with `render.frame`: 0.5 s, 5 s, the first
/// frame of the second clip (7 s), 12.3 s and the last frame (19.9667 s).
const CHECKED: [u32; 5] = [15, 150, 210, 369, 599];

// ----- pictures --------------------------------------------------------------------------------

/// Extra width of a painted canvas: frames slide across it.
const SPAN: usize = 512;
/// Pixels a painted picture moves per frame (even, so 4:2:0 chroma moves with it).
const SLIDE: usize = 4;

/// A smooth picture in limited-range Y'CbCr 4:2:0, wider than the frame by [`SPAN`]: soft
/// gradients, a mid-frequency diagonal texture and slowly varying chroma, the way camera footage
/// compresses. `FakeBackend`'s own sawtooth pattern has hard chroma edges that 4:2:0 cannot
/// carry: a 4:2:0 round trip of it without any H.264 already measures R 37.0 / G 44.3 /
/// B 34.2 dB, so it cannot show the encoder's loss allowance.
struct Canvas {
    width: usize,
    y: Vec<u8>,
    cb: Vec<u8>,
    cr: Vec<u8>,
}

impl Canvas {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn new((width, height): (u32, u32), phase: f64) -> Canvas {
        use std::f64::consts::TAU;
        let (cols, rows) = (width as usize + SPAN, height as usize);
        let code = |v: f64, lo: f64, span: f64| (lo + v.clamp(0.0, 1.0) * span).round() as u8;
        // Separable tables keep this cheap in an unoptimized test build.
        let fx: Vec<f64> = (0..cols)
            .map(|x| 0.2 * (TAU * x as f64 / 700.0 + phase).sin())
            .collect();
        let fy: Vec<f64> = (0..rows)
            .map(|y| 0.15 * (TAU * y as f64 / 450.0).cos())
            .collect();
        let fd: Vec<f64> = (0..cols + 2 * rows)
            .map(|d| 0.06 * (TAU * d as f64 / 37.0).sin())
            .collect();
        let mut luma = Vec::with_capacity(cols * rows);
        for row in 0..rows {
            for col in 0..cols {
                luma.push(code(
                    0.5 + fx[col] + fy[row] + fd[col + 2 * row],
                    16.0,
                    219.0,
                ));
            }
        }
        let (cw, ch) = (cols / 2, rows.div_ceil(2));
        let mut cb = Vec::with_capacity(cw * ch);
        let mut cr = Vec::with_capacity(cw * ch);
        for row in 0..ch {
            for col in 0..cw {
                let blue = 0.5 + 0.25 * (TAU * col as f64 / 450.0 + phase).sin();
                let red = 0.5 + 0.25 * (TAU * row as f64 / 300.0 - phase).cos();
                cb.push(code(blue, 16.0, 224.0));
                cr.push(code(red, 16.0, 224.0));
            }
        }
        Canvas {
            width: cols,
            y: luma,
            cb,
            cr,
        }
    }

    /// Overwrites the frame's planes with the canvas slid by its frame number.
    fn paint(&self, frame: &mut cutweave_media::VideoFrame) {
        let n = fake_frame_number(frame).unwrap_or(0) as usize;
        let shift = (n * SLIDE) % SPAN;
        let (fw, fh) = (frame.width as usize, frame.height as usize);
        let (cw, ch) = (fw.div_ceil(2), fh.div_ceil(2));
        let copy = |plane: &mut cutweave_media::Plane,
                    src: &[u8],
                    src_w: usize,
                    w: usize,
                    rows: usize,
                    at: usize| {
            let stride = plane.stride as usize;
            for r in 0..rows {
                plane.data[r * stride..r * stride + w]
                    .copy_from_slice(&src[r * src_w + at..r * src_w + at + w]);
            }
        };
        let half = self.width / 2;
        match frame.format {
            PixelFormat::Yuv420p => {
                copy(&mut frame.planes[0], &self.y, self.width, fw, fh, shift);
                copy(&mut frame.planes[1], &self.cb, half, cw, ch, shift / 2);
                copy(&mut frame.planes[2], &self.cr, half, cw, ch, shift / 2);
            }
            PixelFormat::Nv12 => {
                copy(&mut frame.planes[0], &self.y, self.width, fw, fh, shift);
                let plane = &mut frame.planes[1];
                let stride = plane.stride as usize;
                for r in 0..ch {
                    for c in 0..cw {
                        let at = r * half + shift / 2 + c;
                        plane.data[r * stride + 2 * c] = self.cb[at];
                        plane.data[r * stride + 2 * c + 1] = self.cr[at];
                    }
                }
            }
            other => panic!("the painter does not paint {other:?}"),
        }
    }
}

/// `FakeBackend` (timestamps, frame selection, layouts) with its pictures repainted from a
/// [`Canvas`] per file.
struct Painted {
    inner: FakeBackend,
    canvases: HashMap<PathBuf, Arc<Canvas>>,
}

impl Painted {
    fn new<'a>(
        inner: FakeBackend,
        files: impl IntoIterator<Item = (&'a String, (u32, u32), f64)>,
    ) -> Painted {
        let canvases = files
            .into_iter()
            .map(|(path, size, phase)| (PathBuf::from(path), Arc::new(Canvas::new(size, phase))))
            .collect();
        Painted { inner, canvases }
    }
}

struct PaintedDecoder {
    inner: Box<dyn VideoDecoder>,
    canvas: Option<Arc<Canvas>>,
}

impl PaintedDecoder {
    fn painted(&self, mut frame: cutweave_media::VideoFrame) -> cutweave_media::VideoFrame {
        if let Some(canvas) = &self.canvas {
            canvas.paint(&mut frame);
        }
        frame
    }
}

impl VideoDecoder for PaintedDecoder {
    fn next_frame(&mut self) -> Result<Option<cutweave_media::VideoFrame>, MediaError> {
        Ok(self.inner.next_frame()?.map(|f| self.painted(f)))
    }

    fn seek_exact(
        &mut self,
        t: cutweave_model::Time,
    ) -> Result<cutweave_media::VideoFrame, MediaError> {
        let frame = self.inner.seek_exact(t)?;
        Ok(self.painted(frame))
    }
    fn index(&self) -> std::sync::Arc<cutweave_media::FrameIndex> {
        self.inner.index()
    }

    fn set_cancel(&mut self, cancel: Option<cutweave_media::CancelToken>) {
        self.inner.set_cancel(cancel);
    }
}

impl MediaBackend for Painted {
    fn probe(&self, path: &Path) -> Result<ProbeInfo, MediaError> {
        self.inner.probe(path)
    }

    fn index_media(&self, path: &Path) -> Result<FrameIndex, MediaError> {
        self.inner.index_media(path)
    }

    fn open_decoder(
        &self,
        path: &Path,
        opts: &DecodeOpts,
    ) -> Result<Box<dyn VideoDecoder>, MediaError> {
        Ok(Box::new(PaintedDecoder {
            inner: self.inner.open_decoder(path, opts)?,
            canvas: self.canvases.get(path).cloned(),
        }))
    }

    fn open_audio_decoder(
        &self,
        path: &Path,
        stream: Option<u32>,
    ) -> Result<Box<dyn AudioDecoder>, MediaError> {
        self.inner.open_audio_decoder(path, stream)
    }

    fn open_encoder(&self, spec: &EncodeSpec) -> Result<Box<dyn Encoder>, MediaError> {
        self.inner.open_encoder(spec)
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
}

// ----- tools -----------------------------------------------------------------------------------

/// `ffprobe` or `ffmpeg` from the bundle the tests link against.
fn tool(name: &str) -> PathBuf {
    if let Ok(dir) = std::env::var("FFMPEG_DIR") {
        let exe = Path::new(&dir)
            .join("bin")
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        if exe.is_file() {
            return exe;
        }
    }
    PathBuf::from(name)
}

/// A compositor on the test adapter, or `None` (skip).
fn compositor() -> Option<Compositor> {
    let choice = AdapterChoice::from_env_or(AdapterChoice::Auto);
    match GpuContext::new(&choice).and_then(|c| Compositor::new(c, CompositorOptions::default())) {
        Ok(c) => {
            eprintln!(
                "adapter: {} ({})",
                c.context().info().key,
                c.context().info().driver
            );
            Some(c)
        }
        Err(e) if std::env::var("CUTWEAVE_GPU_REQUIRED").as_deref() != Ok("1") => {
            eprintln!("skipping GPU export test: {e}");
            None
        }
        Err(e) => panic!("GPU required: {e}"),
    }
}

/// An engine exporting the 20 s timeline with the GPU renderer and the FFmpeg encoder.
struct Rig {
    dir: TempDir,
    engine: Engine,
    conn: Connection,
    renderer: SharedBackend,
    /// Frames the tap kept, by index.
    tapped: Arc<Mutex<BTreeMap<u32, Image>>>,
}

impl Rig {
    fn new(tag: &str, encoder_threads: u16) -> Option<Rig> {
        let compositor = compositor()?;
        let dir = TempDir::new(tag);
        let paths = [dir.file("a.mp4"), dir.file("b.mp4"), dir.file("c.mp4")];
        let fake = FakeBackend::new()
            .with_media(
                &paths[0],
                clip(
                    8,
                    (1920, 1080),
                    FrameRate::FPS_30,
                    PixelFormat::Yuv420p,
                    440.0,
                    SampleRate::HZ_48_000,
                ),
            )
            .with_media(
                &paths[1],
                clip(
                    8,
                    (1280, 720),
                    FrameRate::FPS_30,
                    PixelFormat::Nv12,
                    660.0,
                    SampleRate::HZ_48_000,
                ),
            )
            .with_media(
                &paths[2],
                clip(
                    7,
                    (1920, 1080),
                    FrameRate::FPS_25,
                    PixelFormat::Yuv420p,
                    880.0,
                    SampleRate::HZ_44_100,
                ),
            );
        let decode: Arc<dyn MediaBackend> = Arc::new(Painted::new(
            fake.clone(),
            [
                (&paths[0], (1920, 1080), 0.0),
                (&paths[1], (1280, 720), 2.0),
                (&paths[2], (1920, 1080), 4.0),
            ],
        ));
        // `render.frame`'s own frame source: decoders separate from the export's.
        let preview = Arc::new(LateSource::default());
        let renderer = GpuFrameBackend::new(compositor, Arc::clone(&preview) as _).shared();
        let tapped: Arc<Mutex<BTreeMap<u32, Image>>> = Arc::default();
        let keep = Arc::clone(&tapped);
        let services = ExportServices::new(
            Arc::new(FfmpegBackend::new().expect("the loaded FFmpeg matches the build")),
            Arc::new(DecoderSources::new(Arc::clone(&decode))),
            Arc::clone(&renderer),
        )
        .with_encoder_threads(encoder_threads)
        .with_frame_tap(Arc::new(move |index, _, image: &Image| {
            if CHECKED.contains(&index) {
                keep.lock().unwrap().insert(index, image.clone());
            }
        }));
        let engine = engine(dir.path(), Some(services));
        let conn = agent(&engine);
        let (seq, video, _) = new_project(&conn);
        let ids: Vec<_> = paths.iter().map(|p| register(&conn, &fake, p)).collect();
        insert(&conn, &seq, &video, ids[0], "0s", "7s", "0s");
        insert(&conn, &seq, &video, ids[1], "7s", "7s", "0.5s");
        insert(&conn, &seq, &video, ids[2], "14s", "6s", "0s");
        preview.bind(decode, &Rig::doc_of(&engine));
        Some(Rig {
            dir,
            engine,
            conn,
            renderer,
            tapped,
        })
    }

    fn doc_of(engine: &Engine) -> Project {
        engine
            .handle()
            .doc()
            .unwrap()
            .committed_snapshot()
            .doc
            .clone()
    }

    /// `render.frame` at frame `index` (30 fps) through the registry, full size, as PNG.
    fn render_frame(&self, index: u32) -> Image {
        let mut registry = Registry::new();
        cutweave_render::register(&mut registry).unwrap();
        let doc = Rig::doc_of(&self.engine);
        let _scope = cutweave_render::query::enter(Arc::clone(&self.renderer));
        let ctx = QueryCtx::new(&doc, Access::STANDARD);
        let t = FrameRate::FPS_30.frame_to_time(i64::from(index)).unwrap();
        let out = registry
            .query_json(
                "render.frame",
                json!({ "t": t.flicks(), "max_size": 4096, "format": "png" }),
                &ctx,
            )
            .unwrap();
        assert_eq!(
            (out["width"].clone(), out["height"].clone()),
            (json!(1920), json!(1080))
        );
        decode_png(&unbase64(out["data"].as_str().unwrap()))
    }

    fn export(&self, params: Value) -> Value {
        must(&self.conn, "export.start", params)
    }

    /// PSNR (dB) of frame `n` decoded from `out` against `render.frame` at that frame.
    fn psnr_at(&self, out: &Path, n: u32) -> f64 {
        psnr(&decoded_rgb(out, n), &rgb(&self.render_frame(n)))
    }

    fn close(self) {
        drop(self.conn);
        self.engine.shutdown();
    }
}

fn unbase64(s: &str) -> Vec<u8> {
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
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for chunk in s.as_bytes().chunks(4) {
        let pad = chunk.iter().rev().take_while(|&&c| c == b'=').count();
        let mut n = 0u32;
        for &c in &chunk[..4 - pad] {
            n = (n << 6) | val(c);
        }
        n <<= 6 * u32::try_from(pad).unwrap();
        out.extend_from_slice(&n.to_be_bytes()[1..4 - pad]);
    }
    out
}

/// Decodes a PNG (RGB or RGBA) to RGBA8.
fn decode_png(bytes: &[u8]) -> Image {
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

/// `ffprobe -show_format -show_streams` as flat `key → value` pairs.
fn probe(path: &Path, extra: &[&str]) -> BTreeMap<String, String> {
    let out = Command::new(tool("ffprobe"))
        .args([
            "-v",
            "error",
            "-show_format",
            "-show_streams",
            "-of",
            "flat=s=_",
        ])
        .args(extra)
        .arg(path)
        .output()
        .expect("run ffprobe (source scripts/ffmpeg/env.sh)");
    assert!(
        out.status.success() && out.stderr.is_empty(),
        "ffprobe: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.trim_matches('"').to_owned()))
        .collect()
}

fn get<'a>(info: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    info.get(key).map_or_else(
        || panic!("ffprobe has no `{key}`: {info:#?}"),
        String::as_str,
    )
}

/// Runs `ffmpeg -v error <args>`; returns stdout, asserting success and an empty stderr.
fn ffmpeg(args: &[&str]) -> Vec<u8> {
    let out = Command::new(tool("ffmpeg"))
        .args(["-v", "error", "-nostdin"])
        .args(args)
        .output()
        .expect("run ffmpeg");
    assert!(
        out.status.success() && out.stderr.is_empty(),
        "ffmpeg {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

/// Frame `n` of the MP4 as RGB24, decoded with the BT.709 limited-range matrix the export
/// writes. `setparams` pins the frame's colour tags first: FFmpeg 8 negotiates colour spaces in
/// filter graphs, so a stream whose tags differ from what the graph expects (hardware encoders
/// and x264 tag alike, but the comparison must not depend on it) would be converted again and
/// lose about 25 dB (docs/reference/export-quality.md). The reference side is `render.frame`'s
/// PNG, decoded in Rust.
fn decoded_rgb(path: &Path, n: u32) -> Vec<u8> {
    let filter = format!(
        "select=eq(n\\,{n}),setparams=range=tv:colorspace=bt709:color_primaries=bt709:\
         color_trc=bt709,scale=in_color_matrix=bt709:in_range=tv:out_range=pc:\
         flags=accurate_rnd+full_chroma_int,format=rgb24"
    );
    ffmpeg(&[
        "-i",
        &path.to_string_lossy(),
        "-vf",
        &filter,
        "-frames:v",
        "1",
        "-f",
        "rawvideo",
        "-",
    ])
}

/// PSNR in dB of two equally long 8-bit buffers.
#[allow(clippy::cast_precision_loss)]
fn psnr(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    let sse: u64 = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| u64::from(x.abs_diff(y)).pow(2))
        .sum();
    if sse == 0 {
        return f64::INFINITY;
    }
    let mse = sse as f64 / a.len() as f64;
    10.0 * (255.0 * 255.0 / mse).log10()
}

fn rgb(image: &Image) -> Vec<u8> {
    image
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect()
}

#[test]
fn a_three_clip_1080p_export_is_a_clean_mp4_that_matches_render_frame() {
    let Some(rig) = Rig::new("e2e", 0) else {
        return;
    };
    let path = rig.dir.file("timeline.mp4");
    let started_at = Instant::now();
    let started = rig.export(json!({ "path": path }));
    assert_eq!(
        (
            started["width"].clone(),
            started["height"].clone(),
            started["frames"].clone()
        ),
        (json!(1920), json!(1080), json!(600))
    );
    let info = wait(&rig.conn, &started["job_id"], WAIT);
    assert_succeeded(&info);
    let elapsed = started_at.elapsed().as_secs_f64();
    println!(
        "exported 20 s of 1080p30 in {elapsed:.2} s: {:.1} fps, {:.2}x realtime ({})",
        600.0 / elapsed,
        20.0 / elapsed,
        info.progress.message.as_deref().unwrap_or_default()
    );

    // Preview matches export: the frames handed to the encoder equal `render.frame`.
    let tapped = rig.tapped.lock().unwrap().clone();
    assert_eq!(tapped.keys().copied().collect::<Vec<_>>(), CHECKED);
    for (&index, image) in &tapped {
        let frame = rig.render_frame(index);
        let same = frame == *image;
        println!("frame {index}: export == render.frame: {same}");
        assert!(same, "frame {index} differs from render.frame");
    }

    // ffprobe: H.264 1920x1080 at exactly 30/1, 600 frames, 20 s ±1 frame, AAC stereo 48 kHz.
    let out = Path::new(&path);
    assert_clean_mp4(out, (1920, 1080), 600, 20.0);

    // The decoded frame at t = 5 s is within the H.264 loss allowance of `render.frame`.
    let db = rig.psnr_at(out, 150);
    println!("PSNR at t = 5 s (frame 150): {db:.2} dB");
    assert!(db >= 38.0, "PSNR {db:.2} dB < 38 dB");
    rig.close();
}

/// ffprobe: H.264 `size` at exactly 30/1 with `frames` frames, `seconds` ±1 frame long, AAC
/// stereo 48 kHz, BT.709; and `ffmpeg` decodes it without a complaint.
fn assert_clean_mp4(out: &Path, size: (u32, u32), frames: u32, seconds: f64) {
    let info = probe(out, &["-count_frames"]);
    let (v, a) = if get(&info, "streams_stream_0_codec_type") == "video" {
        ("streams_stream_0_", "streams_stream_1_")
    } else {
        ("streams_stream_1_", "streams_stream_0_")
    };
    let key = |prefix: &str, k: &str| format!("{prefix}{k}");
    println!(
        "ffprobe: video {} {}x{} {} fps, {} frames; audio {} {} Hz x{}; duration {} s",
        get(&info, &key(v, "codec_name")),
        get(&info, &key(v, "width")),
        get(&info, &key(v, "height")),
        get(&info, &key(v, "r_frame_rate")),
        get(&info, &key(v, "nb_read_frames")),
        get(&info, &key(a, "codec_name")),
        get(&info, &key(a, "sample_rate")),
        get(&info, &key(a, "channels")),
        get(&info, "format_duration"),
    );
    assert_eq!(get(&info, "format_nb_streams"), "2");
    assert_eq!(get(&info, &key(v, "codec_name")), "h264");
    assert_eq!(get(&info, &key(v, "width")), size.0.to_string());
    assert_eq!(get(&info, &key(v, "height")), size.1.to_string());
    assert_eq!(get(&info, &key(v, "r_frame_rate")), "30/1");
    assert_eq!(get(&info, &key(v, "nb_read_frames")), frames.to_string());
    assert_eq!(get(&info, &key(v, "color_space")), "bt709");
    assert_eq!(get(&info, &key(a, "codec_name")), "aac");
    assert_eq!(get(&info, &key(a, "sample_rate")), "48000");
    assert_eq!(get(&info, &key(a, "channels")), "2");
    let duration: f64 = get(&info, "format_duration").parse().unwrap();
    assert!(
        (duration - seconds).abs() <= 1.0 / 30.0,
        "duration {duration} s is not {seconds} s ±1 frame"
    );
    ffmpeg(&["-i", &out.to_string_lossy(), "-f", "null", "-"]);
}

/// The hardware H.264 encoders FFmpeg's probe found here, in `auto` order.
fn hardware_encoders() -> Vec<String> {
    FfmpegBackend::new()
        .expect("the loaded FFmpeg matches the build")
        .capabilities()
        .hw_encode
        .into_iter()
        .filter(|c| c.codec == "h264")
        .map(|c| c.implementation)
        .collect()
}

/// WP-1E.02 with export (arch §19: ≥ 2× realtime with hardware encoding): the 20 s 1080p30
/// timeline through each hardware encoder this machine has, forced, then `auto`. Reference
/// machines only: `CUTWEAVE_HW_TESTS=1 … --run-ignored all`.
#[test]
#[ignore = "needs a hardware H.264 encoder; run with CUTWEAVE_HW_TESTS=1 and --run-ignored all"]
fn hardware_exports_run_at_least_twice_realtime() {
    if std::env::var("CUTWEAVE_HW_TESTS").as_deref() != Ok("1") {
        eprintln!("skipping: set CUTWEAVE_HW_TESTS=1 on a machine with a hardware encoder");
        return;
    }
    let encoders = hardware_encoders();
    println!("hardware encoders here: {encoders:?}");
    assert!(
        !encoders.is_empty(),
        "no hardware H.264 encoder passed the probe"
    );
    let Some(rig) = Rig::new("hw-speed", 0) else {
        return;
    };
    let runs = encoders
        .iter()
        .map(|name| (name.clone(), json!({ "force": name })))
        .chain([("auto".to_owned(), json!("auto"))]);
    for (label, hw) in runs {
        let path = rig.dir.file(&format!("hw-{label}.mp4"));
        let started_at = Instant::now();
        let started = rig.export(json!({ "path": path, "hw": hw }));
        let info = wait(&rig.conn, &started["job_id"], WAIT);
        assert_succeeded(&info);
        let elapsed = started_at.elapsed().as_secs_f64();
        let message = info.progress.message.unwrap_or_default();
        let realtime = 20.0 / elapsed;
        let db = rig.psnr_at(Path::new(&path), 150);
        println!(
            "{label}: 20 s of 1080p30 in {elapsed:.2} s: {:.1} fps, {realtime:.2}x realtime, \
             PSNR at 5 s {db:.2} dB ({message})",
            600.0 / elapsed
        );
        let used = if label == "auto" {
            &encoders[0]
        } else {
            &label
        };
        assert!(message.ends_with(&format!("with {used}")), "{message}");
        assert_clean_mp4(Path::new(&path), (1920, 1080), 600, 20.0);
        assert!(db >= 38.0, "{label}: PSNR {db:.2} dB < 38 dB");
        assert!(
            realtime >= 2.0,
            "{label}: {realtime:.2}x realtime is below the 2x export budget"
        );
    }
    rig.close();
}

/// A hardware encoder failing mid-export, on any machine: the pretend hardware encoder fails
/// after 45 frames, the export starts over with x264 from frame 0, and exactly one clean file
/// remains (no partial file of the failed attempt).
#[test]
fn a_hardware_failure_mid_export_leaves_one_clean_software_file() {
    let dir = TempDir::new("hw-fallback");
    let paths = [dir.file("a.mp4"), dir.file("b.mp4"), dir.file("c.mp4")];
    let fake = FakeBackend::new()
        .with_media(
            &paths[0],
            clip(
                3,
                (320, 180),
                FrameRate::FPS_30,
                PixelFormat::Yuv420p,
                440.0,
                SampleRate::HZ_48_000,
            ),
        )
        .with_media(
            &paths[1],
            clip(
                3,
                (320, 180),
                FrameRate::FPS_30,
                PixelFormat::Nv12,
                660.0,
                SampleRate::HZ_48_000,
            ),
        );
    let pseudo = PseudoHw::new(
        FfmpegBackend::new().expect("the loaded FFmpeg matches the build"),
        Some(45),
    );
    let services = ExportServices::new(
        Arc::new(pseudo.clone()),
        Arc::new(DecoderSources::new(Arc::new(fake.clone()))),
        ProbeRenderer::new(Arc::new(LateSource::default()) as _).shared(),
    );
    let engine = engine(dir.path(), Some(services));
    let conn = agent(&engine);
    let (seq, video, _) = new_project(&conn);
    let ids: Vec<_> = paths[..2]
        .iter()
        .map(|p| register(&conn, &fake, p))
        .collect();
    insert(&conn, &seq, &video, ids[0], "0s", "2s", "0s");
    insert(&conn, &seq, &video, ids[1], "2s", "1s", "1s");
    let path = dir.file("out.mp4");
    let started = must(
        &conn,
        "export.start",
        json!({ "path": path, "preset": {"custom": {"width": 320, "height": 180}} }),
    );
    let info = wait(&conn, &started["job_id"], WAIT);
    assert_succeeded(&info);
    let message = info.progress.message.unwrap_or_default();
    println!("{message}");
    assert_eq!(
        message,
        format!(
            "exported 90 frames (3.00 s) with libx264 ({PSEUDO_HW} failed during the export, so \
             it was encoded again in software)"
        )
    );
    assert_eq!(
        pseudo.opened(),
        [
            cutweave_media::HwEncode::Auto,
            cutweave_media::HwEncode::Off
        ]
    );
    assert_eq!(dir.entries(), ["out.mp4"], "only the finished file remains");
    assert_clean_mp4(Path::new(&path), (320, 180), 90, 3.0);
    drop(conn);
    engine.shutdown();
}

#[test]
fn cancelling_an_export_at_half_leaves_no_file() {
    let Some(rig) = Rig::new("cancel", 0) else {
        return;
    };
    // The 1 s budget is how fast a cancel is honoured, and contention only inflates it: a real
    // regression (an encoder that ignores the token) is slow on every attempt, a descheduled
    // thread on one. So the correctness (cancelled, no file, no partial) is asserted on every
    // attempt and the budget on the best of up to three.
    const BUDGET: StdDuration = StdDuration::from_secs(1);
    let mut best = StdDuration::MAX;
    let mut tries = 0;
    while tries < 3 && best >= BUDGET {
        tries += 1;
        let path = rig.dir.file(&format!("cancelled-{tries}.mp4"));
        let started = rig.export(json!({ "path": path }));
        let id = started["job_id"].clone();
        let half = wait_until(&rig.conn, &id, WAIT, |j| j.progress.fraction >= 0.5);
        let asked = Instant::now();
        job(&rig.conn, "job.cancel", json!({ "id": id }));
        let done = wait(&rig.conn, &id, WAIT);
        let took = asked.elapsed();
        println!(
            "cancelled at {:.0}% ({}); stopped in {took:?} (attempt {tries})",
            half.progress.fraction * 100.0,
            half.progress.message.as_deref().unwrap_or_default()
        );
        assert_eq!(done.state, JobState::Cancelled, "{done:?}");
        assert!(
            !Path::new(&path).exists(),
            "the cancelled export left its file"
        );
        assert_eq!(
            rig.dir.entries(),
            Vec::<String>::new(),
            "no partial file is left"
        );
        best = best.min(took);
    }
    assert!(
        best < BUDGET,
        "the export took {best:?} to stop (best of {tries})"
    );
    rig.close();
}

#[test]
fn two_exports_of_one_snapshot_decode_to_identical_frames() {
    let Some(rig) = Rig::new("determinism", 1) else {
        return;
    };
    let range = json!({ "in": "4s", "out": "9s" });
    let mut hashes = Vec::new();
    let mut revs = Vec::new();
    for name in ["first.mp4", "second.mp4"] {
        let path = rig.dir.file(name);
        // x264 with one thread (the rig's services): identical output run to run. Hardware
        // encoders promise no such thing, so `hw` is off explicitly.
        let started = rig.export(json!({ "path": path, "range": range, "hw": "off" }));
        assert_eq!(started["frames"], json!(150));
        assert_eq!(started["in"], json!(4 * SEC));
        revs.push(started["rev"].clone());
        assert_succeeded(&wait(&rig.conn, &started["job_id"], WAIT));
        // Decoded video and audio, frame by frame.
        let md5 = ffmpeg(&["-i", &path, "-map", "0", "-f", "framemd5", "-"]);
        let text = String::from_utf8(md5).unwrap();
        let frames = text.lines().filter(|l| !l.starts_with('#')).count();
        println!("{name}: {frames} decoded video+audio frames hashed");
        assert!(frames > 150);
        hashes.push(text);
    }
    assert_eq!(revs[0], revs[1], "both exports rendered the same snapshot");
    assert_eq!(
        hashes[0], hashes[1],
        "the two exports decode to different frames"
    );
    let same_bytes = std::fs::read(rig.dir.file("first.mp4")).unwrap()
        == std::fs::read(rig.dir.file("second.mp4")).unwrap();
    println!("files byte-identical: {same_bytes}");
    rig.close();
}
