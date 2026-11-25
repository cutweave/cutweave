# ADR-0024: Keyframes: item-relative flicks, per-segment easing, exact edit rules

- **Status:** Proposed
- **Date:** 2026-11-25
- **Deciders:** Cutweave maintainers (drafted in WP-2C.01 for the integrator)
- **Supersedes:** none. It fills in arch §4.2's `Animatable<T>`, which schema v1 rejects in its keyed form, and narrows "every field is Animatable" for `blend_mode`.
- **Architecture refs:** `docs/01-architecture.md` §4.1, §4.2, §4.3, §5.1, §5.2, §8, §14, §16

## Context

Arch §4.2 declares `Animatable<T> = Static(T) | Keyframes(...)` with item-relative flick times and `hold | linear | bezier | spring` interpolation. Schema v1 ships only the static form: `cutweave-model` rejects `{"keyframes": [...]}` with "keyframed values are not supported by this version". Phase 2 needs keyframes on every animatable property (epic 2C). Dogfood B's manual 9:16 reframe, WP-2C.08's layouts and WP-2E.01's volume and pan all depend on them.

Keyframes change three contracts at once, so they need an ADR (AGENTS.md: contracts change only via ADR):

- **The project format:** the keyed wire form, the interpolation types and the new `VisualProps` fields.
- **The command schemas:** `keyframe.*`, `item.set_props{auto_key, at}`, and `prune_keyframes` on the trim family.
- **The MCP surface:** `keyframe_set` as a core tool.

The spec, [docs/spec/keyframes.md](../spec/keyframes.md), has the full rules, its golden vectors and a reference evaluator. This ADR records the choices that are hard to reverse once projects exist.

## Decision drivers

- **Determinism** (arch §8): evaluation is a pure function. Preview equals export, and the wasm module in the UI agrees with the engine.
- **Edits must not change what plays:** splitting a clip must leave every frame identical (WP-2C.03's acceptance test), whatever the easing.
- **One model for every value type** (numbers, vectors, angles, colours, booleans), so presets, the curve editor and the wasm API are shared.
- **Agent-friendly:** one call to animate a property, keys addressed the way `item.get` shows them, and errors that say how to recover. It must also fit the MCP tool budget (≤ 36 tools / ≤ 8k tokens).
- **Familiar to editors:** CapCut-style presets and NLE behaviour on trims and splits.
- **Compact and forward-compatible documents:** 50,000 keys in a project must stay cheap.

## Considered options

**Time base**

1. **Item-relative timeline flicks** (chosen). Moves, ripples, speed changes and slips leave keys alone; head trims shift them by the trimmed amount.
2. **Source (media) time.** Keys would follow the picture through slips, but a speed change would stretch them and a still or a generator has no source time. Rejected.
3. **Absolute sequence time.** Every move and ripple would rewrite every key of every shifted item. Rejected.

**Interpolation**

1. **Per-segment easing `p(u)` with CSS-style normalized bezier handles, plus physical springs** (chosen). Value-type independent, so one preset works on opacity, position or a colour.
2. **Per-key in/out tangents in value space** (After Effects, Blender). Richer for the graph editor, but handles depend on the value type, and presets cannot be shared. Rejected for P2; spatial and per-component curves can be added later as compatible extensions.

**Split**

1. **Each half keeps the keys inside it plus the nearest key on each side** (chosen). Bit-exact for every interpolation, and inserts nothing.
2. **Insert a boundary key in both halves** (the WBS wording). Exact only for hold and linear: bezier needs its handles rewritten by subdivision, and springs cannot be represented at all. Rejected.
3. **Keep every key in both halves** (exact too). Rejected: keys multiply with each cut, which is ruinous for dense auto-reframe curves cut into jump cuts.

**Addressing**

1. **`(item, prop, t)`** (chosen); `t` is unique per property.
2. **An ID per keyframe.** About 40 bytes per key, and nothing to gain. Rejected.

## Decision

**Decided by:** maintainer review of WP-2C.01 (its acceptance criterion is human approval), confirmed when WP-2C.02 passes every row of `docs/spec/keyframes-vectors.json` (within 1e-6, exact rows bit-for-bit) and WP-2C.03 passes the split-identity property test. Until then this is a proposal. WP-2C.02 may build the evaluator against the spec, but nothing writes keyed values into documents before the schema bump below.

We will:

- **Store keys as item-relative flicks.** `{"keyframes": [{"t", "value", "interp", "preset"?}]}`, with 1 to 100,000 keys and strictly increasing `t`. A static value stays the bare `T`. `Keyframe` keeps unknown fields for forward compatibility, while `Interp` is a closed tagged union: `hold`, `linear`, `bezier{x1, y1, x2, y2}` (x in [0, 1], y in [−10, 10]) and `spring{stiffness, damping, mass, bounce}`.
- **Interpolate per segment** with the interp of the key that starts it. Every kind is an easing `p(u)` applied to one lerp per value type:
  - vec2 components share it;
  - colours mix in linear light, premultiplied;
  - rotation accumulates;
  - booleans and enums hold.
  - Springs run in seconds, and a residual term lands them on the next key.
  - Easing presets are expanded on write, with the name kept as a hint.
- **Evaluate deterministically** in f64: exact at keys and outside them, never NaN or infinite. Hold and linear are bit-exact on every platform; bezier, spring and colour agree within 1e-9 across platforms.
- **Animate these properties in P2:** position, scale, rotation, anchor, opacity, the four crop edges, flip (two hold-only booleans), audio gain and audio pan. `blend_mode` is a static enum.
- **Apply exact edit rules:**
  - a split keeps each half's keys plus one bracketing key on each side;
  - head trims shift keys so they stay on the timeline;
  - trims keep out-of-range keys unless `prune_keyframes` is set;
  - slips, moves, ripples, speed changes and duplicates leave keys alone.
- **Add these commands:**
  - `keyframe.set{keys[], replace?}` (MCP core), `keyframe.delete`, `keyframe.move` (reorders; landing on another key is `Invalid`), `keyframe.set_interp`, `keyframe.clear`, `keyframe.copy` and `keyframe.paste`, `clip.reset_props` and the query `keyframe.eval`.
  - `item.set_props` gains `auto_key` and `at`. An animated property is keyed at `at`, a static one is set.
  - Key times are `t` (item time) or `at` (sequence time), both snapped to the frame grid in sequence time.
- **Bump the schema once:** keyed values, `VisualProps.{crop, flip, blend_mode}` and `audio.pan` become legal together in the schema version that WP-2C.03's migration introduces. It chains after WP-2F.07's v2 in merge order (WBS N18). The migration only adds the new fields with their defaults, since older documents contain no keys.

## Consequences

**Positive**

- One evaluator serves render, audio, the Inspector, the lanes and the graph editor. It ships in the UI as wasm with no per-frame RPC.
- Splits, trims, roll, slide and slip never change a rendered frame, provably and bit for bit, and the property tests can say so.
- Agents animate a property in one `keyframe_set` call, with the same time words (`at`) as every other edit command, and can write back what `item_get` returns.
- Presets look the same on every property, and old projects never change appearance when the preset table grows.

**Negative / costs**

- A schema bump with migration goldens (WP-2C.03). Older apps cannot open a project that contains keys at all: they fail on the keyed form instead of opening it read-only, because v1 rejects that form. Their error message names the cause.
- Normalized bezier handles cannot overshoot a flat segment, and vec2 components cannot ease independently in P2. Both can be added later without changing the stored meaning of existing keys.
- Springs depend on the segment's length in seconds, so moving a key reshapes the spring next to it. The curve editor warns when a spring does not settle.
- The split rule leaves no key at the cut. People who want one add it with ◇ at the playhead.

**Follow-ups**

- WP-2C.02: the model types and evaluator, passing the vectors. It should use the pure-Rust `libm` crate (already in `Cargo.lock`) for native and wasm parity.
- WP-2C.03: the commands, the `prune_keyframes` hooks, the model fields and their migration (including `audio.pan`, which WP-2E.01's Area does not cover).
- WP-2C.06: wasm parity on the same vectors.
- WP-2D.01: speed changes leave keys alone. Whether a speed change should stretch them instead is an open question for the product owner.
- The integrator should resolve the WBS deltas listed in the spec's §14, for example with an R-resolution in `docs/05-work-breakdown.md`.

## Links

- Spec: [docs/spec/keyframes.md](../spec/keyframes.md); vectors: [keyframes-vectors.json](../spec/keyframes-vectors.json); reference evaluator: [keyframes_ref.py](../spec/keyframes_ref.py)
- Architecture: [§4.2, §4.3, §5, §8, §16](../01-architecture.md)
- Work packages: [WP-2C.01 to WP-2C.08, WP-2E.01, WP-2H.02, WP-2H.03, WP-2H.06](../wbs/P2-core-editor.md). WP-2A.01's edit-semantics spec defines the edits themselves.
- Related ADRs: ADR-0004 (flicks), ADR-0005 (patches and undo), ADR-0006 (project format), ADR-0009 (MCP hybrid catalog), ADR-0015 (wire format), ADR-0017 (gesture transactions)
