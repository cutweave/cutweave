// SPDX-License-Identifier: GPL-3.0-or-later
//! `keyframe.eval` (keyframes §9.10): samples a property's evaluated value, for agents and
//! tests. The UI evaluates locally through the wasm module and never calls this per frame.

use cutweave_commands::{CmdError, Query, QueryCtx, query, wire};
use cutweave_model::{ItemId, PropId, Time, TimeInput, sample_times};

use super::{KeyValue, Wire, on_prop, sequence_time};

/// Most samples one `keyframe.eval` call returns.
pub const MAX_EVAL_SAMPLES: u32 = 1_000;

/// One evaluated sample.
#[wire]
pub struct EvalSample {
    /// Sequence time.
    pub at: Time,
    /// Item time.
    pub t: Time,
    /// The value there: property-clamped, in the evaluated form.
    pub value: KeyValue,
    /// `t` lies inside the item's `[0, duration)`.
    pub in_item: bool,
}

/// Result of `keyframe.eval`.
#[wire]
pub struct KeyframeEvalOutput {
    /// The property.
    pub prop: PropId,
    /// The samples, from the first time to the last.
    pub samples: Vec<EvalSample>,
}

/// Evaluates one property at a sequence time `at`, or at `samples` times from `from` to `to`
/// (both included; 2 to 1,000 samples, `to >= from`). Times are sequence times and are not
/// snapped. Read-only.
#[query(
    name = "keyframe.eval",
    summary = "Evaluate an item's property at a time or over a range",
    mcp = "catalog"
)]
pub struct KeyframeEval {
    /// The item.
    pub item: ItemId,
    /// The property.
    pub prop: PropId,
    /// One sample at this sequence time.
    pub at: Option<TimeInput>,
    /// The first sample's sequence time.
    pub from: Option<TimeInput>,
    /// The last sample's sequence time.
    pub to: Option<TimeInput>,
    /// How many samples, 2 to 1,000.
    pub samples: Option<u32>,
}

impl Query for KeyframeEval {
    type Output = KeyframeEvalOutput;

    fn run(&self, ctx: &QueryCtx<'_>) -> Result<KeyframeEvalOutput, CmdError> {
        let at = ctx.item(self.item)?;
        let clip = at
            .item
            .as_clip()
            .ok_or_else(|| CmdError::unsupported("only clips have animatable properties"))?;
        let rate = at.sequence.format.frame_rate;
        let target = super::Target {
            id: self.item,
            start: clip.start,
            duration: clip.duration.flicks(),
            rate,
            samples: at.sequence.format.sample_rate,
        };
        let resolve = |pointer: &str, input: TimeInput| -> Result<i64, CmdError> {
            input
                .resolve(Some(rate))
                .map(Time::flicks)
                .map_err(|e| CmdError::invalid(pointer, e.to_string()))
        };
        let item_time = |pointer: &str, abs: i64| -> Result<Time, CmdError> {
            abs.checked_sub(clip.start.flicks())
                .and_then(|v| Time::from_flicks(v).ok())
                .ok_or_else(|| CmdError::invalid(pointer, "the time is out of range"))
        };
        let times: Vec<Time> = match (self.at, self.from, self.to, self.samples) {
            (Some(a), None, None, None) => vec![item_time("/at", resolve("/at", a)?)?],
            (None, Some(from), Some(to), Some(n)) => {
                if !(2..=MAX_EVAL_SAMPLES).contains(&n) {
                    return Err(CmdError::invalid(
                        "/samples",
                        format!("give 2 to {MAX_EVAL_SAMPLES} samples"),
                    ));
                }
                let t0 = item_time("/from", resolve("/from", from)?)?;
                let t1 = item_time("/to", resolve("/to", to)?)?;
                if t1 < t0 {
                    return Err(CmdError::invalid("/to", "`to` must not be before `from`"));
                }
                sample_times(t0, t1, n as usize).collect()
            }
            _ => {
                return Err(CmdError::invalid(
                    "",
                    "give `at`, or `from`, `to` and `samples`",
                ));
            }
        };
        let clamp = self.prop.spec().clamp;
        let samples = times
            .into_iter()
            .map(|t| EvalSample {
                at: sequence_time(&target, t),
                t,
                value: on_prop!(clip, self.prop, a => a.eval_clamped(t, clamp).wrap()),
                in_item: !super::outside(&target, t),
            })
            .collect();
        Ok(KeyframeEvalOutput {
            prop: self.prop,
            samples,
        })
    }
}
