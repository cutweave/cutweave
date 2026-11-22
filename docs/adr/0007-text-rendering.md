# ADR-0007: Text rendering: cosmic-text + swash shaping with an MSDF atlas, and a Vello variant

- **Status:** Accepted (provisional, 2026-11-22), on Spike S4 text evidence (spikes/s4-compositor/README.md). **MSDF only for v1**: cosmic-text shaping and our own MSDF generator (fdsm lacks error correction and overlap handling). 1000 animated glyphs with outline and shadow take 0.70 ms GPU on the AMD iGPU and 0.06 ms on the RTX 3080, with 1.6 MB of memory. 4K crispness SSIM is 0.9976, with no shimmer. Cross-vendor difference is ≤1 LSB. Follow-ups: a high-resolution atlas tier (96 px/em) for thin features, and a colour-emoji atlas. **Vello is rejected for v1** (10–25× slower, ~210 MB, crashes on WARP, up to 28-code cross-vendor drift). skia-safe is not needed. It becomes final after reference-machine measurement (WP-0F.03).
- **Date:** 2026-11-22
- **Deciders:** Cutweave maintainers (decision pending Spike S4)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §8, §11, §12

## Context

Animated, styled captions and titles are CapCut's moat (arch §11): outline, shadow, glow, gradient, per-word pop and karaoke, per-glyph transforms, plus bidi, CJK, emoji and variable fonts. Text must be crisp at 4K, cheap for thousands of animated glyphs, and `cutweave-text` must stay wasm32-clean. Research 03 §3.4 covers shaping and rendering options.

## Decision drivers

- Quality: crisp scalable text at 4K; full shaping coverage.
- Performance: 1000 animated glyphs in < 1 ms; 1080p60 compositing < 8 ms GPU on an iGPU.
- Determinism and wasm-safety.
- License compatibility and maintenance health.

## Considered options

1. **Shaping with `cosmic-text` + `swash`; render glyphs through an MSDF atlas; use Vello for very large or complex glyphs** (leading hypothesis).
2. **`parley` for shaping** (alternative named in the stack doc); same renderers.
3. **Vello for all text.** Possibly simpler but untested on iGPU budgets for thousands of glyphs.
4. **`skia-safe`** (the stack doc's fallback if both MSDF and Vello fail): proven text quality, but a heavy native dependency, a large build, and a weaker fit for the wasm-clean core.
5. **Bitmap atlas per size.** Rejected: blurry when scaled, expensive for animation.

## Decision

**Decided by: Spike S4** (WP-0F.03), measured inside the S4 compositor test with these pass criteria: **1080p60 at < 8 ms GPU on an iGPU**; **PSNR > 45 dB** versus an independent reference and **ΔE < 2** for color; **crisp 4K text**; and **1000 animated glyphs < 1 ms**. The spike evaluates MSDF and Vello variants side by side and selects the primary renderer (or a hybrid). **On fail, evaluate `skia-safe`.** Not decided here.

## Consequences

**Positive** (if confirmed)

- Scalable, animatable text with rich effects from one atlas; Vello covers extreme cases.
- Shaping and layout live in a wasm-safe crate.

**Negative / costs**

- MSDF has known artifacts at tiny sizes and sharp corners; an own renderer is custom code to maintain.
- Two render paths (MSDF and Vello) raise testing cost; golden references are per GPU adapter class (R4).
- `skia-safe` as a fallback would add a large native dependency and a build-time cost.

**Follow-ups**

- WP-0F.01-0F.03 (spike); WP-0J.02 (accept); Epic 3A (text engine).

## Links

- Architecture: [§8](../01-architecture.md), [§11](../01-architecture.md), [§12](../01-architecture.md)
- Research: [03-tech-stack.md](../research/03-tech-stack.md) §3.4 (text rendering for animated captions)
- Stack: [02-tech-stack.md](../02-tech-stack.md); roadmap S4 in [04-roadmap.md](../04-roadmap.md); WBS [0F](../wbs/P0-foundations.md)
- Related: ADR-0012 (compositions render text through this path)
