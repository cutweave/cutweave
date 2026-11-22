// SPDX-License-Identifier: GPL-3.0-or-later
//! What readers and subscribers see: [`DocSnapshot`] (an immutable document at a revision) and
//! [`DocEvent`] (what changed, in revision order).

use cutweave_model::Project;
use serde::{Deserialize, Serialize};

use super::{
    identity::Actor,
    request::{RequestId, TxId},
};
use crate::{error::Rev, patch::Patch};

/// An immutable document at a revision (arch §5.2): what render, audio and queries read.
///
/// Built from persistent collections, so holding one is O(1) and it never changes, whatever
/// is committed later.
#[derive(Clone, Debug, PartialEq)]
pub struct DocSnapshot {
    /// The revision of this document state.
    pub rev: Rev,
    /// The last committed revision when this snapshot was published.
    pub committed_rev: Rev,
    /// The document.
    pub doc: Project,
}

impl DocSnapshot {
    /// Whether this is a committed state: no open gesture or proposal had changes in it.
    /// (`rev == committed_rev` implies it; the converse can fail right after a rollback.)
    #[must_use]
    pub fn is_committed(&self) -> bool {
        self.rev == self.committed_rev
    }
}

/// Why the live document changed.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    specta::Type,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PatchCause {
    /// A command committed in its own implicit transaction.
    Command,
    /// A command inside an open gesture or proposal (live, not committed yet).
    Live,
    /// An open transaction committed. For a gesture or proposal the document already showed
    /// its changes, so `ops` is usually empty; for a batch it carries the batch's changes.
    Commit,
    /// An open transaction was cancelled or rolled back; `ops` undoes its live changes.
    Rollback,
}

/// A change of the live document, broadcast in revision order (`doc.patched`, arch §5.1).
///
/// Applying `ops` to the document at `prev_rev` gives the document at `rev`, on the typed
/// document and on its JSON mirror alike (the UI docStore contract). A subscriber that sees
/// `prev_rev` differ from the last revision it applied has missed events and must resync from
/// a snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocPatched {
    /// The live revision after the change.
    pub rev: Rev,
    /// The live revision before it.
    pub prev_rev: Rev,
    /// The last committed revision after the change.
    pub committed_rev: Rev,
    /// Who made it.
    pub actor: Actor,
    /// The transaction it belongs to.
    pub tx_id: TxId,
    /// Why.
    pub cause: PatchCause,
    /// The request that caused it, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<RequestId>,
    /// The ops that turn the `prev_rev` document into the `rev` document.
    pub ops: Patch,
}

/// The live document changed in a way that could not be expressed as ops (a defensive path
/// when open transactions had to be re-composed): every subscriber must resync from a
/// snapshot at `rev` or later.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocReset {
    /// The live revision after the change.
    pub rev: Rev,
    /// The live revision before it.
    pub prev_rev: Rev,
    /// The last committed revision.
    pub committed_rev: Rev,
}

/// A command panicked; its transaction was rolled back and the actor kept running.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandPanicked {
    /// The command's registry name.
    pub command: String,
    /// Who called it.
    pub actor: Actor,
    /// The request.
    pub request_id: RequestId,
    /// The (unchanged) live revision.
    pub rev: Rev,
    /// The panic message (withheld for commands with `#[secret]` params).
    pub message: String,
}

/// Everything the document actor broadcasts to subscribers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event")]
#[non_exhaustive]
pub enum DocEvent {
    /// `doc.patched`: the live document changed.
    #[serde(rename = "doc.patched")]
    Patched(DocPatched),
    /// `doc.reset`: resync required.
    #[serde(rename = "doc.reset")]
    Reset(DocReset),
    /// `doc.command_panicked`.
    #[serde(rename = "doc.command_panicked")]
    CommandPanicked(CommandPanicked),
}

impl DocEvent {
    /// The live revision this event leads to, for events that change the document.
    #[must_use]
    pub fn rev(&self) -> Option<Rev> {
        match self {
            DocEvent::Patched(p) => Some(p.rev),
            DocEvent::Reset(r) => Some(r.rev),
            DocEvent::CommandPanicked(_) => None,
        }
    }

    /// The live revision before this event, for events that change the document.
    #[must_use]
    pub fn prev_rev(&self) -> Option<Rev> {
        match self {
            DocEvent::Patched(p) => Some(p.prev_rev),
            DocEvent::Reset(r) => Some(r.prev_rev),
            DocEvent::CommandPanicked(_) => None,
        }
    }

    /// The `doc.patched` payload, if this is one.
    #[must_use]
    pub fn as_patched(&self) -> Option<&DocPatched> {
        match self {
            DocEvent::Patched(p) => Some(p),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use serde_json::json;

    use super::*;

    #[test]
    fn events_serialize_with_their_wire_names() {
        let e = DocEvent::Patched(DocPatched {
            rev: Rev(3),
            prev_rev: Rev(2),
            committed_rev: Rev(3),
            actor: Actor::agent("claude").unwrap(),
            tx_id: TxId::new(7),
            cause: PatchCause::Command,
            request_id: None,
            ops: Patch::new(),
        });
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(
            v,
            json!({
                "event": "doc.patched",
                "rev": 3,
                "prev_rev": 2,
                "committed_rev": 3,
                "actor": "agent:claude",
                "tx_id": "tx_7",
                "cause": "command",
                "ops": []
            })
        );
        assert_eq!(serde_json::from_value::<DocEvent>(v).unwrap(), e);
        assert_eq!(e.rev(), Some(Rev(3)));
        assert_eq!(e.prev_rev(), Some(Rev(2)));
        assert!(e.as_patched().is_some());
    }
}
