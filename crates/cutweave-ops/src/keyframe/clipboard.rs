// SPDX-License-Identifier: GPL-3.0-or-later
//! `keyframe.copy` and `keyframe.paste` (keyframes §9.8): the clipboard payload and the
//! commands that fill and read it.

use cutweave_commands::{CmdError, Command, Query, QueryCtx, Tx, command, query, wire};
use cutweave_model::{Clip, Interp, ItemId, Project, PropId, Time, TimeInput};

use super::{KeyRef, KeySet};
use super::{
    KeyReq, KeyValue, MAX_KEYS_PER_CALL, Target, TimeArg, Wire, check_fine, check_value_type,
    on_prop, outside, sequence_time, set_keys, snap_to_item_time, targets,
};

/// One copied key.
#[wire]
pub struct ClipboardKey {
    /// The property it was on.
    pub prop: PropId,
    /// Flicks after the earliest copied key.
    pub dt: Time,
    /// The value.
    pub value: KeyValue,
    /// How the segment that starts at the key moves to the next one (stored form).
    pub interp: Interp,
}

/// Copied keys, as `application/x-cutweave-keyframes+json` on the system clipboard.
#[wire]
pub struct KeyframeClipboard {
    /// Always 1.
    pub version: u32,
    /// The keys, by property then time.
    pub keys: Vec<ClipboardKey>,
}

/// Result of `keyframe.copy`.
#[wire]
pub struct KeyframeCopyOutput {
    /// The payload to paste.
    pub clipboard: KeyframeClipboard,
}

/// Reads keys into a clipboard payload (nothing changes). `NotFound` if a key does not exist.
#[query(
    name = "keyframe.copy",
    summary = "Copy keys of an item into a clipboard payload",
    mcp = "catalog"
)]
pub struct KeyframeCopy {
    /// The item.
    pub item: ItemId,
    /// The keys, by property and exact item time.
    pub keys: Vec<KeyRef>,
}

impl Query for KeyframeCopy {
    type Output = KeyframeCopyOutput;

    fn run(&self, ctx: &QueryCtx<'_>) -> Result<KeyframeCopyOutput, CmdError> {
        if self.keys.is_empty() || self.keys.len() > MAX_KEYS_PER_CALL {
            return Err(CmdError::invalid(
                "/keys",
                format!("give 1 to {MAX_KEYS_PER_CALL} keys"),
            ));
        }
        let at = ctx.item(self.item)?;
        let clip = at
            .item
            .as_clip()
            .ok_or_else(|| CmdError::unsupported("only clips have animatable properties"))?;
        let mut keys = Vec::with_capacity(self.keys.len());
        for k in &self.keys {
            let found = on_prop!(clip, k.prop, a => a.keyframes().and_then(|ks| {
                ks.index_of(k.t).map(|i| {
                    let key = &ks.keys()[i];
                    (key.value.wrap(), key.interp.clone())
                })
            }));
            let (value, interp) = found.ok_or_else(|| {
                CmdError::not_found(
                    cutweave_commands::ResourceKind::Keyframe,
                    format!("{} {} at t = {}", self.item, k.prop, k.t.flicks()),
                )
            })?;
            keys.push((k.prop, k.t, value, interp));
        }
        keys.sort_by_key(|(p, t, _, _)| (*p, *t));
        keys.dedup_by_key(|(p, t, _, _)| (*p, *t));
        let first = keys
            .iter()
            .map(|(_, t, _, _)| t.flicks())
            .min()
            .unwrap_or(0);
        let keys = keys
            .into_iter()
            .map(|(prop, t, value, interp)| {
                Ok(ClipboardKey {
                    prop,
                    dt: Time::from_flicks(t.flicks() - first)
                        .map_err(|e| CmdError::invalid("/keys", e.to_string()))?,
                    value,
                    interp,
                })
            })
            .collect::<Result<Vec<_>, CmdError>>()?;
        Ok(KeyframeCopyOutput {
            clipboard: KeyframeClipboard { version: 1, keys },
        })
    }
}

/// Where `keyframe.paste` put a key.
#[wire]
pub struct PastedKey {
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
    /// A new key (`false`: it replaced a key at that time).
    pub created: bool,
    /// The key lies outside the item's `[0, duration)`.
    pub outside: bool,
}

/// Result of `keyframe.paste`.
#[wire]
pub struct KeyframePasteOutput {
    /// The keys written.
    pub keys: Vec<PastedKey>,
}

/// Pastes copied keys onto an item (or each of `items`): the earliest key goes to `t` (item
/// time) or `at` (sequence time), the others keep their distance (`dt`), each snapped to
/// frames; a key already at a time is replaced. `as_prop` pastes a single-property payload onto
/// another property of the same type (crop left keys onto crop right). Values must fit the
/// property; `Locked` if an item's track is locked.
#[command(
    name = "keyframe.paste",
    summary = "Paste copied keys onto an item at a time",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct KeyframePaste {
    /// The item (or `items`).
    pub item: Option<ItemId>,
    /// Items to paste onto.
    pub items: Option<Vec<ItemId>>,
    /// What `keyframe.copy` returned.
    pub payload: KeyframeClipboard,
    /// Item time of the earliest key.
    pub t: Option<TimeInput>,
    /// Sequence time of the earliest key.
    pub at: Option<TimeInput>,
    /// Paste a single-property payload onto this property instead.
    pub as_prop: Option<PropId>,
    /// Snap to the audio sample grid (audio properties only).
    pub fine: Option<bool>,
}

type Grouped = Vec<(PropId, Vec<(KeyReq, usize)>)>;

impl KeyframePaste {
    /// The keys per property for one item: time resolved and snapped, values checked.
    fn plan_item(&self, target: &Target) -> Result<Grouped, CmdError> {
        let payload = &self.payload;
        if payload.version != 1 {
            return Err(CmdError::invalid(
                "/payload/version",
                "only clipboard version 1 is known",
            ));
        }
        if payload.keys.is_empty() || payload.keys.len() > MAX_KEYS_PER_CALL {
            return Err(CmdError::invalid(
                "/payload/keys",
                format!("the payload needs 1 to {MAX_KEYS_PER_CALL} keys"),
            ));
        }
        let fine = self.fine.unwrap_or(false);
        for k in &payload.keys {
            check_fine(self.as_prop.unwrap_or(k.prop), fine)?;
        }
        let origin = TimeArg {
            t: self.t,
            at: self.at,
            pointer: "",
        };
        let as_prop = match self.as_prop {
            Some(p) => {
                let first = payload.keys[0].prop;
                if payload.keys.iter().any(|k| k.prop != first) {
                    return Err(CmdError::invalid(
                        "/as_prop",
                        "`as_prop` needs a payload of one property",
                    ));
                }
                if p.spec().value_type != first.spec().value_type {
                    return Err(CmdError::invalid(
                        "/as_prop",
                        format!("`{first}` and `{p}` are not the same type"),
                    ));
                }
                Some(p)
            }
            None => None,
        };
        // The earliest key lands on the origin; the others keep their distance.
        let base = origin.item_time(target, as_prop.or(Some(payload.keys[0].prop)), fine)?;
        let mut grouped: Grouped = Vec::new();
        for (i, k) in payload.keys.iter().enumerate() {
            let prop = as_prop.unwrap_or(k.prop);
            check_value_type(&format!("/payload/keys/{i}/value"), prop, &k.value)?;
            let want = target
                .start
                .flicks()
                .checked_add(base.flicks())
                .and_then(|v| v.checked_add(k.dt.flicks()))
                .ok_or_else(|| CmdError::invalid("/payload", "a key leaves the time range"))?;
            let t = snap_to_item_time(&format!("/payload/keys/{i}"), want, target, fine)?;
            let req = KeyReq {
                t,
                value: Some(k.value.clone()),
                interp: Some(k.interp.clone()),
            };
            match grouped.iter_mut().find(|(p, _)| *p == prop) {
                Some((_, v)) => v.push((req, i)),
                None => grouped.push((prop, vec![(req, i)])),
            }
        }
        Ok(grouped)
    }

    fn run(&self, clip: &mut Clip, target: &Target) -> Result<Vec<PastedKey>, CmdError> {
        let mut out = Vec::new();
        for (prop, reqs) in self.plan_item(target)? {
            let (list, idx): (Vec<KeyReq>, Vec<usize>) = reqs.into_iter().unzip();
            let (set, _) = set_keys(clip, prop, &list, false, &|n| {
                format!("/payload/keys/{}", idx[n])
            })?;
            out.extend(
                set.into_iter()
                    .map(|KeySet { t, value, created }| PastedKey {
                        item: target.id,
                        prop,
                        t,
                        at: sequence_time(target, t),
                        value,
                        created,
                        outside: outside(target, t),
                    }),
            );
        }
        Ok(out)
    }
}

fn clip_of(doc: &Project, id: ItemId) -> Result<&Clip, CmdError> {
    doc.find_item(&id)
        .and_then(|at| at.item.as_clip())
        .ok_or_else(|| CmdError::unsupported("only clips have animatable properties"))
}

impl Command for KeyframePaste {
    type Output = KeyframePasteOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        for target in targets(ctx.doc(), self.item, self.items.as_deref())? {
            let mut clip = clip_of(ctx.doc(), target.id)?.clone();
            self.run(&mut clip, &target)?;
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<KeyframePasteOutput, CmdError> {
        let all = targets(tx.doc(), self.item, self.items.as_deref())?;
        let mut keys = Vec::new();
        for target in all {
            let mut done = Vec::new();
            tx.update_item(target.id, |item| {
                let clip = item.as_clip_mut().ok_or_else(|| {
                    CmdError::unsupported("only clips have animatable properties")
                })?;
                done = self.run(clip, &target)?;
                Ok(())
            })?;
            keys.extend(done);
        }
        Ok(KeyframePasteOutput { keys })
    }
}
