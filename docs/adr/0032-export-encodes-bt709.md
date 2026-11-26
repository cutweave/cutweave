# ADR-0032: Exports encode the BT.709 transfer curve their files are tagged with

- **Status:** Accepted (product owner, 2026-11-26)
- **Date:** 2026-11-26
- **Deciders:** product owner (issue #110)
- **Supersedes:** none (settles the open colour note of arch §7 and §8)
- **Architecture refs:** `docs/01-architecture.md` §7, §8; ADR-0003 (media backend; the colour tags it reads and writes)

## Context

WP-1E.03 renders export frames to the same sRGB-encoded RGBA8 target as `render.frame`, so preview and export are bit-identical before encoding. It then converts to YUV with the BT.709 matrix and tags the file BT.709 (primaries, transfer and matrix). The pixels' transfer curve (sRGB) does not match the tag (BT.709), so strict players shift mid-tones slightly.

## Considered options

1. **Encode with the BT.709 OETF** (chosen): the file's pixels match its tag.
2. Keep sRGB values tagged BT.709 (what many editors and browsers effectively do).
3. Tag the file sRGB (IEC 61966-2-1): correct, but player support is uneven.

## Decision

Export encodes the BT.709 OETF (`OutputTransfer::Bt709`) before the BT.709 matrix, and keeps the BT.709 tags. `render.frame` gains a matching transfer parameter so the preview == export bit-identity check still compares like with like; the preview itself stays sRGB for display. Golden frames follow through the documented update path.

## Consequences

- An implementation work package (export transfer, the `render.frame` parameter, goldens and an `ffprobe`/pixel check) follows; until it lands, exports keep today's behaviour.
- Colour-managed HDR and other output transfers stay out of scope (Phase 4).
