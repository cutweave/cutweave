// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2C.06: the module's exports, run natively, write the parity fixture that the wasm build is
//! checked against in Node (`packages/protocol/src/wasm/math.test.ts`): the same calls must give
//! the same bits there.
//!
//! Inputs: the keyframe reference vectors (`docs/spec/keyframes-vectors.json`), the time-input
//! fixture (`crates/cutweave-model/tests/fixtures/time-input.json`) and a matrix of frame
//! conversions and timecode labels at every common rate, odd and invalid rates, and the edges of
//! the safe time range. Every result is also checked here against the model called directly, so
//! the fixture records the model's own answers.
//!
//! `CUTWEAVE_BLESS=1 cargo test -p cutweave-wasm --test parity` rewrites the fixture after an
//! intended change.

// Exact comparisons of floats are the point of a parity test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]

use std::{fs, path::PathBuf};

use cutweave_model::{
    AnimValue, Animatable, Color, FrameRate, Interp, MAX_SAFE_FLICKS, PropId, ShortestAngle, Time,
    Vec2,
};
use cutweave_wasm as m;
use serde_json::{Value, json};

const FPS: i64 = 705_600_000;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read_json(rel: &str) -> Value {
    let path = root().join(rel);
    let text =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&text).expect("JSON")
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/parity.json")
}

/// The output buffer after a call that returned a length (or failed).
fn text(len: i32) -> Value {
    if len < 0 {
        json!({ "error": m::output() })
    } else {
        let out = m::output();
        assert_eq!(out.len(), usize::try_from(len).unwrap());
        json!(out)
    }
}

/// A time or frame as a decimal string (i64 does not fit a JSON number in every reader).
fn int(v: i64) -> Value {
    if v == m::ERR_TIME {
        json!({ "error": m::output() })
    } else {
        json!(v.to_string())
    }
}

/// The number buffer after a call that returned a count (or failed).
fn numbers(count: i32) -> Value {
    if count < 0 {
        return json!({ "error": m::output() });
    }
    let values = m::numbers();
    assert_eq!(values.len(), usize::try_from(count).unwrap());
    json!(values)
}

const ROUNDINGS: [(&str, u32); 2] = [("floor", 0), ("nearest", 1)];

/// Common rates and odd ones (inexact frame lengths, the fastest and slowest allowed), as
/// `(num, den)`.
fn rates() -> Vec<(u32, u32)> {
    let mut rates: Vec<(u32, u32)> = FrameRate::COMMON
        .iter()
        .map(|r| (r.num(), r.den()))
        .collect();
    rates.extend([(7, 3), (1000, 1), (1, 60), (352_800_000, 1), (48_000, 2002)]);
    rates
}

/// Rates [`FrameRate::new`] refuses.
const INVALID_RATES: [(u32, u32); 3] = [(0, 1), (1, 0), (352_800_001, 1)];

/// Times outside the safe range.
const BAD_TIMES: [i64; 2] = [MAX_SAFE_FLICKS + 1, i64::MIN];

/// A small deterministic generator (no RNG crate: the fixture must not change between runs).
fn lcg(seed: &mut u64) -> i64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    i64::try_from(*seed >> 11).unwrap()
}

fn times() -> Vec<i64> {
    let mut ts = vec![
        0,
        1,
        -1,
        11_759_999,
        11_760_000,
        11_760_001,
        23_520_000,
        29_429_400,
        352_800_000,
        FPS,
        -FPS,
        FPS * 3600,
        MAX_SAFE_FLICKS,
        -MAX_SAFE_FLICKS,
    ];
    let mut seed = 0x2C06;
    for _ in 0..10 {
        // Within ±10 hours.
        ts.push(lcg(&mut seed) % (FPS * 36_000) - FPS * 18_000);
    }
    ts
}

fn frames() -> Vec<i64> {
    vec![0, 1, -1, 29, 30, 1799, 1800, 107_892, -86_400, 10_368_000]
}

/// Frames whose start is outside the safe range at every rate.
const BAD_FRAMES: [i64; 3] = [MAX_SAFE_FLICKS, i64::MAX, i64::MIN];

/// Frame conversions at every rate: `frame_to_time` over [`frames`], `time_to_frame` and
/// `snap_to_frame` over [`times`] with both roundings.
fn conversions() -> Value {
    let mut out = Vec::new();
    for (num, den) in rates() {
        let rate = FrameRate::new(num, den).ok();
        let mut row = json!({ "rate": { "num": num, "den": den } });
        let frames: Vec<Value> = frames()
            .into_iter()
            .map(|f| {
                let got = m::cw_frame_to_time(f, num, den);
                let want = rate.and_then(|r| r.frame_to_time(f).ok());
                assert_eq!(want.map(Time::flicks).unwrap_or(m::ERR_TIME), got);
                json!([f.to_string(), int(got)])
            })
            .collect();
        row["frame_to_time"] = json!(frames);
        for (name, code) in ROUNDINGS {
            let mut to_frame = Vec::new();
            let mut snap = Vec::new();
            for t in times() {
                let (got_frame, got_snap) = (
                    m::cw_time_to_frame(t, num, den, code),
                    m::cw_snap_to_frame(t, num, den, code),
                );
                let (frame, snapped) = (int(got_frame), int(got_snap));
                to_frame.push(json!([t.to_string(), frame]));
                snap.push(json!([t.to_string(), snapped]));
            }
            row[format!("time_to_frame_{name}")] = json!(to_frame);
            row[format!("snap_to_frame_{name}")] = json!(snap);
        }
        out.push(row);
    }
    json!(out)
}

/// Calls that fail: invalid rates, times and frames outside the safe range, drop-frame timecode
/// at a rate without it, as `[call, input, num, den, result]`.
fn conversion_errors() -> Value {
    let mut out = Vec::new();
    let mut push = |call: &str, input: i64, (num, den): (u32, u32), result: Value| {
        assert!(result.get("error").is_some(), "{call} {input} {num}/{den}");
        out.push(json!([call, input.to_string(), num, den, result]));
    };
    for rate in INVALID_RATES {
        let (num, den) = rate;
        push(
            "frame_to_time",
            1,
            rate,
            int(m::cw_frame_to_time(1, num, den)),
        );
        push(
            "time_to_frame",
            1,
            rate,
            int(m::cw_time_to_frame(1, num, den, 0)),
        );
        push(
            "snap_to_frame",
            1,
            rate,
            int(m::cw_snap_to_frame(1, num, den, 1)),
        );
        push("timecode", 1, rate, text(m::cw_timecode(1, num, den, 0, 0)));
    }
    for t in BAD_TIMES {
        push(
            "time_to_frame",
            t,
            (30, 1),
            int(m::cw_time_to_frame(t, 30, 1, 0)),
        );
        push(
            "snap_to_frame",
            t,
            (30, 1),
            int(m::cw_snap_to_frame(t, 30, 1, 1)),
        );
        push("timecode", t, (30, 1), text(m::cw_timecode(t, 30, 1, 0, 0)));
    }
    for f in BAD_FRAMES {
        push(
            "frame_to_time",
            f,
            (24, 1),
            int(m::cw_frame_to_time(f, 24, 1)),
        );
    }
    // A label of a negative time; drop-frame at 30 fps.
    push(
        "timecode",
        -1,
        (30, 1),
        text(m::cw_timecode(-1, 30, 1, 0, 0)),
    );
    push(
        "timecode_drop",
        0,
        (30, 1),
        text(m::cw_timecode(0, 30, 1, 1, 0)),
    );
    // A rounding code that does not exist (the TypeScript API cannot send one).
    assert_eq!(m::cw_time_to_frame(0, 30, 1, 2), m::ERR_TIME);
    json!(out)
}

/// Timecode labels: non-negative times at every rate with both roundings, and drop-frame at the
/// rates that have it.
fn timecodes() -> Value {
    let mut out = Vec::new();
    let drop_rates = [(30_000, 1001), (60_000, 1001)];
    let rows = rates()
        .into_iter()
        .map(|r| (r, false))
        .chain(drop_rates.into_iter().map(|r| (r, true)));
    for ((num, den), drop) in rows {
        {
            for (name, code) in ROUNDINGS {
                let labels: Vec<Value> = times()
                    .into_iter()
                    .filter(|t| *t >= 0)
                    .chain([FPS * 60, FPS * 600 + 1, FPS * 86_399])
                    .map(|t| {
                        json!([
                            t.to_string(),
                            text(m::cw_timecode(t, num, den, u32::from(drop), code))
                        ])
                    })
                    .collect();
                out.push(json!({
                    "rate": { "num": num, "den": den },
                    "drop_frame": drop,
                    "rounding": name,
                    "labels": labels,
                }));
            }
        }
    }
    json!(out)
}

/// `Time`'s text: canonical and approximate.
fn formats() -> Value {
    let mut out = Vec::new();
    for t in times()
        .into_iter()
        .chain([44_100_000_000, 29_400_000, 5, MAX_SAFE_FLICKS + 1])
    {
        for precision in [-1, 0, 3, 9, 18, 19] {
            out.push(json!([
                t.to_string(),
                precision,
                text(m::cw_time_format(t, precision))
            ]));
        }
    }
    json!(out)
}

/// `TimeInput` resolution over the model's time-input fixture: must agree with it exactly. The
/// successes are the fixture's own `flicks`; this records the messages of the refusals without a
/// rate and at one rate (`parse_errors`).
fn parses() -> Value {
    let doc = read_json("crates/cutweave-model/tests/fixtures/time-input.json");
    let mut out = Vec::new();
    let extra = [
        json!({ "input": "é", "rate": null }),
        json!({ "input": "1:2", "rate": { "num": 0, "den": 1 } }),
    ];
    let cases = doc["cases"].as_array().unwrap().iter().chain(extra.iter());
    for case in cases {
        let input = case["input"].as_str().unwrap();
        let (num, den) = match &case["rate"] {
            Value::Null => (0, 0),
            r => (
                u32::try_from(r["num"].as_u64().unwrap()).unwrap(),
                u32::try_from(r["den"].as_u64().unwrap()).unwrap(),
            ),
        };
        let [len] = m::write_input([input]);
        let got = m::cw_time_parse(len, num, den);
        if let Some(want) = case.get("flicks") {
            let want = want.as_i64().unwrap_or(m::ERR_TIME);
            assert_eq!(got, want, "{input:?} at {num}/{den}");
        }
        if got == m::ERR_TIME && (num == 0 || (num, den) == (24_000, 1001)) {
            out.push(json!([input, { "num": num, "den": den }, int(got)]));
        }
    }
    json!(out)
}

fn sample_time_rows() -> Value {
    let mut out = Vec::new();
    for (t0, t1, n) in [
        (0, FPS, 5),
        (0, 10, 4),
        (5, -5, 3),
        (7, 9, 1),
        (7, 9, 0),
        (-MAX_SAFE_FLICKS, MAX_SAFE_FLICKS, 3),
        (0, MAX_SAFE_FLICKS + 1, 2),
        (0, 1, m::MAX_SAMPLES + 1),
    ] {
        out.push(json!([
            t0.to_string(),
            t1.to_string(),
            n,
            numbers(m::cw_sample_times(t0, t1, n))
        ]));
    }
    json!(out)
}

fn progress(interp: &Value, dt: i64, span: i64) -> f64 {
    let text = interp.to_string();
    let [len] = m::write_input([text.as_str()]);
    m::cw_easing_progress(len, dt, span)
}

/// Easing progress of every `easing` vector, and whole curves of every named easing.
fn easings(vectors: &Value) -> (Value, Value) {
    let mut rows = Vec::new();
    for row in vectors["easing"].as_array().unwrap() {
        let (dt, span) = (row["dt"].as_i64().unwrap(), row["span"].as_i64().unwrap());
        let got = progress(&row["interp"], dt, span);
        let want = Interp::from_json(&row["interp"])
            .unwrap()
            .progress(dt, span);
        assert_eq!(got.to_bits(), want.to_bits(), "{}", row["id"]);
        rows.push(json!([row["id"], got]));
    }
    let mut curves = Vec::new();
    let presets: Value = {
        let len = m::cw_easing_presets();
        serde_json::from_str(&m::output()[..usize::try_from(len).unwrap()]).unwrap()
    };
    for preset in presets.as_array().unwrap() {
        for span in [FPS / 2, FPS * 2] {
            let text = preset["interp"].to_string();
            let [len] = m::write_input([text.as_str()]);
            let values = numbers(m::cw_easing_sample(len, span, 17));
            curves.push(json!([preset["name"], span.to_string(), 17, values]));
        }
    }
    // Invalid interpolations fail with the model's message.
    for bad in [
        json!({"kind": "bezier", "x1": 2.0, "y1": 0.0, "x2": 1.0, "y2": 1.0}),
        json!({"kind": "wobble"}),
        json!("linear"),
    ] {
        let got = progress(&bad, 1, 2);
        assert!(got.is_nan());
        curves.push(json!([bad, "1", 2, { "error": m::output() }]));
    }
    (json!(rows), json!(curves))
}

/// One evaluation through the module, checked bit for bit against the model called directly.
fn eval_native<T: AnimValue>(row: &Value, t: Time) -> Vec<f64>
where
    T::Eval: Into<Components>,
{
    let anim = Animatable::<T>::from_json(&row["animatable"]).unwrap();
    let direct: Components = match row["prop"].as_str() {
        Some(prop) => anim.eval_clamped(t, prop.parse::<PropId>().unwrap().spec().clamp),
        None => anim.eval(t),
    }
    .into();
    let handle = compile(row);
    assert_ne!(handle, 0, "{}: {}", row["id"], m::output());
    let count = m::cw_anim_eval(handle, t.flicks());
    let got = m::numbers();
    assert_eq!(usize::try_from(count).unwrap(), got.len());
    assert_eq!(m::cw_anim_free(handle), 1);
    let bits = |v: &[f64]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&got), bits(&direct.0), "{}", row["id"]);
    got
}

/// A flat list of an evaluated value's components, as the number buffer lays them out.
struct Components(Vec<f64>);

impl From<f64> for Components {
    fn from(v: f64) -> Components {
        Components(vec![v])
    }
}

impl From<bool> for Components {
    fn from(v: bool) -> Components {
        Components(vec![if v { 1.0 } else { 0.0 }])
    }
}

impl From<Vec2> for Components {
    fn from(v: Vec2) -> Components {
        Components(vec![v.x, v.y])
    }
}

impl From<cutweave_model::LinearRgba> for Components {
    fn from(v: cutweave_model::LinearRgba) -> Components {
        Components(vec![v.r, v.g, v.b, v.a])
    }
}

/// Compiles a vector row's animatable by its value type, with its property when it names one.
fn compile(row: &Value) -> u32 {
    let kind = row["type"].as_str().unwrap();
    let prop = row["prop"].as_str().unwrap_or("");
    let json = row["animatable"].to_string();
    let [a, b, c] = m::write_input([kind, prop, json.as_str()]);
    m::cw_anim_new(a, b, c)
}

/// Every `eval` vector, and sampled ranges of every distinct keyed one.
fn evals(vectors: &Value) -> (Value, Value) {
    let mut rows = Vec::new();
    let mut ranges = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for row in vectors["eval"].as_array().unwrap() {
        let t = Time::from_flicks(row["t"].as_i64().unwrap()).unwrap();
        let got = match row["type"].as_str().unwrap() {
            "scalar" | "angle" => eval_native::<f64>(row, t),
            "angle_shortest" => eval_native::<ShortestAngle>(row, t),
            "vec2" => eval_native::<Vec2>(row, t),
            "bool" => eval_native::<bool>(row, t),
            "color" => eval_native::<Color>(row, t),
            other => panic!("unknown type {other}"),
        };
        rows.push(json!([row["id"], got]));
        let Some(keys) = row["animatable"]["keyframes"].as_array() else {
            continue;
        };
        if !seen.insert(format!(
            "{}{}{}",
            row["type"], row["prop"], row["animatable"]
        )) {
            continue;
        }
        let first = keys[0]["t"].as_i64().unwrap();
        let last = keys[keys.len() - 1]["t"].as_i64().unwrap();
        let pad = ((last - first) / 8).max(1);
        let (t0, t1) = (
            (first - pad).max(-MAX_SAFE_FLICKS),
            (last + pad).min(MAX_SAFE_FLICKS),
        );
        let handle = compile(row);
        let values = numbers(m::cw_anim_eval_range(handle, t0, t1, 24));
        assert_eq!(m::cw_anim_free(handle), 1);
        ranges.push(json!([
            row["id"],
            t0.to_string(),
            t1.to_string(),
            24,
            values
        ]));
    }
    (json!(rows), json!(ranges))
}

/// Every `invalid` vector fails to compile with the model's message.
fn invalids(vectors: &Value) -> Value {
    let mut out = Vec::new();
    for row in vectors["invalid"].as_array().unwrap() {
        let handle = compile(row);
        assert_eq!(handle, 0, "{}", row["id"]);
        let message = m::output();
        let code = row["error"].as_str().unwrap();
        assert!(message.starts_with(code), "{}: {message}", row["id"]);
        out.push(json!([row["id"], message]));
    }
    // Errors of the compile call itself.
    for (kind, prop, json) in [
        ("", "", "0"),
        ("vector3", "", "0"),
        ("scalar", "opacity_x", "0"),
        ("vec2", "opacity", "0"),
        ("scalar", "", "{"),
        ("bool", "flip.horizontal", "1.5"),
    ] {
        let [a, b, c] = m::write_input([kind, prop, json]);
        assert_eq!(m::cw_anim_new(a, b, c), 0, "{kind} {prop} {json}");
        out.push(json!([format!("{kind}|{prop}|{json}"), m::output()]));
    }
    json!(out)
}

fn generate() -> Value {
    let vectors = read_json("docs/spec/keyframes-vectors.json");
    let (easing, easing_sample) = easings(&vectors);
    let (eval, eval_range) = evals(&vectors);
    let version_len = m::cw_version();
    let version = m::output();
    let presets_len = m::cw_easing_presets();
    let presets: Value = serde_json::from_str(&m::output()).unwrap();
    assert_eq!(usize::try_from(presets_len).unwrap(), m::output().len());
    let invalid = invalids(&vectors);
    assert_eq!(m::cw_anim_live(), 0, "every test handle was freed");
    json!({
        "comment": "Generated by crates/cutweave-wasm/tests/parity.rs from the module's exports run natively; do not edit (CUTWEAVE_BLESS=1 cargo test -p cutweave-wasm --test parity). Times and frames are decimal strings; a failed call is {\"error\": message}.",
        "version": { "len": version_len, "text": version },
        "conversions": conversions(),
        "conversion_errors": conversion_errors(),
        "timecode": timecodes(),
        "format": formats(),
        "parse_errors": parses(),
        "sample_times": sample_time_rows(),
        "presets": presets,
        "easing": easing,
        "easing_sample": easing_sample,
        "eval": eval,
        "eval_range": eval_range,
        "invalid": invalid,
    })
}

/// One line per row, so the file reviews as a table and diffs per case.
fn render(doc: &Value) -> String {
    let mut out = String::from("{\n");
    let fields = doc.as_object().unwrap();
    for (i, (key, value)) in fields.iter().enumerate() {
        out.push_str(&format!("  {}: ", serde_json::to_string(key).unwrap()));
        match value {
            Value::Array(rows) if !rows.is_empty() => {
                out.push_str("[\n");
                for (j, row) in rows.iter().enumerate() {
                    out.push_str("    ");
                    out.push_str(&row.to_string());
                    out.push_str(if j + 1 < rows.len() { ",\n" } else { "\n" });
                }
                out.push_str("  ]");
            }
            other => out.push_str(&other.to_string()),
        }
        out.push_str(if i + 1 < fields.len() { ",\n" } else { "\n" });
    }
    out.push_str("}\n");
    out
}

#[test]
fn parity_fixture_matches_the_native_exports() {
    let text = render(&generate());
    let path = fixture_path();
    if std::env::var_os("CUTWEAVE_BLESS").is_some() {
        fs::write(&path, &text).unwrap();
        return;
    }
    let on_disk = fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        on_disk == text,
        "{} is out of date: run `CUTWEAVE_BLESS=1 cargo test -p cutweave-wasm --test parity`",
        path.display()
    );
}

#[test]
fn handles_are_reused_and_bad_handles_fail() {
    let [a, b, c] = m::write_input(["scalar", "", "0.5"]);
    let first = m::cw_anim_new(a, b, c);
    let [a, b, c] = m::write_input(["scalar", "", "0.25"]);
    let second = m::cw_anim_new(a, b, c);
    assert!(first != 0 && second != 0 && first != second);
    assert_eq!(m::cw_anim_type(first), 6);
    assert_eq!(m::output(), "scalar");
    assert_eq!(m::cw_anim_free(first), 1);
    assert_eq!(m::cw_anim_free(first), 0, "freed twice");
    assert_eq!(m::cw_anim_eval(first, 0), -1);
    assert_eq!(m::output(), format!("no animatable with handle {first}"));
    assert_eq!(m::cw_anim_eval(0, 0), -1);
    let [a, b, c] = m::write_input(["", "opacity", "1"]);
    let third = m::cw_anim_new(a, b, c);
    assert_eq!(third, first, "the freed slot is reused");
    assert_eq!(m::cw_anim_type(third), 6);
    assert_eq!(m::output(), "scalar", "the property's type");
    assert_eq!(m::cw_anim_eval(second, MAX_SAFE_FLICKS + 1), -1);
    assert!(m::output().contains("safe range"), "{}", m::output());
    assert_eq!(m::cw_anim_free(second), 1);
    assert_eq!(m::cw_anim_free(third), 1);
    assert_eq!(m::cw_anim_live(), 0);
}

#[test]
fn input_longer_than_the_buffer_fails() {
    let _ = m::write_input(["ab"]);
    assert_eq!(m::cw_time_parse(3, 0, 0), m::ERR_TIME);
    assert!(m::output().contains("input buffer"), "{}", m::output());
    let [len] = m::write_input(["62.5s"]);
    assert_eq!(m::cw_time_parse(len, 0, 0), 62 * FPS + FPS / 2);
}
