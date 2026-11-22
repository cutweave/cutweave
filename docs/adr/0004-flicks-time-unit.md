# ADR-0004: Represent time as flicks (`i64`, 1/705,600,000 s)

- **Status:** Accepted
- **Date:** 2026-11-22
- **Deciders:** Cutweave maintainers (product owner approved)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §4.1, §5.1 (wire format), §16 (shared wasm math)

## Context

An editor mixes frame rates (23.976, 29.97, 59.94 are rational) and audio rates (44.1 kHz, 48 kHz, 96 kHz). Float seconds drift and compare badly. Per-sequence rationals force every operation to reduce fractions and make cross-rate comparison awkward. Frame-exact edits, snapping, golden-frame tests and deterministic rendering (arch §8) need exact integer math.

## Decision drivers

- Frame-exact, drift-free arithmetic across all common video and audio rates.
- Cheap comparison, sorting and interval math (timeline hot paths).
- Easy and lossless on the wire (JSON, TypeScript) and in wasm-shared math.
- Agent-friendly inputs (timecode, seconds) without lossy internal state.

## Considered options

1. **Flicks, `i64` counts of 1/705,600,000 s** (chosen).
2. **Rational time per sequence** (`{num, den}`, as in OTIO `RationalTime`; research/05 §4.2 suggests rationals on the MCP surface). Rejected for internal storage: every cross-rate operation needs a gcd and an overflow guard, and a rational per field bloats the model. We still accept timecode and seconds strings at the boundary.
3. **Float seconds (`f64`).** Rejected: accumulated drift, non-exact frame boundaries, non-deterministic equality.
4. **Integer microseconds or nanoseconds.** Rejected: 1/24 s, 1/30000-style NTSC frames and 44.1 kHz samples do not divide evenly, so rounding error appears at every frame.

## Decision

Time is `Time(i64)` in flicks, in a newtype in `cutweave-model`. Each sequence still carries a rational `FrameRate`. The helpers `frame_to_time`, `time_to_frame` (floor or nearest) and `snap_to_frame` are the **only** allowed conversions. Media source times stay in the stream timebase until mapped through the media index; VFR sources are never assumed constant-fps.

On the wire `Time` is a JSON **integer** and maps to TS `number`, validated as a safe integer (|t| < 2^53 flicks, about 147 days, far beyond any timeline). Inputs also accept timecode strings (`"00:01:02:03"`, `"62.5s"`, `"f:1500"`), which are normalized on parse (ADR-0015).

### Divisibility table (verified)

1 flick = 1/705,600,000 s. Flicks per frame or sample for every rate the architecture names (arch §4.1); the 9 frame rates and 3 audio rates all divide exactly:

| Rate | Exact fraction | Flicks per frame / sample |
|---|---|---|
| 23.976 fps | 24000/1001 | 29,429,400 |
| 24 fps | 24 | 29,400,000 |
| 25 fps | 25 | 28,224,000 |
| 29.97 fps | 30000/1001 | 23,543,520 |
| 30 fps | 30 | 23,520,000 |
| 50 fps | 50 | 14,112,000 |
| 59.94 fps | 60000/1001 | 11,771,760 |
| 60 fps | 60 | 11,760,000 |
| 120 fps | 120 | 5,880,000 |
| 44.1 kHz | 44100 | 16,000 |
| 48 kHz | 48000 | 14,700 |
| 96 kHz | 96000 | 7,350 |

Check (re-run to verify): for each `n/d`, `705600000 * d % n == 0`.

```sh
python -c "F=705600000; print(all(F*d % n == 0 for n,d in [(24000,1001),(24,1),(25,1),(30000,1001),(30,1),(50,1),(60000,1001),(60,1),(120,1),(44100,1),(48000,1),(96000,1)]))"
```

## Consequences

**Positive**

- Frame-exact integer math, with no drift and exact equality in property tests and golden frames.
- Trivial ordering and interval arithmetic; a single scalar in every model struct.
- The same helpers compile to wasm and ship in `packages/protocol`, so the UI snaps and draws keyframe curves with the engine's exact math (arch §16).

**Negative / costs**

- Rates outside the table (for example 1000/1001-derived oddities, 8 kHz or 22.05 kHz audio, 144 fps) may not divide exactly. Conversion helpers define the rounding rule, and such rates need an explicit test and, if needed, an ADR amendment.
- Wire values are large integers, so TS must treat them as safe integers, never floats; the specta export needs a `Time` type override.
- Humans never see flicks. Every UI and agent-facing output includes a display timecode.

**Follow-ups**

- Epic 1A implements the newtype, helpers and property tests; the conversion helper is the only permitted path (lint or review rule in AGENTS.md).

## Links

- Architecture: [§4.1](../01-architecture.md), [§5.1](../01-architecture.md), [§16](../01-architecture.md)
- Research: [03-tech-stack.md](../research/03-tech-stack.md) §5.1 (rational time in OTIO), [05-mcp-integration.md](../research/05-mcp-integration.md) §4.2 (time on the MCP surface)
- Related: ADR-0015 (wire protocol)
