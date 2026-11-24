# Export quality and hardware encoders

How `cutweave-media` turns one quality setting and one speed preset into settings for x264 and
for each hardware H.264 encoder, which encoder an export uses, and the measurements behind the
mapping (WP-1E.02). Architecture: [§7](../01-architecture.md#7-media-pipeline-cutweave-media-cutweave-cache)
(encode/export) and [§19](../01-architecture.md#19-performance-budgets-ci-benchmarked-on-reference-machines)
(export budget); the backend decision is [ADR-0003](../adr/0003-media-backend-ffmpeg.md).

## Which encoder

`EncodeSpec.hw` decides:

| Policy | Video encoder |
|---|---|
| `Auto` (default) | The first hardware encoder of this OS's list that passed the probe and opens for the stream; x264 if none does. An encoder that fails to open is logged and skipped. |
| `Off` | x264. Its output is identical run to run for a given thread count (the export determinism test uses `Off` with one thread). |
| `Force(name)` | That hardware encoder (`h264_nvenc`, `h264_amf`, `h264_qsv`, `h264_videotoolbox`, `h264_vaapi`) and nothing else: when it cannot open, or fails later, the encode fails with `HwUnavailable`. |

| OS | Order (`Auto`, and of `Capabilities::hw_encode`) |
|---|---|
| Windows | NVENC, AMF, QSV |
| macOS | VideoToolbox |
| Linux | NVENC, QSV, VA-API |

- **Probe.** The first `capabilities()` call (or the first `Auto` encode) tries a 256×256 NV12
  encode of three frames with every encoder of the list that the FFmpeg build has. Those that
  return a packet are `hw_encode`; the result is kept for the process. A machine without a GPU
  gets an empty list, and every encode is x264.
- **Failure after opening.** A hardware encoder that fails while frames are pushed or while
  `finish` flushes ends the encode with `HwUnavailable` and removes the partial file, like any
  failure. `encode_with_fallback` (what the export job uses) then logs it and encodes everything
  again from the first frame with `Off`. A cancel is never retried.
- **Devices.** Never addressed by adapter index (S3 finding H7): NVENC opens the CUDA device,
  AMF and QSV find their vendor's adapter themselves, and VA-API uses the default render node.
- **Input.** Hardware encoders take NV12; RGBA frames are converted by swscale (BT.709, limited
  range, slice-threaded from 720p up), YUV 4:2:0 frames are re-laid out. VA-API takes only GPU
  surfaces, so its frames are uploaded into a frames pool first.
- **Limits.** H.264 hardware encoders stop at 4096 pixels wide, and AMF refuses frames smaller
  than about 192×128 here: `Auto` uses x264 for those. `Crf(0)` is lossless only in x264.
- **Not used.** `h264_mf` (Media Foundation) runs a vendor's encoder MFT, here AMD's, with a
  quality scale each vendor defines. `h264_d3d12va` and `h264_vulkan` take only GPU surfaces;
  on the reference machine D3D12VA's H.264 is damaged on the Radeon and refused on the RTX, and
  Vulkan finds no encode queue on the default adapter. The tests use D3D12VA only to exercise
  the VA-API upload path on Windows.

## The quality mapping

One setting, `RateControl::Crf(c)` (default 20, lower is better), becomes each encoder's
constant-quality mode, offset so that its PSNR matches x264 at the same CRF.
`RateControl::Bitrate(b)` becomes each encoder's average or peak-constrained VBR.

| Encoder | `Crf(c)` becomes | `Bitrate(b)` becomes | Offset status |
|---|---|---|---|
| x264 (`libx264`) | `crf=c` | ABR `b` | Reference |
| NVENC (`h264_nvenc`) | `rc=vbr cq=c+6 b=0`, `tune=hq` | `rc=vbr b` | Measured |
| AMF (`h264_amf`) | `rc=cqp qp_i=qp_p=qp_b=c+1`, `usage=transcoding` | `rc=vbr_peak b maxrate=2b` | Measured |
| QSV (`h264_qsv`) | ICQ `global_quality=c+2` | `b` (AVBR/VBR) | Estimate |
| VideoToolbox | constant quality `q:v = 100 − 2c` | `b` | Estimate |
| VA-API (`h264_vaapi`) | `rc_mode=CQP qp=c+1` | `rc_mode=VBR b maxrate=2b` | Estimate |

Values are kept on each scale (1–51; VideoToolbox 1–100). Every hardware encoder writes H.264
High with BT.709 limited-range VUI, like x264. QSV, VideoToolbox and VA-API could not be measured
on the reference machine, which has no Intel GPU, Mac or Linux install. VideoToolbox's constant
quality needs Apple Silicon: on an Intel Mac the probe fails and exports use x264.

| `SpeedPreset` | x264 | NVENC | AMF `quality` | QSV | VA-API `quality` | VideoToolbox |
|---|---|---|---|---|---|---|
| Ultrafast | ultrafast | p1 | speed | veryfast | 7 | `prio_speed=1` |
| Superfast | superfast | p2 | speed | veryfast | 7 | `prio_speed=1` |
| Veryfast | veryfast | p3 | speed | veryfast | 6 | `prio_speed=1` |
| Faster | faster | p3 | speed | faster | 5 | `prio_speed=1` |
| Fast | fast | p4 | balanced | fast | 5 | `prio_speed=0` |
| **Medium** (default) | medium | p5 | balanced | medium | 4 | `prio_speed=0` |
| Slow | slow | p6 | quality | slow | 3 | `prio_speed=0` |
| Slower | slower | p7 | quality | slower | 2 | `prio_speed=0` |
| Veryslow | veryslow | p7 | quality | veryslow | 1 | `prio_speed=0` |

## Measurements

Reference machine: Windows 11 Pro, AMD Ryzen 9 5900HX, NVIDIA GeForce RTX 3080 Laptop (driver
32.0.16.1692) and its AMD Radeon iGPU (1002:1638, driver 31.0.21925.1001), FFmpeg 8.1.3 (BtbN
GPL bundle), debug build of the tests, 2026-11-24. Both hardware encoders passed the probe;
QSV did not (no Intel GPU).

### Quality against x264

The encoders' own output through `cutweave-media` at the default preset (Medium), luma PSNR
against the reference frames, and the bitrate:

| Clip | CRF | x264 | NVENC | AMF |
|---|---|---|---|---|
| Sintel trailer, 480p upscaled to 1080p30, 15–25 s | 18 | 50.13 dB, 4553 kb/s | 50.53 dB, 4353 kb/s | 50.01 dB, 6434 kb/s |
| Same | **20** | **49.12 dB, 3499 kb/s** | **49.50 dB, 3367 kb/s** | **48.98 dB, 4876 kb/s** |
| Same | 23 | 47.58 dB, 2359 kb/s | 47.85 dB, 2346 kb/s | 47.27 dB, 3404 kb/s |
| testsrc2 1080p60 at 30 fps, 8 s | 20 | 49.87 dB, 4743 kb/s | 52.01 dB, 6182 kb/s | 52.88 dB, 7984 kb/s |

- On the natural clip both mappings stay within 0.4 dB of x264 from CRF 18 to 23. NVENC needs
  about x264's bitrate for it; AMF about 1.4 times as much.
- On synthetic graphics the hardware encoders' fixed quantizers spend 30–70 % more than x264
  (whose CRF adapts to the content) and land 2–3 dB higher. The offsets were set on the natural
  clip, the case exports are made for.
- The CLI sweep that chose the offsets gave NVENC `cq` 26 = 49.16 dB and AMF `qp` 21 = 48.98 dB
  against x264 CRF 20 = 49.12 dB on the same clip.

### Speed

10 s of 1080p30 with stereo AAC, from opening the encoder to `finish` (synthetic pictures; arch
§19 asks for at least 2× realtime with hardware encoding):

| Encoder | RGBA input (render readback) | NV12 input |
|---|---|---|
| NVENC | 1.65 s (6.1× realtime) | 1.58 s (6.3×) |
| AMF | 1.76 s (5.7×) | 1.32 s (7.6×) |
| x264 (`medium`, 16 threads) | 2.29 s (4.4×) | 2.91 s (3.4×) |

RGBA input costs a CPU conversion. On one thread it took about 14 ms per 1080p frame and held
both hardware encoders at 1.6× realtime, so the conversion runs on swscale's slice threads. Other
processes loading the CPU slow these numbers down: AMF on the iGPU shares the APU's power with
the CPU.

## Measuring again

The hardware tests are `#[ignore]`d and skip unless `CUTWEAVE_HW_TESTS=1` (FFmpeg setup:
[docs/dev/ffmpeg.md](../dev/ffmpeg.md)):

```sh
CUTWEAVE_HW_TESTS=1 cargo nextest run -p cutweave-media --features ffmpeg --run-ignored all --no-capture \
  -E 'test(export_quality_table) | test(each_hardware_encoder_writes)'
```

`export_quality_table` needs the small-tier corpus (`cargo xtask fetch-corpus --tier small`, or
`CUTWEAVE_CORPUS_DIR`). It prints the table above and fails if a hardware encoder's PSNR at CRF 20
is more than 1 dB from x264's on the Sintel clip. Change the offsets in
`crates/cutweave-media/src/ffmpeg/hwenc.rs` when a machine with QSV, VideoToolbox or VA-API runs
it, and update this page.

When comparing frames with FFmpeg's `psnr` filter, give both inputs the same color tags
(`setparams=range=tv:colorspace=bt709:...`). FFmpeg 8 negotiates color spaces in filter graphs,
so a tagged export against an untagged reference is converted first and loses about 25 dB.
