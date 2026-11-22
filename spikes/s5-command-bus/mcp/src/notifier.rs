// SPDX-License-Identifier: GPL-3.0-or-later
//! Change notifications (arch §14: "use the negotiated revision's mechanism").
//!
//! One internal feed ([`Notifier`], fed by the engine's `doc.patched` events) and one forwarding
//! loop ([`forward`]) that writes to whichever sink the negotiated revision provides:
//!
//! | Revision | Client opt-in | Delivery |
//! |---|---|---|
//! | 2025-11-25 (and older) | `resources/subscribe {uri}` | `notifications/resources/updated` on the session (stdio, or the HTTP GET stream) |
//! | 2026-07-28 | `subscriptions/listen {resourceSubscriptions:[uri]}` | `notifications/resources/updated` tagged with the subscription id, on the listen request's own stream |
//!
//! Neither path uses `ping`: the 2026-07-28 revision removed it, and liveness of a `listen`
//! stream is the stream itself.

use std::{future::Future, sync::mpsc};

use rmcp::{Peer, RoleServer, model::ResourceUpdatedNotificationParam, service::SubscriptionSink};
use s5_bus::engine::Event;
use tokio::sync::broadcast::{self, error::RecvError};

/// One document change, as fanned out to subscribers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocChange {
    /// New revision.
    pub rev: u64,
    /// Actor, e.g. `agent:claude`.
    pub actor: String,
    /// What happened, e.g. `clip.split`.
    pub label: String,
}

/// Async fan-out of the engine's events.
#[derive(Clone)]
pub struct Notifier {
    tx: broadcast::Sender<DocChange>,
}

impl Notifier {
    /// Bridges the engine's (sync, `std::sync::mpsc`) event stream onto a broadcast channel. The
    /// bridge thread ends when the engine is dropped.
    #[must_use]
    pub fn bridge(events: mpsc::Receiver<Event>) -> Notifier {
        let (tx, _) = broadcast::channel(256);
        let fwd = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("s5-mcp-notifier".into())
            .spawn(move || {
                while let Ok(event) = events.recv() {
                    match event {
                        Event::DocPatched {
                            rev, actor, label, ..
                        } => {
                            // No receivers is fine: nobody is subscribed right now.
                            let _ = fwd.send(DocChange {
                                rev: rev.0,
                                actor,
                                label,
                            });
                        }
                    }
                }
            });
        if let Err(e) = spawned {
            tracing::error!("cannot start the notifier thread, notifications are off: {e}");
        }
        Notifier { tx }
    }

    /// A new receiver; it sees every change sent after this call.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<DocChange> {
        self.tx.subscribe()
    }
}

/// Where a revision's notifications go.
pub(crate) enum ChangeSink {
    /// 2025-11-25: server-initiated notification on the session.
    Legacy(Peer<RoleServer>),
    /// 2026-07-28: notification on the `subscriptions/listen` stream (filter-enforcing sink).
    Modern(SubscriptionSink),
}

impl ChangeSink {
    async fn resource_updated(&self, uri: &str) -> bool {
        match self {
            ChangeSink::Legacy(peer) => peer
                .notify_resource_updated(ResourceUpdatedNotificationParam::new(uri))
                .await
                .is_ok(),
            ChangeSink::Modern(sink) => sink.notify_resource_updated(uri).await.is_ok(),
        }
    }
}

/// Forwards every change as `notifications/resources/updated` for each of `uris` until `stop`
/// resolves, the sink fails (peer gone) or the engine is dropped. A lagging receiver still
/// produces one notification ("something changed"), which is all `resources/updated` promises.
pub(crate) async fn forward(
    mut changes: broadcast::Receiver<DocChange>,
    sink: ChangeSink,
    uris: Vec<String>,
    stop: impl Future<Output = ()>,
) {
    tokio::pin!(stop);
    loop {
        tokio::select! {
            () = &mut stop => return,
            change = changes.recv() => match change {
                Ok(_) | Err(RecvError::Lagged(_)) => {
                    for uri in &uris {
                        if !sink.resource_updated(uri).await {
                            return;
                        }
                    }
                }
                Err(RecvError::Closed) => return,
            },
        }
    }
}
