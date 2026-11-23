// SPDX-License-Identifier: GPL-3.0-or-later
//! The JSON-RPC endpoint against a scripted backend (WP-1I.02): framing, the envelope rules,
//! routing and ordering, subscriptions, backpressure (close 4001) and connection lifetime. The
//! engine-backed acceptance tests are in `rpc_engine.rs`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names,
    clippy::too_many_lines
)]

mod rpc_support;

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cutweave_api::rpc::{
    self, BoxFuture, CLOSE_GOING_AWAY, CLOSE_RESYNC_REQUIRED, RpcBackend, RpcCall, RpcConfig,
    RpcConnection, RpcEvent, RpcEventError, RpcEvents, RpcReply, RpcSnapshot, RpcStatus, codes,
};
use cutweave_api::{ApiConfig, ApiServer, Credential, Mounts, RunningServer};
use cutweave_commands::{ActorCtx, CmdError, ConflictReason, ConnId, Registry, Rev, SCHEMA_HASH};
use rpc_support::{Rpc, close_code, upgrade_request, with_meta, with_rid};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

// ----- a scripted backend ----------------------------------------------------------------------

/// What the fake engine saw: `start <name> <tag>` / `end <name> <tag>` lines.
type Log = Arc<Mutex<Vec<String>>>;

struct Fake {
    registry: Arc<Registry>,
    events: Mutex<Option<broadcast::Sender<RpcEvent>>>,
    /// Held for writing, it stops the hub from reading events (to make it lag).
    gate: Arc<tokio::sync::RwLock<()>>,
    event_capacity: usize,
    log: Log,
    open: Arc<AtomicUsize>,
    next_conn: AtomicU64,
    rev: Arc<AtomicU64>,
}

impl Fake {
    fn new(event_capacity: usize) -> Arc<Fake> {
        let (tx, _) = broadcast::channel(event_capacity);
        Arc::new(Fake {
            registry: Arc::new(cutweave_ops::testing::registry()),
            events: Mutex::new(Some(tx)),
            gate: Arc::default(),
            event_capacity,
            log: Arc::default(),
            open: Arc::default(),
            next_conn: AtomicU64::new(100),
            rev: Arc::default(),
        })
    }

    fn emit(&self, name: &'static str, payload: Value, origin: Option<ConnId>) {
        if let Some(tx) = self.events.lock().unwrap().as_ref() {
            let _ = tx.send(RpcEvent {
                name: name.into(),
                payload,
                origin,
            });
        }
    }

    /// Ends the event fan-out (the engine stopped).
    fn stop_events(&self) {
        self.events.lock().unwrap().take();
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

struct FakeConn {
    ctx: ActorCtx,
    log: Log,
    open: Arc<AtomicUsize>,
    rev: Arc<AtomicU64>,
}

impl Drop for FakeConn {
    fn drop(&mut self) {
        self.open.fetch_sub(1, Ordering::SeqCst);
    }
}

impl RpcConnection for FakeConn {
    fn ctx(&self) -> &ActorCtx {
        &self.ctx
    }

    /// Echoes the call. Params: `sleep_ms` delays it, `fail` makes it fail, `tag` names it in
    /// the log.
    fn call(&self, call: RpcCall) -> BoxFuture<'static, Result<RpcReply, CmdError>> {
        let log = Arc::clone(&self.log);
        let rev = Arc::clone(&self.rev);
        Box::pin(async move {
            let tag = call.params["tag"].as_str().unwrap_or("").to_owned();
            log.lock()
                .unwrap()
                .push(format!("start {} {tag}", call.name));
            if let Some(ms) = call.params["sleep_ms"].as_u64() {
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
            log.lock().unwrap().push(format!("end {} {tag}", call.name));
            if call.params["fail"] == "conflict" {
                return Err(CmdError::conflict(ConflictReason::StaleRev, "stale").with_rev(Rev(7)));
            }
            let now = Rev(rev.fetch_add(1, Ordering::SeqCst) + 1);
            Ok(RpcReply {
                output: json!({
                    "name": call.name,
                    "params": call.params,
                    "request_id": call.request_id.map(|r| r.to_string()),
                    "expected_rev": call.expected_rev,
                    "tx": call.tx.map(|t| t.to_string()),
                }),
                rev: now,
                committed_rev: now,
                request_id: call.request_id,
                tx_id: None,
            })
        })
    }
}

struct FakeEvents(broadcast::Receiver<RpcEvent>, Arc<tokio::sync::RwLock<()>>);

impl RpcEvents for FakeEvents {
    fn recv(&mut self) -> BoxFuture<'_, Result<RpcEvent, RpcEventError>> {
        Box::pin(async move {
            let gate = Arc::clone(&self.1);
            let _open = gate.read().await;
            match self.0.recv().await {
                Ok(e) => Ok(e),
                Err(broadcast::error::RecvError::Lagged(n)) => Err(RpcEventError::Lagged(n)),
                Err(broadcast::error::RecvError::Closed) => Err(RpcEventError::Closed),
            }
        })
    }
}

impl RpcBackend for Fake {
    fn version(&self) -> String {
        "9.9.9-fake".into()
    }

    fn registry(&self) -> Arc<Registry> {
        Arc::clone(&self.registry)
    }

    fn connect(&self, credential: &Credential) -> Result<Arc<dyn RpcConnection>, CmdError> {
        let conn = ConnId::new(self.next_conn.fetch_add(1, Ordering::SeqCst));
        let ctx = credential
            .actor_ctx(conn)
            .map_err(|e| CmdError::invalid("", e.to_string()))?;
        self.open.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(FakeConn {
            ctx,
            log: Arc::clone(&self.log),
            open: Arc::clone(&self.open),
            rev: Arc::clone(&self.rev),
        }))
    }

    fn subscribe(&self) -> Box<dyn RpcEvents> {
        let rx = match self.events.lock().unwrap().as_ref() {
            Some(tx) => tx.subscribe(),
            None => broadcast::channel(self.event_capacity).1,
        };
        Box::new(FakeEvents(rx, Arc::clone(&self.gate)))
    }

    fn status(&self) -> Result<RpcStatus, CmdError> {
        let rev = Rev(self.rev.load(Ordering::SeqCst));
        Ok(RpcStatus {
            rev,
            committed_rev: rev,
            project: json!({"state": "no_project", "name": "Fake"}),
            shm: None,
        })
    }

    fn snapshot(&self) -> Result<RpcSnapshot, CmdError> {
        let rev = Rev(self.rev.load(Ordering::SeqCst));
        Ok(RpcSnapshot {
            rev,
            committed_rev: rev,
            project: json!({"name": "Fake"}),
        })
    }
}

struct Server {
    fake: Arc<Fake>,
    api: RunningServer,
    _dir: tempfile::TempDir,
}

impl Server {
    async fn start(config: RpcConfig) -> Server {
        Server::with_fake(Fake::new(1024), config).await
    }

    async fn with_fake(fake: Arc<Fake>, config: RpcConfig) -> Server {
        let dir = tempfile::tempdir().unwrap();
        let server = ApiServer::start(ApiConfig {
            run_dir: Some(dir.path().to_path_buf()),
            mounts: Mounts {
                rpc: Some(rpc::router(
                    Arc::clone(&fake) as Arc<dyn RpcBackend>,
                    config,
                )),
                ..Mounts::default()
            },
            ..ApiConfig::default()
        })
        .await
        .unwrap();
        Server {
            fake,
            api: server,
            _dir: dir,
        }
    }

    fn token(&self) -> String {
        self.api.credentials().agent_tokens()[0]
            .token
            .expose()
            .to_owned()
    }

    async fn client(&self) -> Rpc {
        Rpc::connect(self.api.port(), &self.token()).await
    }
}

async fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ----- framing ---------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn framing_errors_follow_json_rpc() {
    let s = Server::start(RpcConfig::default()).await;
    let mut c = s.client().await;

    c.send_text("{not json").await;
    let r = c.next().await.unwrap();
    assert_eq!(r["id"], Value::Null);
    assert_eq!(r["error"]["code"], codes::PARSE_ERROR);
    assert_eq!(r["error"]["data"]["code"], "invalid");

    c.send(&json!({"jsonrpc": "1.0", "id": 4, "method": "timeline.get"}))
        .await;
    let r = c.next().await.unwrap();
    assert_eq!(r["id"], 4);
    assert_eq!(r["error"]["code"], codes::INVALID_REQUEST);

    let e = c.error("clip.nope", json!({})).await;
    assert_eq!(e["code"], codes::METHOD_NOT_FOUND);
    assert_eq!(e["data"]["code"], "not_found");
    assert_eq!(e["data"]["kind"], "command");

    let e = c.error("rpc.nope", json!({})).await;
    assert_eq!(e["code"], codes::METHOD_NOT_FOUND);

    c.send(&json!({"jsonrpc": "2.0", "id": 5, "method": "timeline.get", "params": [1]}))
        .await;
    let r = c.next().await.unwrap();
    assert_eq!(r["error"]["code"], codes::INVALID_PARAMS);

    c.send_message(Message::binary(vec![1u8, 2, 3])).await;
    let r = c.next().await.unwrap();
    assert_eq!(r["error"]["code"], codes::INVALID_REQUEST);

    // A notification gets no response: the next message answers the request after it.
    c.send(&json!({"jsonrpc": "2.0", "method": "timeline.get", "params": {}}))
        .await;
    let out = c.call("timeline.get", json!({"tag": "after"})).await;
    assert_eq!(out["params"]["tag"], "after");
}

#[tokio::test(flavor = "multi_thread")]
async fn batches_answer_in_order_without_notifications() {
    let s = Server::start(RpcConfig::default()).await;
    let mut c = s.client().await;
    c.send(&json!([
        {"jsonrpc": "2.0", "id": "a", "method": "project.rename",
         "params": with_rid(json!({"tag": "a", "sleep_ms": 50}))},
        {"jsonrpc": "2.0", "method": "timeline.get", "params": {}},
        {"jsonrpc": "2.0", "id": "b", "method": "timeline.get", "params": {"tag": "b"}},
        {"jsonrpc": "2.0", "id": "c", "method": "clip.nope"},
        {"bogus": true},
    ]))
    .await;
    let r = c.next().await.unwrap();
    let ids: Vec<&Value> = r.as_array().unwrap().iter().map(|x| &x["id"]).collect();
    assert_eq!(ids, [&json!("a"), &json!("b"), &json!("c"), &Value::Null]);
    assert_eq!(r[0]["result"]["params"]["tag"], "a");
    assert_eq!(r[2]["error"]["code"], codes::METHOD_NOT_FOUND);
    assert_eq!(r[3]["error"]["code"], codes::INVALID_REQUEST);

    c.send(&json!([])).await;
    let r = c.next().await.unwrap();
    assert_eq!(r["error"]["code"], codes::INVALID_REQUEST);

    // Only notifications: no response at all.
    c.send(&json!([{"jsonrpc": "2.0", "method": "timeline.get", "params": {}}]))
        .await;
    let out = c.call("timeline.get", json!({"tag": "next"})).await;
    assert_eq!(out["params"]["tag"], "next");
}

// ----- the envelope ----------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn mutating_calls_need_a_request_id() {
    let s = Server::start(RpcConfig::default()).await;
    let mut c = s.client().await;
    let e = c.error("project.rename", json!({"name": "x"})).await;
    assert_eq!(e["code"], codes::INVALID_PARAMS);
    assert_eq!(e["data"]["code"], "invalid");
    assert_eq!(e["data"]["path"], "/_meta/request_id");
    assert!(s.fake.log().is_empty(), "the call never reached the engine");

    // Read-only entries and `view.*` need none.
    c.call("timeline.get", json!({})).await;
    c.call("job.wait", json!({})).await;
    c.call("view.set_playhead", json!({})).await;
    c.call("rpc.hello", json!({"client": "test", "version": "1"}))
        .await;
    c.call("project.snapshot", json!({})).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_envelope_reaches_the_engine_and_comes_back_as_meta() {
    let s = Server::start(RpcConfig::default()).await;
    let mut c = s.client().await;
    let rid = rpc_support::request_id();
    let out = c
        .call(
            "project.rename",
            with_meta(
                json!({"name": "x"}),
                json!({"request_id": rid, "expected_rev": 0, "tx": "tx_3", "actor": "user"}),
            ),
        )
        .await;
    assert_eq!(out["request_id"], rid);
    assert_eq!(out["expected_rev"], 0);
    assert_eq!(out["tx"], "tx_3");
    assert_eq!(
        out["params"],
        json!({"name": "x"}),
        "`_meta` is not a param"
    );
    assert_eq!(out["_meta"]["rev"], 1);
    assert_eq!(out["_meta"]["committed_rev"], 1);
    assert_eq!(out["_meta"]["request_id"], rid);

    let e = c
        .error(
            "project.rename",
            with_meta(json!({}), json!({"request_id": "x"})),
        )
        .await;
    assert_eq!(e["data"]["path"], "/_meta/request_id");
    let e = c
        .error("project.rename", with_rid(json!({"fail": "conflict"})))
        .await;
    assert_eq!(e["code"], codes::CONFLICT);
    assert_eq!(e["data"]["code"], "conflict");
    assert_eq!(e["data"]["reason"], "stale_rev");
    assert_eq!(e["data"]["current_rev"], 7);
}

#[tokio::test(flavor = "multi_thread")]
async fn hello_reports_the_engine_and_the_credential_actor() {
    let s = Server::start(RpcConfig::default()).await;
    let mut c = s.client().await;
    let hello = c
        .call("rpc.hello", json!({"client": "test", "version": "1.0"}))
        .await;
    assert_eq!(hello["engine_version"], "9.9.9-fake");
    assert_eq!(hello["schema_hash"], SCHEMA_HASH);
    assert_eq!(hello["rev"], 0);
    assert_eq!(hello["actor"], "agent:cli");
    assert_eq!(hello["project"]["name"], "Fake");
    let e = c.error("rpc.hello", json!({"client": 3})).await;
    assert_eq!(e["code"], codes::INVALID_PARAMS);

    let snap = c.call("project.snapshot", json!({})).await;
    assert_eq!(
        snap,
        json!({"rev": 0, "committed_rev": 0, "project": {"name": "Fake"}})
    );
    let e = c.error("project.snapshot", json!({"x": 1})).await;
    assert_eq!(e["data"]["path"], "/x");
}

// ----- ordering --------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn mutations_run_in_order_and_reads_wait_for_earlier_mutations() {
    let s = Server::start(RpcConfig::default()).await;
    let mut c = s.client().await;
    // A slow mutation, a fast one, then a read: the fast mutation waits for the slow one, the
    // read for both.
    let a = c
        .start(
            "project.rename",
            with_rid(json!({"tag": "a", "sleep_ms": 150})),
        )
        .await;
    let b = c
        .start("project.rename", with_rid(json!({"tag": "b"})))
        .await;
    let r = c.start("timeline.get", json!({"tag": "r"})).await;
    for id in [a, b, r] {
        assert!(c.response(id).await.get("result").is_some());
    }
    assert_eq!(
        s.fake.log(),
        [
            "start project.rename a",
            "end project.rename a",
            "start project.rename b",
            "end project.rename b",
            "start timeline.get r",
            "end timeline.get r",
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slow_read_does_not_hold_up_later_mutations() {
    let s = Server::start(RpcConfig::default()).await;
    let mut c = s.client().await;
    let wait = c
        .start("job.wait", json!({"tag": "w", "sleep_ms": 400}))
        .await;
    let m = c
        .start("project.rename", with_rid(json!({"tag": "m"})))
        .await;
    // The mutation's response comes first.
    let first = c.next().await.unwrap();
    assert_eq!(first["id"], m);
    let second = c.next().await.unwrap();
    assert_eq!(second["id"], wait);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_in_flight_limit_stops_reading_the_socket() {
    let s = Server::start(RpcConfig {
        max_in_flight: 1,
        ..RpcConfig::default()
    })
    .await;
    let mut c = s.client().await;
    let slow = c
        .start("job.wait", json!({"tag": "slow", "sleep_ms": 300}))
        .await;
    let fast = c.start("timeline.get", json!({"tag": "fast"})).await;
    // With one request in flight the fast read is not even read until the slow one is done.
    assert_eq!(c.next().await.unwrap()["id"], slow);
    assert_eq!(c.next().await.unwrap()["id"], fast);
}

// ----- events ----------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn subscriptions_filter_events() {
    let s = Server::start(RpcConfig::default()).await;
    let mut c = s.client().await;
    let e = c.error("rpc.subscribe", json!({})).await;
    assert_eq!(e["data"]["path"], "/events");
    let e = c
        .error("rpc.subscribe", json!({"events": ["doc.patched", "Bad"]}))
        .await;
    assert_eq!(e["data"]["path"], "/events/1");
    let e = c
        .error("rpc.subscribe", json!({"events": ["doc.patched"], "x": 1}))
        .await;
    assert_eq!(e["data"]["path"], "/x");

    let out = c.subscribe(&["doc.patched", "job.progress"]).await;
    assert_eq!(out["events"], json!(["doc.patched", "job.progress"]));
    s.fake.emit("media.indexed", json!({"n": 0}), None);
    s.fake.emit("doc.patched", json!({"n": 1}), None);
    assert_eq!(c.event("doc.patched").await, json!({"n": 1}));
    let out = c
        .call("rpc.unsubscribe", json!({"events": ["doc.patched"]}))
        .await;
    assert_eq!(out["events"], json!(["job.progress"]));
    s.fake.emit("doc.patched", json!({"n": 2}), None);
    s.fake.emit("job.progress", json!({"n": 3}), None);
    assert_eq!(c.event("job.progress").await, json!({"n": 3}));
    assert!(c.notes.is_empty(), "nothing else arrived: {:?}", c.notes);

    let out = c.subscribe(&["*"]).await;
    assert_eq!(out["events"], json!(["*"]));
    s.fake.emit("media.indexed", json!({"n": 4}), None);
    assert_eq!(c.event("media.indexed").await, json!({"n": 4}));
    let out = c.call("rpc.unsubscribe", json!({})).await;
    assert_eq!(out["events"], json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_stops_reading_is_closed_with_resync_required() {
    let s = Server::start(RpcConfig {
        event_queue: 4,
        ..RpcConfig::default()
    })
    .await;
    let mut slow = s.client().await;
    let mut fine = s.client().await;
    slow.subscribe(&["*"]).await;
    fine.subscribe(&["job.progress"]).await;
    // 64 KiB events until the slow client's socket buffers and its 4-event queue are full.
    let big = "x".repeat(64 << 10);
    for i in 0..400 {
        s.fake
            .emit("doc.patched", json!({"i": i, "pad": big}), None);
        if i % 50 == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    let frame = slow.until_closed().await.expect("a close frame");
    assert_eq!(close_code(&frame), CLOSE_RESYNC_REQUIRED);
    assert_eq!(frame.reason.as_str(), "resync_required");
    // Other connections are unaffected.
    s.fake.emit("job.progress", json!({"ok": true}), None);
    assert_eq!(fine.event("job.progress").await, json!({"ok": true}));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lagging_hub_closes_subscribed_clients_with_resync_required() {
    // The hub's own subscription holds 2 events; it misses some while it is held up.
    let s = Server::with_fake(Fake::new(2), RpcConfig::default()).await;
    let mut subscribed = s.client().await;
    let mut idle = s.client().await;
    subscribed.subscribe(&["doc.patched"]).await;
    // The hub is parked in `recv`, holding the gate's read side. Queue a writer: the next event
    // wakes the hub, the writer gets the gate, and the hub cannot read the burst that follows.
    let gate = Arc::clone(&s.fake.gate);
    let (held_tx, held_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let _held = gate.write().await;
        let _ = held_tx.send(());
        let _ = release_rx.await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    s.fake.emit("doc.patched", json!({"first": true}), None);
    held_rx.await.unwrap();
    for i in 0..8 {
        s.fake.emit("doc.patched", json!({"i": i}), None);
    }
    release_tx.send(()).unwrap();
    let frame = subscribed.until_closed().await.expect("a close frame");
    assert_eq!(close_code(&frame), CLOSE_RESYNC_REQUIRED);
    // An unsubscribed connection keeps working.
    idle.call("timeline.get", json!({})).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_engine_stopping_closes_connections_with_going_away() {
    let s = Server::start(RpcConfig::default()).await;
    let mut c = s.client().await;
    c.subscribe(&["doc.patched"]).await;
    s.fake.stop_events();
    let frame = c.until_closed().await.expect("a close frame");
    assert_eq!(close_code(&frame), CLOSE_GOING_AWAY);
}

#[tokio::test(flavor = "multi_thread")]
async fn view_changes_skip_the_connection_that_made_them() {
    let s = Server::start(RpcConfig::default()).await;
    let mut a = s.client().await;
    let mut b = s.client().await;
    a.subscribe(&["view.changed", "doc.patched"]).await;
    b.subscribe(&["view.changed"]).await;
    // The fake numbers connections from 100 in connect order.
    s.fake
        .emit("view.changed", json!({"by": "a"}), Some(ConnId::new(100)));
    s.fake.emit("doc.patched", json!({"marker": true}), None);
    assert_eq!(b.event("view.changed").await, json!({"by": "a"}));
    // `a` gets the marker but never its own view change.
    assert_eq!(a.event("doc.patched").await, json!({"marker": true}));
    assert!(a.notes.is_empty(), "{:?}", a.notes);
}

// ----- connections -----------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn each_socket_owns_one_connection_closed_with_it() {
    let s = Server::start(RpcConfig::default()).await;
    let a = s.client().await;
    let mut b = s.client().await;
    b.call("timeline.get", json!({})).await;
    assert_eq!(s.fake.open.load(Ordering::SeqCst), 2);
    a.close().await;
    wait_until("the closed socket's connection to drop", || {
        s.fake.open.load(Ordering::SeqCst) == 1
    })
    .await;
    // A slow call in flight keeps the connection until it returns.
    b.start("project.rename", with_rid(json!({"sleep_ms": 300})))
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(b);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(s.fake.open.load(Ordering::SeqCst), 1);
    wait_until("the last connection to drop", || {
        s.fake.open.load(Ordering::SeqCst) == 0
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_subprotocol_is_echoed() {
    let s = Server::start(RpcConfig::default()).await;
    let protocol = format!("cutweave.token.{}", s.token());
    let mut req = upgrade_request(s.api.port(), "unused");
    req.headers_mut().remove("Authorization");
    req.headers_mut()
        .insert("Sec-WebSocket-Protocol", protocol.parse().unwrap());
    let (mut c, response) = Rpc::connect_with(req).await;
    assert_eq!(
        response
            .headers()
            .get("sec-websocket-protocol")
            .unwrap()
            .to_str()
            .unwrap(),
        protocol
    );
    assert_eq!(
        c.call("rpc.hello", json!({"client": "browser", "version": "1"}))
            .await["actor"],
        "agent:cli"
    );
}
