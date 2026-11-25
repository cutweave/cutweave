// SPDX-License-Identifier: GPL-3.0-or-later
//! The `keyframe` namespace (WP-2C.03; `docs/spec/keyframes.md` §9, ADR-0024): `keyframe.set`,
//! `.delete`, `.move`, `.set_interp`, `.clear`, `.paste` and the queries `keyframe.copy` and
//! `keyframe.eval` ([`commands`], [`clipboard`], [`eval`]), and the retiming rules splits and
//! trims apply to keys ([`retime`], §8). The key-list work itself is
//! [`cutweave_model::Animatable`]'s; these commands resolve items and times, check locks, map
//! errors and apply the result through `Tx`.
//!
//! # Contract (for the UI and for agents)
//!
//! * **Addressing.** A key is `(item, prop, t)` with `prop` a [`PropId`] and `t` its exact stored
//!   item time (flicks since the item's first frame). Addressing never snaps.
//! * **Times in.** A new key time is `t` (item time) or `at` (sequence time), exactly one. Both
//!   snap to the sequence's frame grid *in sequence time* (`fine: true`, audio properties only:
//!   the sample grid). Results state `t` and `at`.
//! * **Values** are validated against the property's stored range and never clamped silently.
//! * **Gestures.** `keyframe.set`, `keyframe.move` and `item.set_props` are idempotent at one
//!   key time, so repeating them under a gesture transaction collapses into one undo step.
//! * **Linked partners** are not affected: keyframing a video clip leaves its audio alone.

mod clipboard;
mod commands;
mod eval;
pub(crate) mod retime;

use std::collections::BTreeSet;

use cutweave_commands::{CmdError, Registry, RegistryError, ResourceKind, wire};
use cutweave_model::{
    AnimValue, Animatable, Clip, Easing, EntityKind, FrameRate, Interp, ItemId, KeyframeError,
    Project, PropId, PropSpec, Rounding, SampleRate, Time, TimeInput, ValueType, Vec2,
};

pub use clipboard::{
    ClipboardKey, KeyframeClipboard, KeyframeCopy, KeyframeCopyOutput, KeyframePaste,
    KeyframePasteOutput,
};
pub use commands::{
    KeyframeClear, KeyframeClearOutput, KeyframeDelete, KeyframeDeleteOutput, KeyframeMove,
    KeyframeMoveOutput, KeyframeSet, KeyframeSetInterp, KeyframeSetInterpOutput, KeyframeSetOutput,
    MovedKey,
};
pub use eval::{EvalSample, KeyframeEval, KeyframeEvalOutput};

use crate::{
    item::{Located, ensure_unlocked, members},
    timeline::edit::given_items,
};

/// Most keys one `keyframe.set` or `keyframe.paste` call writes (per item and property).
pub const MAX_KEYS_PER_CALL: usize = 10_000;

/// Registers the `keyframe.*` commands and queries.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<KeyframeSet>()?
        .register::<KeyframeDelete>()?
        .register::<KeyframeMove>()?
        .register::<KeyframeSetInterp>()?
        .register::<KeyframeClear>()?
        .register::<KeyframePaste>()?
        .register::<KeyframeCopy>()?
        .register::<KeyframeEval>()?;
    Ok(())
}

// ----- wire types ------------------------------------------------------------------------------

/// A property value: a number (opacity, crop edges, rotation, gain, pan), `{x, y}` (position,
/// scale, anchor) or a boolean (flip). The property decides which; a mismatch is `Invalid`.
#[wire]
#[serde(untagged)]
pub enum KeyValue {
    /// A boolean.
    Bool(bool),
    /// A number.
    Number(#[specta(type = specta_typescript::Number)] f64),
    /// A 2D vector.
    Vec2(Vec2),
}

/// How the segment that starts at a key moves to the next key, as commands take it: the stored
/// form ([`Interp`]) with `bounce` optional (default `false`).
#[wire]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InterpInput {
    /// The key's value until the next key, then a jump.
    Hold,
    /// Constant speed.
    Linear,
    /// A CSS-style cubic Bezier from (0, 0) to (1, 1).
    Bezier {
        /// First control point, time axis, `0..=1`.
        #[specta(type = specta_typescript::Number)]
        x1: f64,
        /// First control point, progress axis, `-10..=10`.
        #[specta(type = specta_typescript::Number)]
        y1: f64,
        /// Second control point, time axis, `0..=1`.
        #[specta(type = specta_typescript::Number)]
        x2: f64,
        /// Second control point, progress axis, `-10..=10`.
        #[specta(type = specta_typescript::Number)]
        y2: f64,
        /// A hint naming the preset the numbers came from.
        #[serde(default)]
        preset: Option<String>,
    },
    /// A damped spring.
    Spring {
        /// Stiffness, `1..=10000`.
        #[specta(type = specta_typescript::Number)]
        stiffness: f64,
        /// Damping, `0..=1000`.
        #[specta(type = specta_typescript::Number)]
        damping: f64,
        /// Mass, `0.1..=100`.
        #[specta(type = specta_typescript::Number)]
        mass: f64,
        /// A hard stop at the target (default `false`).
        #[serde(default)]
        bounce: Option<bool>,
        /// A hint naming the preset the numbers came from.
        #[serde(default)]
        preset: Option<String>,
    },
}

impl InterpInput {
    fn into_interp(self) -> Interp {
        match self {
            InterpInput::Hold => Interp::Hold,
            InterpInput::Linear => Interp::Linear,
            InterpInput::Bezier {
                x1,
                y1,
                x2,
                y2,
                preset,
            } => Interp::Bezier {
                x1,
                y1,
                x2,
                y2,
                preset,
            },
            InterpInput::Spring {
                stiffness,
                damping,
                mass,
                bounce,
                preset,
            } => Interp::Spring {
                stiffness,
                damping,
                mass,
                bounce: bounce.unwrap_or(false),
                preset,
            },
        }
    }
}

/// A key to address: its property and exact item time.
#[wire]
#[derive(Copy, Eq)]
pub struct KeyRef {
    /// The property.
    pub prop: PropId,
    /// The key's stored item time in flicks, as `item.get` returns it.
    pub t: Time,
}

// ----- the value types of the registry ---------------------------------------------------------

/// A property value type that crosses the wire as a [`KeyValue`].
pub(crate) trait Wire: AnimValue<Eval = Self> + Sized {
    /// The value in a [`KeyValue`], if it has this type.
    fn pick(value: &KeyValue) -> Option<Self>;
    /// The value as a [`KeyValue`].
    fn wrap(&self) -> KeyValue;
}

impl Wire for f64 {
    fn pick(value: &KeyValue) -> Option<f64> {
        match value {
            KeyValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    fn wrap(&self) -> KeyValue {
        KeyValue::Number(*self)
    }
}

impl Wire for Vec2 {
    fn pick(value: &KeyValue) -> Option<Vec2> {
        match value {
            KeyValue::Vec2(v) => Some(*v),
            _ => None,
        }
    }

    fn wrap(&self) -> KeyValue {
        KeyValue::Vec2(*self)
    }
}

impl Wire for bool {
    fn pick(value: &KeyValue) -> Option<bool> {
        match value {
            KeyValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    fn wrap(&self) -> KeyValue {
        KeyValue::Bool(*self)
    }
}

/// Runs `$body` with `$a` bound to the mutable animatable of property `$id` of `$clip`, once
/// per value type (so `$body` is generic code over [`Wire`]).
macro_rules! on_prop_mut {
    ($clip:expr, $id:expr, $a:ident => $body:expr) => {
        match $clip.prop_mut($id) {
            cutweave_model::PropMut::Scalar($a) => $body,
            cutweave_model::PropMut::Vec2($a) => $body,
            cutweave_model::PropMut::Bool($a) => $body,
        }
    };
}

/// The shared-reference twin of [`on_prop_mut`].
macro_rules! on_prop {
    ($clip:expr, $id:expr, $a:ident => $body:expr) => {
        match $clip.prop($id) {
            cutweave_model::PropRef::Scalar($a) => $body,
            cutweave_model::PropRef::Vec2($a) => $body,
            cutweave_model::PropRef::Bool($a) => $body,
        }
    };
}
pub(crate) use on_prop;
pub(crate) use on_prop_mut;

/// Sets a property to a static value (dropping any keys), returning whether its value
/// changed. A static property that already has the value is untouched; the value must have the
/// property's type and fit its range (callers validate).
pub(crate) fn set_static(clip: &mut Clip, id: PropId, value: &KeyValue) -> bool {
    fn one<T: Wire>(a: &mut Animatable<T>, value: &KeyValue) -> bool {
        match T::pick(value) {
            Some(v) if *a != Animatable::Static(v.clone()) => {
                a.set(v);
                true
            }
            _ => false,
        }
    }
    on_prop_mut!(clip, id, a => one(a, value))
}

/// Whether the property is animated.
pub(crate) fn is_animated(clip: &Clip, id: PropId) -> bool {
    on_prop!(clip, id, a => a.is_animated())
}

// ----- errors ----------------------------------------------------------------------------------

/// A model error as a command error. `pointer` is where the animatable's own paths
/// (`/value`, `/interp/kind`) start in the request params; `what` names a missing key.
pub(crate) fn map_error(e: KeyframeError, pointer: &str, what: &str) -> CmdError {
    match e {
        KeyframeError::Invariant { path, msg, rule } => {
            CmdError::invalid(format!("{pointer}{path}"), format!("{rule}: {msg}"))
        }
        KeyframeError::Invalid(msg) => CmdError::invalid(pointer, msg),
        KeyframeError::NotFound(msg) => {
            CmdError::not_found(ResourceKind::Keyframe, format!("{what}: {msg}"))
        }
    }
}

/// Maps the error of a retiming rule inside a placement edit.
pub(crate) fn retime_error(e: &KeyframeError) -> CmdError {
    CmdError::invalid("", format!("keyframes cannot follow this edit: {e}"))
}

// ----- items and times -------------------------------------------------------------------------

/// One clip a keyframe command addresses, with what time resolution needs.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Target {
    pub(crate) id: ItemId,
    pub(crate) start: Time,
    pub(crate) duration: i64,
    pub(crate) rate: FrameRate,
    pub(crate) samples: SampleRate,
}

/// Exactly one of `item` and `items`, resolved to unlocked clips (deduplicated, in order).
pub(crate) fn targets(
    doc: &Project,
    item: Option<ItemId>,
    items: Option<&[ItemId]>,
) -> Result<Vec<Target>, CmdError> {
    let ids = match (item, items) {
        (Some(i), None) => vec![i],
        (None, Some(list)) => list.to_vec(),
        _ => {
            return Err(CmdError::invalid(
                "",
                "give exactly one of `item` or `items`",
            ));
        }
    };
    let (sequence, ids) = given_items(doc, &ids)?;
    let seq = doc
        .sequences
        .get(&sequence)
        .ok_or_else(|| CmdError::not_found(EntityKind::Sequence, sequence))?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let (_, group) = members(doc, id, false)?;
        ensure_unlocked(doc, &group)?;
        let Located { item, .. } = &group[0];
        let clip = item
            .as_clip()
            .ok_or_else(|| CmdError::unsupported("only clips have animatable properties"))?;
        out.push(Target {
            id,
            start: clip.start,
            duration: clip.duration.flicks(),
            rate: seq.format.frame_rate,
            samples: seq.format.sample_rate,
        });
    }
    Ok(out)
}

/// `Invalid` unless a single item was addressed (commands that take `item` only).
pub(crate) fn one_target(doc: &Project, item: ItemId) -> Result<Target, CmdError> {
    targets(doc, Some(item), None).map(|mut t| t.remove(0))
}

/// Whether the property belongs to audio (the sample grid applies with `fine`).
pub(crate) fn is_audio(prop: PropId) -> bool {
    matches!(prop, PropId::AudioGain | PropId::AudioPan)
}

/// A time as a command takes it: item time `t` or sequence time `at`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TimeArg<'a> {
    pub(crate) t: Option<TimeInput>,
    pub(crate) at: Option<TimeInput>,
    /// The JSON Pointer of the object holding `t` and `at`.
    pub(crate) pointer: &'a str,
}

fn flicks(pointer: &str, v: i128) -> Result<i64, CmdError> {
    i64::try_from(v)
        .ok()
        .filter(|f| Time::from_flicks(*f).is_ok())
        .ok_or_else(|| CmdError::invalid(pointer, "the time is out of range"))
}

/// Resolves a time input against the sequence's frame rate.
fn resolve(pointer: &str, input: TimeInput, rate: FrameRate) -> Result<i64, CmdError> {
    input
        .resolve(Some(rate))
        .map(Time::flicks)
        .map_err(|e| CmdError::invalid(pointer, e.to_string()))
}

/// A sequence time snapped to the frame grid (or the sample grid, `fine`), in flicks.
fn snap(pointer: &str, abs: i64, target: &Target, fine: bool) -> Result<i64, CmdError> {
    let time = Time::from_flicks(abs).map_err(|e| CmdError::invalid(pointer, e.to_string()))?;
    let snapped = if fine {
        let n = target.samples.time_to_samples(time, Rounding::Nearest);
        target.samples.samples_to_time(n)
    } else {
        target.rate.snap_to_frame(time, Rounding::Nearest)
    };
    snapped
        .map(Time::flicks)
        .map_err(|e| CmdError::invalid(pointer, e.to_string()))
}

impl TimeArg<'_> {
    /// The item time of the key, snapped in sequence time (keyframes §3.2, §9.1).
    pub(crate) fn item_time(
        self,
        target: &Target,
        prop: Option<PropId>,
        fine: bool,
    ) -> Result<Time, CmdError> {
        if fine && prop.is_some_and(|p| !is_audio(p)) {
            return Err(CmdError::invalid(
                format!("{}/fine", parent(self.pointer)),
                "`fine` applies to audio properties only",
            ));
        }
        let start = target.start.flicks();
        let (pointer, abs) = match (self.t, self.at) {
            (Some(t), None) => {
                let p = format!("{}/t", self.pointer);
                let rel = resolve(&p, t, target.rate)?;
                let abs = flicks(&p, i128::from(start) + i128::from(rel))?;
                (p, abs)
            }
            (None, Some(at)) => {
                let p = format!("{}/at", self.pointer);
                let abs = resolve(&p, at, target.rate)?;
                (p, abs)
            }
            _ => {
                return Err(CmdError::invalid(
                    self.pointer,
                    "give exactly one of `t` (item time) and `at` (sequence time)",
                ));
            }
        };
        snap_to_item_time(&pointer, abs, target, fine)
    }
}

/// The item time of the sequence time `abs` (flicks), snapped to the frame grid (or the sample
/// grid, `fine`): `snap(abs) − start` (keyframes §3.2).
pub(crate) fn snap_to_item_time(
    pointer: &str,
    abs: i64,
    target: &Target,
    fine: bool,
) -> Result<Time, CmdError> {
    let snapped = snap(pointer, abs, target, fine)?;
    let rel = flicks(
        pointer,
        i128::from(snapped) - i128::from(target.start.flicks()),
    )?;
    Time::from_flicks(rel).map_err(|e| CmdError::invalid(pointer, e.to_string()))
}

/// `/keys/3` → `/keys`... the pointer of the params object a key list entry sits in.
fn parent(pointer: &str) -> &str {
    pointer.rsplit_once('/').map_or("", |(p, _)| p)
}

/// `t` as a sequence time (`start + t`), saturating at the ends of the time range.
pub(crate) fn sequence_time(target: &Target, t: Time) -> Time {
    let flicks = target.start.flicks().saturating_add(t.flicks());
    Time::from_flicks(flicks.clamp(Time::MIN.flicks(), Time::MAX.flicks())).unwrap_or(Time::ZERO)
}

/// Whether item time `t` lies outside the item's `[0, duration)`.
pub(crate) fn outside(target: &Target, t: Time) -> bool {
    t.flicks() < 0 || t.flicks() >= target.duration
}

// ----- interpolation inputs --------------------------------------------------------------------

/// The interpolation a command asks for: `interp` (stored form) or `easing` (a preset name),
/// never both; `preset` is the hint stored with a bezier or spring.
pub(crate) fn resolve_interp(
    pointer: &str,
    interp: Option<&InterpInput>,
    easing: Option<Easing>,
    preset: Option<&str>,
) -> Result<Option<Interp>, CmdError> {
    let mut out = match (interp, easing) {
        (Some(_), Some(_)) => {
            return Err(CmdError::invalid(
                pointer,
                "give either `interp` or `easing`, not both",
            ));
        }
        (Some(i), None) => Some(i.clone().into_interp()),
        (None, Some(e)) => Some(e.interp()),
        (None, None) => None,
    };
    if let Some(name) = preset {
        match &mut out {
            Some(Interp::Bezier { preset, .. } | Interp::Spring { preset, .. }) => {
                *preset = Some(name.to_owned());
            }
            _ => {
                return Err(CmdError::invalid(
                    format!("{pointer}/preset"),
                    "a preset hint goes with a bezier or spring `interp` or an `easing` that is one",
                ));
            }
        }
    }
    Ok(out)
}

// ----- setting keys ----------------------------------------------------------------------------

/// A key to write: its snapped item time, its value (`None`: the current value there) and its
/// interpolation (`None`: keep, inherit or default, keyframes §9.2).
#[derive(Clone, Debug)]
pub(crate) struct KeyReq {
    pub(crate) t: Time,
    pub(crate) value: Option<KeyValue>,
    pub(crate) interp: Option<Interp>,
}

/// What [`set_keys`] did to one key.
#[derive(Clone, Debug)]
pub(crate) struct KeySet {
    pub(crate) t: Time,
    pub(crate) value: KeyValue,
    pub(crate) created: bool,
}

/// Writes `keys` to property `id` of `clip` (keyframes §9.2): see
/// [`Animatable::set_key`]. `pointer(i)` is where key `i` sits in the request params. Returns
/// what happened to each key, in order, and whether the property changed.
pub(crate) fn set_keys(
    clip: &mut Clip,
    id: PropId,
    keys: &[KeyReq],
    replace: bool,
    pointer: &dyn Fn(usize) -> String,
) -> Result<(Vec<KeySet>, bool), CmdError> {
    let spec = id.spec();
    on_prop_mut!(clip, id, a => set_keys_on(a, spec, keys, replace, pointer))
}

fn set_keys_on<T: Wire>(
    a: &mut Animatable<T>,
    spec: &PropSpec,
    keys: &[KeyReq],
    replace: bool,
    pointer: &dyn Fn(usize) -> String,
) -> Result<(Vec<KeySet>, bool), CmdError> {
    let original = a.clone();
    let mut cur = if replace {
        Animatable::Static(original.value().clone())
    } else {
        original.clone()
    };
    let mut seen = BTreeSet::new();
    let mut created = Vec::with_capacity(keys.len());
    for (i, key) in keys.iter().enumerate() {
        if !seen.insert(key.t) {
            return Err(CmdError::invalid(
                pointer(i),
                format!(
                    "two keys land on the same frame (t = {}); give each key its own time",
                    key.t.flicks()
                ),
            ));
        }
        let value = match &key.value {
            Some(v) => Some(T::pick(v).ok_or_else(|| {
                CmdError::invalid(
                    format!("{}/value", pointer(i)),
                    format!("`{}` takes {}", spec.id, type_hint(spec.value_type)),
                )
            })?),
            // On `replace` the current value comes from the animation being replaced.
            None if replace => Some(T::from_eval(&original.eval_clamped(key.t, spec.clamp))),
            None => None,
        };
        created.push(
            original
                .keyframes()
                .is_none_or(|k| k.index_of(key.t).is_none()),
        );
        cur = cur
            .set_key(key.t, value, key.interp.clone(), spec.stored, spec.clamp)
            .map_err(|e| map_error(e, &pointer(i), ""))?;
    }
    let out = keys
        .iter()
        .zip(created)
        .map(|(key, created)| KeySet {
            t: key.t,
            value: cur
                .keyframes()
                .and_then(|k| k.index_of(key.t).map(|i| k.keys()[i].value.wrap()))
                .unwrap_or_else(|| cur.value().wrap()),
            created,
        })
        .collect();
    let changed = cur != original;
    if changed {
        *a = cur;
    }
    Ok((out, changed))
}

/// What a property takes, for messages.
fn type_hint(t: ValueType) -> &'static str {
    match t {
        ValueType::Vec2 => "a vector `{x, y}`",
        ValueType::Bool => "a boolean",
        _ => "a number",
    }
}

/// A value as the property's type, or `Invalid` at `pointer`.
pub(crate) fn check_value_type(
    pointer: &str,
    id: PropId,
    value: &KeyValue,
) -> Result<(), CmdError> {
    let ok = match (id.spec().value_type, value) {
        (ValueType::Vec2, KeyValue::Vec2(_)) | (ValueType::Bool, KeyValue::Bool(_)) => true,
        (ValueType::Scalar | ValueType::Angle, KeyValue::Number(_)) => true,
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(CmdError::invalid(
            pointer,
            format!("`{id}` takes {}", type_hint(id.spec().value_type)),
        ))
    }
}

/// Applies a retiming rule to a clip inside a placement edit's clone of the item.
pub(crate) fn rekey(clip: &mut Clip, rule: retime::Retime) -> Result<(), CmdError> {
    retime::retime_clip(clip, rule).map_err(|e| retime_error(&e))
}
