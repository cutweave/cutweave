// SPDX-License-Identifier: GPL-3.0-or-later
//! The event hub (WP-1I.02): one subscription to the engine's event fan-out per mounted
//! router, fanned out to the WebSocket connections that subscribed (`rpc.subscribe`).
//!
//! Each event is serialized once, into a notification shared by every connection that wants
//! it. Every connection has a **bounded** event queue: when it is full (the client does not
//! read fast enough), the hub stops delivering to it and asks it to close with code 4001
//! `resync_required`. When the hub itself lags behind the engine (it missed events), every
//! subscribed connection is closed the same way. Either way the client reconnects and resyncs
//! from `project.snapshot`; nothing is ever dropped silently.
//!
//! `view.changed` is not sent back to the connection that made the change (the event's
//! `origin`).

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use axum::extract::ws::Utf8Bytes;
use cutweave_commands::ConnId;
use tokio::sync::{mpsc, watch};

use super::wire::notification_text;
use super::{RpcEvent, RpcEventError, RpcEvents};

/// Why the server closes a WebSocket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloseReason {
    /// The connection's event queue overflowed, or the hub missed events: the client must
    /// resync (close code 4001 `resync_required`).
    ResyncRequired,
    /// The engine's event fan-out ended (close code 1001).
    EngineStopped,
}

impl CloseReason {
    /// The WebSocket close code.
    pub(crate) const fn code(self) -> u16 {
        match self {
            CloseReason::ResyncRequired => super::CLOSE_RESYNC_REQUIRED,
            CloseReason::EngineStopped => super::CLOSE_GOING_AWAY,
        }
    }

    /// The close frame's reason text.
    pub(crate) const fn reason(self) -> &'static str {
        match self {
            CloseReason::ResyncRequired => "resync_required",
            CloseReason::EngineStopped => "engine_stopped",
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The events a connection subscribed to: exact names, or all (`*`).
#[derive(Debug, Default)]
struct Subscriptions {
    all: bool,
    names: BTreeSet<String>,
}

impl Subscriptions {
    fn wants(&self, name: &str) -> bool {
        self.all || self.names.contains(name)
    }

    fn any(&self) -> bool {
        self.all || !self.names.is_empty()
    }

    fn list(&self) -> Vec<String> {
        if self.all {
            vec![super::ALL_EVENTS.to_owned()]
        } else {
            self.names.iter().cloned().collect()
        }
    }
}

/// What the hub and a connection share: its subscriptions, its bounded event queue and the
/// signal that asks it to close.
#[derive(Debug)]
pub(crate) struct Subscriber {
    /// The engine connection id (to skip its own `view.changed`).
    engine_conn: ConnId,
    subs: Mutex<Subscriptions>,
    events: mpsc::Sender<Utf8Bytes>,
    close: watch::Sender<Option<CloseReason>>,
}

impl Subscriber {
    /// A subscriber for the engine connection `engine_conn`, delivering into `events`.
    pub(crate) fn new(engine_conn: ConnId, events: mpsc::Sender<Utf8Bytes>) -> Subscriber {
        Subscriber {
            engine_conn,
            subs: Mutex::new(Subscriptions::default()),
            events,
            close: watch::Sender::new(None),
        }
    }

    /// Adds `names` (`*` = every event); returns the subscriptions now held.
    pub(crate) fn subscribe(&self, names: &[String]) -> Vec<String> {
        let mut subs = lock(&self.subs);
        for name in names {
            if name == super::ALL_EVENTS {
                subs.all = true;
            } else {
                subs.names.insert(name.clone());
            }
        }
        subs.list()
    }

    /// Removes `names` (`*` = the wildcard only; `None` = everything); returns the
    /// subscriptions now held.
    pub(crate) fn unsubscribe(&self, names: Option<&[String]>) -> Vec<String> {
        let mut subs = lock(&self.subs);
        match names {
            None => *subs = Subscriptions::default(),
            Some(names) => {
                for name in names {
                    if name == super::ALL_EVENTS {
                        subs.all = false;
                    } else {
                        subs.names.remove(name);
                    }
                }
            }
        }
        subs.list()
    }

    /// A receiver of the close request.
    pub(crate) fn closed(&self) -> watch::Receiver<Option<CloseReason>> {
        self.close.subscribe()
    }

    /// Asks the connection to close (the first reason wins).
    pub(crate) fn close(&self, reason: CloseReason) {
        self.close.send_if_modified(|current| {
            if current.is_none() {
                *current = Some(reason);
                true
            } else {
                false
            }
        });
    }

    fn wants(&self, name: &str, origin: Option<ConnId>) -> bool {
        origin != Some(self.engine_conn) && lock(&self.subs).wants(name)
    }

    fn subscribed(&self) -> bool {
        lock(&self.subs).any()
    }
}

/// The connections of one mounted router and the task that feeds them.
#[derive(Debug, Default)]
pub(crate) struct Hub {
    subscribers: Mutex<HashMap<u64, Arc<Subscriber>>>,
    next: AtomicU64,
    started: AtomicBool,
}

impl Hub {
    /// Starts the task reading `events` (once; later calls do nothing). Must run inside the
    /// engine runtime. The task ends when the engine's fan-out closes or the hub is dropped.
    pub(crate) fn start(self: &Arc<Self>, events: impl FnOnce() -> Box<dyn RpcEvents>) {
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let hub = Arc::downgrade(self);
        tokio::spawn(run(hub, events()));
    }

    /// Registers a connection; the key unregisters it.
    pub(crate) fn register(&self, subscriber: Arc<Subscriber>) -> u64 {
        let key = self.next.fetch_add(1, Ordering::Relaxed);
        lock(&self.subscribers).insert(key, subscriber);
        key
    }

    /// Stops delivering to the connection `key`.
    pub(crate) fn unregister(&self, key: u64) {
        lock(&self.subscribers).remove(&key);
    }

    /// How many connections are registered.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        lock(&self.subscribers).len()
    }

    /// Delivers `event` to every connection that wants it; full queues close theirs.
    pub(crate) fn publish(&self, event: RpcEvent) {
        let RpcEvent {
            name,
            payload,
            origin,
        } = event;
        let mut payload = Some(payload);
        let mut text: Option<Utf8Bytes> = None;
        let mut subscribers = lock(&self.subscribers);
        subscribers.retain(|_, s| {
            if !s.wants(&name, origin) {
                return true;
            }
            let text = text.get_or_insert_with(|| {
                Utf8Bytes::from(notification_text(&name, payload.take().unwrap_or_default()))
            });
            match s.events.try_send(text.clone()) {
                Ok(()) => true,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::warn!(event = %name, "rpc client too slow; closing it (resync_required)");
                    s.close(CloseReason::ResyncRequired);
                    false
                }
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            }
        });
    }

    /// The hub missed `missed` events: every subscribed connection must resync.
    pub(crate) fn lagged(&self, missed: u64) {
        let mut subscribers = lock(&self.subscribers);
        let before = subscribers.len();
        subscribers.retain(|_, s| {
            if s.subscribed() {
                s.close(CloseReason::ResyncRequired);
                false
            } else {
                true
            }
        });
        tracing::warn!(
            missed,
            closed = before - subscribers.len(),
            "rpc event hub lagged; closing subscribed clients (resync_required)"
        );
    }

    /// The engine's fan-out ended: closes every connection.
    pub(crate) fn close_all(&self, reason: CloseReason) {
        for (_, s) in lock(&self.subscribers).drain() {
            s.close(reason);
        }
    }
}

async fn run(hub: Weak<Hub>, mut events: Box<dyn RpcEvents>) {
    loop {
        let next = events.recv().await;
        let Some(hub) = hub.upgrade() else {
            return;
        };
        match next {
            Ok(event) => hub.publish(event),
            Err(RpcEventError::Lagged(missed)) => hub.lagged(missed),
            Err(RpcEventError::Closed) => {
                hub.close_all(CloseReason::EngineStopped);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use serde_json::json;

    use super::*;

    fn event(name: &'static str, origin: Option<ConnId>) -> RpcEvent {
        RpcEvent {
            name: name.into(),
            payload: json!({"n": 1}),
            origin,
        }
    }

    fn subscriber(conn: u64, capacity: usize) -> (Arc<Subscriber>, mpsc::Receiver<Utf8Bytes>) {
        let (tx, rx) = mpsc::channel(capacity);
        (Arc::new(Subscriber::new(ConnId::new(conn), tx)), rx)
    }

    #[test]
    fn delivers_only_subscribed_events_serialized_once() {
        let hub = Hub::default();
        let (a, mut a_rx) = subscriber(1, 8);
        let (b, mut b_rx) = subscriber(2, 8);
        assert_eq!(a.subscribe(&["doc.patched".into()]), vec!["doc.patched"]);
        assert_eq!(b.subscribe(&["*".into()]), vec!["*"]);
        hub.register(Arc::clone(&a));
        hub.register(Arc::clone(&b));
        hub.publish(event("doc.patched", None));
        hub.publish(event("job.progress", None));
        let first = a_rx.try_recv().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(first.as_str()).unwrap(),
            json!({"jsonrpc": "2.0", "method": "doc.patched", "params": {"n": 1}})
        );
        assert!(a_rx.try_recv().is_err());
        assert_eq!(b_rx.try_recv().unwrap(), first);
        assert!(b_rx.try_recv().unwrap().as_str().contains("job.progress"));
    }

    #[test]
    fn view_changes_skip_the_connection_that_made_them() {
        let hub = Hub::default();
        let (a, mut a_rx) = subscriber(1, 8);
        let (b, mut b_rx) = subscriber(2, 8);
        a.subscribe(&["view.changed".into()]);
        b.subscribe(&["view.changed".into()]);
        hub.register(a);
        hub.register(b);
        hub.publish(event("view.changed", Some(ConnId::new(1))));
        assert!(a_rx.try_recv().is_err());
        assert!(b_rx.try_recv().is_ok());
    }

    #[test]
    fn a_full_queue_closes_only_that_connection() {
        let hub = Hub::default();
        let (slow, _slow_rx) = subscriber(1, 2);
        let (fast, mut fast_rx) = subscriber(2, 16);
        slow.subscribe(&["*".into()]);
        fast.subscribe(&["*".into()]);
        let slow_closed = slow.closed();
        hub.register(Arc::clone(&slow));
        hub.register(Arc::clone(&fast));
        for _ in 0..3 {
            hub.publish(event("doc.patched", None));
        }
        assert_eq!(*slow_closed.borrow(), Some(CloseReason::ResyncRequired));
        assert_eq!(hub.len(), 1, "the slow connection gets nothing more");
        for _ in 0..3 {
            assert!(fast_rx.try_recv().is_ok());
        }
        assert_eq!(*fast.closed().borrow(), None);
    }

    #[test]
    fn lagging_closes_subscribed_connections_only() {
        let hub = Hub::default();
        let (subscribed, _rx1) = subscriber(1, 4);
        let (idle, _rx2) = subscriber(2, 4);
        subscribed.subscribe(&["doc.patched".into()]);
        hub.register(Arc::clone(&subscribed));
        hub.register(Arc::clone(&idle));
        hub.lagged(10);
        assert_eq!(
            *subscribed.closed().borrow(),
            Some(CloseReason::ResyncRequired)
        );
        assert_eq!(*idle.closed().borrow(), None);
        assert_eq!(hub.len(), 1);
        hub.close_all(CloseReason::EngineStopped);
        assert_eq!(*idle.closed().borrow(), Some(CloseReason::EngineStopped));
        // The first reason wins.
        assert_eq!(
            *subscribed.closed().borrow(),
            Some(CloseReason::ResyncRequired)
        );
    }

    #[test]
    fn unsubscribe_removes_names_or_everything() {
        let (s, _rx) = subscriber(1, 1);
        s.subscribe(&["a.b".into(), "c.d".into(), "*".into()]);
        assert_eq!(s.unsubscribe(Some(&["*".into()])), vec!["a.b", "c.d"]);
        assert_eq!(s.unsubscribe(Some(&["a.b".into()])), vec!["c.d"]);
        assert_eq!(s.unsubscribe(None), Vec::<String>::new());
        assert!(!s.subscribed());
    }
}
