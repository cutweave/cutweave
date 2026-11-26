// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-wasm`: the UI's shared math module (WP-2C.06, arch §16). The editor and the timeline
//! convert times, evaluate keyframes and sample easings with exactly the engine's code, compiled
//! to `wasm32-unknown-unknown`, without an RPC per curve point.
//!
//! This crate holds no math of its own: every result comes from `cutweave-model` (time and
//! timecode, arch §4.1; keyframes and easing, `docs/spec/keyframes.md`). It only marshals values
//! across a small C ABI that needs no `wasm-bindgen`: `just wasm` builds it with plain
//! `cargo build --target wasm32-unknown-unknown --profile wasm`, and the hand-written loader in
//! `@cutweave/protocol/wasm` wraps it.
//!
//! # The ABI
//!
//! - **Numbers** cross as themselves. `i64` (flicks, frames) is a `BigInt` in JavaScript, `u32`,
//!   `i32` and `f64` are numbers. Rates are `(num, den)`; rounding is `0` (floor) or `1`
//!   (nearest).
//! - **Text in.** The caller asks [`cw_in`] for an input buffer of `n` bytes, writes UTF-8 there
//!   (several strings one after another), and passes each string's byte length to the call.
//! - **Text out.** A call that returns a length leaves the text in the output buffer
//!   ([`cw_out_ptr`], [`cw_out_len`]).
//! - **Numbers out in bulk.** A call that returns a count leaves that many `f64` in the number
//!   buffer ([`cw_f64_ptr`]), e.g. `n` interleaved samples of an animatable.
//! - **Errors.** A failed call returns [`ERR_TIME`] (`i64::MIN`, never a valid time or frame),
//!   `-1` (lengths, counts), `0` (handles) or NaN (easing progress), and leaves the message in the
//!   output buffer.
//! - **Animatables** are compiled once ([`cw_anim_new`] parses the keyframes §3.1 wire form) and
//!   evaluated by handle, many times, then freed ([`cw_anim_free`]).
//!
//! The pointers these functions return stay valid until the next call (any call may grow the
//! linear memory). Everything is single-threaded: the buffers and handles are per thread.
//!
//! - **Bulk integers in.** [`cw_in_i64`] hands out an aligned `i64` buffer (the byte buffer of
//!   [`cw_in`] is not 8-aligned), for tens of thousands of snap candidates without JSON.
//! - **Snap indexes** ([`cw_snap_index_new`]) hold the candidates of a sequence by handle;
//!   [`cw_snap`] snaps a time against one and leaves the target in the number buffer.

use std::{cell::RefCell, fmt::Display};

use cutweave_model::{
    AnimValue, Animatable, Color, Easing, Evaluator, FrameRate, Interp, LinearRgba, PropId,
    PropSpec, Rounding, ShortestAngle, SnapIndex, SnapKind, SnapKinds, SnapOptions, Time,
    TimeInput, Timecode, ValueRange, ValueType, Vec2, sample_times,
};
use serde_json::Value;

/// What a failed call that returns a time or a frame returns: never a valid value, since times
/// and frames stay within ±(2⁵³ − 1).
pub const ERR_TIME: i64 = i64::MIN;

/// Most samples one bulk call returns, so a wrong count fails instead of exhausting memory.
pub const MAX_SAMPLES: u32 = 1 << 20;

/// Most decimals of an approximate seconds string.
const MAX_PRECISION: i32 = 18;

/// The value types in the order of the model's [`ValueType`] (looked up by name).
const VALUE_TYPES: [ValueType; 6] = [
    ValueType::Scalar,
    ValueType::Vec2,
    ValueType::Angle,
    ValueType::ShortestAngle,
    ValueType::Color,
    ValueType::Bool,
];

thread_local! {
    static INPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static OUTPUT: RefCell<String> = const { RefCell::new(String::new()) };
    static NUMBERS: RefCell<Vec<f64>> = const { RefCell::new(Vec::new()) };
    static ANIMS: RefCell<Vec<Option<Entry>>> = const { RefCell::new(Vec::new()) };
    static INPUT_I64: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };
    static SNAPS: RefCell<Vec<Option<SnapIndex>>> = const { RefCell::new(Vec::new()) };
}

/// Defines one export of the module: a `cw_`-prefixed `extern "C"` function over numbers.
macro_rules! export {
    ($(#[$meta:meta])* fn $name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty $body:block) => {
        $(#[$meta])*
        // SAFETY: `no_mangle` is unsafe because two items exporting one symbol name would be
        // undefined behaviour at link time. Every export is `cw_`-prefixed and defined once, here.
        #[allow(unsafe_code)]
        #[unsafe(no_mangle)]
        #[must_use]
        pub extern "C" fn $name($($arg: $ty),*) -> $ret $body
    };
}

/// A failed call's message.
struct Fail(String);

impl<E: Display> From<E> for Fail {
    fn from(e: E) -> Fail {
        Fail(e.to_string())
    }
}

type Res<T> = Result<T, Fail>;

fn set_output(text: &str) -> usize {
    OUTPUT.with_borrow_mut(|out| {
        out.clear();
        out.push_str(text);
        out.len()
    })
}

fn length(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

fn size(n: u32) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// A time or frame result, or [`ERR_TIME`] with the message in the output buffer.
fn int_reply(result: Res<i64>) -> i64 {
    result.unwrap_or_else(|Fail(message)| {
        set_output(&message);
        ERR_TIME
    })
}

/// Text in the output buffer and its length, or -1 with the message there instead.
fn text_reply(result: Res<String>) -> i32 {
    match result {
        Ok(text) => length(set_output(&text)),
        Err(Fail(message)) => {
            set_output(&message);
            -1
        }
    }
}

/// The count of numbers left in the number buffer, or -1 with the message in the output buffer.
fn count_reply(result: Res<usize>) -> i32 {
    match result {
        Ok(n) => length(n),
        Err(Fail(message)) => {
            set_output(&message);
            -1
        }
    }
}

/// Replaces the number buffer with what `fill` pushes and returns the count.
fn fill_numbers(fill: impl FnOnce(&mut Vec<f64>)) -> usize {
    NUMBERS.with_borrow_mut(|numbers| {
        numbers.clear();
        fill(numbers);
        numbers.len()
    })
}

/// The strings the caller wrote one after another into the input buffer, by byte length.
fn inputs<const N: usize>(lens: [u32; N]) -> Res<[String; N]> {
    INPUT.with_borrow(|buf| {
        let mut out: [String; N] = std::array::from_fn(|_| String::new());
        let mut at = 0_usize;
        for (slot, len) in out.iter_mut().zip(lens) {
            let end = at.saturating_add(size(len));
            let bytes = buf
                .get(at..end)
                .ok_or("the input is longer than the input buffer (call cw_in first)")?;
            std::str::from_utf8(bytes)?.clone_into(slot);
            at = end;
        }
        Ok(out)
    })
}

fn time(flicks: i64) -> Res<Time> {
    Ok(Time::from_flicks(flicks)?)
}

fn rate(num: u32, den: u32) -> Res<FrameRate> {
    Ok(FrameRate::new(num, den)?)
}

fn rounding(code: u32) -> Res<Rounding> {
    match code {
        0 => Ok(Rounding::Floor),
        1 => Ok(Rounding::Nearest),
        other => Err(Fail(format!(
            "unknown rounding {other} (0 is floor, 1 is nearest)"
        ))),
    }
}

fn samples(n: u32) -> Res<usize> {
    if n > MAX_SAMPLES {
        return Err(Fail(format!("{n} samples is more than {MAX_SAMPLES}")));
    }
    Ok(size(n))
}

// ---------------------------------------------------------------------------------------------
// Buffers and version
// ---------------------------------------------------------------------------------------------

export! {
    /// The input buffer, resized to `len` bytes (zeroed), for the caller to write UTF-8 into.
    fn cw_in(len: u32) -> *mut u8 {
        INPUT.with_borrow_mut(|buf| {
            buf.clear();
            buf.resize(size(len), 0);
            buf.as_mut_ptr()
        })
    }
}

export! {
    /// The output buffer: the text of the last call that returned a length, or the last error.
    fn cw_out_ptr() -> *const u8 {
        OUTPUT.with_borrow(|out| out.as_ptr())
    }
}

export! {
    /// The output buffer's length in bytes.
    fn cw_out_len() -> u32 {
        OUTPUT.with_borrow(|out| u32::try_from(out.len()).unwrap_or(u32::MAX))
    }
}

export! {
    /// The number buffer: the `f64`s of the last call that returned a count.
    fn cw_f64_ptr() -> *const f64 {
        NUMBERS.with_borrow(Vec::as_ptr)
    }
}

export! {
    /// The module's version (the workspace version, which `@cutweave/protocol` shares) into the
    /// output buffer; returns its length.
    fn cw_version() -> i32 {
        length(set_output(env!("CARGO_PKG_VERSION")))
    }
}

// ---------------------------------------------------------------------------------------------
// Time (arch §4.1)
// ---------------------------------------------------------------------------------------------

export! {
    /// [`FrameRate::frame_to_time`]: the start of `frame` at `num/den` fps, in flicks.
    fn cw_frame_to_time(frame: i64, num: u32, den: u32) -> i64 {
        int_reply(rate(num, den).and_then(|r| Ok(r.frame_to_time(frame)?.flicks())))
    }
}

export! {
    /// [`FrameRate::time_to_frame`]: the frame of `t` (flicks) at `num/den` fps.
    fn cw_time_to_frame(t: i64, num: u32, den: u32, round: u32) -> i64 {
        int_reply(time_to_frame(t, num, den, round))
    }
}

fn time_to_frame(t: i64, num: u32, den: u32, round: u32) -> Res<i64> {
    Ok(rate(num, den)?.time_to_frame(time(t)?, rounding(round)?))
}

export! {
    /// [`FrameRate::snap_to_frame`]: `t` (flicks) moved onto the frame grid of `num/den` fps.
    fn cw_snap_to_frame(t: i64, num: u32, den: u32, round: u32) -> i64 {
        int_reply(snap_to_frame(t, num, den, round))
    }
}

fn snap_to_frame(t: i64, num: u32, den: u32, round: u32) -> Res<i64> {
    Ok(rate(num, den)?
        .snap_to_frame(time(t)?, rounding(round)?)?
        .flicks())
}

export! {
    /// [`TimeInput`] parsing and resolution: the time named by the input text (flicks, seconds
    /// `"62.5s"`, frame `"f:1500"` or timecode `"00:01:02:03"`, drop-frame `"00:01:02;03"`) at
    /// `num/den` fps; `0/0` is no frame rate (flicks and seconds only).
    fn cw_time_parse(len: u32, num: u32, den: u32) -> i64 {
        int_reply(parse_time(len, num, den))
    }
}

fn parse_time(len: u32, num: u32, den: u32) -> Res<i64> {
    let [text] = inputs([len])?;
    let rate = if num == 0 && den == 0 {
        None
    } else {
        Some(rate(num, den)?)
    };
    Ok(text.parse::<TimeInput>()?.resolve(rate)?.flicks())
}

export! {
    /// [`Time`]'s text into the output buffer: the lossless canonical form (`"62.5s"` or raw
    /// flicks) when `precision` is negative, else seconds with that many decimals (`"1.042s"`).
    fn cw_time_format(t: i64, precision: i32) -> i32 {
        text_reply(format_time(t, precision))
    }
}

fn format_time(t: i64, precision: i32) -> Res<String> {
    let t = time(t)?;
    if precision > MAX_PRECISION {
        return Err(Fail(format!("at most {MAX_PRECISION} decimals")));
    }
    Ok(match usize::try_from(precision) {
        Err(_) => t.to_string(),
        Ok(p) => format!("{t:.p$}"),
    })
}

export! {
    /// [`Timecode::from_time`]'s label of `t` at `num/den` fps into the output buffer
    /// (`drop_frame` non-zero for drop-frame, 29.97 and 59.94 fps only).
    fn cw_timecode(t: i64, num: u32, den: u32, drop_frame: u32, round: u32) -> i32 {
        text_reply(timecode(t, num, den, drop_frame, round))
    }
}

fn timecode(t: i64, num: u32, den: u32, drop_frame: u32, round: u32) -> Res<String> {
    let label = Timecode::from_time(time(t)?, rate(num, den)?, drop_frame != 0, rounding(round)?)?;
    Ok(label.to_string())
}

export! {
    /// [`sample_times`]: `n` times from `t0` to `t1` (keyframes §6) into the number buffer, as
    /// `f64` (exact: |t| < 2⁵³); returns `n`.
    fn cw_sample_times(t0: i64, t1: i64, n: u32) -> i32 {
        count_reply(times(t0, t1, n))
    }
}

fn times(t0: i64, t1: i64, n: u32) -> Res<usize> {
    let (t0, t1, n) = (time(t0)?, time(t1)?, samples(n)?);
    Ok(fill_numbers(|out| {
        out.extend(sample_times(t0, t1, n).map(exact_f64));
    }))
}

#[allow(clippy::cast_precision_loss)] // |t| < 2^53: exact in f64.
fn exact_f64(t: Time) -> f64 {
    t.flicks() as f64
}

// ---------------------------------------------------------------------------------------------
// Easing (keyframes §4)
// ---------------------------------------------------------------------------------------------

/// The interpolation written into the input buffer as its wire form (validated, KF-5 and KF-7).
fn interp(len: u32) -> Res<Interp> {
    let [text] = inputs([len])?;
    Ok(Interp::from_json(&serde_json::from_str::<Value>(&text)?)?)
}

export! {
    /// [`Interp::progress`]: the easing progress `dt` flicks into a segment of `span` flicks, for
    /// the interpolation in the input buffer (`len` bytes of its JSON). NaN on error.
    fn cw_easing_progress(len: u32, dt: i64, span: i64) -> f64 {
        match interp(len) {
            Ok(interp) => interp.progress(dt, span),
            Err(Fail(message)) => {
                set_output(&message);
                f64::NAN
            }
        }
    }
}

export! {
    /// The curve of the interpolation in the input buffer over a segment of `span` flicks:
    /// [`Interp::progress`] at the `n` [`sample_times`] from 0 to `span`, into the number buffer.
    fn cw_easing_sample(len: u32, span: i64, n: u32) -> i32 {
        count_reply(easing_sample(len, span, n))
    }
}

fn easing_sample(len: u32, span: i64, n: u32) -> Res<usize> {
    let (interp, end, n) = (interp(len)?, time(span)?, samples(n)?);
    Ok(fill_numbers(|out| {
        out.extend(sample_times(Time::ZERO, end, n).map(|dt| interp.progress(dt.flicks(), span)));
    }))
}

export! {
    /// Every easing name a command accepts with its interpolation ([`Easing::ALL`]: hold, linear,
    /// then the §4.6 presets in picker order), as JSON `[{"name", "interp"}]` into the output
    /// buffer.
    fn cw_easing_presets() -> i32 {
        text_reply(presets())
    }
}

fn presets() -> Res<String> {
    let list: Vec<Value> = Easing::ALL
        .iter()
        .map(|e| serde_json::json!({ "name": e.name(), "interp": e.interp() }))
        .collect();
    Ok(serde_json::to_string(&list)?)
}

// ---------------------------------------------------------------------------------------------
// Animatables (keyframes §3, §6, §7)
// ---------------------------------------------------------------------------------------------

/// How an evaluated value is laid out in the number buffer.
trait Components {
    fn push(&self, out: &mut Vec<f64>);
}

impl Components for f64 {
    fn push(&self, out: &mut Vec<f64>) {
        out.push(*self);
    }
}

impl Components for Vec2 {
    fn push(&self, out: &mut Vec<f64>) {
        out.extend([self.x, self.y]);
    }
}

impl Components for LinearRgba {
    fn push(&self, out: &mut Vec<f64>) {
        out.extend([self.r, self.g, self.b, self.a]);
    }
}

impl Components for bool {
    fn push(&self, out: &mut Vec<f64>) {
        out.push(if *self { 1.0 } else { 0.0 });
    }
}

/// A parsed animatable, its cursor-caching evaluator and, for a property, its clamp (§7).
struct Compiled<T: AnimValue> {
    anim: Animatable<T>,
    evaluator: Evaluator<T>,
    clamp: Option<ValueRange>,
}

impl<T: AnimValue> Compiled<T>
where
    T::Eval: Components,
{
    /// [`Animatable::from_json`] (KF-1 to KF-7) and, for a property, its stored range (KF-4).
    fn new(json: &Value, spec: Option<&PropSpec>) -> Res<Compiled<T>> {
        let anim = Animatable::<T>::from_json(json)?;
        if let Some(spec) = spec {
            anim.check_range(spec.stored)?;
        }
        Ok(Compiled {
            evaluator: anim.evaluator(),
            anim,
            clamp: spec.map(|s| s.clamp),
        })
    }

    /// `eval` or, for a property, `eval_clamped` (§6, step 7).
    fn clamped(&self, value: T::Eval) -> T::Eval {
        match self.clamp {
            Some(range) => T::clamp(value, range),
            None => value,
        }
    }

    fn eval(&mut self, t: Time, out: &mut Vec<f64>) {
        let value = self.evaluator.eval(t);
        self.clamped(value).push(out);
    }

    fn eval_range(&self, t0: Time, t1: Time, n: usize, out: &mut Vec<f64>) {
        for (_, value) in self.anim.eval_range(t0, t1, n) {
            self.clamped(value).push(out);
        }
    }
}

enum Anim {
    /// Scalars and accumulating angles.
    Number(Compiled<f64>),
    Shortest(Compiled<ShortestAngle>),
    Vec2(Compiled<Vec2>),
    Color(Compiled<Color>),
    Bool(Compiled<bool>),
}

/// Runs `$body` with `$c` bound to the compiled animatable, whatever its type.
macro_rules! each {
    ($anim:expr, $c:ident => $body:expr) => {
        match $anim {
            Anim::Number($c) => $body,
            Anim::Shortest($c) => $body,
            Anim::Vec2($c) => $body,
            Anim::Color($c) => $body,
            Anim::Bool($c) => $body,
        }
    };
}

struct Entry {
    kind: ValueType,
    anim: Anim,
}

/// Parses an animatable of value type `kind` (a [`ValueType`] name) or of property `prop` (a
/// [`PropId`], which also sets its clamp); at least one of the two, and they must agree.
fn compile(kind: &str, prop: &str, json: &str) -> Res<Entry> {
    let spec = if prop.is_empty() {
        None
    } else {
        Some(prop.parse::<PropId>()?.spec())
    };
    let named = if kind.is_empty() {
        None
    } else {
        Some(
            VALUE_TYPES
                .into_iter()
                .find(|t| t.name() == kind)
                .ok_or_else(|| Fail(format!("unknown value type `{kind}`")))?,
        )
    };
    let kind = match (named, spec) {
        (Some(named), Some(spec)) if named != spec.value_type => {
            return Err(Fail(format!(
                "property `{prop}` is a {}, not a {}",
                spec.value_type.name(),
                named.name()
            )));
        }
        (Some(kind), _) => kind,
        (None, Some(spec)) => spec.value_type,
        (None, None) => return Err(Fail("give a value type or a property".to_owned())),
    };
    let json: Value = serde_json::from_str(json)?;
    let anim = match kind {
        ValueType::Scalar | ValueType::Angle => Anim::Number(Compiled::new(&json, spec)?),
        ValueType::ShortestAngle => Anim::Shortest(Compiled::new(&json, spec)?),
        ValueType::Vec2 => Anim::Vec2(Compiled::new(&json, spec)?),
        ValueType::Color => Anim::Color(Compiled::new(&json, spec)?),
        ValueType::Bool => Anim::Bool(Compiled::new(&json, spec)?),
    };
    Ok(Entry { kind, anim })
}

fn with_entry<R>(handle: u32, f: impl FnOnce(&mut Entry) -> Res<R>) -> Res<R> {
    ANIMS.with_borrow_mut(|slots| {
        let entry = size(handle)
            .checked_sub(1)
            .and_then(|i| slots.get_mut(i))
            .and_then(Option::as_mut)
            .ok_or_else(|| Fail(format!("no animatable with handle {handle}")))?;
        f(entry)
    })
}

export! {
    /// Compiles an animatable from the input buffer: its value type name (`"scalar"`, `"vec2"`,
    /// `"angle"`, `"angle_shortest"`, `"color"`, `"bool"`; may be empty when a property is
    /// given), a property ID (may be empty; adds its stored-range check and clamp), then the
    /// keyframes §3.1 JSON (a bare value or `{"keyframes": [...]}`). Returns a handle, or 0.
    fn cw_anim_new(type_len: u32, prop_len: u32, json_len: u32) -> u32 {
        let entry = inputs([type_len, prop_len, json_len])
            .and_then(|[kind, prop, json]| compile(&kind, &prop, &json));
        match entry {
            Ok(entry) => ANIMS.with_borrow_mut(|slots| {
                let index = slots.iter().position(Option::is_none).unwrap_or_else(|| {
                    slots.push(None);
                    slots.len() - 1
                });
                slots[index] = Some(entry);
                u32::try_from(index + 1).unwrap_or(0)
            }),
            Err(Fail(message)) => {
                set_output(&message);
                0
            }
        }
    }
}

export! {
    /// The value type name of an animatable into the output buffer; returns its length.
    fn cw_anim_type(handle: u32) -> i32 {
        text_reply(with_entry(handle, |entry| Ok(entry.kind.name().to_owned())))
    }
}

export! {
    /// [`Animatable::eval`] at item time `t` (flicks), clamped for a property, into the number
    /// buffer: 1 number (scalar, angle, bool as 0/1), 2 (vec2: x, y) or 4 (colour: premultiplied
    /// linear r, g, b, a). Returns the count.
    fn cw_anim_eval(handle: u32, t: i64) -> i32 {
        count_reply(anim_eval(handle, t))
    }
}

fn anim_eval(handle: u32, t: i64) -> Res<usize> {
    let t = time(t)?;
    with_entry(handle, |entry| {
        Ok(fill_numbers(
            |out| each!(&mut entry.anim, c => c.eval(t, out)),
        ))
    })
}

export! {
    /// [`Animatable::eval_range`]: `n` samples from `t0` to `t1` at the [`sample_times`],
    /// interleaved in the number buffer as [`cw_anim_eval`] lays out one. Returns the count.
    fn cw_anim_eval_range(handle: u32, t0: i64, t1: i64, n: u32) -> i32 {
        count_reply(anim_eval_range(handle, t0, t1, n))
    }
}

fn anim_eval_range(handle: u32, t0: i64, t1: i64, n: u32) -> Res<usize> {
    let (t0, t1, n) = (time(t0)?, time(t1)?, samples(n)?);
    with_entry(handle, |entry| {
        Ok(fill_numbers(
            |out| each!(&entry.anim, c => c.eval_range(t0, t1, n, out)),
        ))
    })
}

export! {
    /// Frees an animatable; returns 1, or 0 if there was none with that handle.
    fn cw_anim_free(handle: u32) -> u32 {
        ANIMS.with_borrow_mut(|slots| {
            let slot = size(handle).checked_sub(1).and_then(|i| slots.get_mut(i));
            u32::from(slot.and_then(Option::take).is_some())
        })
    }
}

export! {
    /// How many animatables are compiled and not yet freed (leak checks).
    fn cw_anim_live() -> u32 {
        ANIMS.with_borrow(|slots| {
            u32::try_from(slots.iter().filter(|s| s.is_some()).count()).unwrap_or(u32::MAX)
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Snapping (timeline spec §6)
// ---------------------------------------------------------------------------------------------

export! {
    /// The bulk integer input buffer, resized to `n` `i64`s (zeroed): aligned, so the caller can
    /// view it as a `BigInt64Array` and write candidate times and probe offsets directly.
    fn cw_in_i64(n: u32) -> *mut i64 {
        INPUT_I64.with_borrow_mut(|buf| {
            buf.clear();
            buf.resize(size(n), 0);
            buf.as_mut_ptr()
        })
    }
}

/// The first `n` integers of the bulk input buffer.
fn bulk(n: usize) -> Res<Vec<i64>> {
    INPUT_I64.with_borrow(|buf| {
        buf.get(..n).map(<[i64]>::to_vec).ok_or_else(|| {
            Fail("the input is longer than the bulk buffer (call cw_in_i64 first)".to_owned())
        })
    })
}

fn snap_kind(code: i64) -> Res<SnapKind> {
    u32::try_from(code)
        .ok()
        .and_then(SnapKind::from_code)
        .ok_or_else(|| {
            Fail(format!(
                "unknown snap kind {code} (0 playhead, 1 edge, 2 marker, 3 beat, 4 in/out)"
            ))
        })
}

fn with_snap_index<R>(handle: u32, f: impl FnOnce(&mut SnapIndex) -> Res<R>) -> Res<R> {
    SNAPS.with_borrow_mut(|slots| {
        let index = size(handle)
            .checked_sub(1)
            .and_then(|i| slots.get_mut(i))
            .and_then(Option::as_mut)
            .ok_or_else(|| Fail(format!("no snap index with handle {handle}")))?;
        f(index)
    })
}

export! {
    /// Builds a snap index at `num/den` fps from `n` candidates in the bulk buffer, as `n` pairs
    /// `[kind, t]` (kind 0 playhead, 1 edge, 2 marker, 3 beat, 4 in/out; `t` in flicks, moved to
    /// the nearest frame). Returns a handle, or 0 with the message in the output buffer.
    fn cw_snap_index_new(num: u32, den: u32, n: u32) -> u32 {
        match snap_index_new(num, den, n) {
            Ok(index) => SNAPS.with_borrow_mut(|slots| {
                let at = slots.iter().position(Option::is_none).unwrap_or_else(|| {
                    slots.push(None);
                    slots.len() - 1
                });
                slots[at] = Some(index);
                u32::try_from(at + 1).unwrap_or(0)
            }),
            Err(Fail(message)) => {
                set_output(&message);
                0
            }
        }
    }
}

fn snap_index_new(num: u32, den: u32, n: u32) -> Res<SnapIndex> {
    let rate = rate(num, den)?;
    let pairs = bulk(size(n).saturating_mul(2))?;
    let mut candidates = Vec::with_capacity(pairs.len() / 2);
    for pair in pairs.as_chunks::<2>().0 {
        candidates.push((snap_kind(pair[0])?, pair[1]));
    }
    Ok(SnapIndex::new(rate, candidates)?)
}

export! {
    /// Replaces every candidate of `kind` in an index with the `n` times in the bulk buffer (the
    /// incremental update after a document patch). Returns the new candidate count, or -1.
    fn cw_snap_index_set_kind(handle: u32, kind: u32, n: u32) -> i32 {
        count_reply(snap_index_set_kind(handle, kind, n))
    }
}

fn snap_index_set_kind(handle: u32, kind: u32, n: u32) -> Res<usize> {
    let kind = snap_kind(i64::from(kind))?;
    let times = bulk(size(n))?;
    with_snap_index(handle, |index| {
        index.set_kind(kind, times)?;
        Ok(index.len())
    })
}

export! {
    /// How many candidates an index holds, or -1 for a bad handle.
    fn cw_snap_index_len(handle: u32) -> i32 {
        count_reply(with_snap_index(handle, |index| Ok(index.len())))
    }
}

export! {
    /// Snaps the raw time `r` (flicks) against an index (spec §6). The `n` probe offsets are in
    /// the bulk buffer. `enabled` is a bit set of kinds (bit `n` for kind `n`; 0 is snapping
    /// off), `px_per_second` the zoom and `threshold_px` the snap distance. Returns the snapped
    /// time, always on the frame grid, or [`ERR_TIME`] with the message in the output buffer.
    /// The number buffer then holds `[kind, t, probe]`: the target's kind and time and the index
    /// of the winning probe, or `[-1, 0, 0]` when nothing was snapped to.
    fn cw_snap(
        handle: u32,
        r: i64,
        n: u32,
        px_per_second: f64,
        threshold_px: f64,
        enabled: u32,
    ) -> i64 {
        int_reply(snap_call(
            handle,
            r,
            n,
            &SnapOptions {
                px_per_second,
                threshold_px,
                enabled: SnapKinds::from_bits(enabled),
            },
        ))
    }
}

fn snap_call(handle: u32, r: i64, n: u32, options: &SnapOptions) -> Res<i64> {
    let offsets = bulk(size(n))?;
    let snapped = with_snap_index(handle, |index| Ok(index.snap(r, &offsets, options)?))?;
    fill_numbers(|out| match snapped.target {
        Some(target) => {
            #[allow(clippy::cast_precision_loss)] // |t| < 2^53: exact in f64.
            let t = target.t as f64;
            out.extend([
                f64::from(target.kind.code()),
                t,
                f64::from(u32::try_from(snapped.probe).unwrap_or(u32::MAX)),
            ]);
        }
        None => out.extend([-1.0, 0.0, 0.0]),
    });
    Ok(snapped.t)
}

export! {
    /// Frees a snap index; returns 1, or 0 if there was none with that handle.
    fn cw_snap_index_free(handle: u32) -> u32 {
        SNAPS.with_borrow_mut(|slots| {
            let slot = size(handle).checked_sub(1).and_then(|i| slots.get_mut(i));
            u32::from(slot.and_then(Option::take).is_some())
        })
    }
}

export! {
    /// How many snap indexes are built and not yet freed (leak checks).
    fn cw_snap_index_live() -> u32 {
        SNAPS.with_borrow(|slots| {
            u32::try_from(slots.iter().filter(|s| s.is_some()).count()).unwrap_or(u32::MAX)
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Native callers
// ---------------------------------------------------------------------------------------------

/// For native callers (the parity test), what the loader does with [`cw_in`]: writes `parts`
/// one after another into the input buffer and returns their byte lengths.
#[must_use]
pub fn write_input<const N: usize>(parts: [&str; N]) -> [u32; N] {
    INPUT.with_borrow_mut(|buf| {
        buf.clear();
        for part in parts {
            buf.extend_from_slice(part.as_bytes());
        }
    });
    parts.map(|p| u32::try_from(p.len()).unwrap_or(u32::MAX))
}

/// For native callers, what the loader does with [`cw_in_i64`]: writes `values` into the bulk
/// buffer.
pub fn write_bulk(values: &[i64]) {
    INPUT_I64.with_borrow_mut(|buf| {
        buf.clear();
        buf.extend_from_slice(values);
    });
}

/// For native callers: the output buffer's text.
#[must_use]
pub fn output() -> String {
    OUTPUT.with_borrow(Clone::clone)
}

/// For native callers: the number buffer.
#[must_use]
pub fn numbers() -> Vec<f64> {
    NUMBERS.with_borrow(Clone::clone)
}
