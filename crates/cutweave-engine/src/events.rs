// SPDX-License-Identifier: GPL-3.0-or-later
//! The engine's single event fan-out (WP-1I.04; arch §5.1): document events of the current doc
//! actor, project file events (autosave), session events, job progress, media analysis and view
//! changes, in one bounded broadcast. WP-1I.02 pushes them to WebSocket clients as JSON-RPC
//! notifications named [`EngineEvent::name`] with params [`EngineEvent::payload`].
//!
//! The channel is bounded ([`EngineConfig::event_capacity`](crate::EngineConfig)): a subscriber
//! that falls behind receives `Lagged` and must resync (`project.snapshot`), as with the doc
//! actor's own streams. Order is kept per source (document events in revision order, a job's
//! events in order); there is no order across sources.
//!
//! When the document is swapped (`project.open`, `project.close`, `project.save_as` of an
//! untitled document, `project.recover`), the engine sends `doc.reset` (the old revisions no
//! longer apply: resync) and a session event.

use std::sync::Arc;

use cutweave_commands::{ConnId, DocEvent};
use cutweave_jobs::JobEvent;
use cutweave_ops::media::MediaEvent;
use cutweave_ops::view::ViewChanged;
use serde::Serialize;
use serde_json::{Map, Value};
use tokio::sync::broadcast;

use crate::autosave::ProjectEvent;
use crate::session::ProjectStatus;

/// `(name, summary)` of the session events this crate adds to the engine event catalog
/// (alongside [`PROJECT_EVENTS`](crate::autosave::PROJECT_EVENTS)).
pub const SESSION_EVENTS: &[(&str, &str)] = &[
    (
        "project.closed",
        "The open project was closed; the engine now holds an untitled document",
    ),
    (
        "project.opened",
        "The open project changed (opened, saved under a new path, recovery decided); payload: its status",
    ),
];

/// `project.closed`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, specta::Type)]
pub struct ProjectClosed {
    /// The document path of the project that was closed.
    pub path: String,
}

/// A session change: which project the engine holds.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "event")]
#[non_exhaustive]
pub enum SessionEvent {
    /// `project.opened`.
    #[serde(rename = "project.opened")]
    Opened(ProjectStatus),
    /// `project.closed`.
    #[serde(rename = "project.closed")]
    Closed(ProjectClosed),
}

/// One event of the fan-out.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum EngineEvent {
    /// A document event of the current doc actor (`doc.patched`, `doc.reset`, `tx.*`,
    /// `edit.history_changed`, `doc.command_panicked`).
    Doc(Arc<DocEvent>),
    /// A project file event (`project.dirty`, `project.saved`, `project.recovery_available`,
    /// `project.autosave_failed`).
    Project(ProjectEvent),
    /// A session change (`project.opened`, `project.closed`).
    Session(SessionEvent),
    /// `job.progress`.
    Job(JobEvent),
    /// `media.indexed`, `media.analysis_failed`.
    Media(MediaEvent),
    /// `view.changed`; `origin` is the connection that made the change (WP-1I.02 skips it),
    /// `None` when the engine changed the view (a selected item was deleted).
    View {
        /// The change.
        change: ViewChanged,
        /// Who made it.
        origin: Option<ConnId>,
    },
    /// An event of an attached subsystem (`playback.state`, `engine.degraded`, WP-1I.05).
    Subsystem {
        /// The wire name.
        name: &'static str,
        /// The payload (a JSON object).
        payload: Value,
    },
}

impl EngineEvent {
    /// The wire name (`doc.patched`, `project.saved`, `job.progress`, …).
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            EngineEvent::Doc(e) => e.name(),
            EngineEvent::Project(e) => e.name(),
            EngineEvent::Session(SessionEvent::Opened(_)) => "project.opened",
            EngineEvent::Session(SessionEvent::Closed(_)) => "project.closed",
            EngineEvent::Job(_) => "job.progress",
            EngineEvent::Media(MediaEvent::Indexed(_)) => "media.indexed",
            EngineEvent::Media(MediaEvent::AnalysisFailed(_)) => "media.analysis_failed",
            EngineEvent::Media(_) => "media.unknown",
            EngineEvent::View { .. } => "view.changed",
            EngineEvent::Subsystem { name, .. } => name,
        }
    }

    /// The payload: a JSON object without the name (the JSON-RPC notification's `params`).
    #[must_use]
    pub fn payload(&self) -> Value {
        match self {
            EngineEvent::Doc(e) => untagged(&**e, "event"),
            EngineEvent::Project(e) => serde_json::to_value(e)
                .ok()
                .and_then(|mut v| v.get_mut("data").map(Value::take))
                .unwrap_or_else(empty),
            EngineEvent::Session(e) => untagged(e, "event"),
            EngineEvent::Job(e) => untagged(e, "event"),
            EngineEvent::Media(e) => untagged(e, "event"),
            EngineEvent::View { change, .. } => {
                serde_json::to_value(change).unwrap_or_else(|_| empty())
            }
            EngineEvent::Subsystem { payload, .. } => payload.clone(),
        }
    }

    /// `{"event": name, ...payload}`: the shape document events use on the wire.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("event".into(), Value::String(self.name().to_owned()));
        if let Value::Object(fields) = self.payload() {
            out.extend(fields);
        }
        Value::Object(out)
    }

    /// The `doc.patched` payload, if this is one.
    #[must_use]
    pub fn as_doc(&self) -> Option<&DocEvent> {
        match self {
            EngineEvent::Doc(e) => Some(e),
            _ => None,
        }
    }
}

fn empty() -> Value {
    Value::Object(Map::new())
}

/// `value` serialized, without its `tag` field.
fn untagged<T: Serialize + ?Sized>(value: &T, tag: &str) -> Value {
    match serde_json::to_value(value) {
        Ok(Value::Object(mut map)) => {
            map.remove(tag);
            Value::Object(map)
        }
        _ => empty(),
    }
}

/// A subscription to the fan-out (a tokio broadcast receiver: `.recv().await` from async code,
/// `blocking_recv()` from threads).
pub type EventReceiver = broadcast::Receiver<EngineEvent>;

/// The sending side of the fan-out. Cheap to clone; publishing never blocks.
#[derive(Clone, Debug)]
pub struct EventBus {
    tx: broadcast::Sender<EngineEvent>,
}

impl EventBus {
    /// A fan-out keeping up to `capacity` events per subscriber.
    #[must_use]
    pub fn new(capacity: usize) -> EventBus {
        let (tx, _) = broadcast::channel(capacity.max(1));
        EventBus { tx }
    }

    /// Sends `event` to every current subscriber (dropped if there is none).
    pub fn publish(&self, event: EngineEvent) {
        let _ = self.tx.send(event);
    }

    /// A new subscription, starting with the next event.
    #[must_use]
    pub fn subscribe(&self) -> EventReceiver {
        self.tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::autosave::ProjectDirty;
    use cutweave_commands::{DocReset, Rev};

    #[test]
    fn names_and_payloads_drop_the_tag() {
        let reset = EngineEvent::Doc(Arc::new(DocEvent::Reset(DocReset {
            rev: Rev(3),
            prev_rev: Rev(9),
            committed_rev: Rev(3),
        })));
        assert_eq!(reset.name(), "doc.reset");
        assert_eq!(
            reset.payload(),
            serde_json::json!({"rev": 3, "prev_rev": 9, "committed_rev": 3})
        );
        assert_eq!(reset.to_json()["event"], "doc.reset");

        let dirty = EngineEvent::Project(ProjectEvent::Dirty(ProjectDirty { dirty: true }));
        assert_eq!(dirty.name(), "project.dirty");
        assert_eq!(dirty.payload(), serde_json::json!({"dirty": true}));

        let closed = EngineEvent::Session(SessionEvent::Closed(ProjectClosed {
            path: "a.cutweave".into(),
        }));
        assert_eq!(closed.name(), "project.closed");
        assert_eq!(closed.payload(), serde_json::json!({"path": "a.cutweave"}));
        assert!(SESSION_EVENTS.iter().any(|(n, _)| *n == closed.name()));
    }

    #[test]
    fn subscribers_lag_instead_of_blocking() {
        let bus = EventBus::new(2);
        let mut rx = bus.subscribe();
        for _ in 0..5 {
            bus.publish(EngineEvent::Project(ProjectEvent::Dirty(ProjectDirty {
                dirty: false,
            })));
        }
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Lagged(3))
        ));
    }
}
