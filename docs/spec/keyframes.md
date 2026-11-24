# Keyframe and easing model

> **Status: draft for human review (WP-2C.01).** Normative for WP-2C.02 (interpolation core), WP-2C.03 (keyframe and property commands), WP-2C.04 (render), WP-2C.05 to WP-2C.08, WP-2E.01 and the Inspector work packages (WP-2H.02, WP-2H.03, WP-2H.06). The project-format and command-schema parts are proposed in [ADR-0023](../adr/0023-keyframes.md); until a maintainer accepts it, build only what this spec's own work packages need.
>
> - It extends [arch §4.2](../01-architecture.md) (`Animatable<T>`) and relies on §4.1 (flicks), §4.3 (frame snapping), §5 (commands, undo, gestures), §8 (deterministic evaluation) and §16 (shared math via wasm).
> - Executable parts: [keyframes-vectors.json](keyframes-vectors.json) holds the golden vectors and [keyframes_ref.py](keyframes_ref.py) is the reference evaluator that generates and checks them (§12).
> - WP-2A.01's edit-semantics spec (`docs/spec/timeline-edit-semantics.md`) defines what split, trim, ripple and the other edits do to items. This spec owns what they do to keyframes (§8); the edit spec links here instead of restating it.
>
> "Must", "must not" and "should" are normative. Section numbers are stable: cite them as `keyframes §n`.

## 1. Decisions at a glance

| # | Decision | Where | Main alternative, and why not |
|---|---|---|---|
| D1 | Key times are **item-relative timeline flicks**: `t = 0` is the item's first frame. | §3.2 | Source time: speed changes would move keys. Absolute sequence time: every move and ripple would rewrite every key. |
| D2 | Interpolation belongs to the **segment that starts at a key**. Every kind is an easing function `p(u)` fed into one lerp per value type. | §4.1 | Per-key in/out tangents in value space (After Effects, Blender): handles then depend on the value type (what is a tangent of a colour?), and presets could not be shared across properties. |
| D3 | Bezier handles are **CSS-style and normalized** to the segment: `x` in [0, 1], `y` in [−10, 10]. | §4.4 | Absolute (time, value) handles: same objection as D2. The cost is that a flat segment cannot overshoot (§4.4). |
| D4 | Springs run in **real seconds**, plus a residual term that lands them exactly on the next key. | §4.5 | Stretching the spring to the segment: stiffness and mass would stop meaning anything physical. |
| D5 | Presets are **expanded on write** into bezier or spring parameters, keeping the name as a hint. | §4.6 | A stored `preset` kind: changing a preset table would change old projects. |
| D6 | Vec2 components share one easing. Colours interpolate in **linear light, premultiplied**. Rotation **accumulates**. Booleans and enums **hold**. | §5 | Per-component easing ("separate dimensions"): later, as a compatible extension. |
| D7 | Evaluation is f64, exact at keys, clamped before the first and after the last key, and never NaN or infinite. Hold and linear are bit-exact on every platform. | §6 | |
| D8 | Crop is four animatable edges, flip is two hold-only booleans, and `blend_mode` is **not** animatable. | §7 | Animatable blend modes: the major NLEs do not keyframe them, and a hold-only enum would add a lane nobody uses. |
| D9 | **Split keeps**, in each half, the keys inside it plus the nearest key on each side. Both halves evaluate bit-identically and nothing is inserted. | §8 | Inserting a boundary key in both halves (the WBS wording): exact only for hold and linear, not for bezier or spring (§8.3). |
| D10 | Head trims shift keys so they **stay put on the timeline**. Trims keep out-of-range keys unless `prune_keyframes` is set. | §8 | Pruning by default: a trim gesture dragged in and back out would lose keys. |
| D11 | Keys are addressed by **`(item, prop, t)`**. Keyframes have no IDs. | §9.1 | One ID per key: about 40 bytes per key for nothing, since `t` is already unique per property. |
| D12 | Commands take a key time as **`t` (item time) or `at` (sequence time)**. Both snap to the sequence's frame grid in sequence time. | §9.1 | One time base only: agents think in sequence time, while `item.get` shows item time. |
| D13 | `item.set_props` on an animated property **keys it at `at`** (auto-key). On a static property it sets the static value. | §9.3 | |

## 2. Terms

- **F** = 705,600,000 flicks per second (arch §4.1).
- **Item time `t`:** flicks since the item's start on the timeline. **Sequence time `at`:** flicks on the sequence's timeline. For an item starting at `start`, `at = start + t`.
- **Key:** a keyframe. Keys `k` and `k+1` bound **segment `k`**, `[t_k, t_{k+1})`, of length `span_k = t_{k+1} − t_k`.
- **Progress `p`:** an easing's output. It is 0 at the segment's start and tends to 1 at its end, and may leave [0, 1] (overshoot).
- **Evaluated form:** what evaluation returns. It equals the stored value type except for colours (§5.4).
- **Prop ID:** a property's name in commands and outputs, such as `transform.position` or `opacity` (§7).

## 3. Data model

### 3.1 Types

```rust
// cutweave-model (WP-2C.02); arch §4.2
enum Animatable<T> { Static(T), Keyframes(Keyframes<T>) }

struct Keyframes<T> {
    keys: Arc<[Keyframe<T>]>, // sorted by t, t unique, 1..=100_000 keys (§3.3)
    extra: Extra,             // unknown members of the keyed object (arch §4.4)
}

struct Keyframe<T> {
    t: Time,        // item-relative flicks (§3.2)
    value: T,
    interp: Interp, // shapes segment k, from this key to the next; kept but unused on the last key
    extra: Extra,   // unknown fields, so newer files round-trip read-only
}

#[serde(tag = "kind", rename_all = "snake_case")] // a closed set: a new kind is a schema change
enum Interp {
    Hold,
    Linear,
    Bezier { x1: f64, y1: f64, x2: f64, y2: f64, preset: Option<String> },
    Spring { stiffness: f64, damping: f64, mass: f64, bounce: bool, preset: Option<String> },
}
```

On the wire a static value stays the bare `T`, as in schema v1. A keyed value is an object with a `keyframes` array:

```json
"opacity": 1.0,
"opacity": {"keyframes": [
  {"t": 0, "value": 0.0,
   "interp": {"kind": "bezier", "x1": 0.0, "y1": 0.0, "x2": 0.58, "y2": 1.0, "preset": "ease_out"}},
  {"t": 705600000, "value": 1.0, "interp": {"kind": "linear"}}
]},
"position": {"keyframes": [
  {"t": 0, "value": {"x": 0.0, "y": 0.0},
   "interp": {"kind": "spring", "stiffness": 170.0, "damping": 26.0, "mass": 1.0, "bounce": false,
              "preset": "spring"}},
  {"t": 1411200000, "value": {"x": 0.25, "y": 0.0}, "interp": {"kind": "linear"}}
]}
```

- A keyed value is recognized by its `keyframes` member. No animatable value type may have a field named `keyframes`.
- In TypeScript, `type Animatable<T> = T | { keyframes: Keyframe<T>[] }`. The JSON Schema is `oneOf: [T, keyed]`; the two never overlap.
- Snapshots clone items, so the key list must be cheap to clone (shared, such as `Arc<[…]>`). An empty `extra` should not cost a map per key (such as `Option<Box<Extra>>`). WP-2C.02 chooses the representation.
- `preset` is a hint for the UI and agents, never read by evaluation (§4.6). Every other field is required in documents (arch §4.4, strict documents).

### 3.2 Time base

- **`t` is item time,** an `i64` `Time` serialized as a JSON integer (ADR-0004, ADR-0015). It is not source time, so speed changes and slips do not move keys (§8).
- **Keys may lie outside `[0, duration)`,** and `t` may be negative. They appear after head trims, are kept by default (§8) and evaluate as usual. Only `[0, duration)` is ever shown or heard.
- **Commands snap key times** to the sequence's frame grid *in sequence time*, like every time parameter (arch §4.3: nearest frame, exact halves go later): `t ← snap_to_frame(start + t) − start`. An item that starts off the grid (a fine audio edit) therefore gets keys on real frames. Audio properties accept `fine: true`, which snaps to the sequence's sample grid instead.
- **Documents do not require snapped times.** Keys can be off the grid after fine audio edits or a change of the sequence's frame rate. They evaluate exactly, and nothing re-snaps them.

### 3.3 Invariants

`Project::validate` checks these, and the commands keep them. The `invalid` vectors (X rows, §12) cover each one.

| ID | Rule |
|---|---|
| KF-1 | A keyed value has 1 to 100,000 keys. Deleting the last key makes the property static (§9.4). |
| KF-2 | Key times strictly increase: no two keys share a `t`. |
| KF-3 | Every `t` is an integer within ±(2⁵³ − 1) flicks. |
| KF-4 | Every value is of the property's type and within its stored range (§7). Numbers are finite. |
| KF-5 | `interp` is one of the four kinds with exactly its fields. Bezier `x1`, `x2` are in [0, 1] and `y1`, `y2` in [−10, 10]. Spring `stiffness` is in [1, 10,000], `damping` in [0, 1,000], `mass` in [0.1, 100], and `bounce` is a boolean. |
| KF-6 | Discrete properties (booleans, enums) use only `hold`. |
| KF-7 | `preset`, when present, matches `^[a-z][a-z0-9_]{0,63}$` and appears only on bezier and spring. Unknown names are allowed: a newer version may add presets. |

## 4. Interpolation and easing

### 4.1 The easing contract

For `t_k < t < t_{k+1}`:

1. `dt = t − t_k` and `span = t_{k+1} − t_k`, computed exactly in `i64`. Neither can overflow, since `|t| < 2⁵³`.
2. `u = (dt as f64) / (span as f64)`. Each conversion is exact below 2⁵³ flicks and otherwise rounds to nearest-even. The division is IEEE.
3. `p = E(u)` for the segment's kind (§4.2 to §4.5).
4. `value = combine(v_k, v_{k+1}, p)` for the value type (§5).

Every kind except hold satisfies `E(0) = 0` and `E(u) → 1` as `u → 1`, so values are continuous into the next key. The vectors check continuity at one flick before each key, within 1e-6 of the segment's value range. `E` may leave [0, 1] (overshoot); property clamps apply after `combine` (§7).

### 4.2 Hold

The value is `v_k` for the whole segment, with no arithmetic, and jumps to `v_{k+1}` at `t_{k+1}`. Returning `v_k` directly (rather than `combine(v_k, v_{k+1}, 0)`) keeps hold exact even where `v_{k+1} − v_k` would overflow.

### 4.3 Linear

`p = u`.

### 4.4 Bezier

A cubic Bézier from (0, 0) to (1, 1) with control points (`x1`, `y1`) and (`x2`, `y2`): CSS `cubic-bezier()` semantics. In Horner form (as WebKit's `UnitBezier`):

```text
c = 3·p1,  b = 3·(p2 − p1) − c,  a = 1 − c − b          (per axis, with p1, p2 = x1, x2 or y1, y2)
B(s) = ((a·s + b)·s + c)·s
p = By(s*)  where s* ∈ [0, 1] solves Bx(s*) = u
```

- **Handles.** (`x1`, `y1`) shapes the start of the segment (the speed leaving key `k`), and (`x2`, `y2`) its end (the speed arriving at key `k+1`). Both live on key `k`.
- **`x` in [0, 1]** makes `Bx` monotone non-decreasing, so `s*` is unique and the curve is a function of time. Commands reject `x` outside [0, 1] with `Invalid`; the graph editor constrains its handles instead. Evaluation also clamps `x` defensively.
- **`y` is free within [−10, 10]:** `y < 0` or `y > 1` overshoots (back easings). The limit only keeps values sane.
- **A flat segment cannot overshoot.** When `v_{k+1} = v_k`, every `p` gives the same value, as in CSS. To wobble around a resting value, add keys.
- **Solver.** Implementations should use Newton–Raphson from `s = u` (at most 8 steps, stopping when the step is below 1e-14), falling back to bisection on [0, 1] down to a bracket of 2⁻⁵². The requirement is on the result: `p` within **1e-6** of the exact value on every vector (the target is 1e-9). The vectors include a key one flick into a segment whose curve starts vertically, which catches solvers that stop on `|Bx(s) − u| < ε` too early.
- **Conditioning.** Where `Bx′(s*) = 0` and `By′(s*) ≠ 0` the curve has a vertical tangent. There `p` behaves like a cube root of `u`, so no f64 solver agrees with another to better than about 1e-5 at that exact point. That is invisible on screen; the vectors avoid such points.
- **Absolute handles (for WP-2C.07's graph editor).** For a component with values `v_k`, `v_{k+1}` and `Δv = v_{k+1} − v_k`, the handles sit at `(t_k + x1·span, v_k + y1·Δv)` and `(t_k + x2·span, v_k + y2·Δv)`. Dragging maps back by inverting these formulas; with `Δv = 0` the `y` handles are undefined, and the editor keeps them.
- **Aligned handles** through key `k` mean equal slopes on both sides: `(Δv_{k−1}/span_{k−1})·(1 − y2)/(1 − x2) = (Δv_k/span_k)·y1/x1`, with the left factors from key `k−1`'s interp and the right from key `k`'s. P2 stores no tangent mode: the graph editor treats handles as aligned when this holds within 1e-6 and offers break/align as an edit. For a vec2 both components share `p`, so they can only be smooth through a key at once when their value changes are proportional. That is why the graph editor edits the shared easing for vec2 (§11).

### 4.5 Spring

A damped spring released from rest, at distance 1 from its target, in **seconds**:

```text
ω0 = √(k / m)                  k = stiffness, c = damping, m = mass
ζ  = c / (2·√(k·m))            damping ratio
x(τ), the distance still to go (x(0) = 1, x′(0) = 0):
  ζ < 1 − 1e−9  (under):     ωd = ω0·√(1 − ζ²);   x = e^(−ζ·ω0·τ) · (cos(ωd·τ) + (ζ·ω0/ωd)·sin(ωd·τ))
  |ζ − 1| ≤ 1e−9 (critical): x = e^(−ω0·τ) · (1 + ω0·τ)
  ζ > 1 + 1e−9  (over):      q = √(ζ² − 1);  r1 = −ω0/(ζ + q);  r2 = −ω0·(ζ + q)
                             x = (r2·e^(r1·τ) − r1·e^(r2·τ)) / (r2 − r1)
bounce: x ← |x|                (a hard stop at the target: the motion reflects instead of passing)
τ = dt / F,  D = span / F      (seconds, as f64 divisions)
p = 1 − x(τ) + u·x(D)
```

- **Physical time.** The parameters mean what they mean in common spring APIs: stiffness in N/m with mass in kg, so a preset feels the same on any segment that is long enough. The presets (§4.6) settle in 0.5 to 1.5 s.
- **The residual `u·x(D)`** makes `p(1) = 1` exactly, so the curve always lands on the next key. When the spring has settled before the segment ends, `x(D) ≈ 0` and the residual vanishes; the curve then rests at the target. When the segment is too short for the spring, the residual pulls it onto the key, and as `D → 0` the curve tends to linear. WP-2C.05's curve editor should warn when `|x(D)| > 0.01` ("the spring does not settle in this segment").
- **Numerics.** `r1` is written as `−ω0/(ζ + q)` to avoid cancellation when ζ is large. The critical band (±1e-9 around ζ = 1) avoids dividing by a vanishing `ωd` or `r2 − r1`. Inside the band, the critical form differs from the exact curve by about 1e-9.
- **Bounce** never passes the target while the spring has settled. With an unsettled spring the residual can push it past by at most `|x(D)|`.
- **Undamped springs** (`damping = 0`) are allowed: they never settle, and the residual lands them.

### 4.6 Presets

Commands accept `easing: <name>` as an alternative to `interp`. It is expanded on write into the parameters below, with `preset: <name>` kept as a hint. The names `linear` and `hold` are also accepted; they expand to those kinds without a hint.

| Preset | Stored as |
|---|---|
| `ease_in` / `ease_out` / `ease_in_out` | bezier (0.42, 0, 1, 1) / (0, 0, 0.58, 1) / (0.42, 0, 0.58, 1): the CSS keywords |
| `sine_in` / `sine_out` / `sine_in_out` | bezier (0.12, 0, 0.39, 0) / (0.61, 1, 0.88, 1) / (0.37, 0, 0.63, 1) |
| `cubic_in` / `cubic_out` / `cubic_in_out` | bezier (0.32, 0, 0.67, 0) / (0.33, 1, 0.68, 1) / (0.65, 0, 0.35, 1) |
| `expo_in` / `expo_out` / `expo_in_out` | bezier (0.7, 0, 0.84, 0) / (0.16, 1, 0.3, 1) / (0.87, 0, 0.13, 1) |
| `back_in` / `back_out` / `back_in_out` | bezier (0.36, 0, 0.66, −0.56) / (0.34, 1.56, 0.64, 1) / (0.68, −0.6, 0.32, 1.6): overshoot |
| `spring` | spring 170 / 26 / 1 (stiffness / damping / mass), ζ ≈ 1.00 |
| `spring_gentle` / `spring_wobbly` / `spring_stiff` | spring 120 / 14 / 1 (ζ ≈ 0.64), 180 / 12 / 1 (ζ ≈ 0.45), 210 / 20 / 1 (ζ ≈ 0.69) |
| `elastic` | spring 200 / 6 / 1 (ζ ≈ 0.21): several decaying oscillations |
| `bounce` | spring 160 / 10 / 1 with `bounce: true` (ζ ≈ 0.40): each bounce reaches about a quarter of the previous height |

- The sine to back families are the widely used cubic-bezier approximations of Penner's easing equations. The spring numbers are the common physics-based UI presets.
- The table is part of this spec. Changing a preset's numbers means a new name (`ease_out_2`), because documents store the expanded numbers: old projects never change appearance.
- Evaluation ignores `preset`. The UI shows the preset's name only while the stored parameters equal the table's exactly, and "Custom" otherwise. Any parameter edit drops the hint (§9.6).
- **Bounce is approximated.** Penner's bounce has parabolic arcs that shorten. The spring with a wall has arcs of equal length that lower geometrically. A dedicated bounce kind would be a compatible extension later (a new `kind`, with a schema version bump).

## 5. Value types

### 5.1 Scalar

`combine(a, b, p) = a + (b − a)·p`, in exactly this form. It is monotone in `p` for IEEE arithmetic, so linear segments are monotone in `t` (WP-2C.02's property test). Every other form must give the same bits.

### 5.2 Vec2

Each component as a scalar, with the same `p`: the easing is shared ("linked"). Per-component easing, After Effects' "separate dimensions", is not in P2. It can come later as a separate keyed form of a vec2 property without changing this one.

### 5.3 Angle

Degrees, clockwise.

- **Accumulate** (the default, and rotation's rule): a scalar. 0 → 720 makes two turns.
- **Shortest path** is a registry attribute for later properties such as hue; no P2 property uses it. With `d = v_{k+1} − v_k` reduced to `d′ = d − 360·⌊(d + 180)/360⌋` in [−180, 180), the value is `v_k + d′·p`. Exactly opposite angles turn counter-clockwise (−180). The result is not wrapped into [0, 360); consumers wrap it if they need to.

### 5.4 Colour

Stored as the model's `Color` (`#rrggbb[aa]`, sRGB-encoded, straight alpha). P2 has no colour property; this rule is for P3/P4 properties such as text and effect colours.

1. Decode each channel `c = byte/255` with the sRGB EOTF: `c ≤ 0.04045 ? c/12.92 : ((c + 0.055)/1.055)^2.4`. Alpha `a = byte/255` is already linear.
2. Premultiply: `R = r_lin·a`, and the same for G and B.
3. Interpolate R, G, B and A as scalars with the same `p`.
4. Clamp: `A ∈ [0, 1]`, then each of R, G, B to `[0, A]`.

The **evaluated form** is premultiplied linear-light RGBA in f64, the compositor's working space (arch §8). Interpolating premultiplied values means a fade from transparent white to red shows no white fringe. To store an evaluated colour (a key with no explicit value, §9.2), un-premultiply (0 when `A = 0`), apply the sRGB OETF, multiply by 255 and round half to even.

### 5.5 Booleans and enums

Discrete values only hold (KF-6). The value is `v_k`, whatever the stored kind.

## 6. Evaluation contract

`eval(animatable, t) → T` (the evaluated form) is a pure function. It has no I/O, clock, randomness or hidden state (arch §8).

1. `Static(v)` → `v`.
2. Find the last key with `t_k ≤ t` (binary search; `partition_point(|k| k.t <= t) − 1`).
3. If there is none (`t < t_0`), return `v_0` exactly. If `t = t_k` or `k` is the last key, return `v_k` exactly. No arithmetic runs at a key.
4. If the property is discrete or `interp_k` is hold, return `v_k`.
5. Compute `u` and `p` (§4.1) and `combine` (§5).
6. **Sanitize.** A component that is not finite takes `v_k`'s component. It happens only with values near the f64 limits; the vectors pin the rule. Evaluation therefore never returns NaN or infinity.
7. The property layer then clamps to the property's evaluation range (§7). `eval` alone does not clamp.

**Determinism and precision.**

- Evaluation uses f64 throughout, in IEEE 754 binary64 with round-to-nearest-even, no fused multiply-add contraction and no fast-math. Rust's defaults give this. Conversion to f32 happens only at the GPU boundary (WP-2C.04).
- **Bit-exact on every platform:** static values, values at keys and outside the keys, hold, linear (scalar, vec2, accumulating and shortest-path angles) and clamps. These need only basic IEEE operations. The vectors flag such rows `"exact": true`, and they must match as f64 equality.
- **Bezier, spring and colour** use a solver or `exp`, `sin`, `cos` and `pow`. The same binary on the same platform is bit-reproducible, which is what render determinism needs (preview equals export, arch §8). Across platforms (native against wasm, WP-2C.06) results agree within 1e-9. WP-2C.02 should take these functions from the pure-Rust `libm` crate (already in `Cargo.lock`), which makes native and wasm agree bit for bit. Adding it as a direct dependency needs the usual justification.
- No loop may depend on time or on anything other than its inputs. The bezier solver has fixed iteration caps.

**Repeated and bulk evaluation (WP-2C.02).** The `Evaluator` with a cursor and `eval_range` must return exactly the bits of `eval`. `eval_range(t0, t1, n)` samples at `t_i = t0 + ⌊(t1 − t0)·i/(n − 1)⌋`, computed in `i128` with floor division, for `i = 0..n−1` (`n ≥ 2`; `n = 1` samples `t0`). `keyframe.eval` (§9.10) and the wasm `anim.evalRange` (WP-2C.06) use the same sample times.

**Evaluation is at integer flicks:** video at frame times, audio at sample times (exact flicks at every supported rate, arch §4.1). WP-2E.01 evaluates gain and pan at its block boundaries and ramps linearly in between (at most 64 samples). That is the mixer's smoothing, not a change to these curves.

## 7. Property registry (P2)

Every animatable property, its type, unit and ranges. The **stored range** is what keys and static values must satisfy (KF-4) and what commands validate. The **evaluation clamp** is applied after interpolation, because easings can overshoot.

| Prop ID | Document path | Type | Unit and meaning | Stored range | Evaluation clamp | Interp | Default |
|---|---|---|---|---|---|---|---|
| `transform.position` | `visual.transform.position` | vec2 | Offset of the anchor from the canvas centre in canvas units (1.0 = canvas width on x, height on y), +y down | finite | none | any | (0, 0) |
| `transform.scale` | `visual.transform.scale` | vec2 | Multiplier of the aspect-fit size, per axis | each finite, ≥ 0 | each ≥ 0 | any | (1, 1) |
| `transform.rotation` | `visual.transform.rotation` | angle, accumulating | Degrees, clockwise, around the anchor | finite | none | any | 0 |
| `transform.anchor` | `visual.transform.anchor` | vec2 | A point of the item in item units ((0, 0) top-left, (1, 1) bottom-right) | finite | none | any | (0.5, 0.5) |
| `opacity` | `visual.opacity` | scalar | Fraction, 0 transparent to 1 opaque | [0, 1] | [0, 1] | any | 1 |
| `crop.left`, `crop.top`, `crop.right`, `crop.bottom` | `visual.crop.*` | scalar | Fraction of the upright source picture removed from that edge | [0, 1] | [0, 1] | any | 0 |
| `flip.horizontal`, `flip.vertical` | `visual.flip.*` | bool | Mirror the item left-right or top-bottom | — | — | hold only | false |
| `audio_gain` | `audio.gain` | scalar | Clip level in dB, interpolated in dB (UI label "Volume") | [−144, 48] | [−144, 48] | any | 0 |
| `audio_pan` | `audio.pan` | scalar | Stereo position, −1 left to 1 right, 0 centre | [−1, 1] | [−1, 1] | any | 0 |

Not animatable in P2: `blend_mode` (`visual.blend_mode`), a static enum `normal | multiply | screen | overlay | darken | lighten | add | soft_light` (arch §8's core set, default `normal`), and speed (constant in P2; speed ramps are P4).

Notes:

- **Prop IDs match `item.set_props`'s patch paths,** so the names in `keyframe.*`, in `item.set_props` and in its `changed` output are the same. `audio_gain` keeps P1's flat name. Document paths are relative to the item.
- **Units** are the ones P1 defined (`Transform` in `cutweave-model`, confirmed by WP-1F.02). Scale stays non-negative: mirroring is `flip`. The x/y link of scale is an editing aid in the Inspector, not stored (WP-2H.02's linkable vec2 control; linked by default when x = y).
- **Crop and flip, in item space before the transform:** crop masks edges of the source picture after its rotation tag is applied (it masks without moving the picture, WP-1F.02). Flip then mirrors the item about its centre, crop mask included, so flipping a cropped clip mirrors what you see. When `left + right ≥ 1` or `top + bottom ≥ 1` nothing is visible, and the render may cull the layer. Each edge is validated on its own because each animates on its own. WP-2C.04 implements this order; WP-2H.06's crop handles map screen edges through the flip.
- **Model gaps.** P1's model has neither `crop`, `flip` and `blend_mode` (WP-2C.03 adds them, WBS N19) nor `audio.pan`. WP-2E.01 needs pan but its Area has no model change, so WP-2C.03 should add `audio.pan` in the same migration (integrator note).
- **Audio gain in dB.** Interpolating in dB gives perceptually even level changes, which is what NLE volume keyframes do. Fades (WP-2E.01) are separate curves, not keys.
- **Later properties** (P3 text, P4 effect and composition params, masks) declare their types from §5 and register here through their work packages.

## 8. Edits and keyframes

Keys follow the item and its content through edits. Two operations on a key list describe every rule:

- **`shift(K, δ)`:** every `t` becomes `t + δ`, exactly. If a result leaves ±(2⁵³ − 1), the edit fails with `Invalid` naming the property. Keys are never silently dropped.
- **`window(K, a, b)`:** the keys with `a ≤ t < b`, plus the last key before `a` and the first key at or after `b`, when they exist.

**`window` is exact.** For every `τ` in `[a, b)`, `eval(window(K, a, b), τ)` equals `eval(K, τ)` bit for bit. Evaluation at `τ` reads only the key at or before `τ` and the one after it, both of which the window keeps, with their interps untouched. The clamps before the first and after the last key agree too. `shift` leaves every `dt` and `span` unchanged, so shifted keys evaluate to the same bits at the shifted times. The self-tests in `keyframes_ref.py` check both claims on 400 random documents.

### 8.1 What each edit does

For an item of duration `D`, all times item-relative:

| Edit | Keys of the item(s) it changes |
|---|---|
| **Split** at `s` (0 < s < D) | Left half: `window(K, 0, s)`. Right half: `shift(window(K, s, D), −s)`. |
| **Trim the tail** to `D′` (plain or ripple) | Unchanged. With `prune_keyframes`: `window(K, 0, D′)`. |
| **Trim the head** by `Δ` (`Δ > 0` trims in, `Δ < 0` extends; plain or ripple) | `shift(K, −Δ)`, so keys stay on the same timeline position and source frame. With `prune_keyframes`: then `window(…, 0, D − Δ)`. |
| **Roll** | The left item's tail trim and the right item's head trim, by the same `Δ`. |
| **Slide** | The slid item: unchanged. Its neighbours: the left one's tail trim and the right one's head trim. |
| **Slip** | Unchanged: keys stay with the item, and the picture slides under them. |
| **Move, ripple shift, track change, magnetic close-gap** | Unchanged (item-relative). |
| **Lift, extract, ripple delete of a range, overwrite** | The splits and trims they are made of, by the rules above. |
| **Duplicate, copy and paste, compound nesting** (WP-2A.06, WP-2B.04) | Unchanged: keys travel with the item. |
| **Replace clip** (WP-2A.11) | Unchanged. Keys beyond a shorter new duration are kept. |
| **Speed change** (WP-2D.01) | Unchanged. Keys beyond the new end are kept (open question for the owner, §15). |
| **Reverse** (WP-2D.01) | Unchanged: the animation stays with the item, not the picture. |
| **Freeze frame** (WP-2D.01) | The halves follow the split rule. The still item gets static values, each property's value at the freeze time, and no keys. |
| **Paste attributes** (WP-2A.06) | Copies each chosen property's `Animatable` as it is. With an optional `scale_times`, as Premiere offers, keys map to `round_half_even(t·D_to/D_from)` in `i128` and then snap (§3.2). Where two keys land on one time, the later one wins. |

Linked partners (video and its audio) are trimmed and split together by the same amounts, and each applies the rule to its own keys. Static properties are never touched.

### 8.2 Prune

`prune_keyframes: bool` (default `false`) is a parameter of `clip.trim`, `clip.ripple_trim`, `clip.roll` and `clip.slide` (WP-2C.03 adds it to WP-2A.04's commands). It applies `window` after the trim. Splits always use `window`, so they never need it.

- **Trims keep keys by default** because trims are gestures: dragging an edge in and back out under one `gesture` transaction must not lose the keys it passed. Extending a trimmed clip brings its animation back, as in every NLE.
- **Splits window by default** because they duplicate. Keeping every key in both halves would multiply keys with each cut: a clip carrying a key every few frames (auto-reframe, P5) cut into a hundred jump-cut pieces would hold a hundred copies of the whole curve. The window keeps at most two keys per property beyond what each half shows.

### 8.3 Why split does not insert boundary keys

The WBS asks for a boundary value "inserted into both halves so that both halves evaluate identically". A key at the cut would end the segment there and change its shape:

- **Hold and linear** survive with the right value.
- **Bezier** survives only after subdividing the curve (de Casteljau) and renormalizing its handles. That rewrites the user's handles, accumulates rounding, and is undefined when the progress at the cut is 0.
- **Spring** has no such representation: the piece after the cut starts moving.

Keeping the bracketing keys instead is exact for every kind, bit for bit, and inserts nothing the user did not make. WP-2C.05's lanes show segments that enter from beyond an edge, with a marker for keys outside the item, as After Effects does for layers. A person who wants a key at the cut clicks ◇ at the playhead (§11).

## 9. Command surface (for WP-2C.03)

### 9.1 Common rules

- **Namespaces** (arch §5.1): `keyframe.*`, plus `item.set_props` and `clip.reset_props`. Each command is a `#[command]` in `cutweave-ops` applied through `Tx`, undoable as one step, with JSON Schema, an `mcp =` tag (§10) and CLI/MCP/RPC parity.
- **Addressing.** A key is `(item, prop, t)`. `prop` is a prop ID (§7); a property the item's kind does not have is `Unsupported`. `KeyRef = {prop, t}`, where `t` is the key's exact stored item time, as `item.get` returns it. Addressing never snaps: it must match a key exactly.
- **Time inputs.** A new or destination time is given as `t` (item time) or `at` (sequence time), exactly one; giving neither or both is `Invalid`. Both are `TimeInput`. Frame forms resolve at the item's sequence rate, and for `t` they count from the item's start (`"f:12"` is 12 frames in). Both snap as in §3.2, and outputs report `t` and `at`. `fine: true` (audio properties only, otherwise `Invalid`) snaps to the sample grid instead.
- **Keys outside the item** may be set, moved and deleted. Outputs flag them `outside: true`.
- **Values** are validated against the property's stored range (§7): out of range is `Invalid` with the range in the message. Commands never clamp silently.
- **Interpolation inputs.** `interp` is the stored form (§3.1); in commands `bounce` may be omitted and defaults to `false`. `easing` is a preset name or `linear` or `hold` (§4.6). Giving both is `Invalid`. Discrete properties accept only hold (KF-6), and omitting both means hold for them.
- **Errors** (arch §5.1): `NotFound` (item, or no key at a `KeyRef`), `Locked` (the item's track is locked), `Invalid{path, msg}` (ranges, collisions, a missing `at`), `Unsupported` (a property the item does not have), and `Conflict` (another actor's gesture holds the item). Every command is all-or-nothing.
- **Linked partners are not affected.** Keyframing a video clip's opacity does not touch its linked audio clip.
- **Gestures.** `keyframe.set`, `keyframe.move` and `item.set_props` are gesture-friendly: called repeatedly under a `gesture` transaction (dragging a key, scrubbing a slider), they collapse into one undo step (arch §5.2).
- **Patches.** A keyframe edit should emit one `replace` of the whole animatable at its document path, so undo conflicts are detected per property. The P1 diff already replaces arrays whole. If WP-2C.03 measures drags on very long animations as too heavy, element-level ops are a compatible optimization (RFC 6902 is generic).

### 9.2 `keyframe.set`

```text
keyframe.set {
  item: ItemId | items: ItemId[],            // exactly one; items[] applies the same keys to each (multi-select)
  prop: PropId,
  keys: [{ t?: TimeInput, at?: TimeInput,     // exactly one
           value?: T,                          // omitted: the property's current value at that time
           interp?: Interp, easing?: Preset,   // at most one
           preset?: string }],                 // hint, stored as given (write-back of item.get keys)
  replace?: bool = false,                      // true: the result is exactly these keys
  fine?: bool = false
} → { keys: [{ item, prop, t, at, value, created: bool, outside: bool }] }
```

- 1 to 10,000 keys per call. Two keys that snap to one time are `Invalid`. Larger animations take several calls (KF-1 still caps the total at 100,000).
- **A static property becomes animated** with exactly the given keys; the old static value is dropped. One key means the value is constant but the property is animated, so it shows a lane.
- **A key at an occupied time replaces** that key's value and keeps its interp, unless one is given.
- **A new key's interp,** when none is given, is the interp of the key before it, so the "feel" of the animation carries on; before the first key, or on a static property, it is the property default (linear, or hold for discrete properties).
- **`value` omitted** keys the current value at that time after the property clamp, as the Inspector's ◇ does. A static property gives its static value.
- **`replace: true`** first removes every key. The keys `item.get` returns can be written back unchanged, since they are already on the grid. This is also how an agent copies a whole animation.
- MCP core. Annotations: not destructive (undoable), idempotent.

### 9.3 `item.set_props` (auto-key)

P1's command, extended by WP-2C.03 with `crop`, `flip`, `blend_mode` and `audio_pan` (all partial) and with two parameters:

```text
item.set_props {
  item: ItemId | items: ItemId[],
  props: ItemProps,              // bare values only, as in P1, plus the new fields
  auto_key?: bool = true,
  at?: TimeInput                 // sequence time, needed when a given property is animated
} → { item(s), changed: [PropId], keyed: [{ item, prop, t, at }] }
```

| The property is | `auto_key` | Result |
|---|---|---|
| static | any | The static value is set (P1 behaviour), and `at` is ignored. |
| animated | `true` | The value is keyed at `at`, exactly as `keyframe.set` with one key: snapping, replacing at an occupied time, inheriting the interp. `at` must lie in `[start, end)` of the item, otherwise `Invalid` ("the playhead is outside the item"). With no `at` it is `Invalid`: "`opacity` is animated: give `at` to key it, or call `keyframe_clear` first". |
| animated | `false` | `Invalid`. Callers that only mean to change static values set it, so they can never key by accident. |

- **The UI** always passes the playhead as `at` (Inspector edits, WP-2H.06's handle drags, WP-2C.08's layouts). Under a gesture, the first update creates the key and later updates replace its value at the same `t`.
- **Agents** are refused with a recovery hint on an animated property until they say when.
- **`items[]`** applies the rules per item, and the whole call fails if any item fails.
- Keyed values are not accepted here. Use `keyframe.set{replace: true}`; this keeps `item_set_props`'s core schema small (§10).
- MCP core (already, R33).

### 9.4 `keyframe.delete`

`{item, keys: KeyRef[]}` → `{deleted: KeyRef[], now_static: [PropId]}`. Deleting every key of a property makes it static at its first key's value, like `keyframe.clear` without a time. A missing key is `NotFound`. MCP catalog.

### 9.5 `keyframe.move`

`{item, keys: KeyRef[], delta?: TimeInput, to_t?: TimeInput, to_at?: TimeInput, fine?}`, with exactly one of `delta`, `to_t` and `to_at`; the last two need exactly one key. The result is `{moved: [{prop, from_t, t, at}]}`.

- Each moved key's new time is snapped (§3.2). Keys keep their value and interp.
- **Reorder:** a key may move past its neighbours, and the list re-sorts. The moved key keeps its own interp, and the segment that now ends at it keeps the interp of the key before it.
- **Collision:** landing on a key that is not moving, or two moved keys landing on one time, is `Invalid` ("a keyframe already exists at …"). Replacing would delete a neighbour in the middle of a drag even if the drag ends elsewhere, so the UI skips occupied frames while dragging.
- Gesture-friendly. MCP catalog.

### 9.6 `keyframe.set_interp`

`{item, keys: KeyRef[], interp? | easing?}` → `{changed: KeyRef[]}`. It sets the interpolation of the segments starting at those keys. An explicit `interp` carries no `preset` unless given, so editing parameters drops the hint. Discrete properties accept only hold. MCP catalog.

### 9.7 `keyframe.clear`

`{item | items[], prop, t? | at?}` → `{value}`. The property becomes static at its value at that time (property-clamped), or at its first key's value when no time is given. On a static property it does nothing. MCP catalog, annotated destructive like `item.delete`.

### 9.8 `keyframe.copy` and `keyframe.paste`

- **`keyframe.copy`** is a query: `{item, keys: KeyRef[]}` → `KeyframeClipboard{version: 1, keys: [{prop, dt, value, interp}]}`, where `dt` is flicks after the earliest copied key. The UI also puts it on the system clipboard as `application/x-cutweave-keyframes+json`. It lives beside WP-2A.06's item clipboard.
- **`keyframe.paste`** takes `{item | items[], payload, t? | at?, as_prop?: PropId, fine?}`. Each key goes to time + `dt`, snapped, and replaces a key already at that time.
  - Every property must exist on the target, every value must fit its range, and discrete properties must hold.
  - `as_prop` pastes a single-property payload onto another property of the same type (`crop.left` keys onto `crop.right`).
- Both are MCP catalog.

### 9.9 `clip.reset_props`

`{item | items[], props?: PropId[]}`: the given properties (default: every property in §7) become static at their defaults, keys removed. MCP catalog.

### 9.10 Queries

- **`keyframe.eval`** takes `{item, prop, at?}` or `{item, prop, from, to, samples}`. Times are sequence times, with `2 ≤ samples ≤ 1,000` and `to ≥ from`. Sample times follow §6. It returns `{prop, samples: [{at, t, value, in_item}]}`, with property-clamped values in the evaluated form.
  - It is for agents and tests. The UI never calls it per frame: it evaluates locally through WP-2C.06's wasm module (WBS N4), and a lint bans it in `packages/*`.
  - It is read-only, MCP catalog.
- **`item.get`** returns animatable properties in the document form (§3.1), keys included, with `fields` filtering (`["visual.opacity"]`).

### 9.11 Hooks into other commands

| Command (WP) | What changes for keys |
|---|---|
| `clip.split` (WP-2A.03), and anything that splits | §8.1 split rule; no new parameter. |
| `clip.trim`, `clip.ripple_trim`, `clip.roll`, `clip.slide` (WP-2A.04) | §8.1, plus `prune_keyframes?: bool` (WP-2C.03 adds it). |
| `clip.slip`, `item.move`, ripple shifts (WP-2A.03/04) | None. Tests assert that keys are unchanged. |
| `clip.set_speed`, `clip.reverse` (WP-2D.01) | None, by the default rule. |
| `clip.freeze_frame` (WP-2D.01) | Split rule, and the still gets static values at the freeze time. |
| `clip.paste_attributes` (WP-2A.06) | Copies animatables. `scale_times` is optional (§8.1). |
| `item.apply_layout` (WP-2C.08) | Calls the auto-key rule (§9.3), with `at` = the playhead. |

## 10. MCP shape

- **Core tools:** `keyframe_set` and `item_set_props` (WBS 2C.03, R33). **Catalog:** `keyframe_delete`, `keyframe_move`, `keyframe_set_interp`, `keyframe_clear`, `keyframe_copy`, `keyframe_paste`, `keyframe_eval` and `clip_reset_props`, reachable through `cutweave_execute` and `cutweave_query`.
- **Budget.** `keyframe_set`'s listed schema should stay within about 400 tokens. The 23 easing names are one enum, `interp` is a four-way union through `$defs`, and `value` is `number | {x, y} | boolean | string` with the per-property type in the description. `item_set_props` grows only by the new partial fields.
- **Descriptions** (at most 300 characters, one example each, per WP-2I.01):
  - `keyframe_set`: "Add or replace keys on one property. `t` = item time, `at` = sequence time; both snap to frames. A key's `easing` shapes the move to the next key. Example: `{item, prop:"opacity", keys:[{at:"10s", value:0, easing:"ease_out"}, {at:"12s", value:1}]}`".
  - `item_set_props`: "Set item properties (partial). An animated property is keyed at `at` (sequence time, required then). Example: `{item, props:{opacity:0.5, crop:{left:0.25}}}`".
- **Error messages carry a recovery hint**, for example: "Invalid /keys/1/at: a keyframe already exists at 00:00:12:00 on opacity; use keyframe_move or pick another time", or "Invalid /at: transform.scale is animated: give at (sequence time) to key it, or call keyframe_clear first".
- **Results** are small: the keys touched with `t`, `at` and their values, plus `rev`, never the whole curve. Agents read curves with `item_get` (`fields`) or `keyframe_eval`.
- **A worked agent flow,** a 9:16 reframe pan of a 16:9 clip: `item_set_props{item, props:{transform:{scale:{x:3.16, y:3.16}}}}` (fills the height), then `keyframe_set{item, prop:"transform.position", keys:[{at:"3s", value:{x:0.2, y:0}, easing:"ease_in_out"}, {at:"6s", value:{x:-0.2, y:0}}]}`, then `render_frame` at 4.5 s to check.

## 11. UI contract (WP-2C.05 to WP-2C.07, WP-2H)

- **◇ in the Inspector** (WP-2H.02, WP-2H.03), at the playhead `P`:
  - On a static property it calls `keyframe.set{at: P}` with no value, which starts the animation.
  - On an animated property with a key at `P` (the key's `start + t` equals `P` after snapping), it calls `keyframe.delete`.
  - Otherwise it calls `keyframe.set{at: P}`.
  - "Stop animating" calls `keyframe.clear{at: P}`.
  - The diamond is filled exactly when a key sits at `P`.
- **Prev/next keyframe** navigation moves to the nearest key time strictly before or after `P`, across all of the item's animated properties and inside the item only.
- **Lanes** (WP-2C.05) draw keys inside `[0, D)` with a glyph per kind (hold, linear, bezier, spring), show segments that enter from beyond an edge, and mark that keys exist outside. They use `Animatable::sample_times()` and collapse diamonds less than 3 px apart. Dragging keys calls `keyframe.move` under a gesture, skipping occupied frames.
- **The curve editor and graph editor** (WP-2C.05, WP-2C.07) draw with the wasm module's `easing.sample(interp, span, u[])` and `anim.evalRange`, never an RPC. For vec2 they plot both components and edit the one shared easing. Handles map as in §4.4; springs show the residual warning (§4.5); the preset picker lists §4.6 in its order.
- **The wasm API** (WP-2C.06) mirrors §6. Times cross the boundary as integer flicks in `Float64Array`s, which is lossless below 2⁵³. `easing.presets()` returns the §4.6 table.

## 12. Test vectors

[keyframes-vectors.json](keyframes-vectors.json) is generated by [keyframes_ref.py](keyframes_ref.py) (Python standard library only). Run `python docs/spec/keyframes_ref.py` to check that the file is current and to run the self-tests, and `--write` to regenerate it after a deliberate change, then review the diff.

| Section | Rows | Each row | Consumed by |
|---|---|---|---|
| `presets` | 21 | name → the stored interp | WP-2C.02 (`Easing::preset`), WP-2C.06 |
| `easing` (E) | 129 | `interp`, `dt`, `span` → `expected` progress | WP-2C.02, WP-2C.06 (`easing.sample` parity) |
| `eval` (V) | 69 | `type`, `prop` (null = no clamp), `animatable`, `t` → `expected` in the evaluated form | WP-2C.02, WP-2C.06 |
| `retime` (R) | 12 | `op` (split, trim_start, trim_end), `animatable` → `expected` animatable(s) | WP-2C.03 |
| `ops` (O) | 20 | `op` (set, delete, move, clear; times and snapping) → `result` or `error` | WP-2C.03 |
| `invalid` (X) | 14 | an `animatable` that breaks an invariant → its `KF-n` | WP-2C.02 (construction), WP-2C.03 |

- **Tolerance** is 1e-6 absolute per component (`tolerance` in the file), except rows with `"exact": true`, which must be equal as f64.
- **Values** are written in the document form: numbers, `{x, y}`, booleans and colour strings. Evaluated colours are `{r, g, b, a}`, premultiplied and linear.
- **Ops fields.** `set` rows give `t` (item time) or `at`, plus `start` and `rate` (`[num, den]`) when snapping matters; without them the time is already on the grid. `delete` and `move` rows give the addressed `ts` and a `delta`. `error` is the command error kind (`Invalid`, `NotFound`), or the `KF-n` rule behind an `Invalid`.
- **Self-tests** run on 400 random documents covering every kind and type. They check that split and head trims keep evaluation bit-identical, that values are continuous into every non-hold key, that linear segments are monotone, that hold yields no intermediate values, and that nothing is ever NaN or infinite.
- **Gate.** WP-2C.02's Rust tests read this file. The integrator may also add the script's check mode to `just check` (it takes about a second).

## 13. Out of scope and later

- Speed ramps (P4).
- Mask, effect and composition parameters (P4). They reuse §5 and register in §7.
- Spatial (motion-path) bezier for position, and its preview path (P4).
- Per-component easing, per-key tangent modes, expressions, and keys driven by motion tracking (P4, 4I). Auto-reframe keys (P5 5G) should be decimated before they are written.
- A dedicated bounce kind (§4.6).

## 14. Deltas from the WBS text (for the integrator)

- **WP-2C.01 and WP-2C.03 split rule.** The WBS says a boundary value is inserted into both halves. This spec keeps bracketing keys instead (§8.3). WP-2C.03's acceptance test ("each half equals the original within 1e-6") holds, and holds bit-exactly.
- **WP-2C.01 bounce.** Bounce is a spring with `bounce: true` (§4.5, §4.6). The interp set is still hold, linear, bezier and spring.
- **WP-2C.01 "linked handles optional" for vec2:** linked only in P2 (§5.2).
- **WP-2C.03:** `keyframe.set` takes `keys[]` and `replace`. `item.set_props` takes bare values only. `audio.pan` joins WP-2C.03's model migration (§7).
- **Arch §4.2** said every `VisualProps` field is animatable; `blend_mode` is static (D8). Arch §4.2 and §8 now point here.

## 15. Open questions for the owner

The spec decides both of these so work can proceed; either answer changes one rule, not the model.

1. **Should a speed change stretch keys with the clip?** The spec keeps them where they are (§8.1, as the WBS says). Then a zoom animated over a whole clip stops halfway when the clip is sped up to 2×, because its last key now lies beyond the end. The alternative is `clip.set_speed{keyframes: "keep" | "scale"}` with the `scale_times` mapping of §8.1. WP-2D.01 can add it whichever is the default.
2. **Should a split leave a visible key at the cut?** The spec keeps the neighbouring keys outside each half instead (§8.3): it is exact, and the lanes show the curve entering from the edge. Some editors show a key at the cut. Adding one is exact only for hold and linear segments, so it would be an option for those.
