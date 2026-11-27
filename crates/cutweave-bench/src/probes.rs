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
use crate::harness::{BenchEngine, ChildEngine, ChildSpec, call};
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
}

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
            starts: 5,
            stub_media: false,
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

/// Command round trip on P-5K: `scale.commands` mixed commands in process, then as many over
/// the WebSocket (request, response and the `doc.patched` it causes).
/// `command_roundtrip_p95_ms` is the WebSocket p95.
pub fn command_rtt(
    engine: &BenchEngine,
    workload: &mut Workload,
    scale: &Scale,
) -> Result<(Vec<Metric>, RttHistogram), BenchError> {
    let conn = engine.connect()?;
    let mut local = Vec::with_capacity(scale.commands);
    for _ in 0..scale.commands {
        let step = workload.next_step();
        let t0 = Instant::now();
        call(&conn, step.command, step.params)?;
        local.push(ms(t0));
    }
    let mut client = engine.ws()?;
    client.subscribe_patches()?;
    let mut remote = Vec::with_capacity(scale.commands);
    for _ in 0..scale.commands {
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
    let metrics = vec![
        Metric::new("command_roundtrip_p95_ms", r.p95, "ms", r.samples),
        Metric::new("command_roundtrip_p50_ms", r.p50, "ms", r.samples),
        Metric::new("command_roundtrip_max_ms", r.max, "ms", r.samples),
        Metric::new("command_rtt_in_process_p95_ms", l.p95, "ms", l.samples),
        Metric::new("command_rtt_in_process_p50_ms", l.p50, "ms", l.samples),
    ];
    Ok((metrics, histogram))
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
    }
}

/// Engine start (`app_start_engine_ms`): from spawning the engine process to an editable empty
/// project over the WebSocket, the median of `scale.starts`. The engine's share of
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
}

/// Runs every probe at `scale`. `exe` is the `cutweave-bench` executable (the engine process
/// of the start and memory probes).
pub fn run_all(ws: &Workspace, scale: &Scale, exe: &Path) -> Result<Outcome, BenchError> {
    let mut metrics = open_and_query(ws, scale)?;
    let rtt = {
        let engine = ws.stress_engine()?;
        let mut workload = Workload::new(&ws.stress_manifest, scale.stress.seed);
        let (rtt_metrics, rtt) = command_rtt(&engine, &mut workload, scale)?;
        metrics.extend(rtt_metrics);
        metrics.extend(fan_out(&engine, &mut workload, scale)?);
        metrics.extend(ripple(&engine, ws, scale)?);
        rtt
    };
    metrics.extend(engine_start(ws, scale, exe)?);
    metrics.extend(idle_memory(ws, scale, exe)?);
    Ok(Outcome { metrics, rtt })
}
