// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1I.04 in-process engine tests: lifecycle and clean shutdown (threads, run-file, lock),
//! cold start, the session commands, credential-derived actors, the dispatch routes (`job.*`,
//! `media.import` policy, `view.*`, connections), recovery and autosave through the engine,
//! and deterministic ids.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::many_single_char_names,
    missing_docs
)]

mod engine_support;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cutweave_api::{Credential, Token};
use cutweave_commands::{CmdError, DocEvent, Rev, TxKind};
use cutweave_engine::autosave::{AutosaveConfig, ProjectEvent};
use cutweave_engine::project_io::{ProjectIoError, ProjectStore};
use cutweave_engine::{
    Call, Engine, EngineConfig, EngineEvent, EngineMode, SessionState, StartError, Subsystems,
};
use cutweave_jobs::{JobCtx, JobError};
use cutweave_ops::media::{
    AnalysisInput, AnalysisStage, AnalysisStages, FakeProbe, MediaEvent, MediaProbe,
};
use engine_support::{
    TempDir, drain, empty_project, headless, must, new_document, oneshot, settle_threads,
    thread_count, wait_for,
};
use serde_json::json;

const SEC: Duration = Duration::from_secs(1);

fn agent(name: &str) -> Credential {
    Credential::Agent(name.into())
}

fn ui_token() -> Token {
    Token::parse(&"u".repeat(64)).unwrap()
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Acceptance 1: a headless core engine opens a fixture project, runs a command and shuts down
/// with the run-file gone, the lock released and the thread count back to its baseline.
#[test]
fn headless_engine_opens_a_project_runs_a_command_and_shuts_down_cleanly() {
    let t = TempDir::new("lifecycle");
    let folder = empty_project(t.path(), "Lifecycle");
    // Warm up once, so lazily created process resources (OS thread pools, tracing callsites)
    // exist before the baseline is taken.
    Engine::start(headless(t.path())).unwrap().shutdown();
    let baseline = thread_count();

    let engine = Engine::start(headless(t.path())).unwrap();
    let run_file = engine.run_file_path().unwrap().to_path_buf();
    assert!(run_file.exists());
    let run_text = std::fs::read_to_string(&run_file).unwrap();
    assert!(run_text.contains("agent_tokens"));
    let handle = engine.handle();
    assert_eq!(handle.state(), SessionState::NoProject);
    let mut events = handle.subscribe();
    let conn = handle.connect(&agent("cli")).unwrap();

    let opened = must(&conn, "project.open", json!({ "path": path_str(&folder) }));
    assert_eq!(opened["project"]["state"], "open");
    assert_eq!(handle.state(), SessionState::Open);
    assert!(matches!(
        ProjectStore::open(&folder),
        Err(ProjectIoError::Locked { .. })
    ));
    new_document(&conn, "Lifecycle");
    let reply = conn
        .call(Call::new("project.rename", json!({ "name": "Renamed" })))
        .unwrap();
    assert_eq!(reply.rev, Rev(2));
    // The actor of every change is the one the credential derives (ADR-0016, WP-1I.01).
    let patched = wait_for(
        &mut events,
        5 * SEC,
        |e| matches!(e.as_doc(), Some(DocEvent::Patched(p)) if p.rev == Rev(2)),
    );
    assert_eq!(patched.payload()["actor"], "agent:cli");
    assert_eq!(patched.name(), "doc.patched");

    drop(conn);
    engine.shutdown();
    assert!(!run_file.exists(), "the run-file outlived the engine");
    // The lock is released and nothing was saved implicitly: the edits wait in the journal.
    let reopened = ProjectStore::open(&folder).unwrap();
    assert_eq!(reopened.project.meta.name, "Lifecycle");
    assert_eq!(
        reopened.report.recovery.as_ref().map(|r| r.records),
        Some(2)
    );
    drop(reopened);
    let after = settle_threads(baseline, 2);
    println!("threads: baseline {baseline}, after shutdown {after}");
    assert!(
        after <= baseline + 2 && after + 2 >= baseline,
        "thread count {after} is not within 2 of the baseline {baseline}"
    );
    // The handle outlives the engine but refuses work.
    assert!(handle.connect(&agent("cli")).is_err());
}

/// Acceptance 2: cold start to ready (server bound, run-file written) with an empty project.
#[test]
fn cold_start_to_ready_with_an_empty_project_is_under_a_second() {
    let t = TempDir::new("cold");
    let folder = empty_project(t.path(), "Cold");
    let mut config = headless(t.path());
    config.project = Some(folder);
    let started = Instant::now();
    let engine = Engine::start(config).unwrap();
    let elapsed = started.elapsed();
    assert!(engine.port().is_some());
    assert_eq!(engine.handle().state(), SessionState::Open);
    println!(
        "cold start to ready (headless core, empty project, server bound, run-file written): {:.1} ms",
        elapsed.as_secs_f64() * 1000.0
    );
    engine.shutdown();
    assert!(elapsed < SEC, "cold start took {elapsed:?}");
}

#[test]
fn an_untitled_document_is_saved_as_a_project_closed_and_reopened() {
    let t = TempDir::new("saveas");
    let engine = Engine::start(oneshot(t.path())).unwrap();
    assert_eq!(engine.port(), None);
    let h = engine.handle();
    let conn = h.connect(&agent("cli")).unwrap();
    let err = conn.call(Call::new("project.save", json!({}))).unwrap_err();
    assert_eq!(err.code(), "invalid", "{err}");
    new_document(&conn, "Film");
    must(&conn, "project.rename", json!({ "name": "Film draft" }));

    let mut events = h.subscribe();
    let folder = t.path().join("Film");
    let saved = must(
        &conn,
        "project.save_as",
        json!({ "path": path_str(&folder) }),
    );
    assert!(saved["path"].as_str().unwrap().ends_with("Film.cutweave"));
    assert_eq!(h.state(), SessionState::Open);
    let names: Vec<&str> = drain(&mut events).iter().map(EngineEvent::name).collect();
    assert!(names.contains(&"doc.reset"), "{names:?}");
    assert!(names.contains(&"project.opened"), "{names:?}");
    let info = must(&conn, "project.info", json!({}));
    assert_eq!(info["name"], "Film draft");
    assert_eq!(info["sequences"].as_array().unwrap().len(), 1);

    must(&conn, "project.rename", json!({ "name": "Film 2" }));
    assert!(h.status().unwrap().dirty);
    let saved = must(&conn, "project.save", json!({}));
    assert_eq!(saved["rev"], h.status().unwrap().rev.0);
    assert!(!h.status().unwrap().dirty);

    // Save-as of a journaled project moves it (with its history) to the new folder.
    let copy = t.path().join("Film copy");
    must(&conn, "project.save_as", json!({ "path": path_str(&copy) }));
    let status = h.status().unwrap();
    assert!(status.path.unwrap().ends_with("Film copy.cutweave"));
    must(&conn, "edit.undo", json!({}));
    assert_eq!(must(&conn, "project.info", json!({}))["name"], "Film draft");
    must(&conn, "edit.redo", json!({}));

    let closed = must(&conn, "project.close", json!({}));
    assert!(
        closed["closed"]
            .as_str()
            .unwrap()
            .ends_with("Film copy.cutweave")
    );
    assert_eq!(closed["project"]["state"], "no_project");
    assert_eq!(h.state(), SessionState::NoProject);
    // Autosave is on by default, so closing saved the copy's last state.
    let opened = must(&conn, "project.open", json!({ "path": path_str(&copy) }));
    assert_eq!(opened["project"]["name"], "Film 2");
    assert_eq!(opened["project"]["dirty"], false);
    assert!(opened["project"]["recovery"].is_null());
    // The original folder kept its own last save.
    let original = ProjectStore::open(&folder).unwrap();
    assert_eq!(original.project.meta.name, "Film 2");
    drop(original);
    engine.shutdown();
}

#[test]
fn opening_a_locked_project_fails_and_keeps_the_current_document() {
    let t = TempDir::new("locked");
    let folder = empty_project(t.path(), "Busy");
    let holder = ProjectStore::open(&folder).unwrap();
    let engine = Engine::start(oneshot(t.path())).unwrap();
    let conn = engine.handle().connect(&agent("cli")).unwrap();
    new_document(&conn, "Mine");
    let err = conn
        .call(Call::new(
            "project.open",
            json!({ "path": path_str(&folder) }),
        ))
        .unwrap_err();
    assert_eq!(err.code(), "invalid");
    assert!(err.to_string().contains("already open"), "{err}");
    assert_eq!(engine.handle().state(), SessionState::NoProject);
    assert_eq!(must(&conn, "project.info", json!({}))["name"], "Mine");
    // A relative path is refused before touching the disk.
    let err = conn
        .call(Call::new("project.open", json!({ "path": "relative/x" })))
        .unwrap_err();
    assert_eq!(err.code(), "invalid");
    drop(holder);

    // At start, a locked project is exit code 10.
    let holder = ProjectStore::open(&folder).unwrap();
    let mut config = EngineConfig::new(EngineMode::Oneshot);
    config.project = Some(folder.clone());
    let err = Engine::start(config).unwrap_err();
    assert!(matches!(err, StartError::ProjectLocked { .. }), "{err}");
    assert_eq!(err.exit_code(), 10);
    assert_eq!(err.code(), "project_locked");
    drop(holder);
    engine.shutdown();
}

#[test]
fn ui_credentials_exist_only_in_gui_mode() {
    let mut config = EngineConfig::new(EngineMode::Headless);
    config.ui_token = Some(ui_token());
    let err = Engine::start(config).unwrap_err();
    assert_eq!(err.exit_code(), 13);
    let err = Engine::start(EngineConfig::new(EngineMode::Gui)).unwrap_err();
    assert_eq!(err.exit_code(), 13);
    let mut config = EngineConfig::new(EngineMode::Oneshot);
    config.port = Some(0);
    assert_eq!(Engine::start(config).unwrap_err().exit_code(), 2);

    let engine = Engine::start(EngineConfig::new(EngineMode::Headless)).unwrap();
    assert!(engine.handle().connect(&Credential::Ui).is_err());
    let system = engine.handle().connect(&Credential::Internal).unwrap();
    assert_eq!(system.ctx().actor().to_string(), "system");
    engine.shutdown();
}

#[test]
fn a_closed_connection_rolls_back_its_open_transaction() {
    let engine = Engine::start(EngineConfig::new(EngineMode::Oneshot)).unwrap();
    let h = engine.handle();
    let ui = h.connect(&agent("one")).unwrap();
    new_document(&ui, "Before");
    let other = h.connect(&agent("two")).unwrap();
    assert_ne!(ui.ctx().conn(), other.ctx().conn());
    let mut events = h.subscribe();
    let tx = must(
        &other,
        "edit.begin_tx",
        json!({ "kind": "gesture", "label": "Rename" }),
    );
    let tx_id = serde_json::from_value(tx["tx"].clone()).unwrap();
    other
        .call(Call::new("project.rename", json!({ "name": "During" })).in_tx(tx_id))
        .unwrap();
    assert_eq!(h.snapshot().unwrap().doc.meta.name, "During");
    drop(other);
    let closed = wait_for(&mut events, 5 * SEC, |e| e.name() == "tx.closed");
    assert_eq!(closed.payload()["outcome"], "rolled_back");
    assert_eq!(h.snapshot().unwrap().doc.meta.name, "Before");
    let _ = TxKind::Gesture;
    engine.shutdown();
}

#[test]
fn job_commands_run_against_the_job_manager() {
    let engine = Engine::start(EngineConfig::new(EngineMode::Oneshot)).unwrap();
    let h = engine.handle();
    let conn = h.connect(&agent("cli")).unwrap();
    let list = must(&conn, "job.list", json!({}));
    assert_eq!(list["active"], 0);
    let missing = format!("job_{:032x}", 7);
    let err = conn
        .call(Call::new("job.status", json!({ "id": missing })))
        .unwrap_err();
    assert_eq!(err.code(), "not_found");
    // From async code, `job.wait` runs off the async workers.
    let waited = h.runtime().block_on(conn.call_async(Call::new(
        "job.wait",
        json!({ "job_id": missing, "timeout_ms": 10 }),
    )));
    assert_eq!(waited.unwrap_err().code(), "not_found");
    // Engine commands cannot run inside a document transaction.
    let tx = must(&conn, "edit.begin_tx", json!({ "label": "Batch" }));
    let tx_id = serde_json::from_value(tx["tx"].clone()).unwrap();
    let err = conn
        .call(Call::new("job.list", json!({})).in_tx(tx_id))
        .unwrap_err();
    assert_eq!(err.code(), "unsupported");
    engine.shutdown();
}

// ----- the path policy -------------------------------------------------------------------------

/// Makes `link` a directory link to `target`: a symlink, or on Windows a junction when
/// symlinks need a privilege. Returns whether it worked.
fn dir_link(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, link).is_ok()
            || std::process::Command::new("cmd")
                .arg("/C")
                .arg("mklink")
                .arg("/J")
                .arg(link)
                .arg(target)
                .output()
                .is_ok_and(|o| o.status.success())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        false
    }
}

/// `name` with `{"path": path}` fails with `Invalid` at `/path`.
fn refused(conn: &cutweave_engine::Connection, name: &str, path: &Path) {
    let err = conn
        .call(Call::new(name, json!({ "path": path_str(path) })))
        .unwrap_err();
    assert!(
        matches!(&err, CmdError::Invalid { path, .. } if path == "/path"),
        "{name} {}: {err}",
        path.display()
    );
}

#[test]
fn agents_open_and_save_only_inside_allowed_roots_while_the_ui_is_unrestricted() {
    let t = TempDir::new("paths");
    let approved = t.path().join("approved");
    let outside = t.path().join("outside");
    std::fs::create_dir_all(&approved).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let inside_project = empty_project(&approved, "Inside");
    let outside_project = empty_project(&outside, "Outside");
    let mut config = EngineConfig::new(EngineMode::Gui);
    config.ui_token = Some(ui_token());
    config.api.run_dir = Some(t.path().join("run"));
    config.allowed_roots = vec![approved.clone()];
    let engine = Engine::start(config).unwrap();
    let h = engine.handle();
    let mcp = h.connect(&agent("mcp")).unwrap();
    let user = h.connect(&Credential::Ui).unwrap();

    // Outside the roots, by name, by `..` or through a link inside a root: refused.
    refused(&mcp, "project.open", &outside_project);
    new_document(&mcp, "Agent draft");
    refused(&mcp, "project.save_as", &outside.join("Saved"));
    refused(
        &mcp,
        "project.save_as",
        &approved.join("..").join("outside").join("Dots"),
    );
    let link = approved.join("link");
    assert!(dir_link(&outside, &link), "could not create a link");
    refused(&mcp, "project.save_as", &link.join("Linked"));
    refused(&mcp, "project.open", &link.join("Outside"));
    assert!(!outside.join("Saved").exists() && !outside.join("Linked").exists());
    assert_eq!(h.state(), SessionState::NoProject);

    // Inside: fine.
    let saved = approved.join("Saved");
    must(&mcp, "project.save_as", json!({ "path": path_str(&saved) }));
    must(
        &mcp,
        "project.open",
        json!({ "path": path_str(&inside_project) }),
    );

    // The UI credential uses whatever the user picked; the folder of the project it opened
    // is then usable for agents too.
    must(
        &user,
        "project.open",
        json!({ "path": path_str(&outside_project) }),
    );
    let copy = outside_project.join("Copy");
    must(&mcp, "project.save_as", json!({ "path": path_str(&copy) }));
    must(
        &user,
        "project.save_as",
        json!({ "path": path_str(&t.path().join("Anywhere")) }),
    );
    refused(&mcp, "project.open", &outside_project);
    engine.shutdown();
}

/// WP-1J.05: `project.set_allowed_roots` replaces the approved roots (the `--allow-root` ones
/// too), stores them resolved, applies at once to an open agent connection, and is the UI
/// credential's alone; `project.info` reports the roots in effect.
#[test]
fn the_app_sets_the_allowed_roots_live_and_resolved() {
    let t = TempDir::new("setroots");
    let launch_root = t.path().join("launch");
    let footage = t.path().join("footage");
    std::fs::create_dir_all(&launch_root).unwrap();
    std::fs::create_dir_all(footage.join("day1")).unwrap();
    let in_launch = empty_project(&launch_root, "Launch");
    let in_footage = empty_project(&footage, "Footage");
    let mut config = EngineConfig::new(EngineMode::Gui);
    config.ui_token = Some(ui_token());
    config.api.run_dir = Some(t.path().join("run"));
    config.allowed_roots = vec![launch_root.clone()];
    let engine = Engine::start(config).unwrap();
    let h = engine.handle();
    let user = h.connect(&Credential::Ui).unwrap();
    let cli = h.connect(&agent("cli")).unwrap();
    let roots_of = |conn: &cutweave_engine::Connection| {
        must(conn, "project.info", json!({}))["allowed_roots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r.as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(roots_of(&cli), vec![path_str(&launch_root)]);
    must(
        &cli,
        "project.open",
        json!({ "path": path_str(&in_launch) }),
    );
    must(&cli, "project.close", json!({ "save": false }));

    // Bad lists change nothing: relative, `..`, missing, a file, too many.
    let file = footage.join("note.txt");
    std::fs::write(&file, b"x").unwrap();
    for (roots, pointer) in [
        (json!(["relative"]), "/roots/0"),
        (
            json!([
                path_str(&footage),
                path_str(&footage.join("day1").join(".."))
            ]),
            "/roots/1",
        ),
        (json!([path_str(&t.path().join("missing"))]), "/roots/0"),
        (json!([path_str(&file)]), "/roots/0"),
        (
            json!(vec![
                path_str(&footage);
                cutweave_engine::MAX_ALLOWED_ROOTS + 1
            ]),
            "/roots",
        ),
    ] {
        let err = user
            .call(Call::new(
                "project.set_allowed_roots",
                json!({ "roots": roots }),
            ))
            .unwrap_err();
        assert!(
            matches!(&err, CmdError::Invalid { path, .. } if path == pointer),
            "{err}"
        );
    }
    assert_eq!(roots_of(&cli), vec![path_str(&launch_root)]);

    // A link to the footage folder is stored resolved, and replaces the launch root.
    let link = t.path().join("footage-link");
    let root = if dir_link(&footage, &link) {
        link.clone()
    } else {
        footage.clone()
    };
    let out = must(
        &user,
        "project.set_allowed_roots",
        json!({ "roots": [path_str(&root)] }),
    );
    let stored = out["roots"][0].as_str().unwrap().to_owned();
    assert!(
        std::path::Path::new(&stored).ends_with("footage"),
        "resolved: {stored}"
    );
    assert!(!stored.starts_with(r"\\?\"), "{stored}");
    assert_eq!(roots_of(&cli), vec![stored.clone()]);
    // The same open connection: the old root is gone, the new one works at once.
    refused(&cli, "project.open", &in_launch);
    must(
        &cli,
        "project.open",
        json!({ "path": path_str(&in_footage) }),
    );
    // The open project's folder is listed too.
    assert!(roots_of(&cli).contains(&stored));
    // Removing it stops the same connection at once (the open project stays usable).
    must(&user, "project.set_allowed_roots", json!({ "roots": [] }));
    let err = cli
        .call(Call::new(
            "project.save_as",
            json!({ "path": path_str(&footage.join("day1").join("Copy")) }),
        ))
        .unwrap_err();
    assert!(
        err.to_string().contains("Settings > Agents and MCP"),
        "{err}"
    );

    // Agents may not change it.
    let err = cli
        .call(Call::new(
            "project.set_allowed_roots",
            json!({ "roots": [path_str(&footage)] }),
        ))
        .unwrap_err();
    assert_eq!(err.code(), "forbidden");
    engine.shutdown();
}

/// WP-1J.05: agent connections are listed with their last command and announced; the UI's are
/// not.
#[test]
fn agent_connections_are_listed_and_announced() {
    let t = TempDir::new("connections");
    let mut config = EngineConfig::new(EngineMode::Gui);
    config.ui_token = Some(ui_token());
    config.api.run_dir = Some(t.path().join("run"));
    let engine = Engine::start(config).unwrap();
    let h = engine.handle();
    let mut events = h.subscribe();
    let user = h.connect(&Credential::Ui).unwrap();
    let mcp = h.connect(&agent("mcp")).unwrap();
    let opened = wait_for(&mut events, 10 * SEC, |e| {
        e.name() == "project.connection_opened"
    });
    assert_eq!(opened.payload()["actor"], "agent:mcp");
    let _ = mcp.call(Call::new("project.set_autosave", json!({})));
    must(&mcp, "project.info", json!({}));
    let listed = must(&user, "project.connections", json!({}));
    let list = listed["connections"].as_array().unwrap();
    assert_eq!(list.len(), 1, "the UI is not an agent: {listed}");
    assert_eq!(list[0]["actor"], "agent:mcp");
    assert_eq!(list[0]["last_command"], "project.info");
    // Read-only and open to agents too.
    assert_eq!(
        must(&mcp, "project.connections", json!({}))["connections"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    drop(mcp);
    let closed = wait_for(&mut events, 10 * SEC, |e| {
        e.name() == "project.connection_closed"
    });
    assert_eq!(closed.payload()["last_command"], "project.connections");
    assert_eq!(
        must(&user, "project.connections", json!({}))["connections"],
        json!([])
    );
    engine.shutdown();
}

#[test]
fn without_allowed_roots_agents_are_confined_to_the_start_project() {
    let t = TempDir::new("noroots");
    let start = empty_project(t.path(), "Start");
    let other = empty_project(t.path(), "Other");
    let mut config = EngineConfig::new(EngineMode::Headless);
    config.project = Some(start.clone());
    let engine = Engine::start(config).unwrap();
    let cli = engine.handle().connect(&agent("cli")).unwrap();
    refused(&cli, "project.open", &other);
    refused(&cli, "project.save_as", &t.path().join("New"));
    // The start project stays usable after it was closed.
    must(&cli, "project.close", json!({ "save": false }));
    refused(&cli, "project.open", &other);
    must(&cli, "project.open", json!({ "path": path_str(&start) }));
    engine.shutdown();

    // Untitled and without roots: an agent can name no path at all.
    let engine = Engine::start(EngineConfig::new(EngineMode::Oneshot)).unwrap();
    let cli = engine.handle().connect(&agent("cli")).unwrap();
    new_document(&cli, "Nowhere");
    refused(&cli, "project.save_as", &t.path().join("Nowhere"));
    refused(&cli, "project.open", &start);
    engine.shutdown();
}

// ----- media.import ----------------------------------------------------------------------------

struct Index;

impl AnalysisStage for Index {
    fn name(&self) -> &'static str {
        "index"
    }
    fn builds_index(&self) -> bool {
        true
    }
    fn run(&self, _: &AnalysisInput, ctx: &JobCtx) -> Result<(), JobError> {
        ctx.check()
    }
}

/// A media subsystem: a fake prober and one analysis stage.
struct FakeMedia {
    probe: Arc<FakeProbe>,
    stages: Arc<AnalysisStages>,
}

impl FakeMedia {
    fn new(paths: &[&str]) -> FakeMedia {
        let mut probe = FakeProbe::new();
        for (i, p) in paths.iter().enumerate() {
            probe = probe.with(p, FakeProbe::video(p, 10 + i64::try_from(i).unwrap(), true));
        }
        let mut stages = AnalysisStages::new();
        stages.add(Arc::new(Index)).unwrap();
        FakeMedia {
            probe: Arc::new(probe),
            stages: Arc::new(stages),
        }
    }
}

impl Subsystems for FakeMedia {
    fn media_probe(&self) -> Option<Arc<dyn MediaProbe>> {
        Some(Arc::clone(&self.probe) as Arc<dyn MediaProbe>)
    }
    fn analysis_stages(&self) -> Option<Arc<AnalysisStages>> {
        Some(Arc::clone(&self.stages))
    }
}

#[test]
fn media_import_restricts_agents_to_allowed_roots_and_registers_one_undo_step() {
    let t = TempDir::new("import");
    let approved = t.path().join("approved");
    std::fs::create_dir_all(&approved).unwrap();
    let elsewhere = |n: &str| path_str(&t.path().join("elsewhere").join(n));
    let (a, b, c) = (
        elsewhere("a.mp4"),
        elsewhere("b.mp4"),
        path_str(&approved.join("c.mp4")),
    );
    let mut config = EngineConfig::new(EngineMode::Gui);
    config.ui_token = Some(ui_token());
    config.api.run_dir = Some(t.path().join("run"));
    config.allowed_roots = vec![approved.clone()];
    config.subsystems = Some(Box::new(FakeMedia::new(&[&a, &b, &c])));
    let engine = Engine::start(config).unwrap();
    let h = engine.handle();
    let user = h.connect(&Credential::Ui).unwrap();
    let mcp = h.connect(&agent("mcp")).unwrap();
    new_document(&user, "Import");
    let mut events = h.subscribe();

    // The UI imports what the user picked, anywhere: one transaction, analysis queued.
    let out = must(&user, "media.import", json!({ "paths": [a, b] }));
    assert_eq!(out["media_ids"].as_array().unwrap().len(), 2);
    let jobs = out["job_ids"].as_array().unwrap().clone();
    assert_eq!(jobs.len(), 2);
    let done = must(
        &user,
        "job.wait",
        json!({ "job_id": jobs[0], "timeout_ms": 10_000 }),
    );
    assert_eq!(done["state"], "succeeded");
    let indexed = wait_for(&mut events, 10 * SEC, |e| {
        matches!(e, EngineEvent::Media(MediaEvent::Indexed(_)))
    });
    assert_eq!(indexed.name(), "media.indexed");
    wait_for(&mut events, 10 * SEC, |e| e.name() == "job.progress");
    let patched: Vec<_> = drain(&mut events)
        .into_iter()
        .filter(|e| e.name() == "doc.patched")
        .collect();
    assert!(patched.len() <= 1, "the import committed more than once");
    assert_eq!(must(&user, "media.list", json!({}))["total"], 2);

    // An agent may not read outside the project folder and the approved roots.
    let err = mcp
        .call(Call::new(
            "media.import",
            json!({ "paths": [elsewhere("a.mp4")] }),
        ))
        .unwrap_err();
    assert_eq!(err.code(), "invalid");
    assert!(err.to_string().contains("/paths/0"), "{err}");
    let out = must(&mcp, "media.import", json!({ "paths": [c] }));
    assert_eq!(out["media"][0]["duplicate"], false);

    // Each import is one undo step of its own actor.
    must(&user, "edit.undo", json!({}));
    assert_eq!(must(&user, "media.list", json!({}))["total"], 1);
    must(&mcp, "edit.undo", json!({}));
    assert_eq!(must(&user, "media.list", json!({}))["total"], 0);
    engine.shutdown();
}

#[test]
fn without_a_media_backend_every_file_fails_to_import() {
    let t = TempDir::new("nobackend");
    let engine = Engine::start(EngineConfig::new(EngineMode::Oneshot)).unwrap();
    let conn = engine.handle().connect(&Credential::Internal).unwrap();
    new_document(&conn, "Core");
    let mut config_root = t.path().to_path_buf();
    config_root.push("x.mp4");
    // `system` is restricted too (not the UI credential): no roots, no import.
    let err = conn
        .call(Call::new(
            "media.import",
            json!({ "paths": [path_str(&config_root)] }),
        ))
        .unwrap_err();
    assert_eq!(err.code(), "invalid");
    engine.shutdown();

    let mut config = EngineConfig::new(EngineMode::Oneshot);
    config.allowed_roots = vec![t.path().to_path_buf()];
    let engine = Engine::start(config).unwrap();
    let conn = engine.handle().connect(&agent("cli")).unwrap();
    new_document(&conn, "Core");
    let out = must(
        &conn,
        "media.import",
        json!({ "paths": [path_str(&config_root)] }),
    );
    assert_eq!(out["failed"][0]["code"], "io");
    assert_eq!(out["media_ids"], json!([]));
    engine.shutdown();
}

#[test]
fn view_commands_change_the_session_view_state_and_notify_the_others() {
    let t = TempDir::new("view");
    let folder = empty_project(t.path(), "View");
    let mut config = EngineConfig::new(EngineMode::Oneshot);
    config.project = Some(folder.clone());
    let engine = Engine::start(config).unwrap();
    let h = engine.handle();
    let conn = h.connect(&agent("cli")).unwrap();
    let mut events = h.subscribe();
    must(&conn, "view.set_playhead", json!({ "at": "2s" }));
    let changed = wait_for(&mut events, 5 * SEC, |e| e.name() == "view.changed");
    let EngineEvent::View { origin, .. } = &changed else {
        panic!("{changed:?}");
    };
    assert_eq!(*origin, Some(conn.ctx().conn()));
    let view = must(&conn, "view.get", json!({}));
    assert_eq!(view["playhead"], 2 * 705_600_000_i64);
    // Closing flushes the view state with the project.
    must(&conn, "project.close", json!({ "save": false }));
    let ui_state = std::fs::read_to_string(folder.join(".cutweave").join("ui-state.json"));
    assert!(ui_state.unwrap().contains("1411200000"));
    engine.shutdown();
}

// ----- recovery and autosave -------------------------------------------------------------------

/// Leaves `n` unsaved renames in the journal of `folder` (an engine shut down without saving).
fn leave_unsaved_work(folder: &Path, n: usize) {
    let mut config = EngineConfig::new(EngineMode::Oneshot);
    config.project = Some(folder.to_path_buf());
    let engine = Engine::start(config).unwrap();
    let conn = engine.handle().connect(&agent("cli")).unwrap();
    new_document(&conn, "Saved");
    must(&conn, "project.save", json!({}));
    for i in 0..n {
        must(
            &conn,
            "project.rename",
            json!({ "name": format!("Edit {i}") }),
        );
    }
    drop(conn);
    engine.shutdown();
}

#[test]
fn recovery_waits_for_a_decision_then_restores_and_autosaves() {
    let t = TempDir::new("recover");
    let folder = empty_project(t.path(), "Recover");
    leave_unsaved_work(&folder, 3);

    let mut config = oneshot(t.path());
    config.autosave = Some(AutosaveConfig {
        idle: Duration::from_millis(100),
        poll: Duration::from_millis(20),
        ..AutosaveConfig::default()
    });
    let engine = Engine::start(config).unwrap();
    let h = engine.handle();
    let conn = h.connect(&agent("cli")).unwrap();
    let mut events = h.subscribe();
    let opened = must(&conn, "project.open", json!({ "path": path_str(&folder) }));
    assert_eq!(opened["project"]["recovery"]["records"], 3);
    let announced = wait_for(&mut events, 5 * SEC, |e| {
        e.name() == "project.recovery_available"
    });
    assert_eq!(announced.payload()["records"], 3);
    // The saved document is shown; edits and saves wait for the decision.
    assert_eq!(must(&conn, "project.info", json!({}))["name"], "Saved");
    let err = conn
        .call(Call::new("project.rename", json!({ "name": "Too early" })))
        .unwrap_err();
    assert_eq!(err.code(), "unsupported", "{err}");
    assert_eq!(
        conn.call(Call::new("project.save", json!({})))
            .unwrap_err()
            .code(),
        "unsupported"
    );

    let recovered = must(&conn, "project.recover", json!({}));
    assert_eq!(recovered["applied"], 3);
    assert_eq!(recovered["damaged"], false);
    assert!(recovered["project"]["recovery"].is_null());
    assert_eq!(must(&conn, "project.info", json!({}))["name"], "Edit 2");
    // Undo history came back with the journal.
    must(&conn, "edit.undo", json!({}));
    assert_eq!(must(&conn, "project.info", json!({}))["name"], "Edit 1");
    // The restored work is unsaved until autosave writes it.
    let saved = wait_for(&mut events, 10 * SEC, |e| {
        matches!(e, EngineEvent::Project(ProjectEvent::Saved(_)))
    });
    assert!(
        saved.payload()["path"]
            .as_str()
            .unwrap()
            .ends_with("Recover.cutweave")
    );
    let deadline = Instant::now() + 5 * SEC;
    while h.status().unwrap().dirty && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!h.status().unwrap().dirty);
    engine.shutdown();
    let on_disk = ProjectStore::open(&folder).unwrap();
    assert_eq!(on_disk.project.meta.name, "Edit 1");
    assert!(on_disk.report.recovery.is_none());
}

#[test]
fn discarding_recovery_keeps_the_saved_document() {
    let t = TempDir::new("discard");
    let folder = empty_project(t.path(), "Discard");
    leave_unsaved_work(&folder, 2);
    // At start (`--project`), the pending recovery is part of the status.
    let mut config = EngineConfig::new(EngineMode::Oneshot);
    config.project = Some(folder.clone());
    let engine = Engine::start(config).unwrap();
    let h = engine.handle();
    let status = h.status().unwrap();
    assert_eq!(status.state, SessionState::Open);
    assert_eq!(status.recovery.as_ref().map(|r| r.records), Some(2));
    let conn = h.connect(&agent("cli")).unwrap();
    let out = must(&conn, "project.discard_recovery", json!({}));
    assert_eq!(out["applied"], 0);
    assert!(out["project"]["recovery"].is_null());
    assert_eq!(must(&conn, "project.info", json!({}))["name"], "Saved");
    // A decision is made once.
    let err = conn
        .call(Call::new("project.recover", json!({})))
        .unwrap_err();
    assert_eq!(err.code(), "invalid");
    must(&conn, "project.rename", json!({ "name": "After discard" }));
    must(&conn, "project.save", json!({}));
    engine.shutdown();
    let on_disk = ProjectStore::open(&folder).unwrap();
    assert_eq!(on_disk.project.meta.name, "After discard");
    assert!(on_disk.report.recovery.is_none());
}

// ----- determinism -----------------------------------------------------------------------------

#[test]
fn deterministic_ids_seed_the_document_and_the_jobs_separately() {
    let t = TempDir::new("seed");
    let path = path_str(&t.path().join("m").join("a.mp4"));
    let run = || {
        let mut config = EngineConfig::new(EngineMode::Oneshot);
        config.deterministic_ids = Some(42);
        config.allowed_roots = vec![t.path().to_path_buf()];
        config.subsystems = Some(Box::new(FakeMedia::new(&[&path])));
        let engine = Engine::start(config).unwrap();
        let conn = engine.handle().connect(&agent("cli")).unwrap();
        let doc = must(&conn, "project.new", json!({ "name": "Seeded" }));
        let import = must(&conn, "media.import", json!({ "paths": [path] }));
        let out = (doc, import["media_ids"].clone(), import["job_ids"].clone());
        engine.shutdown();
        out
    };
    let first = run();
    let second = run();
    assert_eq!(first, second);
    let doc_id = first.0["sequence"].as_str().unwrap();
    let job_id = first.2[0].as_str().unwrap();
    assert_ne!(
        doc_id.split_once('_').unwrap().1,
        job_id.split_once('_').unwrap().1,
        "job ids must come from their own stream"
    );
}

// ----- dev stub media (R27) --------------------------------------------------------------------

#[test]
fn dev_stub_media_imports_by_extension_and_the_media_goes_on_the_timeline() {
    let t = TempDir::new("stub");
    let file = |n: &str, bytes: &[u8]| {
        let p = t.path().join(n);
        std::fs::write(&p, bytes).unwrap();
        path_str(&p)
    };
    let (mp4, wav, png) = (
        file("a.mp4", b"video"),
        file("b.wav", b"audio"),
        file("c.png", b"image"),
    );
    let (txt, gone) = (file("d.txt", b"text"), path_str(&t.path().join("gone.mp4")));

    let mut config = oneshot(t.path());
    config.subsystems = Some(Box::new(cutweave_engine::DevStubMedia));
    let engine = Engine::start(config).unwrap();
    let conn = engine.handle().connect(&agent("cli")).unwrap();
    let (sequence, track) = new_document(&conn, "Stub");
    let out = must(
        &conn,
        "media.import",
        json!({ "paths": [mp4, wav, png, txt, gone] }),
    );
    let ids = out["media_ids"].as_array().unwrap().clone();
    assert_eq!(ids.len(), 3, "{out}");
    assert_eq!(out["failed"].as_array().unwrap().len(), 2, "{out}");
    assert_eq!(must(&conn, "media.list", json!({}))["total"], 3);

    let inserted = must(
        &conn,
        "timeline.insert",
        json!({ "sequence": sequence, "track": track, "media": ids[0], "at": "0s" }),
    );
    // Video with audio: linked video and audio clips.
    assert_eq!(inserted["clips"].as_array().unwrap().len(), 2);
    let still = must(
        &conn,
        "timeline.insert",
        json!({ "sequence": sequence, "track": track, "media": ids[2], "at": "10s", "duration": "3s" }),
    );
    assert_eq!(still["clips"].as_array().unwrap().len(), 1);
    engine.shutdown();
}

#[test]
fn without_the_dev_stub_an_existing_mp4_still_fails_to_import() {
    let t = TempDir::new("nostub");
    let mp4 = t.path().join("a.mp4");
    std::fs::write(&mp4, b"video").unwrap();
    let engine = Engine::start(oneshot(t.path())).unwrap();
    let conn = engine.handle().connect(&agent("cli")).unwrap();
    new_document(&conn, "Core");
    let out = must(&conn, "media.import", json!({ "paths": [path_str(&mp4)] }));
    assert_eq!(out["failed"][0]["code"], "io");
    assert_eq!(out["media_ids"], json!([]));
    engine.shutdown();
}

// ----- project.set_autosave (WP-1M.10) --------------------------------------------------------

fn fast_autosave(t: &TempDir) -> EngineConfig {
    let mut config = oneshot(t.path());
    config.autosave = Some(AutosaveConfig {
        idle: Duration::from_millis(100),
        poll: Duration::from_millis(20),
        ..AutosaveConfig::default()
    });
    config
}

#[test]
fn set_autosave_never_dirties_and_off_stops_autosaves_across_projects() {
    let t = TempDir::new("set-autosave");
    let a = empty_project(t.path(), "A");
    let b = empty_project(t.path(), "B");
    // The app applies the person's preference to its own engine: the UI credential only.
    let mut config = fast_autosave(&t);
    config.mode = EngineMode::Gui;
    config.ui_token = Some(ui_token());
    config.api.run_dir = Some(t.path().join("run"));
    let engine = Engine::start(config).unwrap();
    let h = engine.handle();
    let conn = h.connect(&Credential::Ui).unwrap();
    let mut events = h.subscribe();
    must(&conn, "project.open", json!({ "path": path_str(&a) }));
    assert!(!h.status().unwrap().dirty);
    // An agent cannot override it (WP-1J.05).
    let err = h
        .connect(&agent("cli"))
        .unwrap()
        .call(Call::new(
            "project.set_autosave",
            json!({ "enabled": false, "interval_s": 60, "keep_backups": 5 }),
        ))
        .unwrap_err();
    assert_eq!(err.code(), "forbidden");

    let out = must(
        &conn,
        "project.set_autosave",
        json!({ "enabled": false, "interval_s": 60, "keep_backups": 5 }),
    );
    assert_eq!(out["interval_s"], 60);
    // Not an edit: the project stays clean, nothing is journaled, the document revision holds.
    let status = h.status().unwrap();
    assert!(!status.dirty);
    let rev = status.rev;
    must(
        &conn,
        "project.set_autosave",
        json!({ "enabled": false, "interval_s": 60, "keep_backups": 5 }),
    );
    assert_eq!(h.status().unwrap().rev, rev);
    assert!(!h.status().unwrap().dirty);
    for bad in [
        json!({ "enabled": true, "interval_s": 0, "keep_backups": 5 }),
        json!({ "enabled": true, "interval_s": 30, "keep_backups": 0 }),
    ] {
        let err = conn
            .call(Call::new("project.set_autosave", bad))
            .unwrap_err();
        assert_eq!(err.code(), "invalid");
    }

    must(&conn, "project.rename", json!({ "name": "Edited" }));
    std::thread::sleep(Duration::from_millis(600));
    assert!(h.status().unwrap().dirty, "autosave is off");
    assert!(
        !std::fs::read_to_string(a.join("A.cutweave"))
            .unwrap()
            .contains("Edited")
    );

    // The preference also holds for the next project the engine opens (a session swap).
    must(&conn, "project.close", json!({ "save": false }));
    must(&conn, "project.open", json!({ "path": path_str(&b) }));
    must(&conn, "project.rename", json!({ "name": "Edited B" }));
    std::thread::sleep(Duration::from_millis(600));
    assert!(h.status().unwrap().dirty);

    // Turning it on saves the pending edit.
    must(
        &conn,
        "project.set_autosave",
        json!({ "enabled": true, "interval_s": 30, "keep_backups": 10 }),
    );
    wait_for(&mut events, 10 * SEC, |e| {
        matches!(e, EngineEvent::Project(ProjectEvent::Saved(_)))
    });
    engine.shutdown();
    assert_eq!(
        ProjectStore::open(&b).unwrap().project.meta.name,
        "Edited B"
    );
}
