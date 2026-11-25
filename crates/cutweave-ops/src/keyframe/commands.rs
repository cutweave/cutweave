// SPDX-License-Identifier: GPL-3.0-or-later
//! `keyframe.set`, `keyframe.delete`, `keyframe.move`, `keyframe.set_interp` and
//! `keyframe.clear` (keyframes §9.2, §9.4 to §9.7).

use std::collections::BTreeMap;

use cutweave_commands::{CmdError, Command, QueryCtx, Tx, command, wire};
use cutweave_model::{Clip, Easing, ItemId, Project, PropId, Rounding, Time, TimeInput};

use super::{
    InterpInput, KeyRef, KeyReq, KeyValue, MAX_KEYS_PER_CALL, Target, TimeArg, Wire, check_fine,
    check_value_type, map_error, on_prop, on_prop_mut, one_target, outside, resolve_interp,
    sequence_time, set_keys, targets,
};

fn update(
    tx: &mut Tx<'_>,
    id: ItemId,
    f: impl FnOnce(&mut Clip) -> Result<(), CmdError>,
) -> Result<(), CmdError> {
    tx.update_item(id, |item| {
        let clip = item
            .as_clip_mut()
            .ok_or_else(|| CmdError::unsupported("only clips have animatable properties"))?;
        f(clip)
    })
}

fn clip_of(doc: &Project, id: ItemId) -> Result<&Clip, CmdError> {
    doc.find_item(&id)
        .and_then(|at| at.item.as_clip())
        .ok_or_else(|| CmdError::unsupported("only clips have animatable properties"))
}

// ----- keyframe.set ----------------------------------------------------------------------------

/// A key to write.
#[wire]
pub struct KeyInput {
    /// Item time (flicks, seconds like "1.5s", or "f:N" frames from the item's start).
    pub t: Option<TimeInput>,
    /// Sequence time. Give `t` or `at`; both snap to frames.
    pub at: Option<TimeInput>,
    /// The value; omitted: the property's current value there.
    pub value: Option<KeyValue>,
    /// Shapes the move to the next key; or use `easing`.
    pub interp: Option<InterpInput>,
    /// A preset name, `linear` or `hold`.
    pub easing: Option<Easing>,
    /// Preset name hint for `interp`.
    pub preset: Option<String>,
}

/// One key `keyframe.set` wrote.
#[wire]
pub struct SetKey {
    /// The item.
    pub item: ItemId,
    /// The property.
    pub prop: PropId,
    /// Item time after snapping.
    pub t: Time,
    /// Sequence time after snapping.
    pub at: Time,
    /// The value the key holds.
    pub value: KeyValue,
    /// A new key (`false`: an existing key's value changed).
    pub created: bool,
    /// The key lies outside the item's `[0, duration)`.
    pub outside: bool,
}

/// Result of `keyframe.set`.
#[wire]
pub struct KeyframeSetOutput {
    /// The keys written, per item and key in request order.
    pub keys: Vec<SetKey>,
}

/// Add or replace keys on one property. `t` = item time, `at` = sequence time; both snap to
/// frames. A key's `easing` shapes the move to the next key. Example: `{item, prop:"opacity",
/// keys:[{at:"10s", value:0, easing:"ease_out"}, {at:"12s", value:1}]}`.
///
/// A static property becomes animated with exactly these keys; a key at an occupied time is
/// replaced; a new key inherits the previous key's easing; `replace` makes the result exactly
/// these keys; `fine` snaps audio properties to samples. `Locked` if the track is locked.
#[command(
    name = "keyframe.set",
    summary = "Add or replace keys on one property of an item",
    undoable,
    mcp = "core",
    annotations(destructive = false, idempotent = true)
)]
pub struct KeyframeSet {
    /// The item (or `items`).
    pub item: Option<ItemId>,
    /// Items to give the same keys.
    pub items: Option<Vec<ItemId>>,
    /// The property.
    pub prop: PropId,
    /// The keys, 1 to 10,000.
    pub keys: Vec<KeyInput>,
    /// Make the result exactly these keys.
    pub replace: Option<bool>,
    /// Snap to audio samples (audio properties).
    pub fine: Option<bool>,
}

struct SetPlan {
    per_item: Vec<(Target, Vec<KeyReq>)>,
}

impl KeyframeSet {
    fn plan(&self, doc: &Project) -> Result<SetPlan, CmdError> {
        if self.keys.is_empty() || self.keys.len() > MAX_KEYS_PER_CALL {
            return Err(CmdError::invalid(
                "/keys",
                format!("give 1 to {MAX_KEYS_PER_CALL} keys"),
            ));
        }
        let fine = self.fine.unwrap_or(false);
        check_fine(self.prop, fine)?;
        let mut per_item = Vec::new();
        for target in targets(doc, self.item, self.items.as_deref())? {
            let mut reqs = Vec::with_capacity(self.keys.len());
            for (i, k) in self.keys.iter().enumerate() {
                let pointer = format!("/keys/{i}");
                if let Some(v) = &k.value {
                    check_value_type(&format!("{pointer}/value"), self.prop, v)?;
                }
                let t = TimeArg {
                    t: k.t,
                    at: k.at,
                    pointer: &pointer,
                }
                .item_time(&target, Some(self.prop), fine)?;
                let interp =
                    resolve_interp(&pointer, k.interp.as_ref(), k.easing, k.preset.as_deref())?;
                reqs.push(KeyReq {
                    t,
                    value: k.value.clone(),
                    interp,
                });
            }
            per_item.push((target, reqs));
        }
        Ok(SetPlan { per_item })
    }
}

impl Command for KeyframeSet {
    type Output = KeyframeSetOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        let plan = self.plan(ctx.doc())?;
        for (target, reqs) in &plan.per_item {
            let mut clip = clip_of(ctx.doc(), target.id)?.clone();
            set_keys(
                &mut clip,
                self.prop,
                reqs,
                self.replace.unwrap_or(false),
                &|i| format!("/keys/{i}"),
            )?;
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<KeyframeSetOutput, CmdError> {
        let plan = self.plan(tx.doc())?;
        let mut keys = Vec::new();
        for (target, reqs) in plan.per_item {
            let mut done = Vec::new();
            update(tx, target.id, |clip| {
                let (set, _) = set_keys(
                    clip,
                    self.prop,
                    &reqs,
                    self.replace.unwrap_or(false),
                    &|i| format!("/keys/{i}"),
                )?;
                done = set;
                Ok(())
            })?;
            keys.extend(done.into_iter().map(|k| SetKey {
                item: target.id,
                prop: self.prop,
                t: k.t,
                at: sequence_time(&target, k.t),
                value: k.value,
                created: k.created,
                outside: outside(&target, k.t),
            }));
        }
        Ok(KeyframeSetOutput { keys })
    }
}

// ----- keyframe.delete -------------------------------------------------------------------------

/// Result of `keyframe.delete`.
#[wire]
pub struct KeyframeDeleteOutput {
    /// The keys removed.
    pub deleted: Vec<KeyRef>,
    /// Properties that lost their last key and are static now (at their first key's value).
    pub now_static: Vec<PropId>,
}

/// Removes the keys at the exact item times given. Deleting every key of a property makes it
/// static at its first key's value. `NotFound` if a key does not exist; `Locked` if the item's
/// track is locked.
#[command(
    name = "keyframe.delete",
    summary = "Delete keys of an item's properties",
    undoable,
    mcp = "catalog",
    annotations(destructive = true, idempotent = false)
)]
pub struct KeyframeDelete {
    /// The item.
    pub item: ItemId,
    /// The keys to delete, by property and exact item time.
    pub keys: Vec<KeyRef>,
}

/// The keys grouped by property, in property order; each time once.
fn by_prop(keys: &[KeyRef]) -> BTreeMap<PropId, Vec<Time>> {
    let mut out: BTreeMap<PropId, Vec<Time>> = BTreeMap::new();
    for k in keys {
        out.entry(k.prop).or_default().push(k.t);
    }
    out
}

fn check_keys(keys: &[KeyRef]) -> Result<(), CmdError> {
    if keys.is_empty() || keys.len() > MAX_KEYS_PER_CALL {
        return Err(CmdError::invalid(
            "/keys",
            format!("give 1 to {MAX_KEYS_PER_CALL} keys"),
        ));
    }
    Ok(())
}

fn missing(item: ItemId, prop: PropId, t: Time) -> CmdError {
    CmdError::not_found(
        cutweave_commands::ResourceKind::Keyframe,
        format!("{item} {prop} at t = {}", t.flicks()),
    )
}

/// `NotFound` for the first addressed key that does not exist on the clip.
fn check_exist(clip: &Clip, item: ItemId, keys: &[KeyRef]) -> Result<(), CmdError> {
    for k in keys {
        let found = on_prop!(clip, k.prop, a => a
            .keyframes()
            .is_some_and(|ks| ks.index_of(k.t).is_some()));
        if !found {
            return Err(missing(item, k.prop, k.t));
        }
    }
    Ok(())
}

impl KeyframeDelete {
    fn run(&self, clip: &mut Clip) -> Result<KeyframeDeleteOutput, CmdError> {
        check_keys(&self.keys)?;
        check_exist(clip, self.item, &self.keys)?;
        let mut now_static = Vec::new();
        for (prop, ts) in by_prop(&self.keys) {
            let was_static = on_prop_mut!(clip, prop, a => {
                let next = a
                    .delete_keys(&ts)
                    .map_err(|e| map_error(e, "/keys", &prop.to_string()))?;
                let now = !next.is_animated();
                *a = next;
                now
            });
            if was_static {
                now_static.push(prop);
            }
        }
        let mut deleted = self.keys.clone();
        deleted.sort_by_key(|k| (k.prop, k.t));
        deleted.dedup();
        Ok(KeyframeDeleteOutput {
            deleted,
            now_static,
        })
    }
}

impl Command for KeyframeDelete {
    type Output = KeyframeDeleteOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        one_target(ctx.doc(), self.item)?;
        let mut clip = clip_of(ctx.doc(), self.item)?.clone();
        self.run(&mut clip).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<KeyframeDeleteOutput, CmdError> {
        one_target(tx.doc(), self.item)?;
        let mut out = None;
        update(tx, self.item, |clip| {
            out = Some(self.run(clip)?);
            Ok(())
        })?;
        out.ok_or_else(|| CmdError::internal("keyframe.delete produced nothing"))
    }
}

// ----- keyframe.move ---------------------------------------------------------------------------

/// One key that moved.
#[wire]
pub struct MovedKey {
    /// The property.
    pub prop: PropId,
    /// Where the key was (item time).
    pub from_t: Time,
    /// Where it is now (item time).
    pub t: Time,
    /// Where it is now (sequence time).
    pub at: Time,
    /// The key lies outside the item's `[0, duration)`.
    pub outside: bool,
}

/// Result of `keyframe.move`.
#[wire]
pub struct KeyframeMoveOutput {
    /// The keys moved.
    pub moved: Vec<MovedKey>,
}

/// Moves keys in time, by `delta`, or to `to_t` (item time) or `to_at` (sequence time) for a
/// single key; give exactly one. Keys keep their value and interpolation, may pass their
/// neighbours (the list re-sorts), and land on frames: a `delta` is rounded to whole frames
/// (`fine`: samples). Landing on a key that stays, or two keys on one time, is `Invalid`.
/// `NotFound` if a key does not exist; `Locked` if the item's track is locked.
#[command(
    name = "keyframe.move",
    summary = "Move keys of an item's properties in time",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct KeyframeMove {
    /// The item.
    pub item: ItemId,
    /// The keys to move, by property and exact item time.
    pub keys: Vec<KeyRef>,
    /// How far to move them (negative: earlier). Use seconds or flicks for negative values.
    pub delta: Option<TimeInput>,
    /// The new item time of the single key.
    pub to_t: Option<TimeInput>,
    /// The new sequence time of the single key.
    pub to_at: Option<TimeInput>,
    /// Snap to the audio sample grid (audio properties only).
    pub fine: Option<bool>,
}

impl KeyframeMove {
    /// The shared delta per property, in flicks.
    fn deltas(&self, target: &Target) -> Result<BTreeMap<PropId, Time>, CmdError> {
        check_keys(&self.keys)?;
        let fine = self.fine.unwrap_or(false);
        let given = [
            self.delta.is_some(),
            self.to_t.is_some(),
            self.to_at.is_some(),
        ]
        .into_iter()
        .filter(|g| *g)
        .count();
        if given != 1 {
            return Err(CmdError::invalid(
                "",
                "give exactly one of `delta`, `to_t` and `to_at`",
            ));
        }
        let props = by_prop(&self.keys);
        for prop in props.keys() {
            check_fine(*prop, fine)?;
        }
        let mut out = BTreeMap::new();
        for (prop, ts) in &props {
            let delta = if let Some(d) = self.delta {
                let rel = d
                    .resolve(Some(target.rate))
                    .map_err(|e| CmdError::invalid("/delta", e.to_string()))?;
                let rel = round_delta(rel.flicks(), target, fine, "/delta")?;
                Time::from_flicks(rel).map_err(|e| CmdError::invalid("/delta", e.to_string()))?
            } else {
                if self.keys.len() != 1 {
                    return Err(CmdError::invalid(
                        "/keys",
                        "`to_t` and `to_at` move a single key: give exactly one key",
                    ));
                }
                let from = ts[0];
                let (t, at) = (self.to_t, self.to_at);
                let to = TimeArg { t, at, pointer: "" }
                    .item_time(target, Some(*prop), fine)
                    .map_err(|e| match (t.is_some(), e) {
                        (true, CmdError::Invalid { msg, .. }) => CmdError::invalid("/to_t", msg),
                        (false, CmdError::Invalid { msg, .. }) => CmdError::invalid("/to_at", msg),
                        (_, other) => other,
                    })?;
                Time::from_flicks(to.flicks() - from.flicks())
                    .map_err(|e| CmdError::invalid("/to_t", e.to_string()))?
            };
            out.insert(*prop, delta);
        }
        Ok(out)
    }

    fn run(&self, clip: &mut Clip, target: &Target) -> Result<KeyframeMoveOutput, CmdError> {
        let deltas = self.deltas(target)?;
        check_exist(clip, self.item, &self.keys)?;
        let mut moved = Vec::new();
        for (prop, ts) in by_prop(&self.keys) {
            let delta = deltas[&prop];
            on_prop_mut!(clip, prop, a => {
                let next = a
                    .move_keys(&ts, delta)
                    .map_err(|e| map_error(e, "/keys", &prop.to_string()))?;
                *a = next;
            });
            let mut seen = Vec::new();
            for from in ts {
                if seen.contains(&from) {
                    continue;
                }
                seen.push(from);
                let t = Time::from_flicks(from.flicks() + delta.flicks())
                    .map_err(|e| CmdError::invalid("/keys", e.to_string()))?;
                moved.push(MovedKey {
                    prop,
                    from_t: from,
                    t,
                    at: sequence_time(target, t),
                    outside: outside(target, t),
                });
            }
        }
        Ok(KeyframeMoveOutput { moved })
    }
}

/// A delta rounded to whole frames (or samples, `fine`), ties away from zero.
fn round_delta(delta: i64, target: &Target, fine: bool, pointer: &str) -> Result<i64, CmdError> {
    let sign = delta.signum();
    let mag = Time::from_flicks(delta.saturating_abs())
        .map_err(|e| CmdError::invalid(pointer, e.to_string()))?;
    let snapped = if fine {
        let n = target.samples.time_to_samples(mag, Rounding::Nearest);
        target.samples.samples_to_time(n)
    } else {
        target.rate.snap_to_frame(mag, Rounding::Nearest)
    }
    .map_err(|e| CmdError::invalid(pointer, e.to_string()))?;
    Ok(sign * snapped.flicks())
}

impl Command for KeyframeMove {
    type Output = KeyframeMoveOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        let target = one_target(ctx.doc(), self.item)?;
        let mut clip = clip_of(ctx.doc(), self.item)?.clone();
        self.run(&mut clip, &target).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<KeyframeMoveOutput, CmdError> {
        let target = one_target(tx.doc(), self.item)?;
        let mut out = None;
        update(tx, self.item, |clip| {
            out = Some(self.run(clip, &target)?);
            Ok(())
        })?;
        out.ok_or_else(|| CmdError::internal("keyframe.move produced nothing"))
    }
}

// ----- keyframe.set_interp ---------------------------------------------------------------------

/// Result of `keyframe.set_interp`.
#[wire]
pub struct KeyframeSetInterpOutput {
    /// The keys whose segment changed.
    pub changed: Vec<KeyRef>,
}

/// Sets how the segments that start at the given keys move to the next key: `interp` (stored
/// form) or `easing` (a preset name, `linear` or `hold`), not both. An explicit `interp`
/// carries no preset hint unless it gives one. Flips accept only hold. `NotFound` if a key does
/// not exist; `Locked` if the item's track is locked.
#[command(
    name = "keyframe.set_interp",
    summary = "Set the easing of the segments starting at keys",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct KeyframeSetInterp {
    /// The item.
    pub item: ItemId,
    /// The keys, by property and exact item time.
    pub keys: Vec<KeyRef>,
    /// The interpolation (stored form).
    pub interp: Option<InterpInput>,
    /// A preset name, `linear` or `hold`.
    pub easing: Option<Easing>,
}

impl KeyframeSetInterp {
    fn run(&self, clip: &mut Clip) -> Result<KeyframeSetInterpOutput, CmdError> {
        check_keys(&self.keys)?;
        let interp = resolve_interp("", self.interp.as_ref(), self.easing, None)?
            .ok_or_else(|| CmdError::invalid("", "give `interp` or `easing`"))?;
        check_exist(clip, self.item, &self.keys)?;
        for (prop, ts) in by_prop(&self.keys) {
            on_prop_mut!(clip, prop, a => {
                let next = a
                    .set_interps(&ts, &interp)
                    .map_err(|e| map_error(e, "", &prop.to_string()))?;
                *a = next;
            });
        }
        let mut changed = self.keys.clone();
        changed.sort_by_key(|k| (k.prop, k.t));
        changed.dedup();
        Ok(KeyframeSetInterpOutput { changed })
    }
}

impl Command for KeyframeSetInterp {
    type Output = KeyframeSetInterpOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        one_target(ctx.doc(), self.item)?;
        let mut clip = clip_of(ctx.doc(), self.item)?.clone();
        self.run(&mut clip).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<KeyframeSetInterpOutput, CmdError> {
        one_target(tx.doc(), self.item)?;
        let mut out = None;
        update(tx, self.item, |clip| {
            out = Some(self.run(clip)?);
            Ok(())
        })?;
        out.ok_or_else(|| CmdError::internal("keyframe.set_interp produced nothing"))
    }
}

// ----- keyframe.clear --------------------------------------------------------------------------

/// Result of `keyframe.clear`.
#[wire]
pub struct KeyframeClearOutput {
    /// The static value the (first) item's property has now.
    pub value: KeyValue,
    /// The items processed.
    pub items: Vec<ItemId>,
}

/// Makes an animated property static: at its value at the given time (`t` item time, `at`
/// sequence time), or at its first key's value without one. Does nothing to a property that is
/// already static. `Locked` if an item's track is locked.
#[command(
    name = "keyframe.clear",
    summary = "Remove all keys of a property, keeping one value",
    undoable,
    mcp = "catalog",
    annotations(destructive = true, idempotent = true)
)]
pub struct KeyframeClear {
    /// The item (or `items`).
    pub item: Option<ItemId>,
    /// Items to clear the same property of.
    pub items: Option<Vec<ItemId>>,
    /// The property.
    pub prop: PropId,
    /// Keep the value at this item time.
    pub t: Option<TimeInput>,
    /// Keep the value at this sequence time.
    pub at: Option<TimeInput>,
}

impl KeyframeClear {
    fn time(&self, target: &Target) -> Result<Option<Time>, CmdError> {
        match (self.t, self.at) {
            (None, None) => Ok(None),
            (t, at) => TimeArg { t, at, pointer: "" }
                .item_time(target, None, false)
                .map(Some),
        }
    }

    fn run(&self, clip: &mut Clip, t: Option<Time>) -> KeyValue {
        let spec = self.prop.spec();
        on_prop_mut!(clip, self.prop, a => {
            let next = a.cleared(t, spec.clamp);
            let value = next.value().wrap();
            *a = next;
            value
        })
    }
}

impl Command for KeyframeClear {
    type Output = KeyframeClearOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        for target in targets(ctx.doc(), self.item, self.items.as_deref())? {
            self.time(&target)?;
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<KeyframeClearOutput, CmdError> {
        let all = targets(tx.doc(), self.item, self.items.as_deref())?;
        let mut value = None;
        let mut items = Vec::new();
        for target in all {
            let t = self.time(&target)?;
            let mut v = None;
            update(tx, target.id, |clip| {
                v = Some(self.run(clip, t));
                Ok(())
            })?;
            value = value.or(v);
            items.push(target.id);
        }
        let value = value.ok_or_else(|| CmdError::internal("keyframe.clear cleared nothing"))?;
        Ok(KeyframeClearOutput { value, items })
    }
}
