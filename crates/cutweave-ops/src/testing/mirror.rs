// SPDX-License-Identifier: GPL-3.0-or-later
//! The UI docStore contract (arch §5.1, §16): a subscriber that starts from a snapshot and then
//! applies only `doc.patched` events to the document's JSON form holds exactly the serialized
//! live document at every revision.

use cutweave_commands::{DocEvent, PatchError, Rev, apply_patch_json, json_eq};
use cutweave_model::Project;
use serde_json::Value;

/// Why events do not replay onto a mirror.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ReplayError {
    /// An event does not continue from the mirror's revision: events were missed or reordered.
    #[error("missed events: the mirror is at rev {mirror}, the next event starts from rev {event}")]
    Gap {
        /// The mirror's revision.
        mirror: Rev,
        /// The event's `prev_rev`.
        event: Rev,
    },
    /// An event that does not move the revision forward.
    #[error("doc.patched from rev {prev} to rev {rev} does not move forward")]
    NotForward {
        /// Its `prev_rev`.
        prev: Rev,
        /// Its `rev`.
        rev: Rev,
    },
    /// `doc.reset`: the change could not be expressed as ops, so the mirror must resync from a
    /// snapshot.
    #[error("doc.reset at rev {0}: the mirror must resync from a snapshot")]
    Reset(Rev),
    /// The ops of an event do not apply to the mirror.
    #[error("the ops of rev {rev} do not apply to the mirror: {error}")]
    Patch {
        /// The event's revision.
        rev: Rev,
        /// Why.
        error: PatchError,
    },
}

/// A JSON mirror of the live document kept up to date from events only, like the UI docStore.
#[derive(Clone, Debug, PartialEq)]
pub struct JsonMirror {
    json: Value,
    rev: Rev,
}

impl JsonMirror {
    /// A mirror of `doc` at `rev` (the snapshot a subscriber starts from; for a new project,
    /// the empty document at revision 0).
    #[must_use]
    pub fn new(doc: &Project, rev: Rev) -> JsonMirror {
        JsonMirror {
            json: to_json(doc),
            rev,
        }
    }

    /// The mirrored document.
    #[must_use]
    pub fn json(&self) -> &Value {
        &self.json
    }

    /// The revision it mirrors.
    #[must_use]
    pub fn rev(&self) -> Rev {
        self.rev
    }

    /// Applies one event: `doc.patched` must continue from the mirror's revision and its ops
    /// must apply; `doc.reset` is an error (resync needed); other events change nothing.
    pub fn apply(&mut self, event: &DocEvent) -> Result<(), ReplayError> {
        match event {
            DocEvent::Patched(p) => {
                if p.prev_rev != self.rev {
                    return Err(ReplayError::Gap {
                        mirror: self.rev,
                        event: p.prev_rev,
                    });
                }
                if p.rev <= p.prev_rev {
                    return Err(ReplayError::NotForward {
                        prev: p.prev_rev,
                        rev: p.rev,
                    });
                }
                apply_patch_json(&mut self.json, &p.ops)
                    .map_err(|error| ReplayError::Patch { rev: p.rev, error })?;
                self.rev = p.rev;
                Ok(())
            }
            DocEvent::Reset(r) => Err(ReplayError::Reset(r.rev)),
            _ => Ok(()),
        }
    }

    /// Whether the mirror holds exactly `doc`'s JSON form (numbers compared by value).
    #[must_use]
    pub fn matches(&self, doc: &Project) -> bool {
        json_eq(&self.json, &to_json(doc))
    }
}

/// Replays `events`, in order, onto a mirror of `start` at `start_rev`, and returns it: the
/// mirror a UI holds after receiving them. Property (6) of the harness: it must equal the live
/// document after the last event.
pub fn replay_events<'a>(
    start: &Project,
    start_rev: Rev,
    events: impl IntoIterator<Item = &'a DocEvent>,
) -> Result<JsonMirror, ReplayError> {
    let mut mirror = JsonMirror::new(start, start_rev);
    for event in events {
        mirror.apply(event)?;
    }
    Ok(mirror)
}

/// `doc` as JSON. Serializing a document never fails (no maps with non-string keys, no
/// non-finite floats: [`Project::validate`] rejects those).
fn to_json(doc: &Project) -> Value {
    serde_json::to_value(doc).unwrap_or(Value::Null)
}
