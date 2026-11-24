# ADR-0003: Media layer: FFmpeg via `ffmpeg-next` behind `MediaBackend`; CPU upload first, zero-copy later

- **Status:** Accepted with fallback (2026-11-24)
- **Date:** 2026-11-22
- **Deciders:** the integrator, under the owner's delegation of go/no-go decisions (2026-11-23), from the WP-0E.01 report
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §7, §19, §21, §22

## Context

Scrubbing, frame-exact seeking and export depend on the media layer. Cutweave is GPL-3.0, so the full GPL FFmpeg build (x264/x265 and others) may be bundled, but patents, hardware acceleration and long-GOP seeking performance are open risks. The browser build (future, arch §22) will need a different backend, so the abstraction matters. Research 03 §2 surveys FFmpeg bindings and hardware decode.

## Decision drivers

- 100% frame-exact seeks, including VFR sources, via a keyframe/GOP index.
- 4K long-GOP scrub p95 < 150 ms (proxy < 33 ms).
- Hardware decode where available with a path that works everywhere.
- A swappable backend (browser via WebCodecs, tests).

## Considered options

1. **`ffmpeg-next` / `ffmpeg-sys-next` behind a `MediaBackend` trait, pinned FFmpeg 8.1.x shared build, CPU-upload first (a), zero-copy interop (b) later behind a feature flag** (leading hypothesis).
2. **GStreamer.** Heavier for scrubbing and packaging (research 03 §2).
3. **`rsmpeg`.** Stale (stack doc); `ffmpeg-the-third` is the documented fallback fork.
4. **Platform media APIs only** (AVFoundation, Media Foundation, VA-API directly). Rejected as the sole backend: three code paths, uneven codec coverage; used through FFmpeg hwaccels instead.
5. **Zero-copy GPU interop from day one** (DX12 shared handles, IOSurface/Metal, dmabuf to Vulkan). Deferred: per-OS complexity and fragility.

## Decision

We will decode, probe and seek media with **FFmpeg 8.1.x** (pinned shared build, `just ffmpeg-fetch`) through **`ffmpeg-next` 9.1 / `ffmpeg-sys-next` 9.1**, behind the `MediaBackend` trait in `cutweave-media`. The crate's FFmpeg module is the only place with FFmpeg `unsafe`: per-stream seek, raw packet flags, pixel-format descriptors, `av_log` routing to `tracing`, and a start-up check that the loaded libav* versions equal the headers the engine was built against.

**Frame-exact seek works from a packet-scan index.** Frames are addressed by pts, never by `n / fps`. A seek starts at the latest keyframe K with `decode(K) <= decode(T)` and `pts(K) <= pts(T)` (open-GOP leading frames need the second condition), verifies where the demuxer landed against the index, and decodes forward with non-reference frames before T skipped. The index, not container metadata, defines frame count and duration. Damaged media resolves to the nearest following frame and is flagged, never blocks.

**v1 uploads decoded planes from the CPU (path a).** Hardware decode is used where WP-0E.02 shows it frame-exact. Zero-copy interop (path b) is deferred behind a feature flag. **Long-GOP media above 1080p is scrubbed through intra proxies** (arch §7 auto-proxy) unless hardware decode meets the 150 ms budget on that machine; the CPU-only long-GOP scrub number is reported, not gated.

**Decoder pool:** frame threads for long-GOP seek and scrub decoders, slice threads for intra and proxy decoders, a separate playback decoder, `skip_frame = NONREF` before the target, and "drain to EOF before flush" after any EOF (an FFmpeg 8.1 HEVC quirk otherwise returns a stale frame after a seek).

**Status: Accepted with fallback** under this record's own partial-pass rule. S3 met the frame-exact criterion and missed the CPU 4K long-GOP budget, which the proxy path covers.

### Spike S3 evidence (WP-0E.01, Windows 11 dev machine, CPU decode, under 70-90 % background load)

| Criterion | Result |
|---|---|
| 100 % frame-exact on 500 random seeks | **Pass.** 16/16 generated files 500/500, checked bit-exactly against the FFmpeg CLI (pts, Y-plane CRC, burned-in barcode); also every edge and open-GOP leading frame; 0 landing retries. Files: H.264 1080p at 5 rates and 4 containers, 4K30 1 s and 8.3 s GOP, VFR phone-like and segmented MKV, rotation, HEVC Main10 4K open GOP and 1080p HDR10, ProRes 422 HQ |
| 4K long-GOP scrub p95 < 150 ms | **Fail on CPU:** 282 ms (H.264, 1 s GOP), 428 ms (HEVC Main10, 2 s GOP), 804 ms (H.264, 8.3 s GOP) |
| Proxy scrub p95 < 33 ms | **Pass:** ProRes Proxy 540p 0.20 ms, MJPEG 540p 1.0 ms |
| Path (a) works on every target OS | **Windows only so far.** Hardware decode: WP-0E.02. macOS and Linux: WP-0E.03 |
| Robustness | 0 panics, crashes or hangs on 7 broken files and 200 fuzz cases. H.264 damage is concealed silently (per-frame flags only); HEVC drops damaged frames |

Report and reproducible harness: [spikes/s3-decode/README.md](../../spikes/s3-decode/README.md).

### Revisit triggers

Reopen this record if WP-0E.02 finds hardware decode cannot be made frame-exact on a target OS, if WP-0E.03 finds `ffmpeg-next` 9.1 unusable on macOS or Linux, or if proxies cannot be generated fast enough for the import flow. Until 0E.02 and 0E.03 report, the per-OS and hardware rows above stay open; Epic 1D proceeds on this decision.

## Consequences

**Positive** (if confirmed)

- Broad codec and hardware coverage with a single, well-understood library; the GPL build avoids encoder gaps.
- The trait keeps a WebCodecs or other backend possible later.

**Negative / costs**

- We own a pinned, per-OS FFmpeg shared-library build (including libvidstab, R6) and must publish the exact FFmpeg source and configure flags for GPL compliance.
- CPU upload costs bandwidth and CPU versus zero-copy; the budget (arch §19) must hold with (a).
- Codec patent exposure for H.264/HEVC/AAC; prefer OS and hardware codecs, document posture, seek legal review (arch §21).
- Binding-crate maintenance risk (fallback fork exists).

**Follow-ups**

- WP-0E.02 (hardware decode, measured with the S3 harness) and WP-0E.03 (macOS, Linux) fill the open rows above.
- WP-1D.01: allow `WTFPL` (the license of `ffmpeg-next` and `ffmpeg-sys-next`, GPL-compatible) in `deny.toml`, and allow `unsafe` in `cutweave-media` for the FFI layer.
- Epic 1D (media pipeline) reuses the spike's seek algorithm and index rules.

## Links

- Architecture: [§7](../01-architecture.md), [§21](../01-architecture.md), [§19](../01-architecture.md)
- Research: [03-tech-stack.md](../research/03-tech-stack.md) §2 (FFmpeg, frame-exact seek, HW matrix, recommendation)
- Stack: [02-tech-stack.md](../02-tech-stack.md) (FFmpeg build policy); roadmap S3 in [04-roadmap.md](../04-roadmap.md); WBS [0E](../wbs/P0-foundations.md)
- Integrator resolutions: R6
