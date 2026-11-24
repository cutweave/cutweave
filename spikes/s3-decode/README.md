# Spike S3: ffmpeg-next decode, frame-exact seek, hardware decode (WP-0E.01, WP-0E.02)

> **Throwaway spike.** It has its own cargo workspace, is excluded from the root workspace
> ([spikes/README.md](../README.md)) and is never imported by `crates/`. Only the findings carry
> forward: into ADR-0003 (WP-0E.03 / WP-0J.02), `MediaBackend` and the FFmpeg build/link work
> (WP-1D.01), probe/index/decode (WP-1D.02/1D.03) and hardware decode (WP-1D.04). Architecture:
> §7 (media pipeline), §19 (budgets), §20 (frame-exact seek against ffprobe truth), §21 (FFmpeg
> packaging).
> **Measured on one Windows 11 dev machine only.** The cross-platform runs are WP-0E.03.
> WP-0E.02 (hardware decode, GPU upload, zero-copy) is the [first part](#wp-0e02-hardware-decode-gpu-upload-and-zero-copy);
> WP-0E.01 (CPU decode, index, frame-exact seek) [follows](#wp-0e01-verdict-cpu-decode).

## WP-0E.02: hardware decode, GPU upload and zero-copy

### WP-0E.02 verdict

**Hardware decode is frame-exact (bit-exact against software) on NVIDIA, meets the 150 ms 4K scrub
budget for camera-like GOPs (1-2 s) on this machine, but not for an 8.3 s GOP. One AMD driver
bug (open-GOP H.264 after a seek) was found. Zero-copy works on Windows.**

> **Status: partial, stopped early.** The owner restarted the machine before the second timing
> pass finished. What was measured and what is still open is marked in every table; the
> follow-up run is listed under [Open items](#open-items-for-a-follow-up-run).

| WP-0E.02 criterion / question | Result on this machine (Windows 11, RTX 3080 Laptop + Radeon iGPU) |
|---|---|
| Path (a) decodes + uploads + renders H.264 4K30 and HEVC 10-bit on every OS available | **Pass (Windows).** NVDEC and D3D11VA (both GPUs) decode 4K H.264 and HEVC Main10, copy to RAM (NV12/P010), upload to wgpu textures and convert to RGB; read-back RGB = CPU conversion ±1/255 and the burned-in barcode is right on every checked frame. macOS, Linux: not available here (run instructions below) |
| Clean software fallback when hwaccel init fails, and with hw forced off | **Pass:** five triggers (H6), each still 500/500 frame-exact in software |
| Frame-exact on the hw path (pts exact, PSNR > 50 dB vs software) | **NVIDIA: pass**, 18/18 files 500/500 on NVDEC and on D3D11VA, every hardware frame **bit-exact** (PSNR ∞). **Radeon D3D11VA: 17/18**; open-GOP H.264 after a seek has the right pts but wrong pixels (PSNR 15-21 dB, H10), fixed by starting one keyframe earlier (40-seek probe). D3D12VA and Vulkan video (RTX 3080): bit-exact on 5 files |
| Open-GOP HEVC (RASL after a seek) | **Pass** on all decoders, both directions (H1) |
| Hardware pipeline delay | **None** beyond software single-thread (2.0 packets per seek); frame threads add 14.6 (H3) |
| `skip_frame = NONREF` honoured | **Yes, by every hwaccel:** frames out per seek halve, seek p50 −34 to −52 % (H4) |
| 4K seek p95 vs 150 ms with hardware decode | **1 s GOP H.264: pass** (NVDEC 116, D3D11VA 134-136 ms). **2 s GOP HEVC Main10: pass on the RTX 3080** (NVDEC 81, D3D11VA 89 ms), **fail on the iGPU** (220 ms). **8.3 s GOP H.264: fail on every decoder** (714-752 ms). CPU: 279 / 418 / 836 ms (H2) |
| Texture-ready (decode → GPU texture) | Seek p95: NVDEC + upload 117 / 90 / 683 ms; zero-copy 125 / 81 / 694 ms (RTX). Playback per frame: 7-13 ms (path a, NVDEC), 3.6-6.4 ms (zero-copy, RTX) |
| CPU % / GPU % | Hardware seeks use 0.1-0.3 CPU cores (D3D11VA on the RTX: 1.0, its read-back), vs 5.4-6.5 on the CPU path; the GPU decode engine runs at 76-100 % during seeks (H5) |
| Zero-copy per OS | **Windows: works** (D3D11VA → NT handle → D3D12 → wgpu-hal, both GPUs, NV12 + P010, exact pixels; H9). **macOS, Linux: not measured** (instructions below) |

### Run it (hardware)

```powershell
cargo build --release                        # path (a); add --features zero-copy for path (b)
.\target\release\s3.exe hwinfo               # hwaccel types in the build, which devices open, wgpu adapters
.\target\release\s3.exe verify --quiet --hw d3d11va:nvidia --threads single      # every file, 500 seeks
.\target\release\s3.exe bench --quiet --monitor --configs frame+skip,cuda/single+skip,d3d11va:nvidia/single+skip `
    media\h264_2160p30_gop30.mp4 media\h264_2160p30_gop250.mp4 media\hevc_main10_2160p30_gop60.mp4
.\target\release\s3.exe gpu --quiet --monitor --dump --gpu nvidia@dx12 --configs cuda/single+skip media\h264_2160p30_gop30.mp4
.\target\release\s3.exe zerocopy --quiet --gpu nvidia@dx12 media\hevc_main10_2160p30_gop60.mp4   # --features zero-copy
$env:S3_HW = "d3d11va:nvidia,cuda"; cargo test --release -- --ignored hw_ gpu_   # default: d3d11va,cuda
# (plain `d3d11va` is the Radeon here, which fails the open-GOP H.264 file: finding H10)
```

- `--hw` takes `d3d11va[:ADAPTER|:nvidia|:amd|:intel]`, `d3d12va[:N]`, `cuda[:N]`, `dxva2`,
  `vulkan[:N]`, `qsv`, `vaapi[:/dev/dri/renderDN]`, `videotoolbox` or `off`. Bench and gpu
  configurations are `[HW/]THREADS[+skip]`, for example `d3d11va:nvidia/single+skip`.
- **Select D3D11VA by vendor, not index.** In a fresh process DXGI adapter 0 is the Radeon iGPU
  and adapter 1 the RTX 3080, but once CUDA has been used in the process the order flips (H7).
  `S3_FFLOG=verbose` makes FFmpeg name the adapter each device opens on.
- `S3_START_EARLIER=1` starts every seek one keyframe earlier (the H10 experiment).
- `--monitor` starts [monitor.ps1](monitor.ps1) next to the run: machine CPU % and the Windows
  "GPU Engine" counters (what Task Manager shows), per phase. Process CPU comes from
  `GetProcessTimes`. Both are Windows only.
- Outputs go to `s3-out/`. The CSVs of the runs below are in [results/win11-dev-hw/](results/win11-dev-hw/).

### How the hardware path works

```text
 packet ──► libavcodec h264/hevc decoder ──► hwaccel (D3D11VA / NVDEC / D3D12VA / Vulkan) ──► GPU surface
            (parsing, refs, RASL skip,         get_format picks the hw format if offered,          │
             skip_frame=NONREF: unchanged)      else the first software format (fallback)          │
                                                                                                   │ only the frame a
 path (a): av_hwframe_transfer_data ──► NV12 / P010 in RAM ──► wgpu write_texture (R8/RG8 or     ◄─┘ request returns
           (D3D11VA: staging copy + map;                        R16/RG16 Uint) ──► compute shader
            CUDA: cuMemcpy2D)                                   NV12/P010 → RGBA8 ──► "texture ready"
 path (b): CopySubresourceRegion into a shared NV12/P010 texture ──► NT handle ──► ID3D12Resource
           on wgpu's device ──► wgpu NV12/P010 texture (Plane0/Plane1 views) ──► same shader maths;
           sync = shared D3D11/D3D12 fence, waited on by wgpu's queue (no CPU wait, no CPU copy)
```

- **Same decoder, same seek.** The hwaccel only replaces slice decoding inside libavcodec, so the
  index, the start-keyframe rule, landing checks, `skip_frame` and the RASL handling of WP-0E.01
  apply unchanged. `src/hw.rs` adds `hw_device_ctx`, a `get_format` callback and the transfer;
  `src/source.rs` copies only the returned frame to RAM (frames dropped on the way to a seek target
  never leave the GPU).
- **Fallback is per source and per stream.** (1) `av_hwdevice_ctx_create` fails: software decode.
  (2) The decoder has no hwaccel for that device type (`avcodec_get_hw_config`; ProRes, MJPEG):
  software. (3) libavcodec does not offer the hw format for this stream (H.264 High 10): the
  callback picks the software format. (4) The hwaccel fails to initialise (unsupported profile,
  level or size): libavcodec calls `get_format` again without the hw format, and the callback
  picks software. Every case is counted (`hw_summary`: `get_format hw N sw N; frames hw N sw N`).
- **Verification.** The pts and barcode checks of WP-0E.01 run unchanged. The Y-CRC check against
  the CLI is replaced by a comparison with the software decode of the same frame: a software
  pre-pass stores CRCs of the three planes, normalised to planar LSB-aligned samples
  (`src/planes.rs`: NV12 de-interleaved, P010 shifted down 6 bits). Equal CRCs mean bit-exact;
  otherwise the frame is decoded again in software and must reach **PSNR ≥ 50 dB** (all three
  planes).
- **Render check.** `s3 gpu` and `s3 zerocopy` read the RGB result back, compare it with the same
  conversion on the CPU ([gpu.rs](src/gpu.rs) `cpu_reference`, max channel difference ≤ 2) and
  read the burned-in barcode off the green channel. The shader is the input stage of Spike S4's
  `decode.wgsl` (range, matrix, MPEG-2 chroma siting) without S4's transfer, primaries and
  tone-map stages, which are the compositor's job.
- **`unsafe`.** FFI only: `hw.rs` (device, hw config, `get_format`, transfer), `ffi.rs`
  (`GetProcessTimes`) and the feature-gated `zerocopy.rs` (D3D11/D3D12/DXGI COM calls and the
  wgpu-hal import). Every block has a `// SAFETY:` comment.

### Results: hardware decode on the Windows 11 dev machine (informal)

> **INFORMAL, shared machine.** Same laptop as WP-0E.01: Ryzen 9 5900HX, 32 GB, RTX 3080 Laptop
> (driver 616.92, NVDEC) + Radeon iGPU (driver 31.0.21925, D3D11VA), Windows 11 26200, FFmpeg
> 8.1.3 BtbN, wgpu 30.0.2 on DX12. Media on the USB HDD, warm cache. **Background load:** other
> agents' `cargo nextest` and builds ran throughout. The monitor saw the whole machine at
> 12-29 % CPU during hardware runs (this process: 0.1-1 core, 1-6 %) and 50-59 % during CPU
> runs (this process 5.4-6.5 cores), so 10-25 % came from other jobs. That is lighter than
> WP-0E.01's 70-90 %. Each configuration ran once (200 or 100 seeks): treat p95 values as
> pessimistic and single-run.

Raw data: [results/win11-dev-hw/](results/win11-dev-hw/). Configurations: `single` = one decode
thread, `frame` = frame threads, `+skip` = `skip_frame = NONREF` before the target. "D3D11VA, RTX"
is `d3d11va:nvidia`, "D3D11VA, iGPU" is `d3d11va:amd` (selected by vendor, H7). The first bench
pass addressed D3D11VA by index after CUDA had run in the same process, which swapped the GPUs;
those rows are kept apart in `bench-first-pass-index-flipped.csv` as evidence for H7 and are not
used in the tables.

#### Frame-exactness on the hardware path

`s3 verify --hw X --threads single`, 500 random seeks per file plus sequential, edge (every
keyframe ±1, every open-GOP leading/RASL frame, both directions) and scrub passes. Every
returned frame: pts = index = ffprobe, barcode = presentation index, and its three planes equal
the software decode's or PSNR ≥ 50 dB.

| File | D3D11VA, RTX 3080 | NVDEC (cuda) | D3D11VA, Radeon iGPU |
|---|---|---|---|
| h264_1080p23976_gop48.mp4 | 500/500, bit-exact 941/941 | 500/500, bit-exact 941/941 | 500/500, bit-exact 941/941 |
| h264_1080p25_gop50.mkv | 500/500, bit-exact 956/956 | 500/500, bit-exact 956/956 | 500/500, bit-exact 956/956 |
| h264_1080p2997_gop60_opengop.mp4 | 500/500, bit-exact 1041/1041 | 500/500, bit-exact 1041/1041 | **105/500**, edges 18/46, scrub 165/195: pts right, pixels wrong (PSNR min 14.6 dB, H10) |
| h264_1080p30_gop30.mov | 500/500, bit-exact 1061/1061 | 500/500, bit-exact 1061/1061 | 500/500, bit-exact 1061/1061 |
| h264_1080p60_gop120.mp4 | 500/500, bit-exact 1301/1301 | 500/500, bit-exact 1301/1301 | 500/500, bit-exact 1301/1301 |
| h264_1080p25_gop25.ts | 500/500, bit-exact 999/999 | 500/500, bit-exact 999/999 | 500/500, bit-exact 999/999 |
| h264_2160p30_gop30.mp4 | 500/500, bit-exact 1061/1061 | 500/500, bit-exact 1061/1061 | 500/500, bit-exact 1061/1061 |
| h264_2160p30_gop250.mp4 | 500/500, bit-exact 1289/1289 | 500/500, bit-exact 1289/1289 | 500/500, bit-exact 1289/1289 |
| h264_vfr_phone_1080p_rot90.mp4 | 500/500, bit-exact 1061/1061 | 500/500, bit-exact 1061/1061 | 500/500, bit-exact 1061/1061 |
| h264_vfr_rates_1080p.mkv | 500/500, bit-exact 1061/1061 | 500/500, bit-exact 1061/1061 | 500/500, bit-exact 1061/1061 |
| h264_720p30_rot270.mp4 | 500/500, bit-exact 947/947 | 500/500, bit-exact 947/947 | 500/500, bit-exact 947/947 |
| hevc_main10_2160p30_gop60.mp4 (open GOP, RASL) | 500/500, bit-exact 941/941 | 500/500, bit-exact 941/941 | 500/500, bit-exact 941/941 |
| hevc_main10_pq_1080p24.mp4 (HDR10, open GOP) | 500/500, bit-exact 861/861 | 500/500, bit-exact 861/861 | 500/500, bit-exact 861/861 |
| prores_hq_1080p25.mov | 500/500 (software: no hwaccel) | 500/500 (software: no hwaccel) | 500/500 (software: no hwaccel) |
| h264_high10_720p30.mp4 | 500/500 (software: hw format not offered) | 500/500 (software: NVDEC init failed) | 500/500 (software: not offered) |
| hevc_main422_10_720p30.mp4 | 500/500 (software: not offered) | 500/500 (software: NVDEC init failed) | 500/500 (software: not offered) |
| proxy_prores_540p.mov | 500/500 (software: no hwaccel) | 500/500 (software: no hwaccel) | 500/500 (software: no hwaccel) |
| proxy_mjpeg_540p.mov | 500/500 (software: no hwaccel) | 500/500, not bit-exact, PSNR ≥ 73.8 dB (H8) | 500/500 (software: no hwaccel) |

"bit-exact N/N" counts returned hardware frames whose three planes equal the software decode.
Each run also passed sequential, edge and scrub passes, except the Radeon open-GOP row.
More configurations, on the three 4K files, the HDR10 file and the open-GOP H.264 file (200 seeks
each): **D3D12VA** and **Vulkan video** on the RTX 3080 (each in a fresh process, H7), D3D11VA
and NVDEC **with frame threads**, and NVDEC **without NONREF skipping**: 25/25 runs exact on every
pass, every hardware frame bit-exact (`verify-d3d12va1-single.csv`, `verify-vulkan1-single.csv`,
`verify-d3d11va1-frame.csv`, `verify-cuda-frame.csv`, `verify-cuda-noskip.csv`). The harness
tests (`cargo test -- --ignored`, `S3_HW=d3d11va:nvidia,cuda`) pass 7/7.

#### Seek latency against the 150 ms budget (frame in system memory)

`s3 bench --seeks 200` (the WP-0E.01 harness): request → decoded frame in RAM, so hardware
includes `av_hwframe_transfer_data`. Cold random seeks; scrub = sweep forward and back with
forward-continue; step = next frame.

| File | Decoder | Seek p50 | **Seek p95** | Scrub p50 / p95 | Step p50 | Sequential fps | CPU cores (seek) | 150 ms |
|---|---|---|---|---|---|---|---|---|
| 4K H.264, 1 s GOP | CPU, frame threads | 197 | **279** | 126 / 259 | 0.8 | 205 | 5.43 | fail |
| | NVDEC | 74 | **116** | 18 / 105 | 5.1 | 151 | 0.22 | **pass** |
| | D3D11VA, RTX ² | 84 | **134** | 28 / 116 | 10.5 | 78 | 1.00 | **pass** |
| | D3D11VA, iGPU ² | 81 | **136** | 22 / 122 | 7.6 | 108 | 0.22 | **pass** |
| 4K H.264, 8.3 s GOP | CPU, frame threads | 419 | **836** | 154 / 801 | 0.5 | 227 | 6.35 | fail |
| | NVDEC | 309 | **714** | 31 / 665 | 5.0 | 154 | 0.12 | fail |
| | D3D11VA, RTX ² | 323 | **727** | 45 / 691 | 11.0 | 69 | 1.00 | fail |
| | D3D11VA, iGPU ² | 330 | **752** | 47 / 703 | 6.6 | 63 | 0.18 | fail |
| 4K HEVC Main10, 2 s GOP | CPU, frame threads | 302 | **418** | 207 / 396 | 1.0 | 129 | 6.53 | fail |
| | NVDEC | 53 | **81** | 15 / 79 | 8.0 | 136 | 0.29 | **pass** |
| | D3D11VA, RTX ³ | 57 | **89** | 24 / 84 | 11.5 | 82 | 1.01 | **pass** |
| | D3D11VA, iGPU ³ | 132 | **220** | 46 / 213 | 18.1 | 48 | 0.13 | fail |
| all three | D3D12VA, Vulkan video (RTX) | not measured (stopped by the restart); both verified bit-exact | | | | | | open |

² Second pass, D3D11VA selected by vendor (`bench-d3d11va-partial.log`; the run was stopped
before it wrote its CSV, so these rows come from its printed log).
³ First pass (`bench-first-pass-index-flipped.csv`), where D3D11VA was addressed by index after
CUDA had flipped the adapter order (H7). The adapter is assigned here by its signature: D3D11VA on
the RTX costs 1.0 CPU core in every vendor-selected run (read-back over PCIe), the iGPU 0.1-0.3
cores; the row labelled `d3d11va:0` shows 1.01 cores, the one labelled `d3d11va:1` 0.13. The
vendor-selected HEVC rerun is open.

**Threading and NONREF skipping** (seek p50 / p95 ms; decoder output delay in packets beyond the
target's own decode position; frames returned per seek):

| File | Decoder | single+skip | single, no skip | frame threads + skip |
|---|---|---|---|---|
| 4K H.264, 1 s GOP | NVDEC | 74 / 116 · delay 2.0 · out 9.0 | 112 / 185 · 2.0 · 16.1 | 148 / 205 · 16.6 · 9.0 |
| | D3D11VA, RTX ² | 84 / 134 · 2.0 · 9.0 | 122 / 201 · 2.0 · 16.1 | 156 / 214 · 16.6 · 9.0 |
| | D3D11VA, iGPU ² | 81 / 136 · 2.0 · 9.0 | 113 / 195 · 2.0 · 16.1 | 171 / 228 · 16.6 · 9.0 |
| | CPU | | | 197 / 279 · 16.6 · 9.0 |
| 4K H.264, 8.3 s GOP | NVDEC | 309 / 714 · 2.0 · 54.8 | 565 / 1 335 · 2.0 · 107.5 | 373 / 796 · 16.7 · 54.8 |
| | D3D11VA, RTX ² | 323 / 727 · 2.0 · 54.8 | 575 / 1 344 · 2.0 · 107.5 | 393 / 797 · 16.7 · 54.8 |
| | CPU | | | 419 / 836 · 16.7 · 54.8 |
| 4K HEVC Main10, 2 s GOP | NVDEC | 53 / 81 · 2.1 · 13.2 | 103 / 176 · 2.1 · 30.9 | 86 / 118 · 16.1 · 13.2 |
| | D3D11VA, iGPU ³ | 132 / 220 · 2.1 · 13.2 | 277 / 488 · 2.1 · 30.9 | 243 / 332 · 16.1 · 13.2 |
| | CPU | | | 302 / 418 · 16.1 · 13.2 |

#### Texture-ready latency (path a) and per-frame playback

`s3 gpu --seeks 100`: decode → (transfer) → `write_texture` → NV12/P010→RGB compute pass → GPU
done, on a wgpu DX12 device on the same GPU as the decoder. "Playback" decodes every frame in
order and makes each one texture-ready before the next (no pipelining, so it is a latency, not a
throughput, figure). Every row passed the render check (read-back RGB vs the CPU conversion: max
difference 1/255 on 9 checked frames per row; barcode correct).

| File | Decode → wgpu device | Format | Seek: decode p50 / p95 | Upload + convert p50 / p95 | **Texture-ready p50 / p95** | Playback per frame p50 / p95 | Playback fps | CPU cores (seek / play) |
|---|---|---|---|---|---|---|---|---|
| 4K H.264, 1 s GOP | CPU → RTX | yuv420p | 182 / 259 | 13.8 / 17.2 | **196 / 273** | 4.6 / 12.8 | 153 | 5.49 / 5.96 |
| | NVDEC → RTX | NV12 | 78 / 113 | 3.7 / 4.6 | **83 / 117** | 7.0 / 13.7 | 117 | 0.24 / 0.49 |
| | D3D11VA RTX → RTX ⁴ | NV12 | 87 / 132 | 3.7 / 4.3 | **91 / 135** | 18.6 / 28.9 | 52 | 0.98 / 0.83 |
| | D3D11VA iGPU → iGPU ⁴ | NV12 | 87 / 118 | 5.8 / 8.3 | **92 / 126** | 13.6 / 38.5 | 53 | 0.26 / 0.48 |
| 4K H.264, 8.3 s GOP | CPU → RTX | yuv420p | 381 / 722 | 13.9 / 16.1 | **395 / 736** | 5.7 / 11.6 | 154 | 6.35 / 7.59 |
| | NVDEC → RTX | NV12 | 296 / 680 | 3.6 / 4.0 | **300 / 683** | 7.9 / 13.8 | 109 | 0.13 / 0.56 |
| | D3D11VA RTX → RTX ⁴ | NV12 | 311 / 695 | 3.6 / 4.0 | **314 / 699** | 29.4 / 42.1 | 34 | 1.01 / 0.79 |
| | D3D11VA iGPU → iGPU ⁴ | NV12 | 314 / 718 | 5.7 / 7.8 | **319 / 725** | 12.7 / 40.2 | 56 | 0.14 / 0.52 |
| 4K HEVC Main10, 2 s GOP | CPU → RTX | yuv420p10le | 281 / 393 | 26.0 / 31.3 | **309 / 419** | 11.1 / 21.6 | 78 | 6.38 / 6.77 |
| | NVDEC → RTX | P010 | 56 / 84 | 6.5 / 7.7 | **63 / 90** | 13.4 / 16.2 | 72 | 0.33 / 0.57 |
| | D3D11VA RTX → RTX ⁴ | P010 | 66 / 116 | 6.5 / 7.7 | **72 / 122** | 19.3 / 22.0 | 51 | 0.95 / 0.77 |
| | D3D11VA iGPU → iGPU ⁴ | P010 | 139 / 215 | 7.3 / 9.9 | **145 / 222** | 30.8 / 58.2 | 32 | 0.11 / 0.51 |

⁴ Addressed by index (`d3d11va:1` in `gpu-rtx-run.csv`, `d3d11va:0` in `gpu-igpu-run.csv`;
`gpu-cpu-nvdec.csv` repeats the CPU and NVDEC rows). The vendor-selected rerun was stopped by
the restart, so the adapter is assigned by the same CPU-time signature as ³ (1.0 core = RTX read-back);
it agrees with the bench and zero-copy numbers for each GPU. Treat these four rows as provisional.

#### Zero-copy (path b, Windows)

`s3 zerocopy --seeks 100` (`--features zero-copy`): decode → GPU copy into the shared texture →
shared fence → conversion on the imported texture → GPU done. Check: 3 frames per file compared
with the CPU conversion of the same frame copied to RAM (max difference 1/255) and the barcode
of every 50th frame of the playback pass read off the RGB result: all correct.

| File | GPU (decoder = wgpu device, LUID-checked) | Seek: decode submit p50 / p95 | Copy + sync + convert p50 / p95 | **Texture-ready p50 / p95** | Playback per frame p50 / p95 | Playback fps |
|---|---|---|---|---|---|---|
| 4K H.264, 1 s GOP | RTX 3080 | 14 / 19 | 66 / 107 | **80 / 125** | 6.4 / 8.8 | 136 |
| | Radeon iGPU | 13 / 30 | 75 / 110 | **87 / 138** | 12.6 / 30.1 | 61 |
| 4K H.264, 8.3 s GOP | RTX 3080 | 199 / 580 | 104 / 116 | **303 / 694** | 6.4 / 8.0 | 142 |
| | Radeon iGPU | 231 / 605 | 97 / 114 | **321 / 718** | 13.0 / 27.9 | 62 |
| 4K HEVC Main10, 2 s GOP | RTX 3080 | 7 / 27 | 45 / 54 | **52 / 81** | 3.6 / 3.9 | 239 |
| | Radeon iGPU | 8 / 86 | 131 / 137 | **140 / 218** | 14.9 / 35.2 | 52 |

"Decode submit" is the CPU side of the decode (D3D11VA returns once the work is queued); the GPU
decode itself finishes inside "copy + sync + convert", which waits for it.

#### Zero-copy status per OS

| OS | Status | Evidence |
|---|---|---|
| Windows 11 (D3D11VA → NT shared handle → D3D12 → wgpu-hal) | **Works** (with one GPU copy; caveats below) | Both GPUs, NV12 and P010, 4K H.264 and HEVC Main10; pixels equal to path (a); texture-ready equal to or better than path (a), playback 2-4× faster with no CPU copy (tables above) |
| macOS (IOSurface → Metal) | **Not measured** | No Mac here. Implementation sketch and FFmpeg-level check in the [run instructions](#run-instructions-for-wp-0e03-macos-linux-not-measured-here) |
| Linux (dmabuf → Vulkan) | **Not measured** | No Linux GPU here (the owner's VM has no video decode). Same |

Caveats on Windows ("works-with-caveats" for production): (1) it needs one GPU copy, because
FFmpeg's decoder texture array is created with `D3D11_BIND_DECODER` only and is not shareable
(FFmpeg's `AVD3D11VADeviceContext.BindFlags/MiscFlags` could make it shareable; not tried);
(2) the decoder and wgpu must be on the same adapter (checked by LUID); (3) the imported texture
is wrapped as COMMON-state and its state is not handed back to D3D11 between frames, which was
correct on both drivers here but is not guaranteed by the D3D12 rules for cross-API resources;
(4) the fence wait is staged through `wgpu::hal::dx12::Queue::add_wait_fence`, a wgpu-hal
interface. CPU sync (`S3_ZC_SYNC=cpu`, an event query) measured the same latency (H9).

### WP-0E.02 findings

#### H1. Hardware decode through libavcodec is frame-exact, and bit-exact on H.264 and HEVC

- **Frame-exact in 53 of 54 runs** (18 files × 3 decoders, 500 random seeks each). No pts
  mismatch, no wrong barcode, no overshoot, no landing retry, in random seeks, sequential decode,
  the edge pass (every open-GOP leading frame and RASL picture, forward and backward) and the
  scrub pass. The 54th run (Radeon, open-GOP H.264) returns the right pts with wrong pixels (H10).
- **Bit-exact, not just close.** Apart from that run, every hardware frame of H.264 (High, 8-bit,
  closed and open GOP, VFR, MKV/TS/MOV) and HEVC (Main10, HDR10, open GOP with CRA/RASL) equals
  the software decode on all three planes, on NVDEC, D3D11VA on the RTX 3080 and D3D11VA on the
  Radeon. That is what the standards require of a conforming decoder: the WBS's PSNR > 50 dB
  criterion is met with room to spare, and anything less than bit-exact on these codecs points at
  a driver bug (H10). The only other non-bit-exact hardware decode is MJPEG on NVDEC (H8).
- **RASL after a seek** is handled by libavcodec before the hwaccel sees a slice, so the
  WP-0E.01 start-keyframe rule (`pts(K) <= pts(T)`) keeps working unchanged: the 8 RASL frames of
  the 4K HEVC file and the 4 of the HDR10 file were exact in both directions on every decoder.
- **D3D12VA, Vulkan video, hardware frame threads and no-skip** are exact and bit-exact too (25 more runs, see above).

#### H2. Seek latency: the hardware decoder is the bottleneck, so the GOP length decides the budget

- **Camera-like GOPs pass.** 4K H.264 with a 1 s GOP and 4K HEVC Main10 with a 2 s GOP stay under
  the 150 ms p95 with hardware decode, where the CPU needed 279 and 418 ms: NVDEC 116 and 81 ms, D3D11VA on the RTX 134 and 89 ms; the iGPU passes on H.264 (136 ms) but not on HEVC (220 ms).
- **An 8.3 s GOP fails on every decoder** (p95 680-760 ms, p50 about 300 ms). The decode engine
  is saturated (95-100 %) and a seek still has to decode about 55 frames on average and up to
  about 125 at p95 (with NONREF skipping). No decoder setting fixes that.
- **Rule of thumb from these numbers:** a hardware seek costs about 25 ms fixed (demuxer seek,
  flush, transfer) plus a per-packet cost from the keyframe to the target: 2.6-2.9 ms for 4K
  H.264 on either GPU (fitted from the 1 s and 8.3 s GOP rows, NVDEC), and about 0.9 ms for x265 HEVC Main10 on the RTX 3080 (about 3 ms on the iGPU). With
  the p95 seek covering about one GOP, the 150 ms p95 holds up to a keyframe interval of roughly
  **1.4 s for 4K30 H.264** and about 4 s for 4K30 HEVC Main10 on the RTX 3080 (about 1.4 s on the iGPU). Phones and cameras use 0.5-2 s;
  screen recorders, x264's default (250 frames) and some drones and action cameras use more.
- **The decision this supports:** scrub originals with hardware decode when the measured GOP
  (the index knows it) is within that bound, and fall back to the intra proxy (WP-0E.01: 0.2-1 ms
  per frame) above it or when no hardware decoder opened. The budget is per machine and per codec:
  the iGPU and the RTX 3080 decode H.264 seeks at the same speed here, but the iGPU is about 2.5×
  slower on HEVC Main10.

#### H3. One decode thread on the hardware path; frame threads only add delay

- With a hwaccel, libavcodec's frame threads still parse in parallel, but the output waits for a
  thread per frame. The decoder's output delay goes from **2.0 packets** (single thread: the
  B-pyramid reorder delay, the same as software single-thread) to **16.6 packets** with frame
  threads, and the seek p50 rises by about 2× on the 1 s GOP (NVDEC 74 → 148 ms, D3D11VA on the
  RTX 84 → 156 ms, iGPU 81 → 171 ms) and by 20-25 % on the 8.3 s GOP.
- Hardware decode does not need CPU parallelism (0.1-0.3 cores during seeks), so the WP-0E.01 rule
  "frame threads for long-GOP seek decoders" is a **software-only** rule. Hardware seek and scrub
  decoders use `thread_count = 1`.
- The hardware "pipeline" itself adds no packets: NVDEC (FFmpeg maps the surface when it is output)
  and D3D11VA (the frame is returned once submitted, the GPU finishes asynchronously) both return
  the target after exactly the same packets as software single-thread decode.

#### H4. `skip_frame = NONREF` is honoured by every hwaccel

The skip decision is made by libavcodec's H.264/HEVC parser before the hwaccel sees the slice, so
it applies to every hwaccel. Frames returned per seek drop from 16.1 to 9.0 (1 s GOP), from
107.5 to 54.8 (8.3 s GOP) and from 30.9 to 13.2 (HEVC), and the seek p50 drops by 34-52 %
(NVDEC: 112 → 74, 565 → 309 and 103 → 53 ms). That is a bigger gain than on the CPU with frame
threads (−5 % on x264), because the hardware decoder is the serial bottleneck. Keep it on.

#### H5. Where a hardware seek spends its time

- **D3D11VA** returns the frame as soon as the decode is submitted; `av_hwframe_transfer_data`
  (copy to a staging texture + `Map`) then waits for the GPU. The `xfer` column (66-105 ms per
  seek, in the CSVs) is therefore mostly the GPU decode, not the copy.
- **NVDEC** synchronises when FFmpeg maps the output surface inside the decode loop; the copy to
  RAM afterwards (`cuMemcpy2D`, 12-24 MB) costs 3.2-6 ms.
- **The decode engine is saturated during seeks**: 76-100 % "Video Decode" utilisation on both
  GPUs (Task Manager counters), with 0.1-0.3 CPU cores. A cold seek costs (packets from the
  keyframe) × (hardware per-frame time), so it scales with the GOP exactly like software, at a
  lower per-frame cost.
- **Copying every frame to RAM is the weak point of path (a) on D3D11VA.** Sequential decode with
  a transfer per frame: NVDEC 136-154 fps at 0.4-0.8 CPU cores; D3D11VA on the iGPU 63-108 fps at
  0.2-0.7 cores (shared memory); **D3D11VA on the RTX 67-82 fps at a full CPU core** (the staging
  read-back over PCIe). Zero-copy removes that copy (H9); a production path (a) should at least
  pool the destination frames.

#### H6. Fallback is clean in all five ways it can be triggered

Every case below decodes in software and passes the same frame-exact checks (500/500 random seeks
plus all passes, in the verify CSVs and `hw_failures_fall_back_to_software_cleanly`):

| Case | Trigger seen here | What the harness reports |
|---|---|---|
| Forced off | no `--hw` (`CUTWEAVE_HW=off` in the product) | `sw (hw off)` |
| 1. No device | `d3d11va:vendor=0xdead` (no such GPU), `d3d11va:2` (the Basic Render Driver: `DXGI_ERROR_UNSUPPORTED`), `dxva2:1`, `vaapi` on Windows, `qsv` without an Intel GPU | `sw fallback: … device failed (…)` |
| 2. No hwaccel for the codec | ProRes HQ, ProRes Proxy (all hwaccels); MJPEG on D3D11VA | `sw fallback: prores has no d3d11va hwaccel` |
| 3. Hw format not offered for the stream | H.264 High 10 and HEVC 4:2:2 10-bit on D3D11VA (libavcodec lists no D3D11 format for them) | `get_format hw 0 sw 1; frames hw 0` |
| 4. Hwaccel initialisation fails | H.264 High 10 and HEVC 4:2:2 on NVDEC (offered, but the RTX 30 has neither); D3D12VA on the Radeon ("requires tier 1 support, but it is not implemented"); Vulkan on the Radeon (no `VK_KHR_video_decode_queue`) | `get_format hw 1 sw 1; frames hw 0` (libavcodec asked again without the hw format) |

The only cost is time: a fallback to single-thread software decode is slow (the D3D12VA/Vulkan
rows that fell back ran at 340-1 500 ms p50, H7). The product should log the reason and, for case
4, re-open the software decoder with its normal (frame-thread) configuration instead of keeping
the hardware decoder's single thread.

#### H7. DXGI adapter indexes change inside a process on a hybrid laptop

- A fresh process sees DXGI adapter 0 = Radeon iGPU, 1 = RTX 3080. **Once CUDA (NVDEC) has been
  used in the process, the order flips**: `d3d11va:1`, `d3d12va:1` and `vulkan:1` then open the
  Radeon (FFmpeg's verbose log, `S3_FFLOG=verbose`: `Selecting d3d11va adapter 1 … Using device
  1002:1638`). Presumably the NVIDIA hybrid-graphics driver marks a process that loads
  `nvcuda.dll` as a dGPU process, and DXGI then lists the high-performance GPU first; the cause
  was not investigated further.
- This silently swapped the D3D11VA rows of the first bench pass (they ran after the CUDA rows in
  one process) and made the "D3D12VA, RTX 3080" and "Vulkan, RTX 3080" rows run on the Radeon,
  where both fail to initialise and fall back (H6, case 4). The H.264 D3D11VA rows of the tables
  come from a second pass that selects D3D11VA by vendor (`d3d11va:nvidia`, FFmpeg's
  `vendor_id` device option); the HEVC D3D11VA rows are first-pass rows reassigned by their
  CPU-time signature (footnote ³). In a fresh process D3D12VA and Vulkan decode on the RTX 3080
  and are bit-exact (verify runs); their latency was not measured before the restart.
- The verify runs are not affected: each used one hwaccel per process and no CUDA, where index
  1 is the RTX and 0 the iGPU (checked with `S3_FFLOG=verbose`); the iGPU's open-GOP failure
  (H10) reproduces with `d3d11va:amd`.
- **A missing index is not an error either.** FFmpeg's D3D11VA treats an adapter index that
  `EnumAdapters` does not have as "no adapter" and opens the default one: `d3d11va:9` decoded in
  hardware here (`verify-nodevice.csv`: `d3d11va:9 attached`). A missing vendor (`vendor_id`) does fail.
- **Rule for WP-1D.04: never address a GPU by index.** Pick the decode adapter by LUID (D3D11/D3D12:
  enumerate DXGI and compare `AdapterLuid` with the wgpu adapter's, as `zerocopy.rs` does) or at
  least by vendor/device id, and re-resolve it at device creation, not once at start-up.

#### H8. MJPEG on NVDEC is close but not bit-exact

NVDEC also decodes MJPEG (the 540p proxy): frame-exact, but no frame is bit-exact (JPEG allows
IDCT implementations to differ), minimum PSNR 73.8 dB, comfortably above 50 dB. Proxies decode in
about 1 ms on the CPU with slice threads (WP-0E.01), so the product keeps intra/proxy decoding on
the CPU and only sends long-GOP codecs to the hardware.

#### H9. Zero-copy works on Windows on both GPUs, with exact pixels

- **It worked first time** on the RTX 3080 and the Radeon, for NV12 (H.264) and P010 (HEVC
  Main10): wgpu 30 exposes everything needed (`TextureFormat::NV12`/`P010` with
  `Plane0`/`Plane1` views, `hal::dx12::Device::texture_from_raw`, `create_texture_from_hal`,
  `hal::dx12::Queue::add_wait_fence`), and the `windows` 0.62 crate is the one wgpu-hal uses, so
  the `ID3D12Resource` passes straight through.
- **Pixels are the same as path (a)**: the converted RGB differs from the CPU conversion of the
  same frame copied to RAM by at most 1/255 (rounding), and every checked barcode is right, in
  seeks and in playback.
- **Latency.** For a cold seek the hardware decode dominates, so texture-ready is about the same
  as path (a). The gain is in playback: no 12-24 MB copy through system memory per frame. On the
  RTX 3080 every frame is texture-ready in 6.4 ms (H.264, 136-142 fps) and 3.6 ms (HEVC Main10,
  239 fps), against 7-13 ms with NVDEC + upload and 19-29 ms with D3D11VA + upload; on the iGPU
  12.6-14.9 ms (52-62 fps) against 13.6-31 ms.
- **Sync.** The GPU-side wait (shared fence) and a CPU event-query wait gave the same seek
  latency on the RTX 3080 (texture-ready p50/p95 with a CPU wait: 81/124, 304/691 and 53/77 ms;
  with the fence: 80/125, 303/694 and 52/81 ms). The fence is the production choice because it
  does not block a CPU thread.
- **Time box:** about half a day of the 4 agent-days, including the D3D12 interop. The caveats
  for production are listed under [Zero-copy status per OS](#zero-copy-status-per-os).

#### H10. The Radeon's D3D11VA decoder corrupts open-GOP H.264 after a seek (the verifier caught it)

- **Symptom.** On `h264_1080p2997_gop60_opengop.mp4` (x264 `open-gop`: non-IDR I-frames with a
  recovery point, 9 leading B-frames per GOP), D3D11VA on the Radeon returns the right frame (pts
  exact) with **wrong pixels**: PSNR 14.6-21 dB against software, max difference up to 240.
  Random seeks: 105/500 correct; edges 18/46; scrub 165/195. Sequential decode from the start is
  exact (300/300). NVDEC and D3D11VA on the RTX 3080 are bit-exact on the same seeks.
- **Trigger: starting the decode at a non-IDR I-frame.** Every failing target lies after the
  first GOP (which starts with an IDR). With `skip_frame` off the result is identical (4/40 in a
  40-seek probe), so it is not NONREF skipping. Starting every seek **one keyframe earlier**
  (`S3_START_EARLIER=1`, so the leading pictures have their real references) makes the Radeon
  bit-exact (40/40, edges 46/46, scrub 195/195 in a 40-seek probe; the 500-seek confirmation run was stopped by the restart).
- **Likely mechanism** (not proven): after a seek, libavcodec gives the leading B-frames
  placeholder references for the missing previous GOP; software conceals and then recovers at the
  recovery point, while the AMD driver's decoder apparently keeps using bad reference data past
  it. HEVC open GOP (CRA + RASL) is not affected: both open-GOP HEVC files are bit-exact on the
  Radeon (500/500 each, edges 36/36 and 28/28), where libavcodec drops the RASL pictures before
  the hwaccel sees them.
- **Consequences for WP-1D.04:**
  - "Hardware decode is used where frame-exact" has to be checked per vendor/driver and per
    stream type; a decoder that is exact on closed GOPs is not automatically exact on open GOPs.
    Keep the pixel check (hash or PSNR against software) in the hardware corpus test.
  - Workaround options: start H.264 hardware seeks at an IDR (or one keyframe earlier) when the
    start keyframe is a non-IDR I-frame (the index can record IDR vs recovery point; costs about one
    more GOP of decode on open-GOP H.264 only), or use software decode for open-GOP H.264 on that
    vendor.
  - Report it to AMD with the reproducer: `s3 verify --hw d3d11va:amd --threads single --seeks 40
    media/h264_1080p2997_gop60_opengop.mp4` (Radeon driver 31.0.21925.1001, Ryzen 9 5900HX iGPU).

### Open items for a follow-up run

The machine was restarted during the second timing pass. Measured and committed: every verify
run (3 decoders × 18 files at 500 seeks, plus 25 runs on D3D12VA, Vulkan, frame threads and
no-skip), the first bench pass, path (a) texture-ready, zero-copy on both GPUs, the 7 harness
tests. Still open:

1. **Vendor-selected D3D11VA timing for the 8.3 s GOP (iGPU no-skip / frame threads) and for HEVC
   Main10 (both GPUs)**, replacing the footnote ³ rows, and path (a) texture-ready with
   `d3d11va:nvidia` / `d3d11va:amd` replacing the ⁴ rows:
   `s3 bench --quiet --monitor --seeks 200 --configs d3d11va:nvidia/single+skip,d3d11va:nvidia/single,d3d11va:nvidia/frame+skip,d3d11va:amd/single+skip,d3d11va:amd/single,d3d11va:amd/frame+skip <three 4K files>`
   and `s3 gpu --quiet --monitor --gpu nvidia@dx12 --configs d3d11va:nvidia/single+skip …`,
   `--gpu radeon@dx12 --configs d3d11va:amd/single+skip …`.
2. **D3D12VA and Vulkan video latency** on the RTX 3080, each in a fresh process:
   `s3 bench --quiet --seeks 100 --configs d3d12va:1/single+skip …` (same for `vulkan:1`).
3. **The 500-seek confirmation of the H10 workaround:** `S3_START_EARLIER=1 s3 verify --quiet
   --hw d3d11va:amd --threads single media/h264_1080p2997_gop60_opengop.mp4
   media/hevc_main10_2160p30_gop60.mp4 media/hevc_main10_pq_1080p24.mp4`, and
   `s3 verify --hw d3d11va:vendor=0xdead` for the missing-vendor fallback in the verify CSVs (the
   harness test already covers it).
4. **Repeat runs** of the budget rows (single run each here), ideally on the WP-0I reference
   machine, before ADR-0003 quotes the GOP bounds.
5. **macOS and Linux** rows (path a, fallback, zero-copy): WP-0E.03, instructions below.

The scratch scripts that drove the runs are not committed; each step above is one `s3` command
(about 45 minutes in total on this machine).

### Proposed ADR-0003 changes (draft for the integrator)

ADR edits are outside this WP's Area; this is the text proposed from WP-0E.02. ADR-0003 stays
**Accepted with fallback**: the proxy fallback is still needed for long GOPs and for machines
without a working hardware decoder.

**Replace the third paragraph of the Decision** ("v1 uploads decoded planes from the CPU …") with:

> **v1 decodes long-GOP codecs in hardware where available and uploads from the CPU (path a).**
> FFmpeg hwaccels (D3D11VA on Windows, VideoToolbox on macOS, VA-API on Linux; NVDEC as an
> option) run inside libavcodec's own decoders, so the index, the seek rule, `skip_frame = NONREF`
> and RASL handling apply unchanged. Only the frame a request returns is copied to system memory
> (`av_hwframe_transfer_data`, NV12/P010) and uploaded. Hardware seek and scrub decoders use one
> thread. Any hwaccel failure (no device, no hwaccel for the codec, stream not offered,
> initialisation failure) falls back to software for that stream; "is hardware" is decided per
> frame, and GPUs are selected by LUID or vendor, never by adapter index. Intra and proxy codecs
> stay on the CPU. A decoder/stream class is used in hardware only where the corpus test shows it
> pixel-exact against software after seeks (WP-0E.02 found one that is not: open-GOP H.264 on AMD
> D3D11VA, which needs the seek to start at an IDR).
>
> **Scrub budget.** With hardware decode the 4K scrub p95 stayed under 150 ms for keyframe
> intervals up to about 1.4 s (H.264) and about 4 s (HEVC Main10, RTX 3080; about 1.4 s on the iGPU) on the S3 machine. Longer-GOP originals, and any
> original on a machine where no pixel-exact hardware decoder opened, are scrubbed through intra
> proxies (arch §7 auto-proxy). The CPU-only long-GOP number is reported, not gated.
>
> **Zero-copy interop (path b) stays behind a feature flag.** It works on Windows (D3D11VA → shared
> NT handle → D3D12 → wgpu-hal import, synchronised by a shared fence; pixel-identical to path (a),
> 2-4× faster playback, no CPU copy) and is not yet measured on macOS (IOSurface → Metal) or Linux
> (dmabuf → Vulkan).

**Evidence table**, replace the "Path (a)" row and add three:

> | Path (a) works on every target OS | **Windows: pass** (WP-0E.02): D3D11VA on an RTX 3080 Laptop and a Radeon iGPU, NVDEC; H.264 4K30 and HEVC Main10 decode, upload and render (read-back check). macOS/Linux: WP-0E.03 |
> | Hardware decode frame-exact | **Pass with one exception:** 18 files × 3 decoders, 500/500 seeks each, every frame bit-exact against software; AMD D3D11VA corrupts open-GOP H.264 after a seek (fixed by starting at an IDR) |
> | 4K long-GOP scrub p95 < 150 ms **with hardware decode** | **Pass for 1-2 s GOPs on the RTX 3080** (1 s H.264: 116-134 ms; 2 s HEVC Main10: 81-89 ms; the iGPU 136 and 220 ms), **fail for an 8.3 s GOP** (714-752 ms on every decoder) |
> | Zero-copy | **Windows: works** (D3D11 → D3D12 shared handle → wgpu); macOS, Linux: not measured |

**Revisit triggers**, add: "if WP-0E.03 measures VideoToolbox or VA-API seeks far above these
Windows numbers, or finds a hardware decoder that is not pixel-exact after seeks on a common
stream type".

### Notes for WP-1D.04 (production hardware decode)

- **Reuse the shape of `src/hw.rs`.** `av_hwdevice_ctx_create` once per device (cache it: 0.2-1.2 s
  to open here), `avcodec_get_hw_config` to decide per codec, `hw_device_ctx` + a `get_format`
  that returns the hw format only if offered and otherwise the first software format. Do not set
  `AV_HWACCEL_FLAG_ALLOW_PROFILE_MISMATCH` or `IGNORE_LEVEL`: the failures they hide are the
  clean fallbacks.
- **`is_hw()` must be per frame, not per decoder.** A decoder with a device attached can still
  produce software frames (cases 3 and 4 of H6), and libavcodec can switch mid-stream when the
  stream parameters change and `get_format` runs again. Check `AVFrame.format` against the hw
  format, as `finish_frame` does, and count both (`Capabilities.hw_decode` should come from a
  probe decode, not from "the device opened").
- **Transfer only the returned frame.** Frames dropped on the way to a seek target stay on the
  GPU. Pool the destination `AVFrame`s (`av_hwframe_transfer_data` into a pre-allocated NV12/P010
  frame) instead of allocating 12 MB per 4K frame as the spike does.
- **Hardware seek decoders: one thread, `skip_frame = NONREF`, same seek rule.** Frame threads
  cost 2× seek latency on the hardware path (H3). The WP-0E.01 "drain to EOF before flush" rule
  stays (it is about libavcodec's output queue, not the hwaccel).
- **Keep intra and proxy codecs on the CPU** (ProRes, DNxHR, MJPEG: slice threads, about 1-2 ms
  per frame); MJPEG on NVDEC is not bit-exact (H8) and gains nothing.
- **Default device on Windows:** D3D11VA on the adapter that renders (the wgpu adapter's LUID →
  DXGI index, `zerocopy::dxgi_index` shows how), NVDEC only as an explicit choice. D3D11VA covers
  NVIDIA, AMD and Intel with one code path and is what path (b) builds on. NVDEC was 5-10 % faster
  per seek here and has the cheaper transfer, but it is NVIDIA-only.
- **The PSNR > 50 dB criterion is easy to meet; bit-exactness is the norm** for H.264/HEVC on
  these decoders (H1). A hardware frame that differs from software is worth a log line: it means a
  driver bug or a non-conformant stream.
- **Verification hooks to keep:** `S3_HW`-style forcing (`CUTWEAVE_HW=off|d3d11va|cuda`), the
  per-source status string, and a corpus test that runs the 100-seek frame-exact check with the
  hardware path forced on and forced off.

### Notes for WP-0E.03 (all platforms)

- Fill the macOS and Linux rows with the [run instructions](#run-instructions-for-wp-0e03-macos-linux-not-measured-here)
  below. Copy `s3-out/*.csv` into `results/<machine>/` as here.
- The Windows numbers here are from a shared, loaded laptop; re-run the bench on the reference
  Windows machine (WP-0I) before quoting them in ADR-0003, especially the 8.3 s GOP rows, which
  decide whether "hardware decode or proxies" or "proxies above N s GOP" is the rule.
- Zero-copy rows: Windows "works" (measured here), macOS and Linux "not measured" until someone
  implements the import (sketched in the run instructions). The FFmpeg-level checks there say
  whether it is worth implementing.

### Run instructions for WP-0E.03 (macOS, Linux: not measured here)

This machine has no Mac and no Linux GPU, so the macOS and Linux rows of every table above are
**not measured**. The harness is platform-neutral for path (a): `--hw videotoolbox` and
`--hw vaapi` go through the same `hw.rs` code, and `av_hwframe_transfer_data` returns NV12/P010
there too. Build as in [SETUP.md](SETUP.md), generate media (`python gen_media.py`), then:

**macOS arm64 (VideoToolbox):**

```sh
./target/release/s3 hwinfo                       # expect: videotoolbox opens; wgpu lists the Metal adapter
./target/release/s3 verify --quiet --hw videotoolbox --threads single            # all files, 500 seeks
./target/release/s3 verify --quiet --hw videotoolbox --threads frame --seeks 200 \
    media/h264_2160p30_gop30.mp4 media/hevc_main10_2160p30_gop60.mp4 media/hevc_main10_pq_1080p24.mp4
/usr/bin/time -l ./target/release/s3 bench --quiet --seeks 200 \
    --configs frame+skip,videotoolbox/single+skip,videotoolbox/single,videotoolbox/frame+skip \
    media/h264_2160p30_gop30.mp4 media/h264_2160p30_gop250.mp4 media/hevc_main10_2160p30_gop60.mp4
./target/release/s3 gpu --quiet --dump --configs frame+skip,videotoolbox/single+skip \
    media/h264_2160p30_gop30.mp4 media/h264_2160p30_gop250.mp4 media/hevc_main10_2160p30_gop60.mp4
S3_HW=videotoolbox cargo test --release -- --ignored hw_ gpu_
sudo powermetrics --samplers cpu_power,gpu_power -i 1000 > powermetrics.txt   # in a 2nd shell during bench
```

- CPU%: the `cores` columns are Windows only; use the `/usr/bin/time -l` user+sys time over the
  wall time, and the `GPU HW active residency` lines of `powermetrics` for GPU%. The decode engine
  has no separate counter on Apple silicon.
- The fallback test expects `videotoolbox:9` to fail as "no device"; if VideoToolbox ignores the
  device string, record that and run the other four fallback cases (`cargo test` reports which).
- **Zero-copy (IOSurface → Metal): not implemented in the spike.** Feasibility at the FFmpeg level,
  no code needed: `ffmpeg -hwaccel videotoolbox -hwaccel_output_format videotoolbox_vld -i
  media/hevc_main10_2160p30_gop60.mp4 -f null -` must decode at full speed with frames kept as
  `CVPixelBuffer`s. The import for WP-1D.04: `AVFrame.data[3]` is a `CVPixelBufferRef` →
  `CVPixelBufferGetIOSurface` → per plane `MTLDevice newTextureWithDescriptor:iosurface:plane:`
  (`R8Unorm`/`RG8Unorm`, or `R16Unorm`/`RG16Unorm` for 10-bit `x420`) →
  `wgpu::hal::metal::Device::texture_from_raw` → `create_texture_from_hal::<Metal>`, two
  single-plane textures and the shader in [yuv_zerocopy.wgsl](src/shaders/yuv_zerocopy.wgsl).
  VideoToolbox outputs a frame only when it is decoded, so no fence is needed. Record "works /
  works-with-caveats / not viable" from that implementation, or "not measured" if 0E.03 does not
  build it.

**Ubuntu 24.04 (VA-API):**

```sh
vainfo                                           # needs a real GPU: a VM without passthrough has no VA-API decode
./target/release/s3 hwinfo                       # expect: vaapi and vaapi:/dev/dri/renderD128 open
./target/release/s3 verify --quiet --hw vaapi --threads single
/usr/bin/time -v ./target/release/s3 bench --quiet --seeks 200 \
    --configs frame+skip,vaapi/single+skip,vaapi/single,vaapi/frame+skip \
    media/h264_2160p30_gop30.mp4 media/h264_2160p30_gop250.mp4 media/hevc_main10_2160p30_gop60.mp4
./target/release/s3 gpu --quiet --dump --gpu @vulkan --configs frame+skip,vaapi/single+skip \
    media/h264_2160p30_gop30.mp4 media/h264_2160p30_gop250.mp4 media/hevc_main10_2160p30_gop60.mp4
S3_HW=vaapi cargo test --release -- --ignored hw_ gpu_
# GPU %: `sudo intel_gpu_top -l` (Intel, "Video" engine), `radeontop -d -` (AMD, "UVD/VCN"),
#        `nvidia-smi dmon -s u` (NVIDIA through nvidia-vaapi-driver, "dec" column)
```

- **In the Linux VM:** with virtio-gpu/VirGL there is no VA-API decode, so `hwinfo` shows
  `vaapi fails`. Record path (a) as "not measured (VM, no GPU video decode)" and run the fallback
  test, which must pass there (every hw case falls back to software). Only a bare-metal machine or
  GPU passthrough gives HW numbers.
- **Zero-copy (dmabuf → Vulkan): not implemented in the spike.** FFmpeg-level feasibility check
  (same import FFmpeg's own `hwmap` does: VA-API surface → DRM PRIME dmabuf → `VkImage` with
  `VK_EXT_external_memory_dma_buf` + `VK_EXT_image_drm_format_modifier`):

  ```sh
  ffmpeg -init_hw_device vaapi=va:/dev/dri/renderD128 -init_hw_device vulkan=vk@va \
    -hwaccel vaapi -hwaccel_device va -hwaccel_output_format vaapi -i media/h264_2160p30_gop30.mp4 \
    -vf 'hwmap=derive_device=vulkan,hwdownload,format=nv12' -frames:v 60 -f framemd5 hw.md5
  ffmpeg -i media/h264_2160p30_gop30.mp4 -frames:v 60 -pix_fmt nv12 -f framemd5 sw.md5
  diff <(cut -d, -f6 hw.md5) <(cut -d, -f6 sw.md5)   # identical = the dmabuf import is exact
  ```

  The wgpu import for WP-1D.04: `av_hwframe_map` to `AV_PIX_FMT_DRM_PRIME` gives an
  `AVDRMFrameDescriptor` (fds, offsets, pitches, modifier) → `vkCreateImage` with
  `VkExternalMemoryImageCreateInfo` + `VkImageDrmFormatModifierExplicitCreateInfoEXT` (one
  `R8`/`R8G8` image per plane, or a multi-planar `G8_B8R8_2PLANE_420`) → `vkImportMemoryFdInfoKHR` →
  `wgpu::hal::vulkan::Device::texture_from_raw` → `create_texture_from_hal::<Vulkan>`. wgpu must
  enable those Vulkan extensions on its device (`open_with_callback`). Sync: VA-API's
  `vaSyncSurface` (FFmpeg maps after it) or an exported sync file.

## WP-0E.01 verdict (CPU decode)

**Recommendation for ADR-0003: GO with changes** ([details](#s3-findings-for-adr-0003-draft-for-the-maintainers)).

| WP-0E.01 acceptance / S3 pass criterion | Result on this machine |
|---|---|
| Harness prints `frame-exact: 500/500` for every file (CPU decode) | **Pass.** 16/16 files: H.264 at 23.976/25/29.97/30/60 fps in MP4/MKV/MOV/TS, closed and open GOP; 4K 1 s and 8.3 s GOP; 2 VFR files; 2 rotated; HEVC Main10 4K and HDR10; ProRes HQ; ProRes Proxy and MJPEG proxies. Each frame is checked by pts against ffprobe, Y-plane CRC against `ffmpeg` and a burned-in frame barcode. No mismatch in sequential, random, edge or scrub passes |
| Index (pts → keyframe/GOP map) by packet scan; VFR through the index | **Pass.** The packet list and frame pts equal ffprobe's on every file. Open-GOP leading frames are handled |
| Scrub benchmark p50/p95 for 4K long GOP and proxy | Printed. **4K long GOP: fail on CPU** (cold-seek p95 282 ms at a 1 s GOP, 428 ms for HEVC Main10, 804 ms at an 8.3 s GOP, vs 150). **Proxy: pass** (p95 0.2 ms ProRes Proxy, 1.0 ms MJPEG, vs 33) |
| Robust failure on broken files | **Pass.** 0 panics, crashes or hangs over 7 broken files and 200 isolated fuzz cases |
| Build/link story on Windows | Documented in [SETUP.md](SETUP.md). FFmpeg 8.1.3 + ffmpeg-next 9.1.0 work unpatched |

**Changes the recommendation asks for:**

- The 4K budget assumes hardware decode (WP-0E.02) or proxies.
- `cutweave-media` owns a small FFI layer (seek, flags, version lock) plus "drain before flush".
- `deny.toml` allows WTFPL.
- Intra decoders use slice threads.

## Run it

Setup, the environment and the build/link story are in **[SETUP.md](SETUP.md)**.

```powershell
. .\env.ps1                          # FFMPEG_DIR, LIBCLANG_PATH, FFmpeg DLLs on PATH
python gen_media.py                  # deterministic synthetic media -> media/ (git-ignored)
cargo test                           # 28 unit tests (no media; 18 in WP-0E.01)
cargo test --release -- --ignored    # 7 harness tests over media/ (4 from WP-0E.01, 3 hardware/GPU)
cargo build --release
.\target\release\s3.exe info         # probe: codec, pix_fmt, rotation, color tags, VFR, GOP
.\target\release\s3.exe verify       # frame-exact: N/500 per file, plus sequential/edge/scrub passes
.\target\release\s3.exe bench        # seek/scrub/step p50/p95 and decode fps per thread config
.\target\release\s3.exe audio        # odd sample rates: decoded sample counts vs ffprobe
.\target\release\s3.exe robust --isolate   # broken files, one child process each
.\target\release\s3.exe fuzz --source media\h264_1080p25_gop50.mkv --cases 40
.\target\release\s3.exe all          # info + verify + audio + robust + fuzz
$env:S3_TRACE=1; .\target\release\s3.exe seek --threads slice media\hevc_main10_pq_1080p24.mp4 142 138
                                     # debug: cold seeks, every packet sent / frame received
```

Every command takes explicit files instead of the manifest, for example corpus files from WP-0I.01:
`s3 verify D:\corpus\clip.mov`. Without the barcode, the check uses pts and the Y-plane CRC only.
Outputs (CSV, ground-truth cache, fuzz cases) go to `s3-out/` (git-ignored). The CSVs of the run
below are committed in [results/win11-dev/](results/win11-dev/).

## How it works

```text
            packet scan (no decode)                 seek(T)
 file ──► FrameIndex: packets in decode order ──► start keyframe K = latest key with
          frames in pts order, start key per frame      decode(K) <= decode(T) and pts(K) <= pts(T)
                │                                     ├─ forward-continue if K <= next packet and pts < T
                │                                     ├─ else avformat_seek_file(stream, ts <= ts(K)), flush,
                │                                     │   drop packets before K (read, never decoded),
                │                                     │   landed after K? retry earlier key, then reopen
                ▼                                     └─ decode forward: frames with pts < T dropped,
 ground truth (same FFmpeg build's CLI):                  their packets sent with skip_frame=NONREF
   ffprobe -show_packets / -show_frames                   pts == T -> done, pts > T -> Overshoot error
   ffmpeg -vf extractplanes=y -f framehash -hash crc32 (one Y CRC per frame)
   ffmpeg -vf select=eq(n\,N) spot checks (first, last, one random frame)
```

- **Index** (`src/index.rs`): one pass of `av_read_frame` over the video stream. It records pts,
  dts, the key flag, `AV_PKT_FLAG_DISCARD`, byte position and size per packet. Frames are packets
  with a timestamp, minus discard packets, sorted by pts. Each frame gets its **start keyframe**.
  Open-GOP leading pictures (decoded after a keyframe but shown before it) start from the previous
  keyframe, and packets before the first keyframe are marked undecodable. VFR has no special case:
  frames are addressed by their own pts, never by `n / fps`.
- **Seek** (`src/source.rs`):
  - **Seek timestamp.** The seek uses `avformat_seek_file` on the video stream in its own time
    base, with `max_ts = ts(K)`. It passes the keyframe's **dts** for MP4/MOV (sample-table index
    in DTS) and MPEG-TS (DTS bisection), and its **pts** for Matroska (cues are pts).
  - **Landing check.** Every packet read after the seek is located in the index by (dts, pts),
    then by pts, then by byte position. Packets before K are dropped without being decoded. If
    the demuxer landed after K, the seek retries from an earlier keyframe, and as a last resort
    reopens the file.
  - **Flush after EOF.** If the decoder was sent EOF, it is drained to `AVERROR_EOF` before the
    flush (F7).
  - **Decode-forward.** Packets shown before T are sent with `skip_frame = AVDISCARD_NONREF`, so
    the decoder skips non-reference B-frames it would only throw away. Frames with pts < T are
    dropped; the first frame with pts == T is the result.
  - **Forward-continue.** When the decoder already sits between K and T (scrubbing forward), it
    just keeps decoding.
  - **Sequential mode.** `next_frame` decodes everything in order.
- **Checks per frame** (`src/verify.rs`, `src/frame_check.rs`):
  1. Its pts equals the index pts and ffprobe's.
  2. Its Y-plane CRC-32 equals the ffmpeg framehash row (FFmpeg's `framehash` CRC32 is zlib's;
     `extractplanes=y` hashes the rows without stride padding, and so does the harness).
  3. Its burned-in 16-bit barcode equals the presentation index. This check is independent of
     FFmpeg.

  Passes:
  - sequential decode of every frame;
  - **500 seeded random cold seeks** (`frame-exact: N/500`);
  - edges: the first and last four frames, every keyframe ±1 and every open-GOP leading frame,
    forward then reversed;
  - scrub: a sweep of about 120 positions plus 45 single-frame steps, with forward-continue.
- **Bench** (`src/bench.rs`, release build): latency is from request to a decoded frame in CPU
  memory, in the native pixel format, with no conversion or upload. It covers 200 random cold
  seeks, a scrub sweep forward then back (about 240 requests with forward-continue), 60
  single-frame steps, and sequential decode fps. The OS file cache is warm because the index scan
  just read the file.
- **Robustness** (`src/robust.rs`): open (index scan + decoder), decode everything, then 25 random
  seeks. Panics are caught in process. With `--isolate` (and always for `fuzz`) each file runs in
  a child process with a 120 s timeout, so a native crash or a hang inside FFmpeg shows up as an
  outcome instead of killing the harness.
- **`unsafe`**: three FFI calls in `src/ffi.rs` (per-stream `avformat_seek_file`, the raw packet
  flags, the pixel-format component descriptor), each with a `// SAFETY:` comment. Nothing else
  in WP-0E.01; WP-0E.02 adds the hwaccel FFI (`src/hw.rs`), a CPU-time probe and the
  feature-gated zero-copy COM code (`src/zerocopy.rs`).

## Generated media

`gen_media.py` makes every file from FFmpeg lavfi sources with the bundle's own `ffmpeg`
(`testsrc2` + temporal noise for a camera-like bitrate + a 16-bit frame-number barcode in the top
two bands). It is deterministic: fixed sources, seeds and encoder settings, and `-fflags
+bitexact`. `media/manifest.tsv` records the sha256 of each file. Total: about 590 MB in about 8
minutes on this machine.

| File | What it exercises |
|---|---|
| `h264_1080p23976_gop48.mp4` | 24000/1001, closed GOP 48, 3 B-frames (pyramid), MP4 edit list (negative dts) |
| `h264_1080p25_gop50.mkv` | Matroska: 1 ms time base, no dts on the first packets of a run, pts cues |
| `h264_1080p2997_gop60_opengop.mp4` | 30000/1001, **open GOP** (x264 `open-gop`): 9 leading B-frames that reference the previous GOP |
| `h264_1080p30_gop30.mov` | QuickTime MOV, GOP 30 |
| `h264_1080p60_gop120.mp4` | 60 fps, GOP 120 |
| `h264_1080p25_gop25.ts` | MPEG-TS: no container index (bisection seek), start time 1.48 s |
| `h264_2160p30_gop30.mp4` | 4K30 H.264 at 50 Mb/s, 1 s GOP (camera-like), AAC 44.1 kHz |
| `h264_2160p30_gop250.mp4` | **4K long GOP**: 250 frames (x264 default, 8.3 s), 50 Mb/s, 20 s |
| `h264_vfr_phone_1080p_rot90.mp4` | **VFR phone-like**: about 30 fps ±4 ms jitter, a dropped frame every 37, a 24 fps stretch; 90 kHz; **display matrix 90°**; AAC 48 kHz |
| `h264_vfr_rates_1080p.mkv` | VFR by concatenated rates: 30 → 60 → 24 fps |
| `h264_720p30_rot270.mp4` | Display matrix 270° |
| `hevc_main10_2160p30_gop60.mp4` | x265 **Main10** 4K, 40 Mb/s, open GOP (CRA + RASL), 4 B-frames |
| `hevc_main10_pq_1080p24.mp4` | x265 Main10 **HDR10**: BT.2020 / PQ / mastering display + MaxCLL SEI |
| `prores_hq_1080p25.mov` | ProRes 422 HQ, 10-bit 4:2:2, intra |
| `proxy_prores_540p.mov`, `proxy_mjpeg_540p.mov` | Intra **proxies** of the 4K long-GOP clip (960x540, ProRes Proxy / MJPEG) |
| `audio_22050_mono_s16.wav`, `audio_44100_stereo_aac.m4a`, `audio_96000_stereo_flac.flac` | Odd sample rates |
| `broken_zero.mp4`, `broken_garbage.mp4`, `broken_header_only.mp4` | Empty, 1 MiB random, first 1 KiB of an MP4 |
| `broken_truncated_moov_at_end.mp4`, `broken_truncated_faststart.mp4` | MP4 cut at 60 %: moov lost / moov intact but samples missing |
| `broken_bitflips.mkv`, `broken_truncated.ts` | One flipped byte every 4 KiB through the middle third; TS cut mid-packet |

Plus `fuzz`: 40 deterministic corruptions (truncation, bit flips, zeroed ranges, header garbage,
swapped blocks) of each of five valid files.

**What synthetic media cannot reproduce (for WP-0E.03 / the WP-0I.01 corpus):**

- **Encoders other than x264/x265/prores_ks:** Apple VideoToolbox HEVC from iPhones (HEVC with
  alpha, Dolby Vision profile 8.4 RPUs, multi-layer MV-HEVC spatial video), Samsung/Pixel
  MediaCodec bitstreams, camera AVC-Intra/XAVC/XF-AVC, Sony S-Log metadata, Apple ProRes RAW
  and Blackmagic RAW. Decoders take different paths for their reference structures, SEI and
  slice layouts.
- **Real VFR from phones:** variable timing driven by the capture clock, with an `elst` that
  starts at a non-zero media time, sometimes `ctts` v1 negative offsets, and a 600 Hz time base
  where two frames can be 1 tick apart. The synthetic VFR here is smooth by construction.
- **Interlaced and telecined content** (field pairs as one packet or two, `repeat_pict`),
  broadcast TS with PCR discontinuities and timestamp wraps (33-bit), and multi-program TS.
- **Damage real files carry:** recordings cut by a dead battery (moov missing, recoverable only
  by `untrunc`-style tools), files still being written, sparse or edit-list-heavy MOVs from NLEs
  (multiple `elst` entries, empty edits), and MKV without cues.
- **Large and long files:** multi-GB, hour-long, 8K and 120 fps files. These change index scan
  time (I/O bound, see findings) and memory.
- **Audio:** AAC priming/padding from other encoders, MP3 in MP4, multichannel layouts, and
  32-bit float / 24-bit PCM in MOV.
- **HDR:** HDR10+ dynamic metadata, Dolby Vision, HLG from phones. The synthetic PQ clip only
  checks that the tags survive.

## Results (Windows 11 dev machine, informal)

> **INFORMAL.** These come from one developer laptop, not the reference hardware (WP-0E.03).
> Other agents were building on the same machine during the runs, and the media sits on a USB
> HDD (warm OS cache for every timed run).

**Machine:** AMD Ryzen 9 5900HX (8C/16T), 32 GB, Windows 11 Pro 26200, media on a WD My Passport
USB HDD (D:). FFmpeg 8.1.3-39-g660b5aa7db (BtbN win64 gpl-shared), ffmpeg-next 9.1.0 /
ffmpeg-sys-next 9.1.1, Rust 1.100.0. Decode is CPU only.

### Frame-exactness: 16/16 files, `frame-exact: 500/500` each, no mismatch anywhere

`s3 verify` uses the default configuration: frame threads, skip non-reference frames before the
target. Every row was checked three ways (pts, Y CRC, barcode). The CSV is
[results/win11-dev/verify.csv](results/win11-dev/verify.csv).

| File | Frames | GOP max | Max decode-forward | Open-GOP leading | Index vs ffprobe (packets / frame pts) | Sequential | **Random seeks** | Edges (fwd+rev) | Scrub |
|---|---|---|---|---|---|---|---|---|---|
| h264_1080p23976_gop48.mp4 | 240 | 48 | 48 | 0 | 240/240 / 240/240 | 240/240 | **500/500** | 36/36 | 165/165 |
| h264_1080p25_gop50.mkv | 250 | 50 | 50 | 0 | 250/250 / 250/250 | 250/250 | **500/500** | 36/36 | 170/170 |
| h264_1080p2997_gop60_opengop.mp4 | 300 | 60 | 64 | **9** | 300/300 / 300/300 | 300/300 | **500/500** | 46/46 | 195/195 |
| h264_1080p30_gop30.mov | 300 | 30 | 30 | 0 | 300/300 / 300/300 | 300/300 | **500/500** | 66/66 | 195/195 |
| h264_1080p60_gop120.mp4 | 600 | 120 | 120 | 0 | 600/600 / 600/600 | 600/600 | **500/500** | 36/36 | 165/165 |
| h264_1080p25_gop25.ts | 200 | 25 | 25 | 0 | 200/200 / 200/200 | 200/200 | **500/500** | 54/54 | 245/245 |
| h264_2160p30_gop30.mp4 | 300 | 30 | 30 | 0 | 300/300 / 300/300 | 300/300 | **500/500** | 66/66 | 195/195 |
| h264_2160p30_gop250.mp4 | 600 | 250 | 250 | 0 | 600/600 / 600/600 | 600/600 | **500/500** | 24/24 | 165/165 |
| h264_vfr_phone_1080p_rot90.mp4 (VFR) | 300 | 30 | 30 | 0 | 300/300 / 300/300 | 300/300 | **500/500** | 66/66 | 195/195 |
| h264_vfr_rates_1080p.mkv (VFR) | 300 | 30 | 30 | 0 | 300/300 / 300/300 | 300/300 | **500/500** | 66/66 | 195/195 |
| h264_720p30_rot270.mp4 | 180 | 30 | 30 | 0 | 180/180 / 180/180 | 180/180 | **500/500** | 42/42 | 225/225 |
| hevc_main10_2160p30_gop60.mp4 | 180 | 60 | 65 | **8** | 180/180 / 180/180 | 180/180 | **500/500** | 36/36 | 225/225 |
| hevc_main10_pq_1080p24.mp4 (HDR10) | 144 | 48 | 51 | **4** | 144/144 / 144/144 | 144/144 | **500/500** | 28/28 | 189/189 |
| prores_hq_1080p25.mov | 100 | 1 | 1 | 0 | 100/100 / 100/100 | 100/100 | **500/500** | 200/200 | 145/145 |
| proxy_prores_540p.mov | 600 | 1 | 1 | 0 | 600/600 / 600/600 | 600/600 | **500/500** | 600/600 | 165/165 |
| proxy_mjpeg_540p.mov | 600 | 1 | 1 | 0 | 600/600 / 600/600 | 600/600 | **500/500** | 600/600 | 165/165 |

- **Counters.** Across all files there were 0 seek-landing retries, 0 reopens, 0 index/demux
  mismatches during decode-forward, 0 decode errors and 0 concealed frames.
- **`select` spot checks.** The `select=eq(n\,N)` checks (first, last and one random frame per
  file) matched the framehash rows 48/48.
- **Slice threads.** The same 16 files under slice threads (`verify --threads slice --seeks 100`,
  [verify-slice.csv](results/win11-dev/verify-slice.csv)) are also exact everywhere: 100/100
  random seeks plus all passes. This was after the F7 workaround.
- **Harness tests.** `cargo test --release -- --ignored` passes 4/4 in 26 min:
  - `every_file_is_frame_exact_on_500_random_seeks` asserts all of the above;
  - `exact_without_skip_and_with_slice_threads` repeats three files with skip off and with slice
    threads;
  - `broken_files_fail_cleanly`;
  - `repeated_and_continued_requests_return_the_target`.

`s3 info` agrees with the generator on every expectation: rotation 90/270 from the display
matrix, VFR on exactly the two VFR files, 10-bit on HEVC Main10 and ProRes, BT.2020/PQ tags on the
HDR10 clip, and GOP sizes (counted in presentation order; in decode order x264's open GOP
looks 61 frames long).

### Timing (release build, CPU decode only, warm cache)

`s3 bench` CSVs: [bench-a.csv](results/win11-dev/bench-a.csv) (8 files × 4 configurations),
[bench-b.csv](results/win11-dev/bench-b.csv) (4K long GOP),
[bench-c.csv](results/win11-dev/bench-c.csv) (single thread).

- **Configurations:** `frame` = frame threads (FFmpeg auto: min(cores + 1, 16) = 16 here);
  `frame:4` = 4 frame threads; `slice` = slice threads; `single` = one thread; `+skip` =
  `AVDISCARD_NONREF` before the target.
- **Measurement:** seek = 200 cold random seeks (100 for the 4K long GOP). The scrub sweep is
  about 240 requests forward then backward with forward-continue. Step is 60 single-frame steps
  forward.
- **Background load:** another agent's builds kept the machine at **70-90 % CPU** during the
  run. Treat the p95 values as pessimistic and the ratios between configurations as the signal.

**Against the S3 / arch §19 budgets** (best configuration per row):

| File | Best config | Seek p50 | **Seek p95** | Scrub sweep p50 / p95 | Step p50 | Decode fps | Budget | Verdict |
|---|---|---|---|---|---|---|---|---|
| 4K H.264, 1 s GOP (`h264_2160p30_gop30`) | frame+skip | 192 ms | **282 ms** | 120 / 253 ms | 0.8 ms | 216 | 4K long GOP p95 < 150 ms | **fail (CPU)** |
| 4K H.264, 8.3 s GOP (`h264_2160p30_gop250`) | frame+skip | 411 ms | **804 ms** | 206 / 950 ms | 0.7 ms | 153 | p95 < 150 ms | **fail (CPU)** |
| 4K HEVC Main10, 2 s GOP | frame+skip | 295 ms | **428 ms** | 218 / 412 ms | 2.5 ms | 115 | p95 < 150 ms | **fail (CPU)** |
| ProRes Proxy 540p (proxy of the 4K long GOP) | slice | 0.17 ms | **0.20 ms** | 0.16 / 0.19 ms | 0.15 ms | 6 449 | proxy p95 < 33 ms (S3), < 80 ms (§19) | **pass** |
| MJPEG 540p proxy | single+skip | 0.6 ms | **1.0 ms** (slice 1.5) | 0.6 / 0.9 ms | 1.1 ms | 1 439 | proxy p95 < 33 ms | **pass** |
| ProRes 422 HQ 1080p (intra source) | slice | 2.0 ms | **2.4 ms** | 2.1 / 2.4 ms | 2.1 ms | 497 | intra p95 < 80 ms (§19) | **pass** |
| 1080p H.264, 2 s GOP | frame+skip | 66 ms | 91 ms | 41 / 89 ms | 0.5 ms | 790 | (no budget; for scale) | — |
| 1080p HEVC Main10 HDR10, 2 s GOP | frame+skip | 63 ms | 86 ms | 46 / 79 ms | 0.3 ms | 567 | — | — |
| 1080p VFR phone clip, 1 s GOP | frame+skip | 28 ms | 95 ms | 17 / 90 ms | 0.4 ms | 713 | — | — |

**Threading and skipping** (seek p50 / p95 ms, packets fed per seek):

| File | frame+skip | frame | frame:4+skip | slice+skip | single+skip | single |
|---|---|---|---|---|---|---|
| 1080p H.264 GOP 48 | **66 / 91** (40) | 70 / 96 (40) | 96 / 158 (29) | 181 / 329 (26) | 180 / 311 (26) | 240 / 456 (26) |
| 4K H.264 GOP 30 | **192 / 282** (33) | 201 / 306 (33) | 224 / 409 (21) | 335 / 666 (18) | 334 / 660 (18) | 474 / 938 (18) |
| 4K H.264 GOP 250 | **411 / 804** (114) | 665 / 1 989 (114) | 861 / 1 958 (102) | 1 639 / 3 752 (99) | n/m | n/m |
| 4K HEVC Main10 GOP 60 | **295 / 428** (50) | 439 / 693 (50) | 382 / 617 (39) | overshoot (F7) | n/m | n/m |
| 1080p HEVC HDR10 GOP 48 | **63 / 86** (41) | 76 / 113 (41) | 86 / 152 (30) | overshoot (F7) | n/m | n/m |
| ProRes HQ 1080p (intra) | 30 / 44 (15) | 35 / 48 (15) | 19 / 20 (4) | **2.0 / 2.4** (1) | 17 / 18 (1) | 17 / 20 (1) |
| ProRes Proxy 540p | 4.3 / 5.1 (16) | 4.3 / 5.1 (16) | 1.1 / 1.3 (4) | **0.17 / 0.20** (1) | 1.1 / 1.6 (1) | 1.1 / 1.5 (1) |
| MJPEG 540p | 0.8 / 1.1 (1) | 0.9 / 1.4 (1) | 1.1 / 1.8 (1) | 1.0 / 1.5 (1) | **0.6 / 1.0** (1) | 0.6 / 0.9 (1) |

`n/m` means not measured. The slice-thread HEVC runs hit the decoder bug described in F7 before
it was worked around; they were not re-run.

What the tables show:

- **Long-GOP seek latency** is decode-forward distance × per-frame time plus the frame-thread
  pipeline. Frame threading wins for long-GOP sources despite its pipeline delay, because x264
  and x265 streams are single-slice: slice threads equal a single thread on them (181 vs 180 ms).
- **On intra sources frame threading is pure loss.** ProRes HQ seeks take 15× longer (30 ms vs
  2 ms), because the decoder must take about one packet per thread before the first frame comes
  out. ProRes decodes in parallel slices, so slice threads also beat a single thread 8× (2 vs
  17 ms). **Intra and proxy decoders must use slice threading.**
- **`skip_frame = NONREF` is worth keeping:**
  - single-threaded it saves 25-30 % on x264 (180 vs 240 ms at 1080p, 334 vs 474 ms at 4K);
  - with frame threads the pipeline hides most of it on x264 (−5 % at 1 s GOP; the GOP-250
    numbers are noisy);
  - it saves **33 % at p50 and 38 % at p95 on x265 4K**, which has more non-reference
    B-frames.
- **Step and scrub with forward-continue are cheap.** A single-frame step forward costs one
  packet (p50 0.3-2.5 ms even at 4K). The sweep p50 is roughly half the cold seek, because only
  backward moves and GOP jumps re-seek.
- **Index scan (open).** It is I/O bound.
  - Warm cache: about 60 ms for a 20 MB 1080p file and 176 ms for the 123 MB 4K file.
  - Cold, from the USB HDD while other jobs read it: 4-22 s per file (the `index_ms` column of
    [info.csv](results/win11-dev/info.csv) is such a run).
  - Decode and seek latency are measured warm.

### Failure handling: no panic, no crash, no hang

Broken files (`s3 robust --isolate`, [results/win11-dev/robust.csv](results/win11-dev/robust.csv)):

| File | Outcome |
|---|---|
| `broken_zero.mp4` (0 B), `broken_garbage.mp4` (1 MiB random), `broken_truncated_moov_at_end.mp4` | `avformat_open_input`: `AVERROR_INVALIDDATA` → `Error::Ffmpeg` at open, 3-140 ms |
| `broken_header_only.mp4` (1 KiB) | `avformat_open_input`: `AVERROR_EOF` at open |
| `broken_truncated_faststart.mp4` | Opens. The moov declares 240 samples; the scan indexes **149**, 148 decode, and a seek to the 149th returns `EndOfStream` (typed). 24/25 random seeks succeed |
| `broken_bitflips.mkv` | All 250 frames decode and 25/25 seeks succeed. **FFmpeg never returns an error**, but **160 of 250 frames come back flagged as concealed** (`decode_error_flags`). The damage is visible only there |
| `broken_truncated.ts` | 104 frames indexed and decoded (2 flagged concealed), 25/25 seeks |

Fuzz (`s3 fuzz`, 40 deterministic corruptions each, every case in its own process with a 120 s
timeout):

| Source | Clean decode | Typed error | Concealed frames | Panics / crashes / hangs |
|---|---|---|---|---|
| h264_1080p23976_gop48.mp4 | 24 | 16 | 582 | **0** |
| h264_1080p25_gop50.mkv | 31 | 9 | 581 | **0** |
| hevc_main10_pq_1080p24.mp4 | 20 | 20 | 0 | **0** |
| h264_1080p25_gop25.ts | 39 | 1 | 446 | **0** |
| prores_hq_1080p25.mov | 24 | 16 | 54 | **0** |

Typed errors seen:

- open failures (`InvalidData`/`EOF`);
- `EmptyIndex` (an MKV cut before its first keyframe);
- `EndOfStream` (truncated tails);
- `Overshoot`: HEVC drops a damaged frame instead of concealing it, so the seek for that pts
  sees the next frame. 3 cases.

The harness has no `unsafe` beyond the three FFI calls, and it ran clean in debug builds (debug
assertions on) as well.

### Audio at odd sample rates

`s3 audio`: the decoded sample count equals ffprobe's for every file. pcm_s16le 22.05 kHz mono
154 350; AAC 44.1 kHz 309 248; FLAC 96 kHz 672 000; AAC 44.1 kHz in the 4K clip 440 320; AAC
48 kHz in the phone clip 504 832. AAC priming is removed by FFmpeg (`skip_samples`), but the
encoder's end padding is not: the 7 s m4a decodes to 309 248 samples, not 308 700.

## Findings

### F1. ffmpeg-next 9.1.0 works with FFmpeg 8.1.3, with no fork and no patch

- **Version.** `ffmpeg-next = "=9.1.0"` / `ffmpeg-sys-next 9.1.1` are the versions the root
  `Cargo.toml` already pins for `cutweave-media` (feature `ffmpeg`). They were in the root
  lockfile and the local registry cache, so nothing was fetched. No newer release was needed.
- **8.1 support.** The sys build script detects lavc 62.28 and enables `ffmpeg_8_0` +
  `ffmpeg_8_1`. ffmpeg-next uses them for the FFmpeg 8 API changes: stream side data via
  `codecpar->coded_side_data`, `AV_FRAME_FLAG_KEY` instead of `key_frame`, and the
  `AV_PKT_DATA_EXIF` variant. The display-matrix rotation is read through that path and is
  correct.
- **No ABI problems seen** in about 9 000 verified seeks, every frame of 16 files decoded
  sequentially, the scrub passes and 200 fuzz cases.
- **`ffmpeg-the-third` was not needed**, so it was not evaluated. The binding layer the project
  needs is thin (F2), which keeps a later switch cheap.

### F2. What `cutweave-media` has to add on top of ffmpeg-next (WP-1D.01)

1. **Three FFI calls ffmpeg-next does not expose safely** (the spike's only `unsafe`, in
   `src/ffi.rs`):
   - **Per-stream seek.** `Input::seek` always passes stream index -1, so FFmpeg reads the
     timestamp as microseconds. A frame-exact seek needs `avformat_seek_file(stream, ts)` in the
     stream time base.
   - **`AV_PKT_FLAG_DISCARD`.** `packet::Flags` knows only KEY and CORRUPT and truncates the
     rest. Edit-list pre-roll packets must be decoded but are never shown.
   - **The pixel-format component descriptor** (depth/step/plane), needed for plane-exact
     handling of 10-bit and 4:2:2.
2. **Panics in ffmpeg-next to fence off:**
   - `format::input` panics on a non-UTF-8 path or an interior NUL (`from_path` unwraps). Paths
     must be validated before the call.
   - `frame::Video::data(i)` panics past `planes()`.
   - `PacketIter` (`input.packets()`) ends **silently** on any error other than EOF/InvalidData,
     so truncation looks like EOF. Drive `Packet::read` directly, as `src/source.rs` does.
3. **Runtime/header version lock.**
   - **The hazard.** bindgen maps C enums to Rust enums, and ffmpeg-next converts with exhaustive
     `match`. A runtime libavcodec **newer than the headers** that returns a new enum value (a
     new `AVPixelFormat`, a new codec id) produces an invalid Rust enum value: undefined
     behaviour, not an error. The `non-exhaustive-enums` feature only turns this into
     `unimplemented!()` panics.
   - **Rule.** The headers we build against and the DLLs we load must be the **same bundle**.
     The engine should compare `avcodec_version()` / `avformat_version()` /
     `avutil_version()` at start-up with the build-time `LIBAV*_VERSION_INT` and refuse to run
     on a mismatch.
4. **Logging.**
   - FFmpeg writes to stderr through `av_log`. Seeks into open GOPs log `mmco: unref short
     failure` hundreds of times; it is harmless (those frames are discarded).
   - ffmpeg-next only offers `log::set_level`. Routing into `tracing` needs
     `av_log_set_callback` with a C-variadic `va_list` callback, which is awkward in Rust. A tiny
     C shim (`cc`) that formats with `av_log_format_line2` is the usual answer.
5. **License policy.** `ffmpeg-next` and `ffmpeg-sys-next` are **WTFPL**, and `cargo deny`
   rejects them today: `licenses FAILED` in this spike. The root `cargo deny check` passes only
   because the `ffmpeg` feature is off. WP-1D.01 must add `WTFPL` to `deny.toml` (the FSF lists
   it as free and GPL-compatible) with a note, or the first `--features ffmpeg` gate fails. The
   FFmpeg bundle's `zimg` is also WTFPL.

### F3. Frame-exact seek works with this rule set (all files exact, 0 retries)

- **Index from a packet scan, frames addressed by pts.** The index pts list equals ffprobe's on
  every file, VFR and TS included. Nothing assumes a constant frame rate.
- **Start keyframe = latest keyframe K with decode(K) ≤ decode(T) and pts(K) ≤ pts(T).**
  - **The open-GOP trap.** The naive rule ("the keyframe before T in decode order") fails on
    open GOPs. The 9 / 8 / 4 leading pictures of the x264 open-GOP, HEVC Main10 and HDR10 files
    are decoded after a keyframe but shown before it, and they reference the previous GOP.
  - **What breaks.** Starting at that keyframe, FFmpeg drops them (HEVC RASL after a flush) or
    conceals them (H.264). The seek then overshoots or returns a wrong picture.
  - **The fix.** The pts(K) ≤ pts(T) condition sends them to the previous keyframe. The edge pass
    checks every one of them in both directions.
- **Never trust where the demuxer lands.**
  - MP4/MOV index entries are DTS, Matroska cues are pts, and MPEG-TS has no index (bisection on
    DTS lands on arbitrary packets).
  - Matroska reports **no dts** for the first packets of a run and reconstructs dts differently
    after a seek. The index lookup therefore falls back from (dts, pts) to pts alone.
  - The harness identifies every packet after a seek in the index, drops (without decoding)
    whatever precedes K, and retries from an earlier keyframe if it landed late. On these files
    that never had to happen (0 retries) because the right timestamp was chosen per demuxer. The
    check is still what makes the result trustworthy on unknown files.
- **Bit-exact with the CLI.** The Y-plane CRC of every returned frame equals the ffmpeg CLI's,
  across frame/slice threads and with or without skipping non-reference frames. Same libavcodec,
  same output: the frame identity is exact, not approximate.
- **VFR needs nothing special.** The jittered phone clip (deltas 2 640–6 847 ticks of 1/90000)
  and the 30→60→24 fps MKV pass like CFR files.

### F4. Decode-forward cost dominates seek latency, and frame threading is both the cure and the tax

The numbers are below in [Timing](#timing-release-build-cpu-decode-only-warm-cache).

- **Frame threading.** libavcodec frame threading (16 threads here) gives the throughput (4K
  H.264 about 216 fps) but adds a pipeline delay of about one frame per thread. A cold seek feeds
  about 15 extra packets before the target comes out (packets per seek: 40 with frame threads vs
  26 with slice threads on the 48-frame GOP).
- **Slice threading.** It adds no delay, but x264/x265 streams are one slice per frame, so it
  degenerates to single-threaded decode (about 113 fps at 1080p, 40 fps at 4K).
- **Net effect: pick threading per codec class.**
  - Long-GOP decoders: frame threads. They win on every long-GOP file despite the delay.
  - Intra and proxy decoders (ProRes, DNxHR, MJPEG): slice threads. Frame threads cost 15×
    there: ProRes HQ 30 ms vs 2 ms.
  - Playback decoders are separate instances tuned for throughput.
- **`skip_frame = NONREF` for pre-target packets.** It is exact (verified in every pass) and
  cuts seek latency by 25-30 % single-threaded on x264 and by 33-38 % on x265 with frame threads.
  With frame threads on x264 it saves only about 5 %, because the B-pyramid keeps one of three
  B-frames as a reference and the pipeline hides the rest. It costs nothing, so keep it.

### F5. Robustness: FFmpeg reports little, the index must carry the truth

- **No panic, crash or hang** on 7 broken files and 200 fuzz cases, with every case in its own
  process.
- **Corruption is mostly silent.**
  - H.264 conceals damaged frames and returns success: 1 663 concealed frames across the fuzz
    set (H.264 and ProRes), with only 63 API-level decode errors (42 of them HEVC). Detection
    means checking `AV_FRAME_FLAG_CORRUPT` / `decode_error_flags` per frame.
  - HEVC drops damaged frames, so a seek to their pts sees the next frame (`Overshoot`).
  - Product behaviour should be: deliver the nearest following frame, flag the asset as
    damaged, and never block the timeline.
- **Container metadata lies on damaged files.** The truncated faststart MP4's moov declares 240
  samples; 149 packets exist and 148 decode. Duration and frame count must come from the index,
  not from `nb_frames`/`duration`.
- **Index scan cost is I/O.**
  - `av_read_frame` reads every payload byte, so the scan is a full file read: 123 MB in 176 ms
    warm (about 0.7 GB/s), and 14-19 s for 20 MB on the busy USB HDD when cold.
  - An hour of 4K at 100 Mb/s is 45 GB. The index must stay a background job (arch §7 already
    says so), with a "seek before the index is ready" fallback: plain demuxer seek, then decode
    forward and check pts. That fallback is exact except for open-GOP leading frames.
  - For MP4/MOV the sample table already holds dts, keyframes and positions (`AVIndexEntry`).
    Pts additionally needs `ctts`, so building from the moov instead of a scan is a possible
    optimisation for WP-1D.02.

### F6. Build and link (details in [SETUP.md](SETUP.md))

- **`FFMPEG_DIR` → MSVC import libs → DLLs.**
  - The first build takes about 6 minutes (bindgen over the FFmpeg headers, a 9 MB
    `bindings.rs`). It is cached by sccache afterwards.
  - The sys build script compiles and runs a C version probe with `cl.exe`, so MSVC Build Tools
    and libclang are build prerequisites.
- **Run-time closure** for demux+decode: `avcodec-62`, `avformat-62`, `avutil-60` **and
  `swresample-6`** (avcodec imports it), plus OS/UCRT DLLs.
- **Every test binary needs the DLLs on the loader path.** Without them the process fails
  before `main` with `STATUS_DLL_NOT_FOUND` (Git Bash misreports it as a missing
  `api-ms-win-crt-*.dll`). WP-1D.01 options: `PATH` in `just`/nextest config, or a build script
  that copies the DLLs next to `target/*/deps`.

### F7. An FFmpeg 8.1 HEVC decoder quirk that only the verifier would catch

- **Symptom.** With HEVC on slice threads or a single thread (not frame threads), a seek returned
  the **wrong frame**. In the benchmark, 1 of 100 random seeks on the HDR10 clip and the slice
  run on the 4K clip ended in `Overshoot`.
- **Reproducer.** `s3 seek --threads slice media/hevc_main10_pq_1080p24.mp4 142 138`.
  1. Reaching frame 142 needs an EOF drain, because its packet is the last in decode order.
  2. The harness returns as soon as 142 comes out, while frame 143 is still queued for output.
  3. `avcodec_flush_buffers` does **not** discard that queued frame: after the seek to frame
     138's keyframe, the decoder emits 143 first.
- **Not involved:** skipping non-reference frames (it happens with `--no-skip-nonref` too). The
  H.264 slice-thread benchmark runs never hit it, but the workaround is applied to every codec.
- **Workaround.** In `VideoSource::land_at`, after an EOF has been sent, drain the decoder to
  `AVERROR_EOF` before flushing. Since then, `verify --threads slice` is exact on these files
  (200/200), and the edge pass now always visits the last four frames to keep this case covered.
- **Lessons:**
  1. The frame-identity check (pts + CRC + barcode) on every returned frame is not optional. Any
     production decoder wrapper needs the same "drain after EOF, then flush" rule, or it should
     recreate the decoder.
  2. This is worth an upstream FFmpeg bug report once reduced to the plain API (the maintainers
     can decide).

## S3 findings for ADR-0003 (draft for the maintainers)

ADR-0003 stays **Proposed**. WP-0E.02 (hardware decode) and WP-0E.03 (all platforms) are still
open, and only maintainers accept ADRs (WP-0J.02). This is the WP-0E.01 input.

**Recommendation: GO with changes.**

**Confirmed on Windows.**

- `ffmpeg-next` 9.1 over the pinned FFmpeg 8.1 shared build is sound. It builds against MSVC
  import libraries, needs no fork, and showed no ABI problems in this run.
- Index-driven frame-exact seek is exact on every case tried: 16 files covering H.264 (closed and
  open GOP, B-pyramid), HEVC Main10 (CRA/RASL), HDR10, ProRes, MJPEG, VFR, rotation, MP4, MOV,
  MKV and TS. That is 500/500 random seeks per file plus every edge frame, checked bit-exactly
  against the CLI.
- Failure handling is clean: no panic, crash or hang on 7 broken files and 200 fuzz cases.

**What fails or needs a change.**

1. **The 4K long-GOP scrub budget (p95 < 150 ms) is not met by CPU decode.** Measured cold
   random-seek p95 here, under heavy background load:
   - **282 ms** for 4K H.264 with a 1 s GOP;
   - **428 ms** for 4K HEVC Main10 with a 2 s GOP;
   - **804 ms** for 4K H.264 with an 8.3 s GOP.

   Even the p50 (192 / 295 / 411 ms) is over budget. The cost is decode-forward distance ×
   per-frame decode time (4K H.264 at about 216 fps, HEVC Main10 at about 115 fps), so no seek
   strategy fixes it. The budget needs one or more of:
   - (a) hardware decode (WP-0E.02 must measure it with this harness);
   - (b) the intra proxy path, already in arch §7 ("auto-proxy for long-GOP or over-1080p
     media"). Proxies measure **0.20 ms p95 (ProRes Proxy, slice threads) and 1.0 ms (MJPEG, single thread)**,
     far inside the 33 ms proxy budget;
   - (c) a GOP cache plus forward-continue for drag-scrubbing. The sweep p50 is already 120 ms
     on the 1 s-GOP 4K file, and single-frame steps cost about 1 ms.

   Suggested ADR/§19 wording: the 150 ms budget applies with hardware decode where available.
   Without it, over-1080p long-GOP media is scrubbed through proxies by default, and the CPU
   long-GOP number is reported, not gated.
2. **The `MediaBackend` FFmpeg module owns a small `unsafe` FFI layer:** per-stream
   `avformat_seek_file`, raw packet flags, the pixel-format descriptor, an `av_log` callback
   shim and a start-up version check. `cutweave-media` must join the crates allowed `unsafe`
   (AGENTS.md: FFI). Without the version lock, a newer runtime libav* than the headers is
   undefined behaviour in the Rust enums (F2.3).
3. **`deny.toml` must allow `WTFPL`** (the license of ffmpeg-next and ffmpeg-sys-next) before
   the `ffmpeg` feature can pass `cargo deny`.
4. **Decoder pool shape:**
   - frame threads for long-GOP seek/scrub decoders (they beat slice threads on single-slice
     streams);
   - **slice threads for intra/proxy decoders** (frame threads cost 15× there);
   - a separate playback decoder;
   - `skip_frame = NONREF` for pre-target packets;
   - "drain to EOF before flush" after any EOF (F7).
5. **The index is the truth for timing and frame count,** not container metadata. Damaged media
   resolves to the nearest following frame and is flagged, never blocks.

**Proposed replacement for the ADR-0003 `Decision` section** (status still Proposed until
0E.02/0E.03):

> We will decode, probe and seek media with FFmpeg 8.1.x (pinned shared build, `just
> ffmpeg-fetch`) through `ffmpeg-next` 9.1 / `ffmpeg-sys-next` 9.1, behind the `MediaBackend`
> trait in `cutweave-media`. The FFmpeg module is the only place with FFmpeg `unsafe`: per-stream
> seek, raw packet flags, pixel-format descriptors, log routing and a start-up check that the
> loaded libav* versions equal the headers the engine was built against.
>
> Frame-exact seek works from a packet-scan index: frames are addressed by pts, never by
> `n / fps`. A seek starts at the latest keyframe K with `decode(K) <= decode(T)` and
> `pts(K) <= pts(T)`, verifies where the demuxer landed against the index, and decodes forward
> with non-reference frames before T skipped. The index, not container metadata, defines frame
> count and duration.
>
> v1 uploads decoded planes from the CPU (path a). Hardware decode is used where WP-0E.02 shows
> it frame-exact. Zero-copy interop is deferred behind a feature flag. Long-GOP media above
> 1080p is scrubbed through intra proxies unless hardware decode meets the 150 ms budget.
>
> Spike S3 evidence: WP-0E.01 (Windows, CPU: 16/16 files 500/500 frame-exact, budgets as
> measured), WP-0E.02, WP-0E.03.

## Notes for WP-0E.02 (hardware decode)

Done: see [WP-0E.02](#wp-0e02-hardware-decode-gpu-upload-and-zero-copy) at the top. Every item
of the WP-0E.01 hand-over (the `--hw` option, the PSNR comparison, pipeline delay, NONREF,
the open-GOP edge pass, both GPUs) is measured there.

## Notes for WP-1D.01 (`MediaBackend` + build/link) and 1D.02/1D.03

- **Linking.** Take `FFMPEG_DIR` from `just ffmpeg-fetch` (`third_party/ffmpeg/ffmpeg.env`).
  Worktrees should read the main checkout's file, as `env.ps1`/`env.sh` do through `git
  rev-parse --git-common-dir`. Link only the needed libs:
  `default-features = false, features = ["codec", "format", "software-scaling",
  "software-resampling"]`. The run-time closure is then avcodec, avformat, avutil, swresample
  and swscale.
- **Tests need the DLLs on `PATH`** (Windows) or `LD_LIBRARY_PATH`/`DYLD_FALLBACK_LIBRARY_PATH`.
  Wire that into `just`/nextest, or copy the DLLs next to the test binaries in a build script.
  CI also needs libclang and MSVC (`cl.exe` runs the ffmpeg-sys version probe).
- **Fence off ffmpeg-next's panics and silent paths** (F2): validate paths as UTF-8 without
  NULs, never use `input.packets()` in the engine, and bound plane access.
- **`deny.toml`: add `WTFPL`.**
- **Pre-flight checks to keep from the spike:**
  - the `avcodec_version()` check against the build headers;
  - mapping `AVERROR_INVALIDDATA` / `AVERROR_EOF` / I/O errors at open to
    `MediaError::{Corrupt, Io}`;
  - `Overshoot` / `EndOfStream` mapped to "nearest following frame + damaged flag" in the
    decoder, not an error to the timeline.
- **1D.02 (index).** The scan is a full read: schedule it as a background job and serialize it
  into `cache.sqlite` (packets: pts, dts, key, discard, pos, size). Address frames by pts. For
  MP4/MOV, consider building from the sample table plus `ctts` to avoid the read.
- **1D.03 (decode).**
  - Reuse the spike's seek algorithm (`src/source.rs`), including landing verification and the
    Matroska pts fallback.
  - Use one decoder per (media, purpose), with forward-continue for scrubbing and a GOP cache
    for backward scrubbing within a GOP.
  - Route `av_log` to `tracing` at debug: open-GOP seeks log `mmco: unref short failure`
    constantly.
- **Audio (1G.01).** FFmpeg applies AAC priming (`skip_samples`) but not end padding. Trim to the
  edit list / `duration` for exact clip length.
