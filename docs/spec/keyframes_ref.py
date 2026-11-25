#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Reference evaluator for the keyframe and easing model (docs/spec/keyframes.md, WP-2C.01).

This is the executable form of the spec. It generates the golden vectors in
docs/spec/keyframes-vectors.json, which the Rust model (WP-2C.02), the keyframe commands
(WP-2C.03) and the wasm module (WP-2C.06) must reproduce, and it property-checks the spec's
claims (split identity, continuity, monotone linear segments, no NaN or infinity).

    python docs/spec/keyframes_ref.py            # check: regenerate in memory, compare, self-test
    python docs/spec/keyframes_ref.py --write    # rewrite the vectors file

Standard library only. It is a reference, not production code: nothing imports it, and it
favours clarity over speed (the bezier solver bisects to full precision instead of using Newton).
"""

from __future__ import annotations

import argparse
import bisect
import json
import math
import random
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
VECTORS = HERE / "keyframes-vectors.json"

# --- Constants (spec §2, §3) -------------------------------------------------------------------

FLICKS_PER_SECOND = 705_600_000
MAX_SAFE_FLICKS = (1 << 53) - 1
MAX_KEYFRAMES = 100_000  # KF-1
BEZIER_Y_LIMIT = 10.0  # KF-5
SPRING_RANGES = {"stiffness": (1.0, 10_000.0), "damping": (0.0, 1_000.0), "mass": (0.1, 100.0)}
CRITICAL_BAND = 1e-9  # spec §4.5: |zeta - 1| <= 1e-9 uses the critically damped form
PRESET_NAME = re.compile(r"^[a-z][a-z0-9_]{0,63}$")
SECOND = FLICKS_PER_SECOND

# --- Easing presets (spec §4.6). Order is the UI order. ----------------------------------------


def _bez(x1: float, y1: float, x2: float, y2: float) -> dict:
    return {"kind": "bezier", "x1": x1, "y1": y1, "x2": x2, "y2": y2}


def _spring(stiffness: float, damping: float, mass: float, bounce: bool) -> dict:
    return {"kind": "spring", "stiffness": stiffness, "damping": damping, "mass": mass,
            "bounce": bounce}


PRESETS: dict[str, dict] = {
    "ease_in": _bez(0.42, 0.0, 1.0, 1.0),
    "ease_out": _bez(0.0, 0.0, 0.58, 1.0),
    "ease_in_out": _bez(0.42, 0.0, 0.58, 1.0),
    "sine_in": _bez(0.12, 0.0, 0.39, 0.0),
    "sine_out": _bez(0.61, 1.0, 0.88, 1.0),
    "sine_in_out": _bez(0.37, 0.0, 0.63, 1.0),
    "cubic_in": _bez(0.32, 0.0, 0.67, 0.0),
    "cubic_out": _bez(0.33, 1.0, 0.68, 1.0),
    "cubic_in_out": _bez(0.65, 0.0, 0.35, 1.0),
    "expo_in": _bez(0.7, 0.0, 0.84, 0.0),
    "expo_out": _bez(0.16, 1.0, 0.3, 1.0),
    "expo_in_out": _bez(0.87, 0.0, 0.13, 1.0),
    "back_in": _bez(0.36, 0.0, 0.66, -0.56),
    "back_out": _bez(0.34, 1.56, 0.64, 1.0),
    "back_in_out": _bez(0.68, -0.6, 0.32, 1.6),
    "spring": _spring(170.0, 26.0, 1.0, False),
    "spring_gentle": _spring(120.0, 14.0, 1.0, False),
    "spring_wobbly": _spring(180.0, 12.0, 1.0, False),
    "spring_stiff": _spring(210.0, 20.0, 1.0, False),
    "elastic": _spring(200.0, 6.0, 1.0, False),
    "bounce": _spring(160.0, 10.0, 1.0, True),
}


def expand_easing(name: str) -> dict:
    """`easing` command input → stored interp (spec §4.6): presets carry their name as a hint."""
    if name in ("linear", "hold"):
        return {"kind": name}
    interp = dict(PRESETS[name])
    interp["preset"] = name
    return interp


# --- Property registry (spec §7) ---------------------------------------------------------------

PROPS: dict[str, dict] = {
    "transform.position": {"type": "vec2", "range": None, "default": {"x": 0.0, "y": 0.0}},
    "transform.scale": {"type": "vec2", "range": (0.0, None), "default": {"x": 1.0, "y": 1.0}},
    "transform.rotation": {"type": "angle", "range": None, "default": 0.0},
    "transform.anchor": {"type": "vec2", "range": None, "default": {"x": 0.5, "y": 0.5}},
    "opacity": {"type": "scalar", "range": (0.0, 1.0), "default": 1.0},
    "crop.left": {"type": "scalar", "range": (0.0, 1.0), "default": 0.0},
    "crop.top": {"type": "scalar", "range": (0.0, 1.0), "default": 0.0},
    "crop.right": {"type": "scalar", "range": (0.0, 1.0), "default": 0.0},
    "crop.bottom": {"type": "scalar", "range": (0.0, 1.0), "default": 0.0},
    "flip.horizontal": {"type": "bool", "range": None, "default": False},
    "flip.vertical": {"type": "bool", "range": None, "default": False},
    "audio_gain": {"type": "scalar", "range": (-144.0, 48.0), "default": 0.0},
    "audio_pan": {"type": "scalar", "range": (-1.0, 1.0), "default": 0.0},
}

# Value types used by vectors that name no property. `angle_shortest` is the shortest-path angle
# mode reserved for later properties (hue); no P2 property uses it.
VALUE_TYPES = {"scalar", "vec2", "angle", "angle_shortest", "color", "bool"}
DISCRETE = {"bool"}


class SpecError(Exception):
    """A rule of the spec rejected an input; `code` is the invariant (KF-n) or command rule."""

    def __init__(self, code: str, msg: str):
        super().__init__(f"{code}: {msg}")
        self.code = code


# --- Validation (spec §3.3) --------------------------------------------------------------------


def is_keyframed(anim) -> bool:
    return isinstance(anim, dict) and "keyframes" in anim


def _finite(x) -> bool:
    return isinstance(x, (int, float)) and not isinstance(x, bool) and math.isfinite(x)


def check_value(value, vtype: str, rng) -> None:
    if vtype in ("scalar", "angle", "angle_shortest"):
        ok = _finite(value) and (rng is None or _in_range(value, rng))
    elif vtype == "vec2":
        ok = (isinstance(value, dict) and set(value) == {"x", "y"}
              and _finite(value["x"]) and _finite(value["y"])
              and (rng is None or (_in_range(value["x"], rng) and _in_range(value["y"], rng))))
    elif vtype == "bool":
        ok = isinstance(value, bool)
    elif vtype == "color":
        ok = isinstance(value, str) and re.fullmatch(r"#[0-9a-fA-F]{6}([0-9a-fA-F]{2})?", value)
    else:
        raise ValueError(vtype)
    if not ok:
        raise SpecError("KF-4", f"value {value!r} is not a valid {vtype} in range {rng}")


def _in_range(x: float, rng) -> bool:
    lo, hi = rng
    return (lo is None or x >= lo) and (hi is None or x <= hi)


def check_interp(interp, vtype: str) -> None:
    kind = interp.get("kind") if isinstance(interp, dict) else None
    allowed = {"hold": {"kind"}, "linear": {"kind"},
               "bezier": {"kind", "x1", "y1", "x2", "y2", "preset"},
               "spring": {"kind", "stiffness", "damping", "mass", "bounce", "preset"}}
    if kind not in allowed or not set(interp) <= allowed[kind]:
        raise SpecError("KF-5", f"unknown interpolation {interp!r}")
    if vtype in DISCRETE and kind != "hold":
        raise SpecError("KF-6", f"{vtype} values only hold, got {kind}")
    if kind == "bezier":
        for k in ("x1", "x2"):
            if not (_finite(interp.get(k)) and 0.0 <= interp[k] <= 1.0):
                raise SpecError("KF-5", f"bezier {k} must be in [0, 1]")
        for k in ("y1", "y2"):
            if not (_finite(interp.get(k)) and -BEZIER_Y_LIMIT <= interp[k] <= BEZIER_Y_LIMIT):
                raise SpecError("KF-5", f"bezier {k} must be in [-10, 10]")
    if kind == "spring":
        for k, (lo, hi) in SPRING_RANGES.items():
            if not (_finite(interp.get(k)) and lo <= interp[k] <= hi):
                raise SpecError("KF-5", f"spring {k} must be in [{lo}, {hi}]")
        if not isinstance(interp.get("bounce"), bool):
            raise SpecError("KF-5", "spring bounce must be a boolean")
    if "preset" in interp and not (isinstance(interp["preset"], str)
                                   and PRESET_NAME.fullmatch(interp["preset"])):
        raise SpecError("KF-7", "preset hint must match ^[a-z][a-z0-9_]{0,63}$")


def validate(anim, vtype: str, rng=None) -> None:
    """Raises SpecError naming the first violated invariant (KF-1..KF-7)."""
    if not is_keyframed(anim):
        check_value(anim, vtype, rng)
        return
    kfs = anim["keyframes"]
    if not isinstance(kfs, list) or not 1 <= len(kfs) <= MAX_KEYFRAMES:
        raise SpecError("KF-1", "a keyframed value has 1 to 100,000 keyframes")
    prev = None
    for k in kfs:
        t = k.get("t")
        if not (isinstance(t, int) and not isinstance(t, bool) and abs(t) <= MAX_SAFE_FLICKS):
            raise SpecError("KF-3", f"t {t!r} is not an integer flick time within +-(2^53-1)")
        if prev is not None and t <= prev:
            raise SpecError("KF-2", f"keyframe times must strictly increase ({prev} then {t})")
        prev = t
        check_value(k.get("value"), vtype, rng)
        check_interp(k.get("interp"), vtype)


# --- Easing functions (spec §4) ----------------------------------------------------------------


def _bezier_coeffs(p1: float, p2: float) -> tuple[float, float, float]:
    c = 3.0 * p1
    b = 3.0 * (p2 - p1) - c
    a = 1.0 - c - b
    return a, b, c


def _bezier_sample(coeffs, s: float) -> float:
    a, b, c = coeffs
    return ((a * s + b) * s + c) * s


def bezier_progress(interp: dict, u: float) -> float:
    """p = By(s) where Bx(s) = u (spec §4.4). x handles are clamped to [0, 1] defensively."""
    x1 = min(max(interp["x1"], 0.0), 1.0)
    x2 = min(max(interp["x2"], 0.0), 1.0)
    cx = _bezier_coeffs(x1, x2)
    cy = _bezier_coeffs(interp["y1"], interp["y2"])
    lo, hi = 0.0, 1.0
    # Bx is monotone non-decreasing on [0, 1] for x handles in [0, 1], so bisection finds the
    # unique solution to the last bit; the reference does not need Newton's speed.
    for _ in range(200):
        mid = 0.5 * (lo + hi)
        if mid in (lo, hi):
            break
        if _bezier_sample(cx, mid) < u:
            lo = mid
        else:
            hi = mid
    return _bezier_sample(cy, 0.5 * (lo + hi))


def spring_displacement(interp: dict, tau: float) -> float:
    """x(tau): distance still to go, 1 at tau = 0, for a spring released from rest (spec §4.5)."""
    k, c, m = interp["stiffness"], interp["damping"], interp["mass"]
    w0 = math.sqrt(k / m)
    zeta = c / (2.0 * math.sqrt(k * m))
    if zeta < 1.0 - CRITICAL_BAND:
        wd = w0 * math.sqrt(1.0 - zeta * zeta)
        x = math.exp(-zeta * w0 * tau) * (math.cos(wd * tau) + (zeta * w0 / wd) * math.sin(wd * tau))
    elif zeta <= 1.0 + CRITICAL_BAND:
        x = math.exp(-w0 * tau) * (1.0 + w0 * tau)
    else:
        q = math.sqrt(zeta * zeta - 1.0)
        r1 = -w0 / (zeta + q)
        r2 = -w0 * (zeta + q)
        x = (r2 * math.exp(r1 * tau) - r1 * math.exp(r2 * tau)) / (r2 - r1)
    return abs(x) if interp["bounce"] else x


def progress(interp: dict, dt: int, span: int) -> float:
    """Easing progress of a segment of `span` flicks, `dt` flicks after its start (0 < dt < span)."""
    u = float(dt) / float(span)
    kind = interp["kind"]
    if kind == "linear":
        return u
    if kind == "bezier":
        return bezier_progress(interp, u)
    if kind == "spring":
        tau = float(dt) / float(FLICKS_PER_SECOND)
        dur = float(span) / float(FLICKS_PER_SECOND)
        return 1.0 - spring_displacement(interp, tau) + u * spring_displacement(interp, dur)
    if kind == "hold":
        return 0.0
    raise ValueError(kind)


# --- Value types (spec §5) ---------------------------------------------------------------------


def _srgb_to_linear(c: float) -> float:
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def color_to_eval(hexstr: str) -> dict:
    """#rrggbb[aa] (sRGB, straight alpha) → premultiplied linear-light RGBA (the evaluated form)."""
    h = hexstr[1:]
    r, g, b = (int(h[i:i + 2], 16) / 255.0 for i in (0, 2, 4))
    a = int(h[6:8], 16) / 255.0 if len(h) == 8 else 1.0
    return {"r": _srgb_to_linear(r) * a, "g": _srgb_to_linear(g) * a,
            "b": _srgb_to_linear(b) * a, "a": a}


def to_eval(value, vtype: str):
    """A stored value in its evaluated form (identity except colors)."""
    if vtype == "color":
        return color_to_eval(value)
    if vtype == "vec2":
        return {"x": float(value["x"]), "y": float(value["y"])}
    if vtype == "bool":
        return value
    return float(value)


def _lerp(a: float, b: float, p: float) -> float:
    return a + (b - a) * p


def combine(v0, v1, p: float, vtype: str):
    """Interpolated value between two evaluated values at progress p (spec §5)."""
    if vtype in ("scalar", "angle"):
        return _lerp(v0, v1, p)
    if vtype == "angle_shortest":
        d = v1 - v0
        d = d - 360.0 * math.floor((d + 180.0) / 360.0)
        return v0 + d * p
    if vtype == "vec2":
        return {"x": _lerp(v0["x"], v1["x"], p), "y": _lerp(v0["y"], v1["y"], p)}
    if vtype == "color":
        a = min(max(_lerp(v0["a"], v1["a"], p), 0.0), 1.0)
        out = {c: min(max(_lerp(v0[c], v1[c], p), 0.0), a) for c in ("r", "g", "b")}
        out["a"] = a
        return out
    raise ValueError(vtype)


def sanitize(v, fallback, vtype: str):
    """Non-finite components take the segment start's component (spec §6, step 7)."""
    if vtype in ("scalar", "angle", "angle_shortest"):
        return v if math.isfinite(v) else fallback
    if vtype in ("vec2", "color"):
        return {c: (x if math.isfinite(x) else fallback[c]) for c, x in v.items()}
    return v


# --- Evaluation (spec §6) ----------------------------------------------------------------------


def evaluate(anim, t: int, vtype: str):
    """`eval(animatable, t)` without property clamps. Exact at keyframes and outside the keys."""
    if not is_keyframed(anim):
        return to_eval(anim, vtype)
    kfs = anim["keyframes"]
    times = [k["t"] for k in kfs]
    i = bisect.bisect_right(times, t) - 1  # last keyframe with t_i <= t
    if i < 0:
        return to_eval(kfs[0]["value"], vtype)
    k0 = kfs[i]
    if t == k0["t"] or i == len(kfs) - 1:
        return to_eval(k0["value"], vtype)
    interp = k0["interp"]
    v0 = to_eval(k0["value"], vtype)
    if vtype in DISCRETE or interp["kind"] == "hold":
        return v0
    k1 = kfs[i + 1]
    p = progress(interp, t - k0["t"], k1["t"] - k0["t"])
    v1 = to_eval(k1["value"], vtype)
    return sanitize(combine(v0, v1, p, vtype), v0, vtype)


def clamp_prop(v, prop: str):
    rng = PROPS[prop]["range"]
    if rng is None:
        return v
    lo, hi = rng

    def c(x: float) -> float:
        if lo is not None and x < lo:
            return lo
        if hi is not None and x > hi:
            return hi
        return x

    if isinstance(v, dict):
        return {k: c(x) for k, x in v.items()}
    return c(v)


def evaluate_prop(anim, t: int, prop: str):
    """The property layer: generic evaluation, then the registry's clamp (spec §7)."""
    return clamp_prop(evaluate(anim, t, PROPS[prop]["type"]), prop)


# --- Retiming on edits (spec §8) ---------------------------------------------------------------


def _with_keyframes(kfs: list) -> dict:
    return {"keyframes": kfs}


def shift(anim, delta: int):
    if not is_keyframed(anim):
        return anim
    out = []
    for k in anim["keyframes"]:
        t = k["t"] + delta
        if abs(t) > MAX_SAFE_FLICKS:
            raise SpecError("Invalid", "a shifted keyframe leaves the time range")
        out.append({**k, "t": t})
    return _with_keyframes(out)


def window(anim, a: int, b: int):
    """The keyframes needed to evaluate [a, b) exactly: those inside plus one on each side."""
    if not is_keyframed(anim):
        return anim
    kfs = anim["keyframes"]
    before = [k for k in kfs if k["t"] < a]
    inside = [k for k in kfs if a <= k["t"] < b]
    after = [k for k in kfs if k["t"] >= b]
    return _with_keyframes(before[-1:] + inside + after[:1])


def split(anim, at: int, duration: int):
    """clip.split at item time `at` (0 < at < duration): (left, right) animatables."""
    return window(anim, 0, at), shift(window(anim, at, duration), -at)


def trim_start(anim, delta: int, duration: int, prune: bool):
    """Head trim by `delta` (> 0 trims in, < 0 extends): keyframes keep their timeline position."""
    out = shift(anim, -delta)
    return window(out, 0, duration - delta) if prune else out


def trim_end(anim, new_duration: int, prune: bool):
    return window(anim, 0, new_duration) if prune else anim


# --- Keyframe edit primitives (spec §9) --------------------------------------------------------


def snap_to_frame(t: int, num: int, den: int) -> int:
    """FrameRate::snap_to_frame(t, Nearest): the nearest frame start, exact halves go later."""
    period_den = FLICKS_PER_SECOND * den
    frame = (2 * t * num + period_den) // (2 * period_den)
    return -((-frame * FLICKS_PER_SECOND * den) // num)  # frame start, ceil to whole flicks


def key_time(start: int, num: int, den: int, t: int | None = None, at: int | None = None) -> int:
    """Item time of a key given as `t` (item time) or `at` (sequence time), snapped on the
    sequence's frame grid in sequence time (spec §3.2): snap(start + t) - start."""
    absolute = at if at is not None else start + t
    return snap_to_frame(absolute, num, den) - start


def default_interp(prop: str) -> dict:
    return {"kind": "hold"} if PROPS[prop]["type"] in DISCRETE else {"kind": "linear"}


def current_value(anim, t: int, prop: str):
    """The value `keyframe.set` stores when no value is given (property-clamped). P2 has no color
    property; a color would be converted back to `#rrggbbaa` here (spec §5.4)."""
    return evaluate_prop(anim, t, prop)


def set_key(anim, t: int, prop: str, value=None, interp=None):
    """keyframe.set for one key at item time t (already snapped)."""
    vtype = PROPS[prop]["type"]
    if value is None:
        value = current_value(anim, t, prop)
    check_value(value, vtype, PROPS[prop]["range"])
    if interp is not None:
        check_interp(interp, vtype)
    if not is_keyframed(anim):
        return _with_keyframes([{"t": t, "value": value,
                                 "interp": interp or default_interp(prop)}])
    kfs = [dict(k) for k in anim["keyframes"]]
    times = [k["t"] for k in kfs]
    i = bisect.bisect_left(times, t)
    if i < len(kfs) and kfs[i]["t"] == t:  # replace: keep the interp unless one is given
        kfs[i] = {**kfs[i], "value": value, "interp": interp or kfs[i]["interp"]}
    else:  # insert: inherit the previous key's interp, else the property default
        inherited = kfs[i - 1]["interp"] if i > 0 else default_interp(prop)
        kfs.insert(i, {"t": t, "value": value, "interp": interp or inherited})
    if len(kfs) > MAX_KEYFRAMES:
        raise SpecError("KF-1", "too many keyframes")
    return _with_keyframes(kfs)


def delete_keys(anim, ts: list[int]):
    if not is_keyframed(anim):
        raise SpecError("NotFound", "the property is not animated")
    kfs = anim["keyframes"]
    present = {k["t"] for k in kfs}
    for t in ts:
        if t not in present:
            raise SpecError("NotFound", f"no keyframe at t={t}")
    left = [k for k in kfs if k["t"] not in set(ts)]
    if not left:  # every key goes: static at the first key's value (as keyframe.clear)
        return kfs[0]["value"]
    return _with_keyframes(left)


def move_keys(anim, ts: list[int], delta: int):
    """keyframe.move by delta: reorders past neighbours; landing on a kept key is Invalid."""
    if not is_keyframed(anim):
        raise SpecError("NotFound", "the property is not animated")
    kfs = anim["keyframes"]
    moving = set(ts)
    if not moving <= {k["t"] for k in kfs}:
        raise SpecError("NotFound", "no keyframe at one of the times")
    staying = [k for k in kfs if k["t"] not in moving]
    moved = [{**k, "t": k["t"] + delta} for k in kfs if k["t"] in moving]
    taken = {k["t"] for k in staying}
    for k in moved:
        if k["t"] in taken:
            raise SpecError("Invalid", f"a keyframe already exists at t={k['t']}")
        if abs(k["t"]) > MAX_SAFE_FLICKS:
            raise SpecError("Invalid", "the moved keyframe leaves the time range")
    return _with_keyframes(sorted(staying + moved, key=lambda k: k["t"]))


def clear(anim, prop: str, t: int | None = None):
    """keyframe.clear: static at the value at t, or at the first keyframe's value."""
    if not is_keyframed(anim):
        return anim
    if t is None:
        return anim["keyframes"][0]["value"]
    return current_value(anim, t, prop)


# --- Vector generation --------------------------------------------------------------------------


def kf(t: int, value, interp: dict | str = "linear") -> dict:
    if isinstance(interp, str):
        interp = expand_easing(interp)
    return {"t": t, "value": value, "interp": interp}


def anim_of(*keys) -> dict:
    return {"keyframes": list(keys)}


def v2(x: float, y: float) -> dict:
    return {"x": x, "y": y}


S = SECOND
FRAME_30 = SECOND // 30  # 23,520,000 flicks


def easing_rows() -> list[dict]:
    rows = []
    fractions = [(1, 10), (1, 4), (1, 2), (3, 4), (9, 10)]
    for name in PRESETS:
        interp = expand_easing(name)
        for num, den in fractions:
            rows.append({"interp": interp, "dt": S * num // den, "span": S})
    custom = [
        ("linear handles (0,0,1,1) give p = u within tolerance", _bez(0.0, 0.0, 1.0, 1.0), S // 3, S),
        # Not at u = 0.5 itself: Bx has a zero-slope inflection there, p is a cube root of u, and
        # no f64 solver is better than ~1e-5 at that point (spec §4.4, conditioning).
        ("x1 = 1, x2 = 0: just after Bx's flat inflection", _bez(1.0, 0.0, 0.0, 1.0), S // 2 + 1000,
         S),
        ("steep start (x1 = 0, y1 = 1), one flick in: the solver must not stop early",
         _bez(0.0, 1.0, 0.0, 1.0), 1, S),
        ("steep start (x1 = 0, y1 = 1), 1 ms in", _bez(0.0, 1.0, 0.0, 1.0), S // 1000, S),
        ("steep end (x2 = 1, y2 = 0), one flick before the end",
         _bez(1.0, 0.0, 1.0, 0.0), S - 1, S),
        ("y handles at the limits overshoot far", _bez(0.5, 10.0, 0.5, -10.0), S // 4, S),
        ("y handles at the limits, middle", _bez(0.5, 10.0, 0.5, -10.0), S // 2, S),
        ("one-flick segment", _bez(0.42, 0.0, 0.58, 1.0), 1, 2),
        ("segment of 2^52 flicks", _bez(0.42, 0.0, 0.58, 1.0), (1 << 51) + 12345, 1 << 52),
        ("spring on a short segment: the residual pulls it onto the target",
         _spring(170.0, 26.0, 1.0, False), S // 20, S // 10),
        ("spring on a long segment: settled long before the end",
         _spring(170.0, 26.0, 1.0, False), 5 * S, 10 * S),
        ("undamped spring (damping 0) never settles; the residual lands it",
         _spring(100.0, 0.0, 1.0, False), S // 2, S),
        ("undamped spring with bounce: |cos|", _spring(100.0, 0.0, 1.0, True), S // 3, S),
        ("critically damped (zeta = 1 exactly)", _spring(100.0, 20.0, 1.0, False), S // 4, S),
        ("zeta = 1 + 5e-10 is inside the critical band", _spring(100.0, 20.00000001, 1.0, False),
         S // 4, S),
        ("zeta = 1 - 5e-10 is inside the critical band", _spring(100.0, 19.99999999, 1.0, False),
         S // 4, S),
        ("zeta = 1 + 1e-6 is overdamped", _spring(100.0, 20.00002, 1.0, False), S // 4, S),
        ("zeta = 1 - 1e-6 is underdamped", _spring(100.0, 19.99998, 1.0, False), S // 4, S),
        ("strongly overdamped (zeta = 50): slow root without cancellation",
         _spring(1.0, 100.0, 1.0, False), S // 2, S),
        ("heavy mass, soft spring", _spring(1.0, 0.5, 100.0, False), S // 2, S),
        ("stiffest light spring", _spring(10_000.0, 10.0, 0.1, False), S // 7, S),
        ("bounce preset on a 2 s segment", PRESETS["bounce"], 3 * S // 10, 2 * S),
        ("bounce preset on a 2 s segment, later", PRESETS["bounce"], 13 * S // 10, 2 * S),
        ("elastic preset on a 0.3 s segment (unsettled)", PRESETS["elastic"], S // 10, 3 * S // 10),
    ]
    for note, interp, dt, span in custom:
        rows.append({"note": note, "interp": interp, "dt": dt, "span": span})
    for i, row in enumerate(rows, 1):
        row["id"] = f"E{i:03d}"
        row["expected"] = progress(row["interp"], row["dt"], row["span"])
        if row["interp"]["kind"] == "linear":
            row["exact"] = True
    return [{"id": r["id"], **{k: r[k] for k in r if k != "id"}} for r in rows]


def eval_rows() -> list[dict]:
    lin01 = anim_of(kf(0, 0.0), kf(S, 1.0))
    rows: list[tuple] = []  # (note, type, prop, animatable, t)

    # Static and single keyframe.
    rows += [
        ("static scalar", "scalar", None, 0.25, 12345),
        ("static vec2", "vec2", None, v2(0.1, -0.2), -5),
        ("static bool", "bool", None, True, 0),
        ("static color is converted to premultiplied linear light", "color", None, "#ff000080", 0),
    ]
    one = anim_of(kf(S, 0.7))
    rows += [("single keyframe, before it", "scalar", None, one, 0),
             ("single keyframe, at it", "scalar", None, one, S),
             ("single keyframe, after it", "scalar", None, one, 10 * S)]

    # Two linear keyframes: clamps, exact keys, interior points, one flick before the end.
    for note, t in [("before the first key: its value", -1), ("at the first key", 0),
                    ("one flick in", 1), ("a quarter", S // 4), ("a third (rounded u)", S // 3),
                    ("half", S // 2), ("one flick before the last key", S - 1),
                    ("at the last key", S), ("after the last key", S + 1)]:
        rows.append((f"linear 0 -> 1: {note}", "scalar", None, lin01, t))

    # Linear between awkward values: the formula v0 + (v1 - v0) * u is normative.
    awkward = anim_of(kf(0, 0.1), kf(3 * FRAME_30, 0.3))
    for f in range(4):
        rows.append((f"linear 0.1 -> 0.3 at frame {f} of 3 (30 fps)", "scalar", None, awkward,
                     f * FRAME_30))
    descending = anim_of(kf(-S, 5.0), kf(S, -3.0))
    rows += [("descending linear across t = 0 (negative key time)", "scalar", None, descending, 0),
             ("descending linear, 1 ms before the end", "scalar", None, descending, S - S // 1000)]

    # Hold.
    held = anim_of(kf(0, 1.0, "hold"), kf(S, 2.0, "linear"), kf(2 * S, 4.0))
    for note, t in [("hold keeps the start value", S // 2), ("hold, one flick before the jump", S - 1),
                    ("hold, at the jump", S), ("after hold, linear resumes", 3 * S // 2)]:
        rows.append((f"hold then linear: {note}", "scalar", None, held, t))

    # Three keys, mixed interps; vec2 per-component with one shared easing.
    pos = anim_of(kf(0, v2(0.0, 0.0), "ease_in_out"), kf(S, v2(0.5, -0.25), "back_out"),
                  kf(2 * S, v2(-0.5, 0.25)))
    for t in (S // 4, S // 2, 3 * S // 4, S, 5 * S // 4, 3 * S // 2, 7 * S // 4):
        rows.append(("vec2: ease_in_out then back_out", "vec2", "transform.position", pos, t))

    # Overshoot and clamping.
    over = anim_of(kf(0, 0.0, "back_out"), kf(S, 1.0))
    rows += [("back_out overshoots above 1 (no property: no clamp)", "scalar", None, over, S // 2),
             ("back_out on opacity is clamped to 1", "scalar", "opacity", over, S // 2)]
    under = anim_of(kf(0, v2(0.02, 0.05), "back_in"), kf(S, v2(1.0, 0.4)))
    rows += [("back_in undershoots below 0 (no property: no clamp)", "vec2", None, under, S // 2),
             ("back_in on scale: the overshoot below 0 is clamped per component",
              "vec2", "transform.scale", under, S // 2)]
    crop = anim_of(kf(0, 0.0, "back_in_out"), kf(S, 1.0))
    rows += [("back_in_out on crop.left below 0 clamps to 0", "scalar", "crop.left", crop, S // 5),
             ("back_in_out on crop.left above 1 clamps to 1", "scalar", "crop.left", crop, 4 * S // 5)]
    gain = anim_of(kf(0, -144.0, "back_in"), kf(S, 0.0))
    rows += [("audio_gain in dB, back_in below -144 clamps", "scalar", "audio_gain", gain, S // 5),
             ("audio_gain in dB, linear region", "scalar", "audio_gain",
              anim_of(kf(0, -12.0), kf(S, 6.0)), S // 3)]
    pan = anim_of(kf(0, -1.0, "spring_wobbly"), kf(S // 2, 1.0))
    rows += [("audio_pan with a wobbly spring overshoots and clamps to 1", "scalar", "audio_pan",
              pan, S // 4)]

    # Springs and bounce in context.
    rot = anim_of(kf(0, 0.0, "bounce"), kf(S, 90.0))
    for t in (S // 10, S // 4, S // 2, 9 * S // 10):
        rows.append(("rotation with the bounce preset stays below 90 (the spring settles in 1 s)", "angle",
                     "transform.rotation", rot, t))
    wob = anim_of(kf(0, v2(0.0, 0.0), "spring_wobbly"), kf(S, v2(1.0, 0.5)))
    for t in (S // 5, 2 * S // 5, 4 * S // 5):
        rows.append(("vec2 wobbly spring overshoots", "vec2", None, wob, t))

    # Angles.
    spin = anim_of(kf(0, 0.0), kf(S, 720.0))
    rows += [("rotation accumulates: 0 -> 720 is two turns", "angle", "transform.rotation", spin,
              S // 4)]
    for note, a, b in [("350 -> 10 goes +20", 350.0, 10.0), ("10 -> 350 goes -20", 10.0, 350.0),
                       ("0 -> 180 (exactly opposite) goes -180", 0.0, 180.0),
                       ("-720 -> 30 goes +30", -720.0, 30.0)]:
        rows.append((f"shortest-path angle mode: {note}", "angle_shortest", None,
                     anim_of(kf(0, a), kf(S, b)), S // 2))

    # Colors: linear light, premultiplied.
    rb = anim_of(kf(0, "#ff0000"), kf(S, "#0000ff"))
    rows += [("color red -> blue at half, in linear light", "color", None, rb, S // 2),
             ("color red -> blue at a quarter", "color", None, rb, S // 4)]
    fade = anim_of(kf(0, "#ffffff00"), kf(S, "#ff000080"))
    rows += [("color from transparent white: premultiplied, so no white fringe", "color", None,
              fade, S // 2)]
    over_c = anim_of(kf(0, "#000000", "back_out"), kf(S, "#ffffff"))
    rows += [("color overshoot clamps rgb to [0, a]", "color", None, over_c, S // 2)]

    # Discrete values hold.
    flip = anim_of(kf(0, False, "hold"), kf(S, True, "hold"), kf(2 * S, False, "hold"))
    for t in (-S, S - 1, S, 3 * S // 2, 2 * S, 3 * S):
        rows.append(("flip.horizontal holds", "bool", "flip.horizontal", flip, t))

    # Item-relative time: keys outside the item after a head trim still evaluate.
    trimmed = anim_of(kf(-2 * S, 0.0), kf(-S, 0.5), kf(S, 1.0))
    rows += [("keys before the item start (after a head trim) bracket t = 0", "scalar",
              "opacity", trimmed, 0)]

    # Precision at large times: u from integer flicks converted to f64.
    big = anim_of(kf(MAX_SAFE_FLICKS - 4 * S, 0.0), kf(MAX_SAFE_FLICKS, 8.0))
    rows += [("keys near 2^53 flicks: u stays exact", "scalar", None, big,
              MAX_SAFE_FLICKS - 2 * S + 7),
             ("span of 2^54 - 2 flicks (both extremes): u rounds once", "scalar", None,
              anim_of(kf(-MAX_SAFE_FLICKS, 0.0), kf(MAX_SAFE_FLICKS, 1.0)), 3)]

    # Sanitization: (v1 - v0) overflows to infinity; the result falls back to the start value.
    huge = anim_of(kf(0, -1.5e308), kf(S, 1.5e308))
    rows += [("v1 - v0 overflows: the non-finite result takes the start value", "scalar", None,
              huge, S // 2),
             ("an overflowing hold is exact (no arithmetic)", "scalar", None,
              anim_of(kf(0, -1.5e308, "hold"), kf(S, 1.5e308)), S // 2)]

    out = []
    for i, (note, vtype, prop, anim, t) in enumerate(rows, 1):
        validate(anim, vtype, PROPS[prop]["range"] if prop else None)
        expected = evaluate_prop(anim, t, prop) if prop else evaluate(anim, t, vtype)
        row = {"id": f"V{i:03d}", "note": note, "type": vtype, "prop": prop, "animatable": anim,
               "t": t, "expected": expected}
        if _is_exact(anim, t, vtype):
            row["exact"] = True
        out.append(row)
    return out


def _is_exact(anim, t: int, vtype: str) -> bool:
    """Rows whose result needs only IEEE basic operations: they must match bit for bit."""
    if vtype == "color":
        return False
    if not is_keyframed(anim):
        return True
    kfs = anim["keyframes"]
    times = [k["t"] for k in kfs]
    i = bisect.bisect_right(times, t) - 1
    if i < 0 or i == len(kfs) - 1 or times[i] == t or vtype in DISCRETE:
        return True
    return kfs[i]["interp"]["kind"] in ("linear", "hold")


def retime_rows() -> list[dict]:
    base = anim_of(kf(0, 0.0, "ease_in_out"), kf(S, 1.0, "spring"), kf(2 * S, 0.5, "hold"),
                   kf(3 * S, 0.25), kf(4 * S, 0.0))
    dur = 4 * S
    rows = []

    def add(note, op, anim, expected):
        rows.append({"note": note, "op": op, "animatable": anim, "expected": expected})

    for at in (S // 2, S, 5 * S // 2):
        left, right = split(base, at, dur)
        add(f"split at {at} flicks: each half keeps the keys inside it plus one on each side",
            {"kind": "split", "at": at, "duration": dur}, base, {"left": left, "right": right})
    left, right = split(0.5, S, dur)
    add("split of a static value: both halves keep it", {"kind": "split", "at": S, "duration": dur},
        0.5, {"left": left, "right": right})
    outside = anim_of(kf(-3 * S, 1.0), kf(-2 * S, 2.0), kf(-S, 3.0), kf(S, 4.0), kf(6 * S, 5.0),
                      kf(7 * S, 6.0))
    left, right = split(outside, 2 * S, dur)
    add("split drops keys beyond the bracketing ones", {"kind": "split", "at": 2 * S,
                                                        "duration": dur},
        outside, {"left": left, "right": right})
    for delta, prune in ((S // 2, False), (S // 2, True), (-S, False), (5 * S // 2, True)):
        add(f"head trim by {delta} flicks, prune={prune}: keys keep their timeline position",
            {"kind": "trim_start", "delta": delta, "duration": dur, "prune": prune}, base,
            trim_start(base, delta, dur, prune))
    for new_dur, prune in ((5 * S // 2, False), (5 * S // 2, True), (6 * S, True)):
        add(f"tail trim to {new_dur} flicks, prune={prune}",
            {"kind": "trim_end", "duration": dur, "new_duration": new_dur, "prune": prune}, base,
            trim_end(base, new_dur, prune))
    for i, row in enumerate(rows, 1):
        row["id"] = f"R{i:03d}"
    return [{"id": r["id"], **{k: r[k] for k in r if k != "id"}} for r in rows]


def ops_rows() -> list[dict]:
    rows = []
    eased = anim_of(kf(0, 0.0, "ease_in"), kf(2 * S, 1.0, "linear"))

    def add(note, prop, anim, op, fn):
        try:
            expected = {"result": fn()}
        except SpecError as e:
            expected = {"error": e.code}
        rows.append({"note": note, "prop": prop, "animatable": anim, "op": op, **expected})

    add("set on a static property: animated with one key", "opacity", 0.8,
        {"kind": "set", "t": S, "value": 0.2}, lambda: set_key(0.8, S, "opacity", 0.2))
    add("set without a value keys the current value", "opacity", eased,
        {"kind": "set", "t": S}, lambda: set_key(eased, S, "opacity"))
    add("set inside a segment inherits the previous key's interp", "opacity", eased,
        {"kind": "set", "t": S, "value": 0.9},
        lambda: set_key(eased, S, "opacity", 0.9))
    add("set before the first key gets the property default (linear)", "opacity", eased,
        {"kind": "set", "t": -S, "value": 0.9},
        lambda: set_key(eased, -S, "opacity", 0.9))
    add("set at an occupied time replaces the value and keeps the interp", "opacity", eased,
        {"kind": "set", "t": 0, "value": 0.3},
        lambda: set_key(eased, 0, "opacity", 0.3))
    add("set with an explicit easing", "opacity", eased,
        {"kind": "set", "t": 2 * S, "value": 0.5, "easing": "back_out"},
        lambda: set_key(eased, 2 * S, "opacity", 0.5, expand_easing("back_out")))
    add("set on a discrete property defaults to hold", "flip.horizontal", False,
        {"kind": "set", "t": 0, "value": True}, lambda: set_key(False, 0, "flip.horizontal", True))
    add("a discrete property refuses linear", "flip.horizontal", False,
        {"kind": "set", "t": 0, "value": True, "easing": "linear"},
        lambda: set_key(False, 0, "flip.horizontal", True, {"kind": "linear"}))
    add("a value outside the property range is refused", "opacity", eased,
        {"kind": "set", "t": S, "value": 1.5}, lambda: set_key(eased, S, "opacity", 1.5))
    add("delete one of two keys", "opacity", eased, {"kind": "delete", "ts": [0]},
        lambda: delete_keys(eased, [0]))
    single = anim_of(kf(S, 0.4))
    add("deleting the last key leaves the property static at its value", "opacity", single,
        {"kind": "delete", "ts": [S]}, lambda: delete_keys(single, [S]))
    add("deleting a missing key is NotFound", "opacity", eased, {"kind": "delete", "ts": [S]},
        lambda: delete_keys(eased, [S]))
    three = anim_of(kf(0, 0.0, "hold"), kf(S, 0.5, "ease_out"), kf(2 * S, 1.0))
    add("moving a key past its neighbour reorders; the key keeps its interp", "opacity", three,
        {"kind": "move", "ts": [S], "delta": 3 * S // 2},
        lambda: move_keys(three, [S], 3 * S // 2))
    add("moving onto another key is Invalid", "opacity", three,
        {"kind": "move", "ts": [S], "delta": S}, lambda: move_keys(three, [S], S))
    add("moving several keys together, past the others", "opacity", three,
        {"kind": "move", "ts": [0, S], "delta": 5 * S // 2},
        lambda: move_keys(three, [0, S], 5 * S // 2))
    # Times: `t` is item time, `at` is sequence time; both snap on the sequence's frame grid.
    add("at (sequence time) on a frame-aligned item at 30 fps: t = at - start", "opacity", 0.8,
        {"kind": "set", "at": 10 * S + 2 * FRAME_30, "start": 10 * S, "rate": [30, 1],
         "value": 0.5},
        lambda: set_key(0.8, key_time(10 * S, 30, 1, at=10 * S + 2 * FRAME_30), "opacity", 0.5))
    add("t between frames snaps to the nearest frame (29.97 fps)", "opacity", 0.8,
        {"kind": "set", "t": 50_000_000, "start": 0, "rate": [30000, 1001], "value": 0.5},
        lambda: set_key(0.8, key_time(0, 30000, 1001, t=50_000_000), "opacity", 0.5))
    add("an item that starts off the grid (a fine audio edit): snapping is in sequence time",
        "audio_gain", 0.0,
        {"kind": "set", "t": FRAME_30, "start": 14_700 * 3, "rate": [30, 1], "value": -6.0},
        lambda: set_key(0.0, key_time(14_700 * 3, 30, 1, t=FRAME_30), "audio_gain", -6.0))
    add("clear without a time keeps the first key's value", "opacity", three,
        {"kind": "clear"}, lambda: clear(three, "opacity"))
    add("clear at a time keeps the value there", "opacity", three,
        {"kind": "clear", "t": 3 * S // 2}, lambda: clear(three, "opacity", 3 * S // 2))
    for i, row in enumerate(rows, 1):
        row["id"] = f"O{i:03d}"
    return [{"id": r["id"], **{k: r[k] for k in r if k != "id"}} for r in rows]


def invalid_rows() -> list[dict]:
    cases = [
        ("an empty keyframe list", "scalar", None, {"keyframes": []}),
        ("two keys at the same time", "scalar", None, anim_of(kf(0, 0.0), kf(0, 1.0))),
        ("keys out of order", "scalar", None, anim_of(kf(S, 0.0), kf(0, 1.0))),
        ("a fractional time", "scalar", None, anim_of(kf(0.5, 0.0))),
        ("a time beyond 2^53 - 1", "scalar", None, anim_of(kf(MAX_SAFE_FLICKS + 1, 0.0))),
        ("opacity above 1 in a key", "scalar", "opacity", anim_of(kf(0, 1.5))),
        ("a vec2 with a missing component", "vec2", None, anim_of(kf(0, {"x": 1.0}))),
        ("bezier x1 above 1", "scalar", None, anim_of(kf(0, 0.0, _bez(1.5, 0.0, 0.5, 1.0)))),
        ("bezier y2 beyond the limit", "scalar", None,
         anim_of(kf(0, 0.0, _bez(0.5, 0.0, 0.5, 11.0)))),
        ("spring mass 0", "scalar", None, anim_of(kf(0, 0.0, _spring(100.0, 10.0, 0.0, False)))),
        ("spring without bounce", "scalar", None,
         anim_of(kf(0, 0.0, {"kind": "spring", "stiffness": 100.0, "damping": 10.0, "mass": 1.0}))),
        ("an unknown interpolation kind", "scalar", None, anim_of(kf(0, 0.0, {"kind": "cubic"}))),
        ("a boolean with linear interpolation", "bool", "flip.vertical",
         anim_of(kf(0, False, {"kind": "linear"}))),
        ("a preset hint that is not a name", "scalar", None,
         anim_of(kf(0, 0.0, {**_bez(0.42, 0.0, 1.0, 1.0), "preset": "Ease In!"}))),
    ]
    rows = []
    for i, (note, vtype, prop, anim) in enumerate(cases, 1):
        try:
            validate(anim, vtype, PROPS[prop]["range"] if prop else None)
        except SpecError as e:
            code = e.code
        else:
            raise AssertionError(f"invalid case {note!r} was accepted")
        rows.append({"id": f"X{i:03d}", "note": note, "type": vtype, "prop": prop,
                     "animatable": anim, "error": code})
    return rows


def generate() -> dict:
    return {
        "format": "cutweave-keyframe-vectors",
        "version": 1,
        "spec": "docs/spec/keyframes.md",
        "generated_by": "docs/spec/keyframes_ref.py",
        "tolerance": 1e-6,
        "flicks_per_second": FLICKS_PER_SECOND,
        "presets": PRESETS,
        "easing": easing_rows(),
        "eval": eval_rows(),
        "retime": retime_rows(),
        "ops": ops_rows(),
        "invalid": invalid_rows(),
    }


def render(doc: dict) -> str:
    """Stable, diffable JSON: one row per line inside each section."""
    def dump(v) -> str:
        return json.dumps(v, ensure_ascii=False, allow_nan=False, separators=(", ", ": "))

    lines = ["{"]
    items = list(doc.items())
    for n, (key, value) in enumerate(items):
        comma = "," if n < len(items) - 1 else ""
        if isinstance(value, list):
            lines.append(f"  {json.dumps(key)}: [")
            for m, row in enumerate(value):
                lines.append(f"    {dump(row)}{',' if m < len(value) - 1 else ''}")
            lines.append(f"  ]{comma}")
        elif isinstance(value, dict):
            lines.append(f"  {json.dumps(key)}: {{")
            sub = list(value.items())
            for m, (k, v) in enumerate(sub):
                lines.append(f"    {json.dumps(k)}: {dump(v)}{',' if m < len(sub) - 1 else ''}")
            lines.append(f"  }}{comma}")
        else:
            lines.append(f"  {json.dumps(key)}: {dump(value)}{comma}")
    lines.append("}")
    return "\n".join(lines) + "\n"


# --- Self-tests: the spec's claims on random documents -------------------------------------------


def _random_anim(rng: random.Random, vtype: str) -> dict:
    n = rng.randint(1, 8)
    times = sorted(rng.sample(range(-3 * S, 6 * S, FRAME_30), n))
    kinds = ["hold", "linear", *PRESETS.keys()]
    keys = []
    for t in times:
        name = "hold" if vtype in DISCRETE else rng.choice(kinds)
        if vtype == "vec2":
            value = v2(rng.uniform(-2, 2), rng.uniform(-2, 2))
        elif vtype == "bool":
            value = rng.random() < 0.5
        elif vtype == "color":
            value = "#" + "".join(f"{rng.randrange(256):02x}" for _ in range(4))
        else:
            value = rng.uniform(-100, 100)
        keys.append(kf(t, value, name))
    return anim_of(*keys)


def _close(a, b, tol: float) -> bool:
    """Absolute closeness, per component."""
    if isinstance(a, dict):
        return all(_close(a[k], b[k], tol) for k in a)
    if isinstance(a, bool):
        return a == b
    return abs(a - b) <= tol


def self_test() -> list[str]:
    rng = random.Random(20261125)
    failures = []
    for case in range(400):
        vtype = rng.choice(["scalar", "vec2", "angle", "color", "bool"])
        anim = _random_anim(rng, vtype)
        validate(anim, vtype)
        dur = rng.randrange(1, 8) * S
        at = rng.randrange(1, dur)
        left, right = split(anim, at, dur)
        probes = [rng.randrange(0, dur) for _ in range(20)] + [0, at - 1, at, dur - 1]
        for t in probes:
            want = evaluate(anim, t, vtype)
            got = evaluate(left, t, vtype) if t < at else evaluate(right, t - at, vtype)
            if got != want:  # bit-identical, not merely close
                failures.append(f"split identity broke: case {case} t={t}")
        # Head and tail trims keep evaluation at every timeline position inside the item.
        delta = rng.randrange(-dur, dur)
        if delta < dur:
            for prune in (False, True):
                trimmed = trim_start(anim, delta, dur, prune)
                for t in probes:
                    if delta <= t < dur and evaluate(trimmed, t - delta, vtype) != evaluate(
                            anim, t, vtype):
                        failures.append(f"head trim moved a value: case {case}")
        # No NaN or infinity, and continuity into every key except after a hold.
        kfs = anim["keyframes"]
        for t in probes:
            v = evaluate(anim, t, vtype)
            values = v.values() if isinstance(v, dict) else [v]
            if any(not isinstance(x, bool) and not math.isfinite(x) for x in values):
                failures.append(f"non-finite value: case {case} t={t}")
        for a, b in zip(kfs, kfs[1:]):
            if vtype in DISCRETE or a["interp"]["kind"] == "hold":
                mid = (a["t"] + b["t"]) // 2
                if evaluate(anim, mid, vtype) != to_eval(a["value"], vtype):
                    failures.append(f"hold produced an intermediate value: case {case}")
                continue
            # One flick before the key, within 1e-6 of the segment's value range.
            near = evaluate(anim, b["t"] - 1, vtype)
            v0, v1 = to_eval(a["value"], vtype), to_eval(b["value"], vtype)
            spread = (max(abs(v1[c] - v0[c]) for c in v0) if isinstance(v0, dict)
                      else abs(v1 - v0))
            if not _close(near, v1, 1e-6 * max(1.0, spread)):
                failures.append(f"discontinuity before key t={b['t']}: case {case} {a['interp']}")
            if a["interp"]["kind"] == "linear" and vtype in ("scalar", "angle"):
                samples = [evaluate(anim, a["t"] + (b["t"] - a["t"]) * j // 64, vtype)
                           for j in range(65)]
                ordered = sorted(samples) if b["value"] >= a["value"] else sorted(samples,
                                                                                    reverse=True)
                if samples != ordered:
                    failures.append(f"linear segment not monotone: case {case}")
    for name, interp in PRESETS.items():
        validate(anim_of(kf(0, 0.0, expand_easing(name))), "scalar")
    return failures


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--write", action="store_true", help="rewrite keyframes-vectors.json")
    args = ap.parse_args()
    text = render(generate())
    failures = self_test()
    for f in failures[:20]:
        print(f"self-test: {f}", file=sys.stderr)
    if failures:
        print(f"{len(failures)} self-test failure(s)", file=sys.stderr)
        return 1
    if args.write:
        VECTORS.write_text(text, encoding="utf-8", newline="\n")
        print(f"wrote {VECTORS.relative_to(HERE.parent.parent)}")
        return 0
    current = VECTORS.read_text(encoding="utf-8") if VECTORS.exists() else ""
    if current != text:
        print("docs/spec/keyframes-vectors.json is stale: run "
              "`python docs/spec/keyframes_ref.py --write` and review the diff", file=sys.stderr)
        return 1
    doc = json.loads(current)
    counts = {k: len(v) for k, v in doc.items() if isinstance(v, list)}
    print(f"keyframe vectors up to date ({counts}); self-tests passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
