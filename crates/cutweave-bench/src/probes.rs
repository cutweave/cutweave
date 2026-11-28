// SPDX-License-Identifier: GPL-3.0-or-later
//! The measurements of the engine suite (WP-2J.01), each returning metrics in the result
//! format: project open, command round trip (in process and over the WebSocket), `doc.patched`
//! fan-out, a placement ripple, `timeline.get`, idle memory and engine start.
//!
//! A [`Scale`] says how big a run is: [`Scale::full`] is the real suite on P-400 and P-5K,
//! [`Scale::smoke`] the same code on a tiny project with a few iterations, run as an ordinary
//! test so the suite keeps working between benchmark runs.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cutweave_engine::{Call, Connection};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::BenchError;
use crate::fixture::{FixtureManifest, FixtureSpec, generate};
use crate::harness::{BenchEngine, ChildEngine, ChildSpec, WsClient, call};
use crate::result::Metric;
use crate::stats::{BUCKETS_MS, Summary, summarize};
use crate::workload::Workload;

/// How big a run is.
#[derive(Clone, Debug)]
pub struct Scale {
    /// `engine` or `engine-smoke`: the `suite` of the result.
    pub suite: &'static str,
    /// The project opened and queried (P-400).
    pub edit: FixtureSpec,
    /// The project edited (P-5K).
    pub stress: FixtureSpec,
    /// Commands per round-trip probe.
    pub commands: usize,
    /// Times the project is opened.
    pub opens: usize,
    /// `timeline.get` calls per depth.
    pub queries: usize,
    /// Subscriber counts of the fan-out probe.
    pub subscribers: Vec<usize>,
    /// Commands per subscriber count.
    pub fan_out_rounds: usize,
    /// Ripples timed.
    pub ripples: usize,
    /// How long the engine idles before its memory is read.
    pub idle: Duration,
    /// Engine starts timed.
    pub starts: usize,
    /// Start the engine process with `--dev-stub-media` (no decoders, GPU or audio).
    pub stub_media: bool,
    /// Start the engine process the way the app does (`Gui` mode) instead of headless.
    pub gui_engine: bool,
    /// Keep the round-trip session going until this many commands have run, then measure
    /// again (`command_rtt_at_50k_*`); `None`: the session ends with the round-trip probe.
    pub long_session: Option<usize>,
    /// Updates of the main-track drag.
    pub drag_updates: usize,
    /// How many clips away the main-track drag carries its clip.
    pub drag_reach: usize,
}

/// The commands of a long session (`cutweave-bench run --long`).
pub const LONG_SESSION: usize = 50_000;

impl Scale {
    /// The real suite: P-400, P-5K, 10,000 commands, 60 s idle.
    #[must_use]
    pub fn full() -> Scale {
        Scale {
            suite: "engine",
            edit: FixtureSpec::p_400(),
            stress: FixtureSpec::p_5k(),
            commands: 10_000,
            opens: 10,
            queries: 200,
            subscribers: vec![1, 8, 32],
            fan_out_rounds: 200,
            ripples: 60,
            idle: Duration::from_secs(60),
            starts: 3,
            stub_media: false,
            gui_engine: true,
            long_session: None,
            drag_updates: 60,
            drag_reach: 300,
        }
    }

    /// The same probes on a tiny project, a few iterations each: seconds, not minutes.
    #[must_use]
    pub fn smoke() -> Scale {
        Scale {
            suite: "engine-smoke",
            edit: FixtureSpec::smoke(),
            stress: FixtureSpec {
                name: "P-smoke-stress".into(),
                seed: 2,
                ..FixtureSpec::smoke()
            },
            commands: 40,
            opens: 2,
            queries: 5,
            subscribers: vec![2],
            fan_out_rounds: 5,
            ripples: 3,
            idle: Duration::from_millis(200),
            starts: 1,
            stub_media: true,
            gui_engine: false,
            long_session: Some(120),
            drag_updates: 4,
            drag_reach: 300,
        }
    }
}

/// The fixtures of a run, written under a scratch folder.
#[derive(Clone, Debug)]
pub struct Workspace {
    /// The scratch folder (git-ignored: under `target/`).
    pub scratch: PathBuf,
    /// The P-400 project folder.
    pub edit: PathBuf,
    /// Its manifest.
    pub edit_manifest: FixtureManifest,
    /// The P-5K project folder.
    pub stress: PathBuf,
    /// Its manifest.
    pub stress_manifest: FixtureManifest,
    /// Whether the P-400 source files exist (made with FFmpeg).
    pub real_media: bool,
}

impl Workspace {
    /// Generates and writes the fixtures of `scale` under `scratch`. `with_media` also makes
    /// the P-400 source files when there is an `ffmpeg`.
    pub fn prepare(
        scale: &Scale,
        scratch: &Path,
        with_media: bool,
    ) -> Result<Workspace, BenchError> {
        let fixtures = scratch.join("fixtures");
        std::fs::create_dir_all(&fixtures).map_err(|e| BenchError::io("create", &fixtures, e))?;
        let edit = generate(&scale.edit)?;
        let stress = generate(&scale.stress)?;
        let edit_folder = edit.write(&fixtures)?;
        let real_media = if with_media {
            crate::media::synthesize(&edit_folder, &edit.manifest)?
        } else {
            crate::media::present(&edit_folder, &edit.manifest)
        };
        Ok(Workspace {
            scratch: scratch.to_path_buf(),
            edit: edit_folder,
            edit_manifest: edit.manifest,
            stress: stress.write(&fixtures)?,
            stress_manifest: stress.manifest,
            real_media,
        })
    }

    fn roots(&self) -> Vec<PathBuf> {
        vec![self.scratch.clone()]
    }

    /// An in-process engine with the stress project open.
    pub fn stress_engine(&self) -> Result<BenchEngine, BenchError> {
        BenchEngine::start(&self.scratch, &self.roots(), Some(&self.stress))
    }

    /// An in-process engine with no project open.
    pub fn empty_engine(&self) -> Result<BenchEngine, BenchError> {
        BenchEngine::start(&self.scratch, &self.roots(), None)
    }
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// The latency distributions of the command round-trip probe, saved next to the result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RttHistogram {
    /// The fixture the commands ran on.
    pub fixture: String,
    /// Upper bounds of the buckets in ms; the last bucket of each histogram is the rest.
    pub buckets_ms: Vec<f64>,
    /// `Connection::call` in the engine's process.
    pub in_process: Summary,
    /// Request, response and `doc.patched` over the WebSocket.
    pub websocket: Summary,
}

/// Opens the project and reads its timeline once: what "interactive" needs from the engine.
pub fn open_once(conn: &Connection, ws: &Workspace) -> Result<(), BenchError> {
    call(
        conn,
        "project.open",
        json!({"path": ws.edit.to_string_lossy()}),
    )?;
    let sequence = &ws.edit_manifest.sequence;
    call(
        conn,
        "timeline.get",
        json!({"sequence": sequence, "depth": 2}),
    )?;
    Ok(())
}

/// Project open (`project_open_1h_ms`: the median of opening P-400 and reading its timeline)
/// and `timeline.get` on it (`timeline_get_p400_ms`: p95 of the summary, depth 1).
pub fn open_and_query(ws: &Workspace, scale: &Scale) -> Result<Vec<Metric>, BenchError> {
    let engine = ws.empty_engine()?;
    let conn = engine.connect()?;
    let mut opens = Vec::new();
    for _ in 0..scale.opens {
        let t0 = Instant::now();
        open_once(&conn, ws)?;
        opens.push(ms(t0));
        call(&conn, "project.close", json!({}))?;
    }
    open_once(&conn, ws)?;
    let sequence = &ws.edit_manifest.sequence;
    let mut by_depth = Vec::new();
    for depth in [1, 2] {
        let mut times = Vec::new();
        for _ in 0..scale.queries {
            let t0 = Instant::now();
            call(
                &conn,
                "timeline.get",
                json!({"sequence": sequence, "depth": depth}),
            )?;
            times.push(ms(t0));
        }
        by_depth.push(summarize(&times));
    }
    let opens = summarize(&opens);
    Ok(vec![
        Metric::new("project_open_1h_ms", opens.p50, "ms", opens.samples),
        Metric::new("project_open_1h_max_ms", opens.max, "ms", opens.samples),
        Metric::new(
            "timeline_get_p400_ms",
            by_depth[0].p95,
            "ms",
            by_depth[0].samples,
        ),
        Metric::new(
            "timeline_get_p400_items_ms",
            by_depth[1].p95,
            "ms",
            by_depth[1].samples,
        ),
    ])
}

/// The in-process calls of one window of the session (`command_rtt_start_*`, `…_at_10k_*`,
/// `…_at_50k_*`): a twentieth of the probe's.
fn session_window(scale: &Scale) -> usize {
    (scale.commands / 20).max(1)
}

fn window_metrics(at: &str, ms: &[f64]) -> [Metric; 2] {
    let s = summarize(ms);
    [
        Metric::new(&format!("command_rtt_{at}_p95_ms"), s.p95, "ms", s.samples),
        Metric::new(&format!("command_rtt_{at}_p50_ms"), s.p50, "ms", s.samples),
    ]
}

/// Command round trip on P-5K: `scale.commands` mixed commands in process and as many over the
/// WebSocket (request, response and the `doc.patched` it causes), taking turns so both see the
/// same document as it grows. `command_roundtrip_p95_ms` is the WebSocket p95.
///
/// How the cost changes over the session (issue #230) is in the in-process calls of two
/// windows: `command_rtt_start_*` (the first ones) and `command_rtt_at_10k_*` (the ones that
/// follow half of the probe's commands: 10,000 at full scale). With `scale.long_session` the
/// session goes on in process until that many commands have run, and one more window is
/// `command_rtt_at_50k_*`.
pub fn command_rtt(
    engine: &BenchEngine,
    workload: &mut Workload,
    scale: &Scale,
) -> Result<(Vec<Metric>, RttHistogram), BenchError> {
    let conn = engine.connect()?;
    let mut client = engine.ws()?;
    client.subscribe_patches()?;
    let mut local = Vec::with_capacity(scale.commands);
    let mut remote = Vec::with_capacity(scale.commands);
    for _ in 0..scale.commands {
        let step = workload.next_step();
        let t0 = Instant::now();
        call(&conn, step.command, step.params)?;
        local.push(ms(t0));
        let step = workload.next_step();
        let t0 = Instant::now();
        client.round_trip(step.command, step.params)?;
        remote.push(ms(t0));
    }
    let histogram = RttHistogram {
        fixture: scale.stress.name.clone(),
        buckets_ms: BUCKETS_MS.to_vec(),
        in_process: summarize(&local),
        websocket: summarize(&remote),
    };
    let (l, r) = (&histogram.in_process, &histogram.websocket);
    let mut metrics = vec![
        Metric::new("command_roundtrip_p95_ms", r.p95, "ms", r.samples),
        Metric::new("command_roundtrip_p50_ms", r.p50, "ms", r.samples),
        Metric::new("command_roundtrip_max_ms", r.max, "ms", r.samples),
        Metric::new("command_rtt_in_process_p95_ms", l.p95, "ms", l.samples),
        Metric::new("command_rtt_in_process_p50_ms", l.p50, "ms", l.samples),
    ];
    let window = session_window(scale).min(local.len());
    let middle = (local.len() / 2).min(local.len() - window);
    metrics.extend(window_metrics("start", &local[..window]));
    metrics.extend(window_metrics("at_10k", &local[middle..middle + window]));
    if let Some(total) = scale.long_session {
        for _ in 2 * scale.commands..total {
            let step = workload.next_step();
            call(&conn, step.command, step.params)?;
        }
        let mut late = Vec::with_capacity(window);
        for _ in 0..window {
            let step = workload.next_step();
            let t0 = Instant::now();
            call(&conn, step.command, step.params)?;
            late.push(ms(t0));
        }
        metrics.extend(window_metrics("at_50k", &late));
    }
    Ok((metrics, histogram))
}

/// One gesture on the main track, as the editor sends it during a drag: `item.move` updates
/// with `_meta.replace` that carry `item` to the start of the main-track clip `away` clips
/// before `from`, one update per entry; the gesture is then cancelled, so the document is as
/// before. Returns the metrics `{id}_rtt_p95_ms`, `{id}_rtt_p50_ms` (an update's round trip:
/// request, response and the `doc.patched` it causes) and `{id}_kb_per_update` (what the
/// socket received for it).
fn gesture_on_main(
    client: &mut WsClient,
    manifest: &FixtureManifest,
    id: &str,
    item: &str,
    from: usize,
    away: &[usize],
) -> Result<Vec<Metric>, BenchError> {
    let Some(main) = manifest.tracks.iter().find(|t| t.main) else {
        return Ok(Vec::new());
    };
    let opened = client.mutate(
        "edit.begin_tx",
        json!({"kind": "gesture", "label": "Move clip"}),
    )?;
    let tx = &opened["tx"];
    let mut times = Vec::with_capacity(away.len());
    let received = client.received_bytes();
    for clips in away {
        let start = main.items[from - clips].start * manifest.frame;
        let t0 = Instant::now();
        client.gesture_update(
            "item.move",
            json!({"item": item, "to_track": main.id, "start": start}),
            tx,
        )?;
        times.push(ms(t0));
    }
    let received = client.received_bytes() - received;
    client.mutate("edit.cancel_tx", json!({"tx": tx}))?;
    let s = summarize(&times);
    #[allow(clippy::cast_precision_loss)]
    let kb = received as f64 / 1024.0 / times.len() as f64;
    Ok(vec![
        Metric::new(&format!("{id}_rtt_p95_ms"), s.p95, "ms", s.samples),
        Metric::new(&format!("{id}_rtt_p50_ms"), s.p50, "ms", s.samples),
        Metric::new(&format!("{id}_kb_per_update"), kb, "KB", s.samples),
    ])
}

/// Drags on the magnetic main track of P-5K (issue #231), over the WebSocket with
/// `doc.patched` subscribed, each update landing on another clip's start `scale.drag_reach`
/// clips towards the start of the track and back in `scale.drag_updates` updates:
///
/// - `main_drag_p5k_*`: the clip in the middle of the main track is carried along it (the
///   clips it passes change places with it);
/// - `main_insert_drag_p5k_*`: a clip of another video track is dragged onto the middle of
///   the main track and along it (every clip after it makes room: half the track ripples).
pub fn main_drag(
    engine: &BenchEngine,
    ws: &Workspace,
    scale: &Scale,
) -> Result<Vec<Metric>, BenchError> {
    let manifest = &ws.stress_manifest;
    let Some(main) = manifest.tracks.iter().find(|t| t.main) else {
        return Ok(Vec::new());
    };
    let from = main.items.len() / 2;
    let reach = scale.drag_reach.min(from);
    let half = scale.drag_updates.div_ceil(2);
    if reach == 0 || half == 0 {
        return Ok(Vec::new());
    }
    let mut away: Vec<usize> = (1..=half)
        .chain((1..half).rev())
        .map(|k| (reach * k).div_ceil(half))
        .collect();
    away.dedup();
    let mut client = engine.ws()?;
    client.subscribe_patches()?;
    let along = &main.items[from].id;
    let mut metrics = gesture_on_main(&mut client, manifest, "main_drag_p5k", along, from, &away)?;
    let visitor = manifest
        .tracks
        .iter()
        .filter(|t| t.video && !t.main)
        .find_map(|t| t.items.first());
    if let Some(visitor) = visitor {
        metrics.extend(gesture_on_main(
            &mut client,
            manifest,
            "main_insert_drag_p5k",
            &visitor.id,
            from,
            &away,
        )?);
    }
    Ok(metrics)
}

/// `doc.patched` fan-out: one command, timed until every one of N WebSocket subscribers has
/// its `doc.patched` (`doc_patched_fanout_<N>_p95_ms`).
pub fn fan_out(
    engine: &BenchEngine,
    workload: &mut Workload,
    scale: &Scale,
) -> Result<Vec<Metric>, BenchError> {
    let conn = engine.connect()?;
    let mut metrics = Vec::new();
    for &count in &scale.subscribers {
        let mut clients = Vec::new();
        for _ in 0..count {
            let mut client = engine.ws()?;
            client.subscribe_patches()?;
            clients.push(client);
        }
        let mut times = Vec::new();
        for _ in 0..scale.fan_out_rounds {
            let step = workload.next_step();
            let t0 = Instant::now();
            let reply = conn
                .call(Call::new(step.command, step.params))
                .map_err(|e| BenchError::command(step.command, &e))?;
            for client in &mut clients {
                client.wait_patched(reply.rev.0)?;
            }
            times.push(ms(t0));
        }
        let s = summarize(&times);
        metrics.push(Metric::new(
            &format!("doc_patched_fanout_{count}_p95_ms"),
            s.p95,
            "ms",
            s.samples,
        ));
    }
    Ok(metrics)
}

/// One insert at the start of the magnetic main track (every item on it moves right), undone
/// afterwards so the next one meets the same document.
pub fn ripple_once(conn: &Connection, manifest: &FixtureManifest) -> Result<Duration, BenchError> {
    let media = manifest
        .media
        .iter()
        .find(|m| m.video)
        .ok_or_else(|| BenchError::Other("the fixture has no video source".into()))?;
    let params = json!({
        "sequence": manifest.sequence,
        "track": manifest.tracks[0].id,
        "media": media.id,
        "at": 0,
        "duration": 5 * manifest.frame,
        "link": false,
    });
    let t0 = Instant::now();
    call(conn, "timeline.insert", params)?;
    let took = t0.elapsed();
    call(conn, "edit.undo", json!({}))?;
    Ok(took)
}

/// Placement ripple on P-5K (`placement_ripple_p5k_ms`: the median of the whole command; the
/// plan alone is `cutweave-ops`'s `placement_ripple_5k` bench).
pub fn ripple(
    engine: &BenchEngine,
    ws: &Workspace,
    scale: &Scale,
) -> Result<Vec<Metric>, BenchError> {
    let conn = engine.connect()?;
    let mut times = Vec::new();
    for _ in 0..scale.ripples {
        times.push(ripple_once(&conn, &ws.stress_manifest)?.as_secs_f64() * 1000.0);
    }
    let s = summarize(&times);
    Ok(vec![
        Metric::new("placement_ripple_p5k_ms", s.p50, "ms", s.samples),
        Metric::new("placement_ripple_p5k_p95_ms", s.p95, "ms", s.samples),
    ])
}

fn child_spec(ws: &Workspace, scale: &Scale, exe: &Path, tag: &str) -> ChildSpec {
    ChildSpec {
        exe: exe.to_path_buf(),
        scratch: ws.scratch.join(tag),
        project: None,
        stub_media: scale.stub_media,
        gui: scale.gui_engine,
        deterministic_ids: None,
        env: Vec::new(),
        roots: Vec::new(),
    }
}

/// Engine start (`app_start_engine_ms`): from spawning the engine process (in `Gui` mode at
/// full scale, as the app starts it) to an editable empty project over the WebSocket, the
/// median of `scale.starts`. The engine's share of
/// `app_start_ms`; Electron's is measured by WP-2J.02.
pub fn engine_start(ws: &Workspace, scale: &Scale, exe: &Path) -> Result<Vec<Metric>, BenchError> {
    let spec = child_spec(ws, scale, exe, "start");
    let mut times = Vec::new();
    let mut ready = Vec::new();
    for _ in 0..scale.starts {
        let t0 = Instant::now();
        let engine = ChildEngine::spawn(&spec)?;
        let mut client = engine.ws()?;
        client.call(
            "rpc.hello",
            &json!({"client": "cutweave-bench", "version": "1"}),
        )?;
        client.mutate("project.new", json!({"name": "Untitled"}))?;
        times.push(ms(t0));
        ready.push(engine.ready_after.as_secs_f64() * 1000.0);
    }
    let (s, r) = (summarize(&times), summarize(&ready));
    Ok(vec![
        Metric::new("app_start_engine_ms", s.p50, "ms", s.samples),
        Metric::new("app_start_engine_max_ms", s.max, "ms", s.samples),
        Metric::new("app_start_engine_ready_ms", r.p50, "ms", r.samples),
    ])
}

/// Idle memory: the engine process opens P-400, its timeline is read once, it idles for
/// `scale.idle`, then the OS is asked for its resident memory. With the real source files and
/// an FFmpeg build this is `idle_memory_1h_1080p_mb`; without them the figure is reported as
/// `idle_memory_no_media_mb` and judged against no budget.
pub fn idle_memory(ws: &Workspace, scale: &Scale, exe: &Path) -> Result<Vec<Metric>, BenchError> {
    let mut spec = child_spec(ws, scale, exe, "idle");
    spec.project = Some(ws.edit.clone());
    let engine = ChildEngine::spawn(&spec)?;
    let mut client = engine.ws()?;
    let sequence = &ws.edit_manifest.sequence;
    client.call("timeline.get", &json!({"sequence": sequence, "depth": 2}))?;
    std::thread::sleep(scale.idle);
    let with_media = ws.real_media && cfg!(feature = "ffmpeg") && !scale.stub_media;
    let id = if with_media {
        "idle_memory_1h_1080p_mb"
    } else {
        "idle_memory_no_media_mb"
    };
    Ok(vec![Metric::new(id, engine.resident_mb()?, "MB", 1)])
}

/// Everything a run measured.
#[derive(Clone, Debug)]
pub struct Outcome {
    /// The metrics, in the order of the report.
    pub metrics: Vec<Metric>,
    /// The round-trip distributions.
    pub rtt: RttHistogram,
    /// Probes of the engine process that could not run (it did not start, say), with the
    /// reason; their metrics are missing.
    pub failed: Vec<String>,
}

/// The probes of one editing session on P-5K, in one engine of this process: the main-track
/// drag, the command round trip, the fan-out and the ripple (`cutweave-bench run --editing`
/// runs these alone: no engine process, no GPU).
pub fn editing(ws: &Workspace, scale: &Scale) -> Result<Outcome, BenchError> {
    let engine = ws.stress_engine()?;
    let mut workload = Workload::new(&ws.stress_manifest, scale.stress.seed);
    // First, on the project as it was opened: the session below grows it.
    let drag = main_drag(&engine, ws, scale)?;
    let (mut metrics, rtt) = command_rtt(&engine, &mut workload, scale)?;
    metrics.extend(drag);
    metrics.extend(fan_out(&engine, &mut workload, scale)?);
    metrics.extend(ripple(&engine, ws, scale)?);
    Ok(Outcome {
        metrics,
        rtt,
        failed: Vec::new(),
    })
}

/// Runs every probe at `scale`. `exe` is the `cutweave-bench` executable (the engine process
/// of the start and memory probes).
pub fn run_all(ws: &Workspace, scale: &Scale, exe: &Path) -> Result<Outcome, BenchError> {
    let mut metrics = open_and_query(ws, scale)?;
    let session = editing(ws, scale)?;
    metrics.extend(session.metrics);
    let rtt = session.rtt;
    // The engine process depends on the machine (GPU, audio device): when it does not come
    // up, the measurements above are still worth reporting.
    let mut failed = Vec::new();
    for (name, probe) in [
        (
            "engine start",
            engine_start as fn(&Workspace, &Scale, &Path) -> _,
        ),
        ("idle memory", idle_memory),
    ] {
        match probe(ws, scale, exe) {
            Ok(found) => metrics.extend(found),
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    Ok(Outcome {
        metrics,
        rtt,
        failed,
    })
}
