# Spike S3: ffmpeg-next decode, keyframe index, frame-exact seek (WP-0E.01)

> **Throwaway spike.** It has its own cargo workspace, is excluded from the root workspace
> ([spikes/README.md](../README.md)) and is never imported by `crates/`. Only the findings carry
> forward: into ADR-0003 (WP-0E.03 / WP-0J.02), `MediaBackend` and the FFmpeg build/link work
> (WP-1D.01), and probe/index/decode (WP-1D.02/1D.03). Architecture: §7 (media pipeline), §19
> (budgets), §20 (frame-exact seek against ffprobe truth), §21 (FFmpeg packaging).
> **Measured on one Windows 11 dev machine only.** The cross-platform runs are WP-0E.03, and
> hardware decode is WP-0E.02.

## Verdict

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
cargo test                           # 18 unit tests (no media)
cargo test --release -- --ignored    # 4 harness tests over media/ (500 seeks per file, broken files)
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
  flags, the pixel-format component descriptor), each with a `// SAFETY:` comment. Nothing else.

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

- **Reuse this harness unchanged.** Add a `--hw d3d11va|cuda|videotoolbox|vaapi` decoder option
  to `open_decoder` (`hw_device_ctx` + `get_format`, then `av_hwframe_transfer_data` to
  NV12/P010). `verify` then checks pts and the barcode on the HW path.
- **Comparison.** The Y-CRC must be replaced by a PSNR threshold (> 50 dB vs the SW frame, per
  the WBS), because hardware decoders are not bit-exact.
- **Two things to watch in HW decode:**
  - Its pipeline delay: NVDEC and D3D11VA surfaces in flight play the role frame threads play
    here.
  - Whether `skip_frame = NONREF` is honoured. Several hwaccels decode everything anyway.
  Measure the seek p95 on the two 4K files and HEVC Main10 against the 150 ms budget. That is
  the number ADR-0003's budget question now depends on.
- **Test media.** The 4K HEVC Main10 file is open-GOP. HW decoders handle RASL after a seek
  differently, so the edge pass is the important one.
- **Machine.** This one has an RTX 3080 Laptop (NVDEC) and an AMD Radeon iGPU (D3D11VA/AMF).
  Both paths can be compared on one Windows box.

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
