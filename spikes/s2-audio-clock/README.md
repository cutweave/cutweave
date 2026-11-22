# Spike S2: audio clock and A/V sync (WP-0D.01)

> **Throwaway spike.** It has its own cargo workspace and is not part of the root workspace
> (`spikes/README.md`). Product code never imports it. Only the findings below carry forward,
> into `cutweave-audio` (1G) and `cutweave-playback` (1H.01). The formal 30-minute measurement
> and verdict are **WP-0D.02**.

The harness checks architecture §9 and §10 end to end:

- an 8-track mixer graph on a cpal output stream;
- `rubato` resampling to the device rate;
- `signalsmith-stretch` time-stretch at 0.5× to 2× with pitch preserved;
- an `ebur128` meter and a limiter on the master bus;
- the **audio device clock as the master clock**, with a video presenter that shows the newest
  frame whose `pts <= audio clock`.

All sources are synthetic: clicks, tones and noise bursts. There is no FFmpeg and no file decode.

## Run it

Prerequisites:

- The repo toolchain (Rust 1.100).
- `signalsmith-stretch` compiles C++ through `cc` and generates bindings with `bindgen`:
  - **Windows:** MSVC Build Tools, plus LLVM (libclang). bindgen finds
    `C:\Program Files\LLVM\bin` on its own; otherwise set `LIBCLANG_PATH`.
  - **Linux:** `clang`/`libclang-dev`, ALSA dev headers (`libasound2-dev`), plus X11/Wayland dev
    libraries for the `window` feature.
  - **macOS:** Xcode command-line tools.

```sh
cd spikes/s2-audio-clock
cargo run --release                          # 8-track mix + window; Esc (or 'q' + Enter) quits
cargo run --release -- --headless --duration 60s
cargo run --release -- --soak 30m            # WP-0D.02 soak: headless, one CSV row per second
cargo run --release -- --scene sync          # loud click + white flash only (camera/mic, photodiode)
cargo run --release -- --scene ref-tone --headless --duration 25s   # -23 LUFS chain check
cargo run -- --headless --duration 60s       # debug build: RT allocation assertion is armed
cargo run -- --list-devices
cargo test && cargo clippy --all-targets -- -D warnings
```

Useful options (`--help` lists them all):

| Option | Default | Effect |
|---|---|---|
| `--buffer <n\|default>` | `256` | Requested frames per callback. The real size is logged; see finding 1. |
| `--av-offset-ms <ms>` | `0` | Shows video earlier (+) or later (−) than the audio clock. This is the latency-compensation knob for WP-0D.02. |
| `--no-latency-comp` | off | Ignores cpal's reported output latency in the clock, for A/B measurement. |
| `--fps`, `--click-interval` | `60`, `1s` | Timeline frame rate and click/flash period. Flash frame of click *k* = `round(k * interval * fps)`. |
| `--ring-ms`, `--workers` | `250`, `2` | Per-track look-ahead ring and number of decode/stretch threads. |
| `--cheaper-stretch` | off | signalsmith "cheaper" preset, for CPU comparison. |
| `--frame-log` | off | Also writes `frames.csv`, one row per presented frame. |
| `--thread-priority <high\|normal>` | `high` | Priority of the presenter and worker threads (Windows `THREAD_PRIORITY_HIGHEST`). The audio callback always gets MMCSS. See finding 6. |
| `--device <substr>` | default device | Picks the output device by name. |

The cargo features `window` (minifb) and `realtime` (cpal MMCSS / RT priority) are both on by
default. `--no-default-features` builds a terminal-only binary.

### Outputs (`s2-out/<scene>-<unix time>/`, git-ignored)

| File | Contents |
|---|---|
| `soak.csv` | One row per `--log-interval`. **Drift**: `drift_audio_wall_ms` is the audio clock minus the wall clock (mean of ~50 anchors per row), plus its jitter. **Video**: video frame vs audio-clock frame (`video_minus_audio_frames`) and vs wall-clock frame count (`video_minus_wall_frames`), presented and skipped frames. **Callbacks**: size min/max, duration mean/max, gap max, load %. **Latency**: cpal `playback − callback`, last/min/max. **Glitches**: `ts_err_us_max` and `device_glitches` (playback-timestamp discontinuities). **Underruns and errors**: track underruns, cpal xruns/errors, `rt_allocs`. **Headroom**: ring minimum (ms), worker CPU %. **Loudness**: LUFS M/S/I, true peak, limiter gain reduction. |
| `av_events.csv` | One row per click/flash. Click timeline time, flash frame, presented frame, wall time of the present, wall time at which the click is audible per the clock, and their difference (`sw_offset_ms`, the software estimate, which excludes display and DAC latency). |
| `frames.csv` | With `--frame-log`. Per frame: skipped-before count, present wall time, clock used vs raw clock, lateness after the frame boundary, present duration, current anchor. |
| `summary.json` | Device and config, start-up callbacks, latency, drift (end, least-squares ppm, extrapolated frames per 30 min), underruns, glitches, xruns, RT allocations, CPU, video counters, A/V offset statistics, loudness. For `ref-tone` it also records `ref_tone_pass`. |

**For WP-0D.02:** run `--soak 30m` per OS for drift, underruns and latency. For the A/V offset, run
`--scene sync` (in a window). Film the window's white flash and record the click, with a phone
high-speed camera or a photodiode on the flash plus a loopback/mic recorder. The offset is
flash-on-glass minus click-at-ear. Apply the result with `--av-offset-ms` and re-run to confirm.
`av_events.csv` gives the software-side part of the budget.

## Design

```text
 workers (2)                         cpal callback (real-time)              readers
 ─────────────                       ─────────────────────────              ───────
 synth source @44.1/48/96k           publish clock anchor ───────────────▶  AudioClock (seqlock)
  → rubato Fft → device rate         8× pop ring → gain/pan (atomics)        ├─ video presenter (main)
  → signalsmith-stretch 0.5–2×  ──▶  Σ → look-ahead limiter → device buf     ├─ logger (CSV, console)
  → rtrb SPSC ring (250 ms)          push master → tap ring ──────────────▶  └─ ebur128 meter thread
```

- **Real-time callback** (`engine::Callback`):
  1. Publish the clock anchor.
  2. Mix 8 rings through per-track gain and pan, ramped per block.
  3. Run the limiter, convert to the device sample format, push a meter tap.
  4. Update health counters (atomics only).

  Rings are `rtrb` SPSC queues, parameters are atomics, and every buffer is allocated up front.
  There are no locks, no allocation and no I/O. A track whose ring runs dry plays silence for the
  missing frames, then drops the same number of frames when data arrives. The track stays on the
  timeline, and the clock never waits for a worker.
- **Allocation check** (`rt_alloc`): a counting `#[global_allocator]` counts every
  alloc/dealloc/realloc made inside an `RtSection`. The callback opens one around all its work.
  Debug builds `debug_assert!` zero per callback; release builds report the count (`rt_allocs`).
  The unit tests assert zero for the mixer and the whole callback. The spike's only `unsafe` code
  is this allocator and the two-function Windows thread-priority FFI in `util`.
- **Master clock** (`clock`):
  - **Anchor.** Each callback publishes an anchor: timeline frame `F`, the first frame of this
    buffer, is audible at wall time `Instant::now() + (playback − callback)`. The two timestamps
    come from cpal's `OutputStreamTimestamp`.
  - **Seqlock.** Anchors go through a seqlock built from atomics. The writer never blocks, and
    readers retry on a torn read.
  - **Extrapolation.** Readers extrapolate from the newest anchor at the nominal rate. The result
    is clamped to the frames submitted so far, because nothing not yet handed to the device can be
    audible. A stalled device therefore freezes the clock instead of running ahead.
  - **Monotonic readers.** Each reader keeps a `MonotonicReader`, so its view never steps back
    when a new anchor arrives slightly behind the extrapolation.
  - **First callback.** It is not used as an anchor (finding 3).
  - **Exact count.** The integer `frames_submitted` is the sample-accurate submitted count.

  This is the value §10 puts in the preview SHM header.
- **Video presenter** (`video`):
  - It computes `frame = floor((clock(now) + av_offset) * fps)`, presents only when the frame
    index advances, and sleeps until about 1 ms before the next boundary. Skipped indices are
    counted. The offset is added after extrapolation, so it never interacts with the stall clamp.
  - The frame for click *k* is all white, with the frame number in black, for a photodiode or
    camera. If that frame was skipped, the flash moves to the next frame and is counted as late.
  - Frames are synthesised on demand: frame number, timecode, a moving bar, and a HUD.
  - It uses a minifb window, GDI-presented with no vsync, or runs headless. Log lines go to a
    writer thread over a channel, and the presenter thread runs at high priority (finding 6).
- **Resampling**: `rubato::Fft`, `FixedSync::Input`, 1024-frame chunks. The resampler's
  `output_delay()` is trimmed at start: 160 frames for 44.1→48 kHz and 64 for 96→48 kHz. A click
  stays on the timeline to ±2 frames (tested).
- **Stretch**: `signalsmith-stretch` `preset_default`, stereo, at the device rate.
  - Each 256-frame output block consumes `round(256 × speed)` input frames, and the fractional
    remainder is carried, so consumed/produced converges exactly to the speed.
  - The start-up latency is dropped so stretched tracks stay on the timeline: `output_latency +
    input_latency / speed`, which is 2880 + 2880 frames at 48 kHz.
  - The mix scene stretches 7 of 8 tracks at 0.5, 2.0, 1.0, 0.75, 1.5, 1.25 and a 0.5↔2.0 sweep.
    The click track is never stretched.
- **Limiter**: 1.5 ms look-ahead.
  - The gain target per frame is `min(1, ceiling/peak)`. A sliding minimum over the look-ahead
    window gives instant attack; the release is exponential (100 ms); a box filter of window
    length smooths the gain.
  - Every averaged value is ≤ the target of the frame leaving the delay line, so the output
    **sample** peak never exceeds the ceiling (−1 dBFS by default). True peak is not oversampled;
    that is P2 work.
- **Loudness**: `ebur128` in M/S/I/TRUE_PEAK mode, on a meter thread that drains the post-limiter
  tap ring.

### Tests (`cargo test`, 31)

| WBS item | Test |
|---|---|
| Ring buffer | `mixer::ring_buffer_wraps_and_preserves_order_across_threads`: 2 M samples, uneven chunk sizes, cross-thread, gap-free. Also `underrun_inserts_silence_then_realigns`. |
| Stretch ratio sanity (output length within 0.5 %) | `track::stretch_output_length_within_half_percent`, plus `stretch_scales_event_spacing` (click spacing scales by 1/speed within 0.5 %) and `stretch_preserves_pitch` (440 Hz stays within 1 % at 0.5× and 2×). |
| −23 LUFS reference within ±0.1 LU | `meter::reference_tone_reads_minus_23_lufs` (EBU 3341 case 1), and `engine::reference_tone_through_full_chain_reads_minus_23_lufs` (44.1 kHz source → rubato → mixer → limiter). |
| No allocation in the callback | `mixer::process_does_not_allocate`, `rt_alloc::counts_allocations_inside_section_only`. |
| 8 tracks + stretch, 256-frame buffers, 0 underruns | `engine::mock_sink_8_tracks_no_underruns_no_allocations`: the real callback, a mock sink and synthetic timestamps, 10 s at 256 frames. |
| Clock | Seqlock tear test, extrapolation and stall clamp, drift baseline, monotonic reader, flash-frame selection, click alignment through resampler and mixer. |

## Informal numbers (this machine)

> **INFORMAL. These are short development runs on one machine, not the WP-0D.02 measurement.**
> They are not on reference hardware, the 10-minute run is not 30 minutes, and there is no camera
> or loopback A/V measurement.

**Machine**

- AMD Ryzen 9 5900HX (8C/16T), 32 GB RAM, Windows 11 Pro build 26200.
- Host WASAPI, shared mode.
- Device: SteelSeries Arctis Nova Pro Wireless headset (default output), 48 kHz, F32, stereo.
- Mix scene: 8 tracks, 7 of them stretched; 2 workers; 250 ms rings; buffer requested 256.

**Soak runs (10 min each, `--soak 10m --frame-log`)**

Column A is a debug build with normal priority, before the priority flag existed. Column B is a
release build with `--thread-priority high`, the default.

| Metric | A | B |
|---|---|---|
| Callback size requested → actual | 256 → **480** (first callback 1056) | 256 → **480** (first 1056) |
| Callbacks | 60 003 | 60 040 |
| Track underruns / cpal xruns / errors | **0 / 0 / 0** | **0 / 0 / 0** |
| RT heap operations in callback (assertion armed in debug) | **0** | **0** |
| Device glitches (playback-timestamp jumps > 1.5 ms) / late callbacks | 1 / 1, both at t=537 s, with a −4.0 ms clock step (finding 4) | **0 / 0**, no clock steps |
| cpal latency `playback − callback`, steady state | 31.7 ms mean (30.7–31.9; one dip to 22.3) | 31.7 ms mean (30.1–31.9) |
| Callback duration, max (per-row p50 / p99) | 295 µs = 2.9 % of 10 ms (56 / 146 µs) | 286 µs = 2.8 % (52 / 176 µs) |
| Worker CPU per thread, mean / max | 8.1 % / 16.1 % | 8.3 % / 12.9 % |
| Ring headroom, minimum | 197 ms of 250 | 226 ms of 250 |
| Drift, audio clock − wall clock | +11.1 ppm before the step, +11.6 after; 0.67 ms/min | **+11.0 ppm** (least-squares, 0 steps); +6.6 ms after 600 s |
| ↳ extrapolated over 30 min | ≈ 20 ms ≈ 1.2 frames @ 60 fps (vs wall clock) | ≈ 20 ms ≈ 1.19 frames @ 60 fps (vs wall clock) |
| Video frames presented / skipped | 35 742 / 257 (0.7 %, clustered in 3 heavy-load windows) | 36 001 / **0** |
| Frame lateness after the boundary, p50 / p99 / max | 0.001 / 8.9 / 16.7 ms | 0.001 / 2.6 / 12.0 ms |
| Software A/V offset (present − click audible), mean | 0.65 ms (max 35 ms on a skipped flash) | 0.11 ms (max 6.0 ms), 600/600 flashes on their frame |
| Master loudness, integrated / true peak | −21.3 LUFS / −3.5 dBTP | −21.3 LUFS / −3.5 dBTP |

**Other runs**

| Run | Result |
|---|---|
| `--scene ref-tone --duration 25s` (live chain) | **−22.99 LUFS** vs −23.0 ± 0.1: PASS |
| `--buffer default` vs `--buffer 256` | Identical: 480-frame callbacks, 1056-frame endpoint buffer, about 31.6 ms reported |
| `--av-offset-ms 80`, 12 s | Video exactly 80 ms early (software offset −80.0 ms), 0 skipped |
| Presenter priority A/B under ~85 % system CPU load (other agents building), 40 s each | normal: **857** and **454** frames skipped, ring headroom down to 29 ms; high: **0** and **0** skipped, headroom 67 and 186 ms; 0 audio underruns in all four |
| 30 s / 12 s windowed runs, idle machine | 0 skipped, frame lateness p99 0.29 ms, software A/V offset 0.1–0.5 ms mean |

Tests (`cargo test`): 31 pass in about 1.5 s. They include the 10 s mock-sink run: 8 tracks,
stretch, 256-frame callbacks, 0 underruns, 0 allocations.

## Findings for Phase 1

1. **cpal on Windows cannot give 256-frame callbacks.**
   - cpal 0.18.2's WASAPI backend is shared-mode only. `BufferSize::Fixed(256)` only sets the
     endpoint buffer duration passed to `IAudioClient::Initialize`; callbacks always arrive at the
     device period.
   - Here that period is 480 frames (10 ms), the endpoint buffer is 1056 frames, and the device
     reports `SupportedBufferSize::Range { 480, 480 }`. `--buffer default` behaves identically.
   - So the S2 pass criterion "0 underruns at a 256-sample buffer" **cannot be tested on Windows
     through cpal**. It holds offline: the mock-sink test drives the real callback with 8 tracks
     and stretch at 256 frames, with 0 underruns and 0 allocations. WP-0D.02 should re-check it on
     CoreAudio and PipeWire, where small periods are possible.
   - Windows low latency needs one of: IAudioClient3 low-latency shared periods (a cpal patch or
     our own WASAPI path), exclusive mode, or ASIO (cpal `asio` feature, out of tree).
   - Suggestion for 1G.02: request 256, accept the device period, and log the actual size. Treat
     10 ms as the Windows baseline.
2. **cpal's "latency" is the software and engine pipeline only.**
   - On WASAPI, `playback − callback` is frames written minus the `IAudioClock` device position,
     plus `GetStreamLatency`, which is about 0 here. The second callback reports exactly
     1056 frames = 22.0 ms with the device not yet running. Steady state is about 31.6 ms.
   - This counts what the device position counts. It does not count what lies beyond the endpoint:
     this device is a 2.4 GHz wireless headset, so link buffering, DAC and acoustics are invisible
     to software.
   - Compensation therefore has two parts:
     - the reported latency, applied automatically in the clock (`--no-latency-comp` turns it off
       for A/B);
     - a measured per-device output offset (`--av-offset-ms`, measured in WP-0D.02).
   - The product should persist a per-device offset and offer a flash/click calibration; this
     harness is its prototype.
   - `--av-offset-ms` is applied after clock extrapolation, so offsets larger than the reported
     latency (likely for Bluetooth or wireless output) are safe.
3. **Start-up transient.**
   - The first callback primes an empty 1056-frame buffer before the device clock runs, and
     reports `playback == callback`, so 0 ms latency. The second reports 22 ms; steady state
     starts at the third. Playback timestamps jump by about 13 ms during the first 0.5 s.
   - The spike never anchors the clock on callback 0, keeps the first 0.5 s out of the
     glitch and latency statistics (it records them as `startup_*` and `warmup_ts_err_ms_max`),
     and holds video until the first real anchor. 1G.02 needs the same warm-up rule.
4. **Drift: the audio clock vs the wall clock is a device property; video vs audio is 0 by
   construction.**
   - On this device the audio clock runs about **+11 ppm** fast against QPC (`Instant`). The
     figure is steady across runs: 10–12 ppm from 15 s to 10 min. Against the wall clock that is
     about 20 ms, or 1.2 frames at 60 fps, per 30 min.
   - Soak A also saw one **clock step** of −4.0 ms at t=537 s. A callback arrived 19 ms after the
     previous one under heavy system load, and the device/engine position re-synced. It was flagged
     as the run's single device glitch.
   - The logger counts row-to-row changes over 1 ms as steps (`drift_steps`) and fits ppm on the
     series with steps removed, because one step dominates an end-point estimate.
   - Consumers must tolerate clock steps. Video simply follows, and the per-reader monotonic clamp
     holds the picture through a backward step.
   - Because video follows the audio clock, video-vs-audio drift does not accumulate. The
     presented frame is always the one the clock asks for. When the presenter is scheduled on time,
     frame lateness after the boundary is p50 0.001 ms and p99 0.3 ms, with 0 skipped frames.
     Skips come from scheduling (finding 6), not from the clock.
   - **WP-0D.02 should state which drift "< 1 frame / 30 min" means.**
     - Audio vs wall drift comes from the device crystal, typically ±20–100 ppm, i.e. up to about
       10 frames per 30 min at 60 fps. It is not something our code controls, and it does not
       affect sync.
     - Video vs audio drift is what matters for sync.
   - Phase 1 rule: anything A/V uses the audio clock. The monotonic wall clock is only for
     playback without an audio stream. Switching clocks mid-playback must re-anchor.
5. **Anchor jitter is small; no smoothing is needed for presentation.**
   - Per-callback anchors jitter by up to about 0.6 ms, from device-position granularity. With
     single-sample logging, a repeating sawtooth of about 0.5 ms with a period of about 3 s was
     visible on this endpoint.
   - That is under 1/25 of a 60 fps frame, so extrapolation plus a per-reader monotonic clamp is
     enough.
   - A PLL or regression smoother over the last N anchors is only worth it for consumers that want
     sub-millisecond smoothness, such as the scrub-audio cursor or waveform playhead.
6. **The presentation thread must be shielded: no I/O, and elevated priority.**
   - **I/O.** The first build wrote CSV rows from the presenter loop. One write stalled for about
     250 ms (file writes on Windows can block, e.g. on antivirus scanning) and 14 frames were
     skipped. Logging now goes to a writer thread over a channel.
   - **Priority.** Under ~85 % system CPU load (other agents building on this shared machine), a
     normal-priority presenter skipped 454–857 frames per 40 s, with lateness up to 16 ms.
   - With `THREAD_PRIORITY_HIGHEST` it skipped **0**. The normal-priority decode/stretch workers
     also let ring headroom fall from 250 ms to 29 ms. That was not an underrun yet, but it was
     close.
   - The audio callback (MMCSS via cpal `realtime`) was unaffected in every run: 0 underruns.
   - Soak A ran at normal priority and lost 0.7 % of frames in three heavy-load windows.
   - Rules for 1G/1H: the presenter and frame-producer threads, and the decode/stretch workers,
     run above normal priority. On Windows that means MMCSS or `THREAD_PRIORITY_HIGHEST`; on
     macOS, QoS user-interactive; on Linux, rtkit/nice. None of them does file I/O. The spike
     defaults to `--thread-priority high`.
7. **The real-time mix is cheap; the cost is in the workers.**
   - The callback (8 tracks, gain/pan ramps, limiter, tap, accounting) takes ≤ 0.3 ms of a
     10 ms period (the p99 of per-second maxima is 0.15 ms, < 3 %).
   - The 7 stereo signalsmith stretchers plus rubato resamplers cost about 8 % of a core per
     worker thread on average (2 workers, max 16 %; 37 % under heavy contention).
   - The 250 ms rings kept ≥ 197 ms of headroom on a normally loaded machine (see finding 6 for
     heavy load).
   - The §9 split (decode and stretch ahead, trivial real-time mix) holds. Worker count should
     scale with the number of stretched tracks; the mix can stay single-threaded.
8. **signalsmith-stretch has large latency and a native build.**
   - The default preset at 48 kHz has 2880 + 2880 frames (60 + 60 ms) of latency. Stretched clips
     must be pre-rolled (`seek()`) and their output delay compensated, in preview **and** export,
     or stretched audio lands about 120 ms late. The spike drops the start-up latency.
   - The 0.1.3 crate builds C++ with `cc` and `bindgen`, so libclang is needed on every dev and CI
     machine. This is the same requirement as the `ffmpeg` feature.
   - It is a thin wrapper, with one bug: `set_formant_base` calls the semitone setter.
   - Rubber Band (GPL) comparison is WP-0D.02's job.
9. **rubato 5.x uses an `audioadapter`-based API** (`InterleavedSlice` adapters, not
   `Vec<Vec<_>>`). `Fft` with `FixedSync::Input` reports `output_delay()` (160 frames at
   44.1→48 kHz, 64 at 96→48), and this must be trimmed or clips land late. The click onset after
   resample and mix is tested to ±2 frames.
10. **Loudness and limiter.**
    - `ebur128` 0.1.11 reads the EBU 3341 reference correctly: within ±0.1 LU offline, and
      −22.99 LUFS live through rubato, the mixer and the limiter (`--scene ref-tone`).
    - The look-ahead limiter guarantees only the **sample** peak; inter-sample (true) peaks can
      exceed the ceiling because detection is not oversampled. P2's product limiter needs
      oversampled true-peak detection, as its WBS already says.
11. **Allocation guard.**
    - `assert_no_alloc` (named in the WBS) is BSD-1-Clause, which is not in the root `deny.toml`
      allow-list.
    - A 60-line counting `#[global_allocator]` (thread-local flag, per-section counts) does the
      same job and works in unit tests. It found no allocations in callback, mixer or limiter, in
      tests or live (debug build).
    - Phase 1: reuse this approach, or add BSD-1-Clause to the allow-list through the
      dependency-justification process.
    - cpal's `realtime` feature (MMCSS) pulls `audio_thread_priority`, which is MPL-2.0 and
      allowed.
    - The spike's full dependency graph passes the root license policy:
      `cargo deny --manifest-path spikes/s2-audio-clock/Cargo.toml --config deny.toml check licenses`
      reports `licenses ok`.
12. **The presentation path determines the real A/V offset.**
    - minifb presents through GDI without vsync. DWM composition adds up to one refresh, then panel
      latency on top. This is exactly what WP-0D.02's camera or photodiode measurement captures.
    - The software part (present time minus click-audible time per the clock) is 0.1–0.65 ms on
      average: render, GDI blit and pacing granularity.
    - S1's WebGPU/`requestAnimationFrame` path has different, vsync-aligned latency, so the offset
      must be measured on the real preview path as well, as part of S1 × S2 integration.
13. **Toolchain note:** Rust 1.100 clippy adds `chunks_exact_to_as_chunks`, which flags
    `chunks_exact(N)` with a constant `N`. Expect churn in DSP code; `as_chunks::<2>()` is the
    idiom now.
