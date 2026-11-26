// SPDX-License-Identifier: GPL-3.0-or-later
//! What readers and subscribers see: [`DocSnapshot`] (an immutable document at a revision) and
//! [`DocEvent`] (what changed, in revision order).

use cutweave_model::{ItemId, Project};
use serde::{Deserialize, Serialize};

use super::{
    history::HistoryChanged,
    identity::Actor,
    request::{RequestId, TxId, TxKind, TxOutcome},
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
    /// Whether `rev == committed_rev`, which means this is the committed document: no open
    /// gesture or proposal has changes in it. Not conversely: a rollback bumps `rev` but not
    /// `committed_rev`, so after a cancel this is `false` although the document equals the
    /// committed one, until the next commit. To compare contents, read the committed snapshot.
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
    /// An open transaction was cancelled, rolled back or expired; `ops` undoes its live
    /// changes.
    Rollback,
    /// `edit.undo` reverted a history entry (`actor` is who undid it).
    Undo,
    /// `edit.redo` re-applied a history entry (`actor` is who redid it).
    Redo,
    /// `edit.restore` brought the document back to a checkpoint.
    Restore,
}

/// A change of the live document, broadcast in revision order (`doc.patched`, arch §5.1).
///
/// Applying `ops` to the document at `prev_rev` gives the document at `rev`, on the typed
/// document and on its JSON mirror alike (the UI docStore contract). A subscriber that sees
/// `prev_rev` differ from the last revision it applied has missed events and must resync from
/// a snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type)]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct DocReset {
    /// The live revision after the change.
    pub rev: Rev,
    /// The live revision before it.
    pub prev_rev: Rev,
    /// The last committed revision.
    pub committed_rev: Rev,
}

/// A command panicked; its transaction was rolled back and the actor kept running.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type)]
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

/// `tx.opened` / `tx.locked`: an open transaction and the items it soft-locks (WP-1B.09), for
/// "agent is editing" indicators. `tx.opened` comes when it opens (no items yet); `tx.locked`
/// each time a gesture or proposal locks more items, after the `doc.patched` that touched them.
/// After `Lagged`, re-read `edit.locks`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct TxActivity {
    /// The transaction.
    pub tx_id: TxId,
    /// `batch`, `gesture` or `proposal`.
    pub kind: TxKind,
    /// Its label.
    pub label: String,
    /// Who opened it.
    pub actor: Actor,
    /// Every item it soft-locks so far, in id order (never any for a batch).
    pub items: Vec<ItemId>,
    /// The live revision.
    pub rev: Rev,
}

/// `tx.closed`: an open transaction ended and released its soft locks (WP-1B.09). Comes after
/// the `doc.patched` of its commit or rollback.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct TxEnded {
    /// The transaction.
    pub tx_id: TxId,
    /// `batch`, `gesture` or `proposal`.
    pub kind: TxKind,
    /// Who opened it.
    pub actor: Actor,
    /// Committed, cancelled, rolled back (connection closed), expired (idle gesture) or
    /// conflicted (discarded after another commit overlapped it).
    pub outcome: TxOutcome,
    /// The items it no longer locks, in id order.
    pub items: Vec<ItemId>,
    /// The live revision afterwards.
    pub rev: Rev,
    /// The committed revision afterwards.
    pub committed_rev: Rev,
}

/// Why the engine cancelled a gesture (`gesture.cancelled`, WP-2A.12).
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
pub enum GestureCancelReason {
    /// No request named it for [`GESTURE_IDLE_TIMEOUT`](crate::GESTURE_IDLE_TIMEOUT) (30 s).
    IdleTimeout,
}

/// `gesture.cancelled`: the engine cancelled an open gesture on its own (WP-2A.12, arch §5.2):
/// its live changes were rolled back and its soft locks released. Comes after the
/// `doc.patched` of the rollback and the `tx.closed` (outcome `expired`). An editor that was
/// still dragging goes back to idle (WP-2A.08); a client never re-sends the gesture's updates
/// on its own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct GestureCancelled {
    /// The gesture.
    pub tx_id: TxId,
    /// Who opened it.
    pub actor: Actor,
    /// Why: `idle_timeout`.
    pub reason: GestureCancelReason,
    /// The items it no longer locks, in id order.
    pub items: Vec<ItemId>,
    /// The live revision afterwards.
    pub rev: Rev,
    /// The committed revision (unchanged: nothing was committed).
    pub committed_rev: Rev,
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
    /// `edit.history_changed`: an actor's undo/redo stacks changed (WP-1B.04).
    #[serde(rename = "edit.history_changed")]
    History(HistoryChanged),
    /// `tx.opened`: a transaction was opened (WP-1B.09).
    #[serde(rename = "tx.opened")]
    TxOpened(TxActivity),
    /// `tx.locked`: a gesture or proposal soft-locked more items (WP-1B.09).
    #[serde(rename = "tx.locked")]
    TxLocked(TxActivity),
    /// `tx.closed`: a transaction ended and released its locks (WP-1B.09).
    #[serde(rename = "tx.closed")]
    TxClosed(TxEnded),
    /// `gesture.cancelled`: the engine cancelled an idle gesture (WP-2A.12).
    #[serde(rename = "gesture.cancelled")]
    GestureCancelled(GestureCancelled),
}

/// One entry of the event catalog (`schema/events.json`, WP-1N.01).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct EventInfo {
    /// The wire name (the `event` tag).
    pub name: &'static str,
    /// What it means.
    pub summary: &'static str,
    /// The Rust payload type.
    pub payload: &'static str,
}

/// Every event the document actor broadcasts today, sorted by name. Later work packages (event
/// bus, view events) append their events here; [`DocEvent::name`] keeps this list honest.
pub const EVENT_CATALOG: &[EventInfo] = &[
    EventInfo {
        name: "doc.command_panicked",
        summary: "A command panicked; its transaction was rolled back",
        payload: "CommandPanicked",
    },
    EventInfo {
        name: "doc.patched",
        summary: "The live document changed; apply `ops` to the document at `prev_rev`",
        payload: "DocPatched",
    },
    EventInfo {
        name: "doc.reset",
        summary: "The document changed in a way ops cannot express; resync from a snapshot",
        payload: "DocReset",
    },
    EventInfo {
        name: "edit.history_changed",
        summary: "An actor's undo/redo stacks changed",
        payload: "HistoryChanged",
    },
    EventInfo {
        name: "gesture.cancelled",
        summary: "The engine cancelled a gesture idle for 30 s: rolled back, locks released",
        payload: "GestureCancelled",
    },
    EventInfo {
        name: "tx.closed",
        summary: "A transaction ended and released its soft locks",
        payload: "TxEnded",
    },
    EventInfo {
        name: "tx.locked",
        summary: "A gesture or proposal soft-locked more items",
        payload: "TxActivity",
    },
    EventInfo {
        name: "tx.opened",
        summary: "A transaction was opened (no items locked yet)",
        payload: "TxActivity",
    },
];

impl DocEvent {
    /// The wire name (`doc.patched`, ...): the `event` tag.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            DocEvent::Patched(_) => "doc.patched",
            DocEvent::Reset(_) => "doc.reset",
            DocEvent::CommandPanicked(_) => "doc.command_panicked",
            DocEvent::History(_) => "edit.history_changed",
            DocEvent::TxOpened(_) => "tx.opened",
            DocEvent::TxLocked(_) => "tx.locked",
            DocEvent::TxClosed(_) => "tx.closed",
            DocEvent::GestureCancelled(_) => "gesture.cancelled",
        }
    }

    /// The live revision this event leads to, for events that change the document.
    #[must_use]
    pub fn rev(&self) -> Option<Rev> {
        match self {
            DocEvent::Patched(p) => Some(p.rev),
            DocEvent::Reset(r) => Some(r.rev),
            DocEvent::CommandPanicked(_)
            | DocEvent::History(_)
            | DocEvent::TxOpened(_)
            | DocEvent::TxLocked(_)
            | DocEvent::TxClosed(_)
            | DocEvent::GestureCancelled(_) => None,
        }
    }

    /// The live revision before this event, for events that change the document.
    #[must_use]
    pub fn prev_rev(&self) -> Option<Rev> {
        match self {
            DocEvent::Patched(p) => Some(p.prev_rev),
            DocEvent::Reset(r) => Some(r.prev_rev),
            DocEvent::CommandPanicked(_)
            | DocEvent::History(_)
            | DocEvent::TxOpened(_)
            | DocEvent::TxLocked(_)
            | DocEvent::TxClosed(_)
            | DocEvent::GestureCancelled(_) => None,
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

    /// The `edit.history_changed` payload, if this is one.
    #[must_use]
    pub fn as_history(&self) -> Option<&HistoryChanged> {
        match self {
            DocEvent::History(h) => Some(h),
            _ => None,
        }
    }

    /// The `gesture.cancelled` payload, if this is one.
    #[must_use]
    pub fn as_gesture_cancelled(&self) -> Option<&GestureCancelled> {
        match self {
            DocEvent::GestureCancelled(g) => Some(g),
            _ => None,
        }
    }

    /// The `tx.closed` payload, if this is one.
    #[must_use]
    pub fn as_tx_closed(&self) -> Option<&TxEnded> {
        match self {
            DocEvent::TxClosed(t) => Some(t),
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
    fn catalog_is_sorted_unique_and_matches_the_wire_tags() {
        let names: Vec<_> = EVENT_CATALOG.iter().map(|e| e.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted);
        let reset = DocEvent::Reset(DocReset {
            rev: Rev(2),
            prev_rev: Rev(1),
            committed_rev: Rev(2),
        });
        assert_eq!(serde_json::to_value(&reset).unwrap()["event"], reset.name());
        assert!(names.contains(&reset.name()));
    }

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
