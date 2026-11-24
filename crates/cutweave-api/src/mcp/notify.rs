// SPDX-License-Identifier: GPL-3.0-or-later
//! Resource change notifications (WP-1J.02; arch §14): one [`Notifier`] per engine, fed by the
//! engine's event fan-out (`doc.patched` and the events that replace the document), serving both
//! MCP revisions.
//!
//! - **2025-11-25** (`resources/subscribe`): the session records the URIs the client subscribed
//!   to and a forwarder task sends `notifications/resources/updated` to that session's peer
//!   ([`Subscriptions`]).
//! - **2026-07-28** (`subscriptions/listen`): the request itself is the subscription; the
//!   handler's `listen` forwards through rmcp's filter-enforcing sink. There is no `ping` and no
//!   session; nothing here depends on either.
//!
//! The [`Notifier`] knows nothing about either: it turns engine events into [`Changed`]
//! broadcasts, and [`updated_uris`] maps one to the URIs a subscriber holds.
//!
//! The engine subscription is made on the first [`Notifier::subscribe`] (the first listener),
//! before it returns, so no change after that call is missed. A listener that falls behind gets
//! a conservative "everything changed" ([`Changed::ALL`]) instead of a silent gap.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use std::time::Duration;

use rmcp::{
    RoleServer,
    model::{ResourceUpdatedNotificationParam, ServerNotification},
    service::Peer,
};
use tokio::sync::broadcast;

use crate::mcp::{dispatch::Dispatch, resources};
use crate::rpc::RpcEventError;

/// Engine events that replace or change the document: each updates the project and the
/// timelines.
const DOCUMENT_EVENTS: [&str; 4] = [
    "doc.patched",
    "doc.reset",
    "project.opened",
    "project.closed",
];

/// How long a forwarder waits for a change before it checks that its session is still there.
const IDLE_CHECK: Duration = Duration::from_secs(30);

/// What changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changed {
    /// `cutweave://project`.
    pub project: bool,
    /// Every `cutweave://sequence/{id}/timeline`.
    pub timelines: bool,
}

impl Changed {
    /// Everything a document change touches.
    pub const ALL: Changed = Changed {
        project: true,
        timelines: true,
    };
}

/// The URIs of `subscribed` that `changed` touches.
pub fn updated_uris<'a>(
    changed: Changed,
    subscribed: impl IntoIterator<Item = &'a String>,
) -> Vec<String> {
    subscribed
        .into_iter()
        .filter(|uri| match resources::kind(uri) {
            Some(resources::Kind::Project) => changed.project,
            Some(resources::Kind::Timeline) => changed.timelines,
            _ => false,
        })
        .cloned()
        .collect()
}

/// Fans the engine's document events out to every MCP listener (see the module docs).
pub struct Notifier {
    dispatch: Arc<dyn Dispatch>,
    changes: broadcast::Sender<Changed>,
    generation: Arc<AtomicU64>,
    started: OnceLock<()>,
}

impl std::fmt::Debug for Notifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notifier")
            .field("started", &self.started.get().is_some())
            .finish_non_exhaustive()
    }
}

impl Notifier {
    /// A notifier over `dispatch`'s events. Nothing runs until the first listener.
    #[must_use]
    pub fn new(dispatch: Arc<dyn Dispatch>) -> Arc<Notifier> {
        Arc::new(Notifier {
            dispatch,
            changes: broadcast::Sender::new(64),
            generation: Arc::new(AtomicU64::new(0)),
            started: OnceLock::new(),
        })
    }

    /// How many document changes the notifier has seen. A listener that records it before it
    /// subscribes can tell whether it missed one in between.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// A listener: every [`Changed`] from now on. Must be called inside a Tokio runtime.
    pub fn subscribe(&self) -> broadcast::Receiver<Changed> {
        let receiver = self.changes.subscribe();
        self.start();
        receiver
    }

    /// Subscribes to the engine's events (once). Must be called inside a Tokio runtime.
    pub fn start(&self) {
        self.started.get_or_init(|| {
            let mut events = self.dispatch.events();
            let tx = self.changes.clone();
            let generation = Arc::clone(&self.generation);
            tokio::spawn(async move {
                loop {
                    let changed = match events.recv().await {
                        Ok(event) => DOCUMENT_EVENTS
                            .contains(&event.name.as_ref())
                            .then_some(Changed::ALL),
                        // Missed events: say that everything may have changed.
                        Err(RpcEventError::Lagged(_)) => Some(Changed::ALL),
                        Err(RpcEventError::Closed) => break,
                    };
                    if let Some(changed) = changed {
                        generation.fetch_add(1, Ordering::SeqCst);
                        // No listener left is fine.
                        let _ = tx.send(changed);
                    }
                }
            });
        });
    }
}

/// One 2025-11-25 session's subscriptions (`resources/subscribe`) and the forwarder that serves
/// them. Shared by the clones of the session's handler; the forwarder ends with the session.
#[derive(Debug, Default)]
pub struct Subscriptions {
    uris: Mutex<BTreeSet<String>>,
    forwarder: OnceLock<()>,
}

impl Subscriptions {
    /// Subscribes the session behind `peer` to `uri` (already validated); the first call starts
    /// the forwarder.
    pub fn add(self: &Arc<Self>, uri: String, notifier: &Notifier, peer: &Peer<RoleServer>) {
        self.uris
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(uri);
        self.forwarder.get_or_init(|| {
            let changes = notifier.subscribe();
            let me = Arc::downgrade(self);
            let peer = peer.clone();
            tokio::spawn(forward(me, peer, changes));
        });
    }

    /// Unsubscribes `uri`.
    pub fn remove(&self, uri: &str) {
        self.uris
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(uri);
    }

    fn matching(&self, changed: Changed) -> Vec<String> {
        let uris = self.uris.lock().unwrap_or_else(PoisonError::into_inner);
        updated_uris(changed, uris.iter())
    }
}

/// Sends `notifications/resources/updated` to `peer` for every change that touches a URI the
/// session subscribed to, until the session is gone.
async fn forward(
    session: Weak<Subscriptions>,
    peer: Peer<RoleServer>,
    mut rx: broadcast::Receiver<Changed>,
) {
    loop {
        let changed = match tokio::time::timeout(IDLE_CHECK, rx.recv()).await {
            Ok(Ok(changed)) => changed,
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => Changed::ALL,
            Ok(Err(broadcast::error::RecvError::Closed)) => return,
            Err(_idle) => {
                if peer.is_transport_closed() || session.strong_count() == 0 {
                    return;
                }
                continue;
            }
        };
        let Some(session) = session.upgrade() else {
            return;
        };
        for uri in session.matching(changed) {
            let note = ServerNotification::ResourceUpdatedNotification(
                rmcp::model::ResourceUpdatedNotification::new(
                    ResourceUpdatedNotificationParam::new(uri),
                ),
            );
            if peer.send_notification(note).await.is_err() {
                return;
            }
        }
    }
}
