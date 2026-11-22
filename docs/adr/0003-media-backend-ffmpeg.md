# ADR-0003: Media layer: FFmpeg via `ffmpeg-next` behind `MediaBackend`; CPU upload first, zero-copy later

- **Status:** Proposed
- **Date:** 2026-11-22
- **Deciders:** Cutweave maintainers (decision pending Spike S3)
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

**Decided by: Spike S3** (WP-0E.03) with these pass criteria: **100% frame-exact on 500 random seeks** across H.264 4K30, HEVC 10-bit, ProRes and a VFR phone clip; **4K long-GOP scrub p95 < 150 ms** (proxy path < 33 ms); and **path (a), hardware decode with CPU upload to wgpu, works on every target OS**. On pass, Accepted as hypothesized (with (b) deferred). On partial pass, **ship (a) only and defer zero-copy** (Accepted with fallback). Not decided here.

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

- WP-0E.01-0E.03 (spike); WP-0J.02 (accept and record numbers); Epic 1D (media pipeline).

## Links

- Architecture: [§7](../01-architecture.md), [§21](../01-architecture.md), [§19](../01-architecture.md)
- Research: [03-tech-stack.md](../research/03-tech-stack.md) §2 (FFmpeg, frame-exact seek, HW matrix, recommendation)
- Stack: [02-tech-stack.md](../02-tech-stack.md) (FFmpeg build policy); roadmap S3 in [04-roadmap.md](../04-roadmap.md); WBS [0E](../wbs/P0-foundations.md)
- Integrator resolutions: R6
