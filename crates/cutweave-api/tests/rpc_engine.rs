// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1I.02 acceptance tests: JSON-RPC over WebSocket against a real engine started in-process
//! (`Engine::start`, headless, a free port, `/rpc` mounted through `ApiOptions.mounts`), driven
//! by a tokio-tungstenite client with the `cli` agent token.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names,
    clippy::too_many_lines
)]

mod rpc_support;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cutweave_api::rpc::{RpcConfig, codes};
use cutweave_commands::{DocEvent, Rev, SCHEMA_HASH, json_eq};
use cutweave_engine::project_io::ProjectStore;
use cutweave_engine::{Engine, EngineConfig, EngineMode, Subsystems, VERSION, rpc_mounts};
use cutweave_model::Project;
use cutweave_ops::media::{FakeProbe, MediaProbe};
use cutweave_ops::testing::{arb_command_for, replay_events};
use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::{Config as ProptestConfig, TestRunner};
use rpc_support::{Rpc, request_id, with_meta, with_rid};
use serde_json::{Value, json};

// ----- fixture ---------------------------------------------------------------------------------

/// The media subsystem of these tests: a fake prober serving one 10-minute video.
struct Media(Arc<FakeProbe>);

impl Subsystems for Media {
    fn media_probe(&self) -> Option<Arc<dyn MediaProbe>> {
        Some(Arc::clone(&self.0) as Arc<dyn MediaProbe>)
    }
}

struct Fixture {
    engine: Engine,
    token: String,
    port: u16,
    /// A video file the fake prober knows (inside the allowed roots).
    clip: String,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn start() -> Fixture {
        Fixture::start_with(RpcConfig::default(), &|_, _| {})
    }

    /// A headless engine serving `/rpc` with `config`; `tweak` adjusts the engine configuration
    /// (it gets the test's temp dir).
    fn start_with(config: RpcConfig, tweak: &dyn Fn(&mut EngineConfig, &Path)) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        let clip = media.join("clip.mp4").to_string_lossy().into_owned();
        let mut c = EngineConfig::new(EngineMode::Headless);
        c.port = Some(0);
        tweak(&mut c, dir.path());
        c.api.run_dir = Some(dir.path().join("run"));
        c.api.mounts = Some(rpc_mounts(config));
        c.allowed_roots = vec![dir.path().to_path_buf()];
        c.subsystems = Some(Box::new(Media(Arc::new(
            FakeProbe::new().with(&clip, FakeProbe::video(&clip, 600, false)),
        ))));
        let engine = Engine::start(c).unwrap();
        let token = engine
            .credentials()
            .unwrap()
            .agent_tokens()
            .iter()
            .find(|t| t.label == "cli")
            .unwrap()
            .token
            .expose()
            .to_owned();
        Fixture {
            port: engine.port().unwrap(),
            engine,
            token,
            clip,
            _dir: dir,
        }
    }

    async fn client(&self) -> Rpc {
        Rpc::connect(self.port, &self.token).await
    }
}

/// `project.new` + `media.import` of the fixture clip: `(sequence, video track, media)`.
async fn new_project(c: &mut Rpc, clip: &str) -> (Value, Value, Value) {
    let out = c.mutate("project.new", json!({"name": "RPC"})).await;
    let import = c.mutate("media.import", json!({"paths": [clip]})).await;
    (
        out["sequence"].clone(),
        out["video_track"].clone(),
        import["media_ids"][0].clone(),
    )
}

fn rev_of(v: &Value) -> Rev {
    Rev(v.as_u64().unwrap())
}

/// The live revision as `rpc.hello` reports it.
async fn live_rev(c: &mut Rpc) -> Rev {
    let hello = c
        .call("rpc.hello", json!({"client": "rpc-tests", "version": "1"}))
        .await;
    rev_of(&hello["rev"])
}

/// `doc.patched` params as the doc actor's event.
fn doc_event(params: &Value) -> DocEvent {
    let mut v = params.clone();
    v["event"] = json!("doc.patched");
    serde_json::from_value(v).unwrap()
}

/// Receives `doc.patched` until one reaches `rev`, skipping those already in the mirror at
/// `from` (the client protocol); checks that the revisions are consecutive.
async fn patches_until(c: &mut Rpc, from: Rev, rev: Rev) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut at = from;
    while at < rev {
        let p = c.event("doc.patched").await;
        if rev_of(&p["rev"]) <= from {
            continue;
        }
        assert_eq!(rev_of(&p["prev_rev"]), at, "a gap or a reorder: {p}");
        assert_eq!(rev_of(&p["rev"]), at.next(), "revisions step by one: {p}");
        at = rev_of(&p["rev"]);
        out.push(p);
    }
    out
}

/// Asserts that replaying `patches` on `start` gives exactly the snapshot `end`.
fn assert_replays_to(start: &Value, patches: &[Value], end: &Value) {
    let start_doc: Project = serde_json::from_value(start["project"].clone()).unwrap();
    let end_doc: Project = serde_json::from_value(end["project"].clone()).unwrap();
    let events: Vec<DocEvent> = patches.iter().map(doc_event).collect();
    let mirror = replay_events(&start_doc, rev_of(&start["rev"]), &events).unwrap();
    assert_eq!(mirror.rev(), rev_of(&end["rev"]));
    assert!(
        mirror.matches(&end_doc),
        "the mirror differs from project.snapshot"
    );
    assert!(json_eq(mirror.json(), &end["project"]));
}

// ----- events, ordering and replay ---------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn edits_stream_consecutive_patches_that_replay_to_the_snapshot() {
    let f = Fixture::start();
    let mut c = f.client().await;
    c.subscribe(&["doc.patched"]).await;
    let start = c.call("project.snapshot", json!({})).await;

    let (seq, track, media) = new_project(&mut c, &f.clip).await;
    let insert = c
        .mutate(
            "timeline.insert",
            json!({"sequence": seq, "track": track, "media": media, "at": 0, "duration": "10s"}),
        )
        .await;
    let item = insert["clips"][0].clone();
    // A forged actor is ignored: the actor is the credential's.
    let split = c
        .call(
            "clip.split",
            with_meta(
                json!({"item": item, "at": "4s"}),
                json!({"request_id": request_id(), "actor": "user"}),
            ),
        )
        .await;
    let undo = c.mutate("edit.undo", json!({})).await;
    let end = c.call("project.snapshot", json!({})).await;

    let patches = patches_until(&mut c, rev_of(&start["rev"]), rev_of(&end["rev"])).await;
    for p in &patches {
        assert_eq!(p["actor"], "agent:cli");
    }
    // Each command's reply names the revision of its patch, in call order.
    let revs: Vec<&Value> = patches.iter().map(|p| &p["rev"]).collect();
    for out in [&insert, &split, &undo] {
        assert!(revs.contains(&&out["_meta"]["rev"]), "{out}");
    }
    assert!(insert["_meta"]["rev"].as_u64() < split["_meta"]["rev"].as_u64());
    assert!(split["_meta"]["rev"].as_u64() < undo["_meta"]["rev"].as_u64());
    let split_patch = patches
        .iter()
        .find(|p| p["rev"] == split["_meta"]["rev"])
        .unwrap();
    assert_eq!(split_patch["request_id"], split["_meta"]["request_id"]);
    assert_eq!(split_patch["tx_id"], split["_meta"]["tx_id"]);

    assert_replays_to(&start, &patches, &end);
    // The undo removed the split's right half again.
    let e = c.error("item.get", json!({"item": split["right"]})).await;
    assert_eq!(e["data"]["code"], "not_found");
    c.call("item.get", json!({"item": split["left"]})).await;
    f.engine.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn random_command_sequences_replay_to_the_snapshot() {
    let f = Fixture::start();
    let mut c = f.client().await;
    c.subscribe(&["doc.patched", "doc.reset"]).await;
    new_project(&mut c, &f.clip).await;
    let mut runner = TestRunner::new_with_rng(
        ProptestConfig::default(),
        proptest::test_runner::TestRng::deterministic_rng(
            proptest::test_runner::RngAlgorithm::ChaCha,
        ),
    );
    for round in 0..4 {
        let mut start = c.call("project.snapshot", json!({})).await;
        let mut resets = 0;
        for _ in 0..30 {
            let doc: Project = serde_json::from_value(
                c.call("project.snapshot", json!({})).await["project"].clone(),
            )
            .unwrap();
            let (name, params) = arb_command_for(&doc)
                .new_tree(&mut runner)
                .unwrap()
                .current();
            // Successes and failures both keep the stream consistent.
            let _ = c.request(&name, with_rid(params)).await;
            // Drain what arrived; a `doc.reset` restarts the mirror from a new snapshot.
            while let Ok(Some(msg)) = c.next_within(Duration::from_millis(1)).await {
                c.notes.push_back(msg);
            }
            if c.notes.iter().any(|n| n["method"] == "doc.reset") {
                c.notes.clear();
                start = c.call("project.snapshot", json!({})).await;
                resets += 1;
            }
        }
        let end = c.call("project.snapshot", json!({})).await;
        let patches = patches_until(&mut c, rev_of(&start["rev"]), rev_of(&end["rev"])).await;
        assert_replays_to(&start, &patches, &end);
        eprintln!(
            "round {round}: {} patches replayed ({resets} resyncs)",
            patches.len()
        );
    }
    f.engine.shutdown();
}

// ----- idempotency, conflicts, envelope --------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn one_request_id_over_two_connections_applies_once() {
    let f = Fixture::start();
    let mut a = f.client().await;
    let mut b = f.client().await;
    a.mutate("project.new", json!({"name": "Before"})).await;
    let rid = request_id();
    let params = with_meta(json!({"name": "After"}), json!({"request_id": rid}));
    let before = live_rev(&mut a).await;
    let first = a.call("project.rename", params.clone()).await;
    let second = b.call("project.rename", params.clone()).await;
    assert_eq!(first, second, "the repeat returns the original result");
    assert_eq!(live_rev(&mut b).await, before.next(), "applied once");
    assert_eq!(first["_meta"]["request_id"], rid);

    // Resent after a reconnect: still the original result.
    a.close().await;
    let mut c = f.client().await;
    assert_eq!(c.call("project.rename", params).await, first);
    assert_eq!(live_rev(&mut c).await, before.next());

    // The same id with another payload is a conflict.
    let e = c
        .error(
            "project.rename",
            with_meta(json!({"name": "Other"}), json!({"request_id": rid})),
        )
        .await;
    assert_eq!(e["code"], codes::CONFLICT);
    assert_eq!(e["data"]["reason"], "request_id_reused");
    f.engine.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stale_expected_rev_is_a_conflict_carrying_the_current_rev() {
    let f = Fixture::start();
    let mut c = f.client().await;
    let out = c.mutate("project.new", json!({"name": "P"})).await;
    let current = out["_meta"]["rev"].clone();
    let e = c
        .error(
            "project.rename",
            with_meta(
                json!({"name": "Q"}),
                json!({"request_id": request_id(), "expected_rev": 0}),
            ),
        )
        .await;
    assert_eq!(e["code"], codes::CONFLICT);
    assert_eq!(e["data"]["code"], "conflict");
    assert_eq!(e["data"]["reason"], "stale_rev");
    assert_eq!(e["data"]["current_rev"], current);
    assert!(e["message"].as_str().unwrap().contains("stale_rev"));
    // The right revision passes.
    c.call(
        "project.rename",
        with_meta(
            json!({"name": "Q"}),
            json!({"request_id": request_id(), "expected_rev": current}),
        ),
    )
    .await;
    f.engine.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn mutating_calls_without_a_request_id_are_invalid() {
    let f = Fixture::start();
    let mut c = f.client().await;
    let e = c.error("project.new", json!({"name": "P"})).await;
    assert_eq!(e["code"], codes::INVALID_PARAMS);
    assert_eq!(e["data"]["path"], "/_meta/request_id");
    assert_eq!(live_rev(&mut c).await, Rev(0));
    // Queries, read-only engine commands and `view.*` need none.
    c.call("project.info", json!({})).await;
    c.call("edit.history", json!({})).await;
    c.call("view.get", json!({})).await;
    // Errors of the engine come back typed.
    let e = c.error("item.get", json!({"item": "itm_nope"})).await;
    assert!(
        [codes::NOT_FOUND, codes::INVALID_PARAMS].contains(&e["code"].as_i64().unwrap()),
        "{e}"
    );
    f.engine.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn hello_reports_the_engine_schema_and_project() {
    let f = Fixture::start();
    let mut c = f.client().await;
    let hello = c
        .call(
            "rpc.hello",
            json!({"client": "rpc-tests", "version": "1.2.3"}),
        )
        .await;
    assert_eq!(hello["engine_version"], VERSION);
    assert_eq!(hello["schema_hash"], SCHEMA_HASH);
    assert_eq!(hello["rev"], 0);
    assert_eq!(hello["actor"], "agent:cli");
    assert_eq!(hello["project"]["state"], "no_project");
    f.engine.shutdown();
}

// ----- connections -----------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn closing_a_socket_rolls_back_its_open_transaction() {
    let f = Fixture::start();
    let mut a = f.client().await;
    let mut b = f.client().await;
    a.mutate("project.new", json!({"name": "Original"})).await;
    b.subscribe(&["tx.closed"]).await;
    let opened = a
        .mutate("edit.begin_tx", json!({"label": "Drag", "kind": "gesture"}))
        .await;
    let tx = opened["tx"].clone();
    a.call(
        "project.rename",
        with_meta(
            json!({"name": "Dragging"}),
            json!({"request_id": request_id(), "tx": tx}),
        ),
    )
    .await;
    let live = b.call("project.snapshot", json!({})).await;
    assert_eq!(live["project"]["meta"]["name"], "Dragging");
    assert_ne!(live["rev"], live["committed_rev"]);

    a.close().await;
    let closed = b.event("tx.closed").await;
    assert_eq!(closed["tx_id"], tx);
    assert_eq!(closed["outcome"], "rolled_back");
    let after = b.call("project.snapshot", json!({})).await;
    assert_eq!(after["project"]["meta"]["name"], "Original");
    // (The rollback is a new live revision on top of the unchanged committed one.)
    let locks = b.call("edit.locks", json!({})).await;
    assert_eq!(locks["transactions"], json!([]), "{locks}");
    f.engine.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn view_changes_reach_the_other_connections_only() {
    let f = Fixture::start();
    let mut a = f.client().await;
    let mut b = f.client().await;
    a.mutate("project.new", json!({"name": "P"})).await;
    a.subscribe(&["view.changed"]).await;
    b.subscribe(&["view.changed"]).await;
    a.call("view.set_playhead", json!({"at": "1s"})).await;
    let seen = b.event("view.changed").await;
    b.call("view.set_playhead", json!({"at": "2s"})).await;
    // `a` gets `b`'s change first: its own never comes back.
    let to_a = a.event("view.changed").await;
    assert!(
        to_a["view_rev"].as_u64() > seen["view_rev"].as_u64(),
        "{to_a} vs {seen}"
    );
    f.engine.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn fifty_connections_with_a_hundred_commands_each_commit_every_success_once() {
    const CLIENTS: usize = 50;
    const COMMANDS: usize = 100;
    const WINDOW: usize = 5;
    const WATCHERS: usize = 3;
    // Room for the whole burst, so the watchers check ordering rather than backpressure (which
    // `rpc_protocol` covers).
    let roomy = RpcConfig {
        event_queue: 1 << 16,
        ..RpcConfig::default()
    };
    let f = Fixture::start_with(roomy, &|c, _| c.event_capacity = 1 << 16);
    let mut setup = f.client().await;
    setup.mutate("project.new", json!({"name": "Load"})).await;
    let start = live_rev(&mut setup).await;

    // Watchers read `doc.patched` while the others edit, until the final revision is known and
    // reached; each checks that revisions arrive in order without gaps.
    let (end_tx, end_rx) = tokio::sync::watch::channel(None::<Rev>);
    let mut watchers = Vec::new();
    for _ in 0..WATCHERS {
        let mut w = f.client().await;
        w.subscribe(&["doc.patched"]).await;
        let end_rx = end_rx.clone();
        watchers.push(tokio::spawn(async move {
            let (mut at, mut seen) = (start, 0usize);
            loop {
                if end_rx.borrow().is_some_and(|end| at >= end) {
                    return seen;
                }
                match w.next_within(Duration::from_millis(50)).await {
                    Err(()) => {}
                    Ok(None) => panic!("a watcher was closed: {:?}", w.close_frame),
                    Ok(Some(msg)) => {
                        let p = &msg["params"];
                        if msg["method"] != "doc.patched" || rev_of(&p["rev"]) <= start {
                            continue;
                        }
                        assert_eq!(rev_of(&p["prev_rev"]), at, "a gap or a reorder: {p}");
                        at = rev_of(&p["rev"]);
                        seen += 1;
                    }
                }
            }
        }));
    }

    let mut clients = Vec::new();
    for _ in 0..CLIENTS {
        clients.push(f.client().await);
    }
    let started = Instant::now();
    let mut tasks = Vec::new();
    for (client, mut c) in clients.into_iter().enumerate() {
        tasks.push(tokio::spawn(async move {
            let mut ok = Vec::new();
            let mut conflicts = 0usize;
            for chunk in (0..COMMANDS).collect::<Vec<_>>().chunks(WINDOW) {
                let mut ids = Vec::new();
                for &k in chunk {
                    let mut meta = json!({"request_id": request_id()});
                    // Every tenth command carries a stale revision and must fail.
                    if k % 10 == 9 {
                        meta["expected_rev"] = json!(0);
                    }
                    let params = with_meta(json!({"name": format!("c{client}-{k}")}), meta);
                    ids.push(c.start("project.rename", params).await);
                }
                for id in ids {
                    let r = c.response(id).await;
                    if let Some(out) = r.get("result") {
                        ok.push(rev_of(&out["_meta"]["rev"]));
                    } else {
                        assert_eq!(r["error"]["data"]["reason"], "stale_rev", "{r}");
                        conflicts += 1;
                    }
                }
            }
            (ok, conflicts)
        }));
    }
    let mut revs = Vec::new();
    let mut conflicts = 0;
    for task in tasks {
        let (ok, n) = tokio::time::timeout(Duration::from_secs(300), task)
            .await
            .expect("no deadlock")
            .unwrap();
        revs.extend(ok);
        conflicts += n;
    }
    let elapsed = started.elapsed();
    let end = live_rev(&mut setup).await;
    end_tx.send_replace(Some(end));
    assert_eq!(conflicts, CLIENTS * COMMANDS / 10);
    assert_eq!(revs.len(), CLIENTS * COMMANDS - conflicts);
    assert_eq!(
        end.0 - start.0,
        u64::try_from(revs.len()).unwrap(),
        "final rev = committed transactions"
    );
    revs.sort_unstable();
    revs.dedup();
    assert_eq!(
        revs.len(),
        CLIENTS * COMMANDS - conflicts,
        "one revision each"
    );
    // Subscribers saw every revision, in order.
    for w in watchers {
        let seen = tokio::time::timeout(Duration::from_secs(60), w)
            .await
            .expect("the watcher caught up")
            .unwrap();
        assert_eq!(seen, revs.len());
    }
    eprintln!(
        "{CLIENTS} connections x {COMMANDS} commands: {} committed, {conflicts} conflicts, {elapsed:?}",
        revs.len()
    );
    f.engine.shutdown();
}

// ----- the command round-trip budget (arch §19) --------------------------------------------------

/// `clip.trim` round trips (request → response and its `doc.patched`) on a 400-item project
/// with a journal. Budget p95 < 16 ms (arch §19); the gate here is 25 ms to allow for CI noise.
#[tokio::test(flavor = "multi_thread")]
async fn clip_trim_round_trip_p95_is_within_budget_on_a_400_item_project() {
    const ITEMS: usize = 400;
    const WARMUP: usize = 50;
    const SAMPLES: usize = 500;
    let with_project = |c: &mut EngineConfig, dir: &Path| {
        let folder = dir.join("bench");
        ProjectStore::create(&folder, &Project::new("Bench"))
            .unwrap()
            .close()
            .unwrap();
        c.project = Some(folder);
    };
    let f = Fixture::start_with(RpcConfig::default(), &with_project);
    let mut c = f.client().await;
    let (seq, track, media) = new_project(&mut c, &f.clip).await;
    let mut last = Value::Null;
    for i in 0..ITEMS {
        let out = c
            .mutate(
                "timeline.insert",
                json!({"sequence": seq, "track": track, "media": media,
                       "at": format!("{i}s"), "duration": "1s", "link": false}),
            )
            .await;
        last = out["clips"][0].clone();
    }
    let info = c.call("project.info", json!({})).await;
    eprintln!("bench project: {info}");
    c.subscribe(&["doc.patched"]).await;

    let mut samples = Vec::with_capacity(SAMPLES);
    for i in 0..WARMUP + SAMPLES {
        // Alternate the last clip's end between 399.5 s and 400 s.
        let to = if i % 2 == 0 { "399.5s" } else { "400s" };
        let t0 = Instant::now();
        let id = c
            .start(
                "clip.trim",
                with_rid(json!({"item": last, "edge": "end", "to": to})),
            )
            .await;
        let r = c.response(id).await;
        let rev = r["result"]["_meta"]["rev"].clone();
        assert!(rev.is_u64(), "{r}");
        loop {
            let p = c.event("doc.patched").await;
            if p["rev"] == rev {
                break;
            }
        }
        if i >= WARMUP {
            samples.push(t0.elapsed());
        }
    }
    samples.sort_unstable();
    let pct = |p: usize| samples[(samples.len() * p / 100).min(samples.len() - 1)];
    let (p50, p95, p99, max) = (pct(50), pct(95), pct(99), samples[samples.len() - 1]);
    eprintln!(
        "clip.trim round trip over /rpc, {ITEMS} items, {SAMPLES} samples: \
         p50 {p50:?} p95 {p95:?} p99 {p99:?} max {max:?}"
    );
    assert!(
        p95 < Duration::from_millis(25),
        "p95 {p95:?} exceeds the 25 ms gate (budget 16 ms)"
    );
    f.engine.shutdown();
}
