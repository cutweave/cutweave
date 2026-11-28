// SPDX-License-Identifier: GPL-3.0-or-later
//! The `audio` namespace (WP-2E.01; arch §9, §5.1): per-clip fades, volume and pan.
//!
//! * [`AudioSet`] (`audio.set`, the one core MCP tool of the namespace): a discriminated union
//!   over `kind: fade | volume | pan`.
//! * [`AudioSetFade`], [`AudioSetVolume`], [`AudioSetPan`]: the same three changes as commands of
//!   their own, for the UI and the CLI. Volume and pan are thin wrappers over `item.set_props`
//!   (the same data: `audio.gain` and `audio.pan`, keyframed, auto-keyed at `at`).
//! * [`AudioSetGain`] (`audio.set_gain`): one flat gain for the whole clip; keys are removed.
//! * [`GetLevels`] (`audio.get_levels`, WP-2E.02): peak, true peak, RMS and loudness of a mix
//!   over a range, measured by the engine ([`levels`]).
//!
//! Fades are stored on the clip as durations and curves (`audio.fade_in`, `audio.fade_out`).
//! They are measured on the clip as placed, so trimming keeps their length. The commands keep
//! `fade_in + fade_out <= duration` by cutting the fade being set to the room the other one
//! leaves; the mixer also scales them down if a later trim makes the clip shorter than both.

use cutweave_commands::{CmdError, Command, QueryCtx, Registry, RegistryError, Tx, command, wire};
use cutweave_model::{Clip, Duration, Fade, FadeCurve, ItemId, Project, PropId, TimeInput};

pub mod levels;

pub use levels::{
    GetLevels, GetLevelsOutput, LevelReading, LevelsAnalyzer, LevelsCtx, LevelsMeasured,
    LevelsRequest, TrackLevels,
};

use crate::{
    item::{ItemProps, ItemSetProps, ItemSetPropsOutput, KeyedProp},
    keyframe::{self, KeyValue, Target},
    util::resolve_time,
};

/// The loudest clip volume the audio commands accept, in dB (the property itself allows more).
pub const MAX_VOLUME_DB: f64 = 12.0;
/// The quietest clip volume in dB; anything at or below is silence.
pub const MIN_VOLUME_DB: f64 = -144.0;

/// Registers the `audio.*` commands.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<AudioSet>()?
        .register::<AudioSetFade>()?
        .register::<AudioSetVolume>()?
        .register::<AudioSetPan>()?
        .register::<AudioSetGain>()?
        .register::<GetLevels>()?;
    Ok(())
}

// ----- the changes -----------------------------------------------------------------------------

/// A fade change: absent fields keep their value.
#[wire]
pub struct FadeChange {
    /// Length of the fade in, `0` removes it. Seconds (`"0.5s"`), flicks or a timecode.
    #[serde(rename = "in")]
    pub fade_in: Option<TimeInput>,
    /// Length of the fade out, `0` removes it.
    pub out: Option<TimeInput>,
    /// The shape of the fades being set (both when neither `in` nor `out` is given): `linear`
    /// (default), `log`, `equal_power`, `s_curve`.
    pub curve: Option<FadeCurve>,
}

/// What `audio.set` changes.
#[wire]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AudioChange {
    /// Fade in and/or out.
    Fade(FadeChange),
    /// Clip volume in dB (`-144..=12`; at or below `-144` is silence). Keyed at `at` when the
    /// volume is animated.
    Volume {
        /// The volume.
        db: f64,
    },
    /// Stereo position, `-1` (left) to `1` (right); centre is `0`. Keyed at `at` when animated.
    Pan {
        /// The position.
        pan: f64,
    },
}

/// Result of the audio commands.
#[wire]
pub struct AudioSetOutput {
    /// The first item.
    pub item: ItemId,
    /// What changed on any item (`audio.fade_in`, `audio.fade_out`, `audio.gain`, `audio.pan`);
    /// empty when everything was already as requested.
    pub changed: Vec<String>,
    /// The first item's fade in after the change (a fade longer than the room left is cut).
    pub fade_in: Fade,
    /// The first item's fade out after the change.
    pub fade_out: Fade,
    /// A requested fade was cut to fit the clip.
    pub clamped: bool,
    /// The keys written on animated properties.
    pub keyed: Vec<KeyedProp>,
}

/// Sets a clip's fades, volume or pan: `{item, set:{kind:"fade", in:"0.5s", out:"1s",
/// curve:"equal_power"}}`, `{item, set:{kind:"volume", db:-6}}` or `{item, set:{kind:"pan",
/// pan:-0.5}}`.
///
/// Volume and pan are set like `item_set_props` does: an animated property is keyed at `at`
/// (the playhead), a static one is set. A fade is cut to the room the other fade leaves in the
/// clip. `Locked` if the track is locked.
#[command(
    name = "audio.set",
    summary = "Set a clip's fades, volume (dB) or pan",
    undoable,
    mcp = "core",
    annotations(destructive = false, idempotent = true)
)]
pub struct AudioSet {
    /// The item (or `items`).
    pub item: Option<ItemId>,
    /// Items to change the same way.
    pub items: Option<Vec<ItemId>>,
    /// What to change.
    pub set: AudioChange,
    /// Key an animated volume or pan at `at` (default true).
    pub auto_key: Option<bool>,
    /// Sequence time (the playhead) to key at.
    pub at: Option<TimeInput>,
}

/// Fade in and out of a clip (`audio.set` with `kind: fade`).
#[command(
    name = "audio.set_fade",
    summary = "Set a clip's fade in and fade out (length and curve)",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct AudioSetFade {
    /// The item (or `items`).
    pub item: Option<ItemId>,
    /// Items to change the same way.
    pub items: Option<Vec<ItemId>>,
    /// Length of the fade in, `0` removes it.
    #[serde(rename = "in")]
    pub fade_in: Option<TimeInput>,
    /// Length of the fade out, `0` removes it.
    pub out: Option<TimeInput>,
    /// The shape of the fades being set.
    pub curve: Option<FadeCurve>,
}

/// Clip volume (`audio.set` with `kind: volume`; a wrapper over `item.set_props`).
#[command(
    name = "audio.set_volume",
    summary = "Set a clip's volume in dB (keyed at the playhead when animated)",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct AudioSetVolume {
    /// The item (or `items`).
    pub item: Option<ItemId>,
    /// Items to change the same way.
    pub items: Option<Vec<ItemId>>,
    /// The volume, `-144..=12` dB.
    pub db: f64,
    /// Key an animated volume at `at` (default true).
    pub auto_key: Option<bool>,
    /// Sequence time (the playhead) to key at.
    pub at: Option<TimeInput>,
}

/// Clip pan (`audio.set` with `kind: pan`; a wrapper over `item.set_props`).
#[command(
    name = "audio.set_pan",
    summary = "Set a clip's stereo pan (-1 left, 1 right; keyed at the playhead when animated)",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct AudioSetPan {
    /// The item (or `items`).
    pub item: Option<ItemId>,
    /// Items to change the same way.
    pub items: Option<Vec<ItemId>>,
    /// The position, `-1..=1`.
    pub pan: f64,
    /// Key an animated pan at `at` (default true).
    pub auto_key: Option<bool>,
    /// Sequence time (the playhead) to key at.
    pub at: Option<TimeInput>,
}

/// One flat gain for the whole clip: any volume keys are removed.
#[command(
    name = "audio.set_gain",
    summary = "Give a clip one flat gain in dB, removing its volume keys",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct AudioSetGain {
    /// The item (or `items`).
    pub item: Option<ItemId>,
    /// Items to change the same way.
    pub items: Option<Vec<ItemId>>,
    /// The gain, `-144..=12` dB.
    pub db: f64,
}

// ----- shared logic ----------------------------------------------------------------------------

fn check_db(pointer: &str, db: f64) -> Result<f64, CmdError> {
    if db.is_nan() || db > MAX_VOLUME_DB {
        return Err(CmdError::invalid(
            pointer,
            format!(
                "the volume must be at most {MAX_VOLUME_DB} dB (at or below {MIN_VOLUME_DB} is silence)"
            ),
        ));
    }
    Ok(db.max(MIN_VOLUME_DB))
}

fn check_pan(pointer: &str, pan: f64) -> Result<f64, CmdError> {
    if !(-1.0..=1.0).contains(&pan) {
        return Err(CmdError::invalid(
            pointer,
            "the pan must be between -1 (left) and 1 (right)",
        ));
    }
    Ok(pan)
}

/// A fade request resolved against one clip.
struct FadePlan {
    fade_in: Option<Duration>,
    out: Option<Duration>,
    curve: Option<FadeCurve>,
}

fn fade_plan(change: &FadeChange, target: &Target) -> Result<FadePlan, CmdError> {
    if change.fade_in.is_none() && change.out.is_none() && change.curve.is_none() {
        return Err(CmdError::invalid(
            "/set",
            "give at least one of in, out and curve",
        ));
    }
    let resolve = |pointer: &str, input: Option<TimeInput>| -> Result<Option<Duration>, CmdError> {
        input
            .map(|i| {
                let t = resolve_time(pointer, i, target.rate)?;
                Duration::try_from(t)
                    .map_err(|_| CmdError::invalid(pointer, "a fade cannot be negative"))
            })
            .transpose()
    };
    Ok(FadePlan {
        fade_in: resolve("/set/in", change.fade_in)?,
        out: resolve("/set/out", change.out)?,
        curve: change.curve,
    })
}

/// Applies a fade plan to a clip; returns what changed and whether a fade was cut.
fn apply_fade(clip: &mut Clip, plan: &FadePlan) -> (Vec<String>, bool) {
    let total = clip.duration;
    let mut clamped = false;
    let mut changed = Vec::new();
    let mut fit = |wanted: Duration, room: Duration| {
        if wanted > room {
            clamped = true;
            room
        } else {
            wanted
        }
    };
    let mut fade_in = clip.audio.fade_in;
    let mut fade_out = clip.audio.fade_out;
    // The fade being set gets the room the other one leaves; with both given, the fade in goes
    // first.
    if let Some(d) = plan.fade_in {
        fade_in.duration = fit(d, total.saturating_sub(fade_out.duration));
    }
    if let Some(d) = plan.out {
        fade_out.duration = fit(d, total.saturating_sub(fade_in.duration));
    }
    if let Some(curve) = plan.curve {
        let both = plan.fade_in.is_none() && plan.out.is_none();
        if both || plan.fade_in.is_some() {
            fade_in.curve = curve;
        }
        if both || plan.out.is_some() {
            fade_out.curve = curve;
        }
    }
    if clip.audio.fade_in != fade_in {
        clip.audio.fade_in = fade_in;
        changed.push("audio.fade_in".to_owned());
    }
    if clip.audio.fade_out != fade_out {
        clip.audio.fade_out = fade_out;
        changed.push("audio.fade_out".to_owned());
    }
    (changed, clamped)
}

fn clip_of(doc: &Project, id: ItemId) -> Result<&Clip, CmdError> {
    doc.find_item(&id)
        .and_then(|at| at.item.as_clip())
        .ok_or_else(|| CmdError::unsupported("only clips have audio properties"))
}

/// The shared implementation of `audio.set` and its thin variants.
struct Request<'a> {
    item: Option<ItemId>,
    items: &'a Option<Vec<ItemId>>,
    change: AudioChange,
    auto_key: Option<bool>,
    at: Option<TimeInput>,
}

impl Request<'_> {
    /// The `item.set_props` request of a volume or pan change.
    fn props(&self) -> Result<Option<ItemSetProps>, CmdError> {
        let (gain, pan) = match &self.change {
            AudioChange::Fade(_) => return Ok(None),
            AudioChange::Volume { db } => (Some(check_db("/set/db", *db)?), None),
            AudioChange::Pan { pan } => (None, Some(check_pan("/set/pan", *pan)?)),
        };
        Ok(Some(ItemSetProps {
            item: self.item,
            items: self.items.clone(),
            props: ItemProps {
                name: None,
                enabled: None,
                opacity: None,
                audio_gain: gain,
                audio_pan: pan,
                transform: None,
                crop: None,
                flip: None,
                blend_mode: None,
            },
            auto_key: self.auto_key,
            at: self.at,
        }))
    }

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        if let Some(props) = self.props()? {
            return props.validate(ctx);
        }
        let AudioChange::Fade(change) = &self.change else {
            return Ok(());
        };
        for target in keyframe::targets(ctx.doc(), self.item, self.items.as_deref())? {
            fade_plan(change, &target)?;
            clip_of(ctx.doc(), target.id)?;
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<AudioSetOutput, CmdError> {
        if let Some(props) = self.props()? {
            let out = props.apply(tx)?;
            return output_of(tx.doc(), out);
        }
        let AudioChange::Fade(change) = &self.change else {
            return Err(CmdError::invalid("/set", "unknown change"));
        };
        let targets = keyframe::targets(tx.doc(), self.item, self.items.as_deref())?;
        let mut changed: Vec<String> = Vec::new();
        let mut clamped = false;
        for target in &targets {
            let plan = fade_plan(change, target)?;
            let (names, cut) = tx.update_item(target.id, |item| {
                let clip = item
                    .as_clip_mut()
                    .ok_or_else(|| CmdError::unsupported("only clips have audio properties"))?;
                Ok(apply_fade(clip, &plan))
            })?;
            clamped |= cut;
            for n in names {
                if !changed.contains(&n) {
                    changed.push(n);
                }
            }
        }
        let first = clip_of(tx.doc(), targets[0].id)?;
        Ok(AudioSetOutput {
            item: targets[0].id,
            changed,
            fade_in: first.audio.fade_in,
            fade_out: first.audio.fade_out,
            clamped,
            keyed: Vec::new(),
        })
    }
}

/// `item.set_props`'s result as an audio result.
fn output_of(doc: &Project, out: ItemSetPropsOutput) -> Result<AudioSetOutput, CmdError> {
    let clip = clip_of(doc, out.item)?;
    Ok(AudioSetOutput {
        item: out.item,
        changed: out.changed,
        fade_in: clip.audio.fade_in,
        fade_out: clip.audio.fade_out,
        clamped: false,
        keyed: out.keyed,
    })
}

impl Command for AudioSet {
    type Output = AudioSetOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.request().validate(ctx)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<AudioSetOutput, CmdError> {
        self.request().apply(tx)
    }
}

impl AudioSet {
    fn request(&self) -> Request<'_> {
        Request {
            item: self.item,
            items: &self.items,
            change: self.set.clone(),
            auto_key: self.auto_key,
            at: self.at,
        }
    }
}

impl Command for AudioSetFade {
    type Output = AudioSetOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.request().validate(ctx)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<AudioSetOutput, CmdError> {
        self.request().apply(tx)
    }
}

impl AudioSetFade {
    fn request(&self) -> Request<'_> {
        Request {
            item: self.item,
            items: &self.items,
            change: AudioChange::Fade(FadeChange {
                fade_in: self.fade_in,
                out: self.out,
                curve: self.curve,
            }),
            auto_key: None,
            at: None,
        }
    }
}

impl Command for AudioSetVolume {
    type Output = AudioSetOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.request().validate(ctx)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<AudioSetOutput, CmdError> {
        self.request().apply(tx)
    }
}

impl AudioSetVolume {
    fn request(&self) -> Request<'_> {
        Request {
            item: self.item,
            items: &self.items,
            change: AudioChange::Volume { db: self.db },
            auto_key: self.auto_key,
            at: self.at,
        }
    }
}

impl Command for AudioSetPan {
    type Output = AudioSetOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.request().validate(ctx)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<AudioSetOutput, CmdError> {
        self.request().apply(tx)
    }
}

impl AudioSetPan {
    fn request(&self) -> Request<'_> {
        Request {
            item: self.item,
            items: &self.items,
            change: AudioChange::Pan { pan: self.pan },
            auto_key: self.auto_key,
            at: self.at,
        }
    }
}

impl Command for AudioSetGain {
    type Output = AudioSetOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        check_db("/db", self.db)?;
        for target in keyframe::targets(ctx.doc(), self.item, self.items.as_deref())? {
            clip_of(ctx.doc(), target.id)?;
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<AudioSetOutput, CmdError> {
        let db = check_db("/db", self.db)?;
        let targets = keyframe::targets(tx.doc(), self.item, self.items.as_deref())?;
        let mut changed = false;
        for target in &targets {
            changed |= tx.update_item(target.id, |item| {
                if item.as_clip().is_none() {
                    return Err(CmdError::unsupported("only clips have audio properties"));
                }
                keyframe::set_static(item, PropId::AudioGain, &KeyValue::Number(db))
            })?;
        }
        let first = clip_of(tx.doc(), targets[0].id)?;
        Ok(AudioSetOutput {
            item: targets[0].id,
            changed: if changed {
                vec!["audio.gain".to_owned()]
            } else {
                Vec::new()
            },
            fade_in: first.audio.fade_in,
            fade_out: first.audio.fade_out,
            clamped: false,
            keyed: Vec::new(),
        })
    }
}
