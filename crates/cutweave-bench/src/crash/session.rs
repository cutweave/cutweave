// SPDX-License-Identifier: GPL-3.0-or-later
//! One run of the crash harness: the engine under test and the reference engine, the work
//! between crashes, the kill at each [`Point`] and the checks after it (see the module docs
//! of [`crate::crash`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::runtime::Runtime;

use super::clients::{AGENT_DRAIN_WAIT, ANSWER_WAIT, Agent, Answer, DRAIN_WAIT, Editor, Pending};
use super::compare::{IdMap, same_document};
use super::script::{Drag, Move, Op, Placed, Script, Who};
use super::{CrashOptions, Fault, Point, Row, disk};
use crate::BenchError;
use crate::fixture::{FixtureSpec, generate};
use crate::harness::{ChildEngine, ChildSpec};

/// How long an engine that aborts itself may take to be gone.
const EXIT_WAIT: Duration = Duration::from_secs(10);
/// Autosave's idle delay (`AutosaveConfig::idle`): a save starts this long after the last edit,
/// at the next poll (250 ms).
const AUTOSAVE_IDLE_MS: u64 = 2000;
/// The most ticks an engine with `CUTWEAVE_TEST_CRASH_AFTER` is given to abort.
const ABORT_TICKS: u32 = 80;

fn other(what: &str, e: impl std::fmt::Display) -> BenchError {
    BenchError::Other(format!("{what}: {e}"))
}

/// The seeded delay between sending the last requests and the kill: it decides how far the
/// engine gets with them. Not a wait for anything.
fn delay_kill(micros: i64) {
    if let Ok(micros @ 1..) = u64::try_from(micros) {
        std::thread::sleep(Duration::from_micros(micros));
    }
}

/// An engine process and its two clients (dropped before it).
struct Live {
    editor: Editor,
    agent: Agent,
    engine: ChildEngine,
}

/// Something the engine under test acknowledged and the reference has not been given yet.
struct Entry {
    rev: u64,
    seq: u64,
    what: What,
    output: Value,
}

enum What {
    Plain(Op),
    Drag(Drag),
}

/// What was open when the engine died.
#[derive(Default)]
struct Flight {
    /// Commands sent (or about to be) and not answered, in order.
    unanswered: Vec<Op>,
    /// A drag whose commit was not answered.
    drag: Option<Drag>,
    /// The engine is gone.
    died: bool,
    /// It died without the harness killing it or asking it to.
    unexpected: bool,
    note: String,
}

/// How a drag ends.
enum DragEnd {
    /// Committed and acknowledged.
    Commit,
    /// The engine is killed with the gesture open and this agent call in flight.
    KillOpen(Op),
    /// The engine is killed this many microseconds after the commit was sent.
    KillAtCommit(i64),
}

/// What the proxy point works on.
#[derive(Default)]
struct ProxyWork {
    /// The source a proxy is made of: its id and file.
    source: Option<(String, PathBuf)>,
    /// The source has a content hash (a proxy is filed under it; the fixture's have none).
    keyed: bool,
    /// A proxy was being made when the engine was killed.
    cut: bool,
}

pub(super) struct Session {
    exe: PathBuf,
    scratch: PathBuf,
    /// The project of the engine under test.
    folder: PathBuf,
    imports: PathBuf,
    seed: u64,
    fault: Option<Fault>,
    rt: Arc<Runtime>,
    script: Script,
    subject: Option<Live>,
    reference: Live,
    pending: Vec<Entry>,
    seq: u64,
    ids: IdMap,
    /// The highest committed revision the engine under test acknowledged.
    max_committed: u64,
    /// The export queue's pause, as acknowledged.
    paused: bool,
    /// Real media: the engine under test is the full engine.
    media: bool,
    export_dir: PathBuf,
    /// Exports acknowledged as queued and not checked yet: `(job, file)`.
    exports: Vec<(String, PathBuf)>,
    proxy: ProxyWork,
    acknowledged: usize,
    refused: usize,
    /// Why the engine refused the last command it refused.
    last_refusal: String,
    /// What the checks of the background jobs saw after the last crash (for the report).
    seen: String,
    life_acknowledged: usize,
}

impl Session {
    /// Makes the project, copies it for the reference and starts both engines; the engine
    /// under test is started for the crash point `first`.
    pub(super) fn start(
        options: &CrashOptions,
        first: Option<Point>,
    ) -> Result<Session, BenchError> {
        let scratch = options.scratch.clone();
        if scratch.exists() {
            std::fs::remove_dir_all(&scratch).map_err(|e| BenchError::io("empty", &scratch, e))?;
        }
        let imports = scratch.join("imports");
        std::fs::create_dir_all(&imports).map_err(|e| BenchError::io("create", &imports, e))?;
        let spec = FixtureSpec {
            duration_secs: 180,
            main_items: Some(12),
            ..FixtureSpec::new("P-crash", options.seed, 48, 6, 96)
        };
        let fixture = generate(&spec)?;
        let folder = fixture.write(&scratch.join("subject"))?;
        let ref_folder = scratch.join("reference").join(&spec.name);
        disk::copy_dir(&folder, &ref_folder)?;
        if options.media && !crate::media::synthesize(&folder, &fixture.manifest)? {
            return Err(BenchError::Other(
                "--with-media needs the FFmpeg bundle (`. scripts/ffmpeg/env.sh`)".into(),
            ));
        }
        let proxy_source = fixture.manifest.media.iter().find(|m| m.video);
        let proxy_source = proxy_source.map(|m| (m.id.clone(), folder.join("media").join(&m.file)));
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| other("start a runtime", e))?;
        let rt = Arc::new(rt);
        let mut script = Script::new(&fixture.manifest, options.seed);
        let base = Base {
            exe: &options.exe,
            scratch: &scratch,
            seed: options.seed,
            rt: &rt,
        };
        let reference = base.spawn("reference-run", &ref_folder, Vec::new(), false)?;
        let env = abort_env(&mut script, first);
        let subject = base.spawn("subject-run", &folder, env, options.media)?;
        Ok(Session {
            exe: options.exe.clone(),
            scratch,
            folder,
            imports,
            seed: options.seed,
            fault: options.fault,
            rt,
            script,
            subject: Some(subject),
            reference,
            pending: Vec::new(),
            seq: 0,
            ids: IdMap::default(),
            max_committed: 0,
            paused: false,
            media: options.media,
            export_dir: options.scratch.join("exports"),
            exports: Vec::new(),
            proxy: ProxyWork {
                source: proxy_source,
                ..ProxyWork::default()
            },
            acknowledged: 0,
            refused: 0,
            last_refusal: String::new(),
            seen: String::new(),
            life_acknowledged: 0,
        })
    }

    fn base(&self) -> Base<'_> {
        Base {
            exe: &self.exe,
            scratch: &self.scratch,
            seed: self.seed,
            rt: &self.rt,
        }
    }

    /// Commands acknowledged and refused so far.
    pub(super) fn totals(&self) -> (usize, usize) {
        (self.acknowledged, self.refused)
    }

    fn record(&mut self, what: What, answer: Answer) {
        match answer {
            Answer::Done {
                output,
                rev,
                committed,
            } => {
                if let What::Plain(op) = &what {
                    self.script.learn(op, &output);
                }
                self.max_committed = self.max_committed.max(committed);
                self.seq += 1;
                self.pending.push(Entry {
                    rev,
                    seq: self.seq,
                    what,
                    output,
                });
                self.acknowledged += 1;
                self.life_acknowledged += 1;
            }
            Answer::Refused(why) => {
                self.refused += 1;
                self.last_refusal = why;
            }
        }
    }

    // ----- work ---------------------------------------------------------------------------

    /// Sends `ops` on the editor's connection without waiting (`project.save` before the op at
    /// `save_at`), with `agent`'s call running, optionally kills the engine `kill_after`
    /// microseconds later, then reads the answers. What got none is in `flight`.
    fn exchange(
        &mut self,
        ops: Vec<Op>,
        agent: Option<Op>,
        save_at: Option<usize>,
        kill_after: Option<i64>,
        flight: &mut Flight,
    ) -> Result<(), BenchError> {
        let Some(mut live) = self.subject.take() else {
            return Err(BenchError::Other(
                "the engine under test is not running".into(),
            ));
        };
        let call = agent.map(|op| {
            let pending = live.agent.start(op.command, &op.params, &op.request_id);
            (op, pending)
        });
        let mut tickets = Vec::new();
        for (i, op) in ops.iter().enumerate() {
            if save_at == Some(i) {
                let _ = live.editor.send("project.save", &json!({}), None, None);
            }
            let sent = live
                .editor
                .send(op.command, &op.params, Some(&op.request_id), None);
            tickets.push(sent.ok());
        }
        if let Some(micros) = kill_after {
            delay_kill(micros);
            live.engine.kill();
        }
        let wait = if kill_after.is_some() {
            DRAIN_WAIT
        } else {
            ANSWER_WAIT
        };
        let mut dead = false;
        for (op, ticket) in ops.into_iter().zip(tickets) {
            let answer = match ticket {
                Some(ticket) if !dead => live.editor.answer(ticket, wait).ok(),
                _ => None,
            };
            if let Some(answer) = answer {
                self.record(What::Plain(op), answer);
            } else {
                dead = true;
                flight.unanswered.push(op);
            }
        }
        if let Some((op, pending)) = call {
            let gone = dead || kill_after.is_some();
            if let Some(answer) = agent_answer(&mut live, pending, gone) {
                self.record(What::Plain(op), answer);
            } else {
                dead = true;
                flight.unanswered.push(op);
            }
        }
        if dead && kill_after.is_none() && !live.engine.wait_exit(EXIT_WAIT) {
            return Err(BenchError::Other(
                "a request got no answer, but the engine is still running".into(),
            ));
        }
        flight.died |= dead || kill_after.is_some();
        self.subject = Some(live);
        Ok(())
    }

    /// The command for a clip that must be looked at first; `None`: the clip is gone or too
    /// short, or the engine is (then `flight.died`).
    fn placed(&mut self, kind: Placed, item: &str, flight: &mut Flight) -> Option<Op> {
        let live = self.subject.as_mut()?;
        match live
            .editor
            .call("item.get", &json!({"item": item}), None, None)
        {
            Ok(Answer::Done { output, .. }) => {
                let (start, duration) = (output["start"].as_i64()?, output["duration"].as_i64()?);
                self.script.place(kind, item, start, duration)
            }
            Ok(Answer::Refused(_)) => None,
            Err(_) => {
                flight.died = true;
                None
            }
        }
    }

    /// One step of ordinary work, everything acknowledged (unless the engine dies: then
    /// `flight.died`).
    fn tick(&mut self, flight: &mut Flight) -> Result<(), BenchError> {
        let agent = (self.script.below(3) != 0).then(|| self.script.agent());
        let mut ops = match self.script.editor() {
            Move::Drag(drag) => return self.drag(drag, DragEnd::Commit, flight).map(|_| ()),
            Move::Plain(op) => vec![op],
            Move::Placed(kind, item) => {
                let op = self.placed(kind, &item, flight);
                if flight.died {
                    return self.confirm_exit();
                }
                vec![op.unwrap_or_else(|| self.script.light())]
            }
        };
        for _ in 0..self.script.below(3) {
            ops.push(self.script.light());
        }
        // Now and then a file is imported, and the project saved (the journal rotates) with
        // the commands behind the save already on their way.
        if !self.media && self.script.below(12) == 0 {
            ops.push(self.new_import()?);
        }
        let save_at = (self.script.below(8) == 0).then_some(0);
        self.exchange(ops, agent, save_at, None, flight)
    }

    /// `media.import` of a new file with content of its own.
    fn new_import(&mut self) -> Result<Op, BenchError> {
        let path = self.imports.join(self.script.import_name());
        let content = format!("stub media for {}", path.display());
        std::fs::write(&path, content).map_err(|e| BenchError::io("write", &path, e))?;
        Ok(self.script.import(&path))
    }

    /// After a request failed without a kill: the engine must really be gone.
    fn confirm_exit(&mut self) -> Result<(), BenchError> {
        let gone = self
            .subject
            .as_mut()
            .is_none_or(|live| live.engine.wait_exit(EXIT_WAIT));
        if gone {
            Ok(())
        } else {
            Err(BenchError::Other(
                "a request got no answer, but the engine is still running".into(),
            ))
        }
    }

    /// Runs a drag on the editor's connection. Returns whether the gesture got as far as
    /// `end` asks (a refused begin, or no accepted update, ends it early with nothing done).
    fn drag(
        &mut self,
        mut drag: Drag,
        end: DragEnd,
        flight: &mut Flight,
    ) -> Result<bool, BenchError> {
        let Some(mut live) = self.subject.take() else {
            return Err(BenchError::Other(
                "the engine under test is not running".into(),
            ));
        };
        let opened = open_drag(&mut live.editor, &mut drag);
        self.refused += opened.refused;
        let reached = match opened.tx {
            Err(()) => {
                flight.died = true;
                false
            }
            Ok(None) => false,
            Ok(Some(_)) => match end {
                DragEnd::Commit => {
                    let op = &drag.commit;
                    let answer =
                        live.editor
                            .call(op.command, &op.params, Some(&op.request_id), None);
                    if let Ok(answer) = answer {
                        self.record(What::Drag(drag), answer);
                    } else {
                        flight.died = true;
                        flight.drag = Some(drag);
                    }
                    true
                }
                DragEnd::KillOpen(agent) => {
                    let pending = live
                        .agent
                        .start(agent.command, &agent.params, &agent.request_id);
                    live.engine.kill();
                    match agent_answer(&mut live, pending, true) {
                        Some(answer) => self.record(What::Plain(agent), answer),
                        None => flight.unanswered.push(agent),
                    }
                    flight.died = true;
                    true
                }
                DragEnd::KillAtCommit(micros) => {
                    let op = &drag.commit;
                    let ticket =
                        live.editor
                            .send(op.command, &op.params, Some(&op.request_id), None);
                    delay_kill(micros);
                    live.engine.kill();
                    match ticket.and_then(|t| live.editor.answer(t, DRAIN_WAIT)) {
                        Ok(answer) => self.record(What::Drag(drag), answer),
                        Err(_) => flight.drag = Some(drag),
                    }
                    flight.died = true;
                    true
                }
            },
        };
        self.subject = Some(live);
        if flight.died && !reached {
            self.confirm_exit()?;
        }
        Ok(reached)
    }

    // ----- the kill -----------------------------------------------------------------------

    /// Works for a while, then kills the engine at `point`.
    fn work_and_kill(&mut self, point: Point) -> Result<Flight, BenchError> {
        let mut flight = Flight::default();
        if point == Point::CommitAbort {
            for _ in 0..ABORT_TICKS {
                self.tick(&mut flight)?;
                if flight.died {
                    return Ok(flight);
                }
            }
            // A release build has no test hook.
            flight.note = "this build has no CUTWEAVE_TEST_CRASH_AFTER: killed instead".into();
            self.exchange(Vec::new(), None, None, Some(0), &mut flight)?;
            return Ok(flight);
        }
        for _ in 0..2 + self.script.below(6) {
            self.tick(&mut flight)?;
            if flight.died {
                flight.unexpected = true;
                return Ok(flight);
            }
        }
        self.strike(point, &mut flight)?;
        Ok(flight)
    }

    fn strike(&mut self, point: Point, flight: &mut Flight) -> Result<(), BenchError> {
        match point {
            Point::Edit | Point::Recover | Point::CommitAbort => self.strike_edit(flight),
            Point::Save => {
                let ops = vec![self.script.light(), self.script.light()];
                let agent = self.script.agent_touch();
                let after = self.script.below(30_000);
                self.exchange(ops, Some(agent), Some(1), Some(after), flight)
            }
            Point::Autosave => {
                let idle = AUTOSAVE_IDLE_MS + u64::try_from(self.script.below(450)).unwrap_or(0);
                std::thread::sleep(Duration::from_millis(idle));
                let agent = self.script.agent_touch();
                self.exchange(Vec::new(), Some(agent), None, Some(0), flight)
            }
            Point::Drag | Point::DragCommit => {
                let end = if point == Point::Drag {
                    DragEnd::KillOpen(self.script.agent_touch())
                } else {
                    DragEnd::KillAtCommit(self.script.below(2000))
                };
                let drag = self.script.drag();
                if self.drag(drag, end, flight)? || flight.died {
                    return Ok(());
                }
                flight.note = "the drag was refused: killed during edits instead".into();
                self.strike_edit(flight)
            }
            Point::TornTail => self.strike_torn_tail(flight),
            Point::Import => {
                let ops = vec![self.new_import()?];
                let agent = self.script.agent_touch();
                let after = self.script.below(25_000);
                self.exchange(ops, Some(agent), None, Some(after), flight)
            }
            Point::Queue => {
                self.set_pause(!self.paused)?;
                let ops = vec![self.script.light()];
                let after = self.script.below(1500);
                self.exchange(ops, None, None, Some(after), flight)
            }
            Point::Export => {
                if self.paused {
                    self.set_pause(false)?;
                }
                std::fs::create_dir_all(&self.export_dir)
                    .map_err(|e| BenchError::io("create", &self.export_dir, e))?;
                for _ in 0..2 {
                    // Five seconds of the sequence, small, in software.
                    let path = self.export_dir.join(self.script.export_name());
                    let params = json!({
                        "sequence": self.script.sequence(),
                        "path": path, "hw": "off",
                        "preset": {"custom": {"width": 320, "height": 180}},
                        "range": {"in": 0, "out": 150 * self.script.frame()},
                    });
                    let queued = self.acknowledged_call("export.enqueue", &params)?;
                    let text = |key: &str| queued[key].as_str().unwrap_or_default().to_owned();
                    self.exports
                        .push((text("job_id"), PathBuf::from(text("path"))));
                }
                self.strike_job(flight)
            }
            Point::Proxy => {
                let Some((source, file)) = self.proxy.source.clone() else {
                    return self.strike_job(flight);
                };
                if !self.proxy.keyed {
                    // An edit like any other: the reference gets it too.
                    // The shape of a real content hash; proxies are filed under it.
                    let hash = format!("blake3-fast:{:064x}", self.seed);
                    let params = json!({"media": source, "path": file, "content_hash": hash});
                    let op = self.script.editor_op("media.set_location", params);
                    let refused = self.refused;
                    self.exchange(vec![op], None, None, None, flight)?;
                    if flight.died {
                        flight.unexpected = true;
                        return Ok(());
                    }
                    if self.refused > refused {
                        let why = &self.last_refusal;
                        return Err(BenchError::command("media.set_location", why));
                    }
                    self.proxy.keyed = true;
                }
                self.acknowledged_call("media.generate_proxy", &json!({"media": [source]}))?;
                self.proxy.cut = true;
                self.strike_job(flight)
            }
        }
    }

    /// A call on the editor's connection that must be acknowledged; returns its output.
    fn acknowledged_call(&mut self, method: &str, params: &Value) -> Result<Value, BenchError> {
        let id = self.script.request_id();
        let Some(live) = self.subject.as_mut() else {
            return Err(BenchError::Other(
                "the engine under test is not running".into(),
            ));
        };
        match live.editor.call(method, params, Some(&id), None)? {
            Answer::Done { output, .. } => Ok(output),
            Answer::Refused(e) => Err(BenchError::command(method, &e)),
        }
    }

    fn set_pause(&mut self, paused: bool) -> Result<(), BenchError> {
        self.acknowledged_call("export.pause_queue", &json!({"paused": paused}))?;
        self.paused = paused;
        Ok(())
    }

    /// The kill while a background job runs: 0.1 to 1.5 s after it was started, with an edit
    /// in flight.
    fn strike_job(&mut self, flight: &mut Flight) -> Result<(), BenchError> {
        let ops = vec![self.script.light()];
        let after = 100_000 + self.script.below(1_400_000);
        self.exchange(ops, None, None, Some(after), flight)
    }

    fn strike_edit(&mut self, flight: &mut Flight) -> Result<(), BenchError> {
        let mut ops = vec![self.script.light()];
        for _ in 0..self.script.below(3) {
            let op = if self.script.below(2) == 0 {
                self.script.marker()
            } else {
                self.script.light()
            };
            ops.push(op);
        }
        let agent = self.script.agent();
        let after = self.script.below(3000);
        self.exchange(ops, Some(agent), None, Some(after), flight)
    }

    /// An acknowledged marker, a kill, and the journal's last record cut short. The marker is
    /// then a command the journal never fully had: it counts as in flight (unless the harness
    /// is testing itself).
    fn strike_torn_tail(&mut self, flight: &mut Flight) -> Result<(), BenchError> {
        let mut before = None;
        for _ in 0..5 {
            let was = self.max_committed;
            let op = self.script.marker();
            self.exchange(vec![op], None, None, None, flight)?;
            if flight.died {
                flight.unexpected = true;
                return Ok(());
            }
            if self.max_committed > was {
                before = Some(was);
                break;
            }
        }
        self.exchange(Vec::new(), None, None, Some(0), flight)?;
        let cut = u64::try_from(1 + self.script.below(24)).unwrap_or(1);
        let torn = match before {
            Some(_) => disk::tear_tail(&self.folder, cut)?,
            None => None,
        };
        let (Some(before), Some(_)) = (before, torn) else {
            flight.note = "no journal record to cut".into();
            return Ok(());
        };
        flight.note = format!("the journal's last record cut by {cut} bytes");
        if self.fault == Some(Fault::LoseAcknowledged) {
            return Ok(());
        }
        if let Some(Entry {
            what: What::Plain(op),
            ..
        }) = self.pending.pop()
        {
            flight.unanswered.push(op);
            self.max_committed = before;
        }
        Ok(())
    }

    // ----- after the kill -----------------------------------------------------------------

    /// One crash: the work, the kill at `point`, the restart (for the crash point `next`) and
    /// the checks.
    pub(super) fn crash(
        &mut self,
        n: u32,
        point: Point,
        next: Option<Point>,
    ) -> Result<Row, BenchError> {
        self.life_acknowledged = 0;
        let flight = self.work_and_kill(point)?;
        let began = Instant::now();
        let mut row = Row {
            n,
            point,
            acknowledged: self.life_acknowledged,
            in_flight: flight.unanswered.len() + usize::from(flight.drag.is_some()),
            journaled: 0,
            replayed: 0,
            recovered_rev: 0,
            note: flight.note.clone(),
            result: Ok(()),
            ms: 0,
        };
        let env = abort_env(&mut self.script, next);
        row.result = if flight.unexpected {
            Err("the engine died by itself during ordinary work".into())
        } else {
            self.recover(point, flight, &env, &mut row)
        };
        let seen = std::mem::take(&mut self.seen);
        if !seen.is_empty() {
            let gap = if row.note.is_empty() { "" } else { "; " };
            row.note = format!("{}{gap}{seen}", row.note);
        }
        row.ms = began.elapsed().as_millis();
        Ok(row)
    }

    /// Starts the engine under test on its project and says hello. `Err`: it did not open.
    fn restart(&mut self, env: &[(String, String)]) -> Result<Value, String> {
        self.subject = None;
        // Every restart is a takeover of a stale lock: a killed engine cannot remove its own.
        if !self.folder.join(".cutweave").join("lock").is_file() {
            return Err("the killed engine left no lock file behind".into());
        }
        let live = self
            .base()
            .spawn("subject-run", &self.folder, env.to_vec(), self.media)
            .map_err(|e| format!("the project did not open after the crash: {e}"))?;
        let live = self.subject.insert(live);
        let hello = live
            .editor
            .query(
                "rpc.hello",
                &json!({"client": "crash-harness", "version": "0"}),
            )
            .map_err(|e| format!("the restarted engine did not answer: {e}"))?;
        let project = hello["project"].clone();
        if project["state"] != "open" {
            return Err(format!(
                "the project is not open after the crash: {project}"
            ));
        }
        if !project["read_only"].is_null() {
            return Err(format!(
                "the project opened read-only after the crash (the dead engine's lock?): {project}"
            ));
        }
        Ok(project)
    }

    fn subject(&mut self) -> Result<&mut Live, String> {
        self.subject
            .as_mut()
            .ok_or_else(|| "the engine under test is not running".to_owned())
    }

    fn recover(
        &mut self,
        point: Point,
        flight: Flight,
        env: &[(String, String)],
        row: &mut Row,
    ) -> Result<(), String> {
        let mut project = self.restart(env)?;
        if point == Point::Recover && !project["recovery"].is_null() {
            // The second kill: while the journal is replayed.
            let id = self.script.request_id();
            let after = self.script.below(4000);
            let live = self.subject()?;
            let _ = live
                .editor
                .send("project.recover", &json!({}), Some(&id), None);
            delay_kill(after);
            live.engine.kill();
            row.note = "killed again during project.recover".into();
            project = self.restart(env)?;
        }
        if !project["recovery"].is_null() {
            let id = self.script.request_id();
            let live = self.subject()?;
            let answer = live
                .editor
                .call("project.recover", &json!({}), Some(&id), None)
                .map_err(|e| format!("project.recover got no answer: {e}"))?;
            match answer {
                Answer::Done { output, .. } => {
                    row.replayed = output["applied"].as_u64().unwrap_or(0);
                }
                Answer::Refused(e) => return Err(format!("project.recover failed: {e}")),
            }
        }
        let snapshot = self.snapshot(Side::Subject)?;
        row.recovered_rev = snapshot["committed_rev"].as_u64().unwrap_or(0);
        if row.recovered_rev < self.max_committed {
            return Err(format!(
                "acknowledged edits are lost: the engine had acknowledged revision {}, the \
                 recovered document is at revision {}",
                self.max_committed, row.recovered_rev
            ));
        }
        self.settle(flight, row)?;
        self.sync_reference()?;
        self.compare()?;
        disk::check(&self.folder, false)?;
        self.check_exports()?;
        self.check_proxy()?;
        if self.script.below(2) == 0 {
            self.save()?;
        }
        Ok(())
    }

    /// A read-only call on the engine under test.
    fn ask(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        self.subject()?
            .editor
            .query(method, params)
            .map_err(|e| format!("`{method}` failed after the crash: {e}"))
    }

    /// The exports queued before the kill: all still queued, in order; each finishes with its
    /// file; no partial file is left.
    fn check_exports(&mut self) -> Result<(), String> {
        let exports = std::mem::take(&mut self.exports);
        if exports.is_empty() {
            return Ok(());
        }
        let queue = self.ask("export.queue", &json!({}))?;
        let acknowledged: Vec<&str> = exports.iter().map(|(id, _)| id.as_str()).collect();
        let listed: Vec<&str> = queue["jobs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|job| job["id"].as_str())
            .filter(|id| acknowledged.contains(id))
            .collect();
        if listed != acknowledged {
            return Err(format!(
                "the export queue after the crash holds {listed:?} of the acknowledged exports \
                 {acknowledged:?}"
            ));
        }
        for (id, path) in &exports {
            let mut job = Value::Null;
            // `job.wait` waits at most a minute a call.
            for _ in 0..5 {
                job = self.ask("job.wait", &json!({"job_id": id, "timeout_ms": 60_000}))?;
                if matches!(
                    job["state"].as_str(),
                    Some("succeeded" | "failed" | "cancelled")
                ) {
                    break;
                }
            }
            if job["state"] != "succeeded" {
                return Err(format!(
                    "the export to {} queued before the crash ended as {}: {}",
                    path.display(),
                    job["state"],
                    job["error"]
                ));
            }
            if !std::fs::metadata(path).is_ok_and(|m| m.len() > 0) {
                return Err(format!(
                    "the finished export {} has no file",
                    path.display()
                ));
            }
        }
        self.seen = format!(
            "{} queued exports finished after the restart",
            exports.len()
        );
        let partials = disk::partials(&self.export_dir);
        if !partials.is_empty() {
            return Err(format!(
                "the interrupted export left partial files: {}",
                partials.join(", ")
            ));
        }
        for (job, _) in &exports {
            let id = self.script.request_id();
            let params = json!({"id": job});
            let _ = self
                .subject()?
                .editor
                .call("export.remove", &params, Some(&id), None);
        }
        Ok(())
    }

    /// The proxy that was being made at the kill: its state is one a restart can leave, a
    /// proxy called ready has its file, and asking for it again gets it made.
    fn check_proxy(&mut self) -> Result<(), String> {
        let (true, Some((source, _))) = (
            std::mem::take(&mut self.proxy.cut),
            self.proxy.source.clone(),
        ) else {
            return Ok(());
        };
        let params = json!({"media": [source]});
        let has_file = |proxy: &Value| {
            let path = proxy["path"].as_str().unwrap_or_default();
            std::fs::metadata(path).is_ok_and(|m| m.len() > 0)
        };
        let proxy = self.ask("media.proxy_status", &params)?["proxies"][0].take();
        let state = proxy["status"].as_str().unwrap_or_default().to_owned();
        if !["none", "queued", "generating", "incomplete", "ready"].contains(&state.as_str()) {
            return Err(format!(
                "after the crash the interrupted proxy is `{state}`: {proxy}"
            ));
        }
        self.seen = format!("the proxy was `{state}` after the restart");
        if state == "ready" && !has_file(&proxy) {
            return Err(format!("a proxy is called ready without its file: {proxy}"));
        }
        let id = self.script.request_id();
        let again = self
            .subject()?
            .editor
            .call("media.generate_proxy", &params, Some(&id), None)
            .map_err(|e| format!("media.generate_proxy got no answer: {e}"))?;
        if let Answer::Refused(e) = again {
            return Err(format!(
                "the proxy cannot be made again after the crash: {e}"
            ));
        }
        let mut proxy = Value::Null;
        for _ in 0..8 {
            proxy = self.ask("media.proxy_status", &params)?["proxies"][0].take();
            match (proxy["status"].as_str(), proxy["job_id"].as_str()) {
                (Some("ready"), _) if has_file(&proxy) => {
                    let id = self.script.request_id();
                    let _ =
                        self.subject()?
                            .editor
                            .call("media.delete_proxy", &params, Some(&id), None);
                    return Ok(());
                }
                (Some("queued" | "generating"), Some(job)) => {
                    let wait = json!({"job_id": job, "timeout_ms": 60_000});
                    self.ask("job.wait", &wait)?;
                }
                _ => break,
            }
        }
        Err(format!(
            "the proxy interrupted by the crash was not made again: {proxy}"
        ))
    }

    /// Sends the commands that were in flight again, under their request ids: the journal
    /// either had one (its original result comes back) or it runs now.
    fn settle(&mut self, flight: Flight, row: &mut Row) -> Result<(), String> {
        let recovered = row.recovered_rev;
        for op in flight.unanswered {
            let live = self.subject()?;
            let answer = match op.who {
                Who::Editor => live
                    .editor
                    .call(op.command, &op.params, Some(&op.request_id), None),
                Who::Agent => live.agent.call(op.command, &op.params, &op.request_id),
            }
            .map_err(|e| format!("`{}` sent again got no answer: {e}", op.command))?;
            // An import is not deduplicated: one the journal had reports its file as a
            // duplicate, and belongs before everything sent after the recovery.
            let import = op.command == "media.import";
            let journaled = match &answer {
                Answer::Done { output, .. } if import => is_duplicate(output),
                Answer::Done { rev, .. } => *rev <= recovered,
                Answer::Refused(_) => false,
            };
            row.journaled += usize::from(journaled);
            let answer = match answer {
                Answer::Done {
                    output, committed, ..
                } if import && journaled => Answer::Done {
                    output,
                    rev: recovered,
                    committed,
                },
                other => other,
            };
            self.record(What::Plain(op), answer);
        }
        if let Some(drag) = flight.drag {
            let op = drag.commit.clone();
            let live = self.subject()?;
            let answer = live
                .editor
                .call(op.command, &op.params, Some(&op.request_id), None)
                .map_err(|e| format!("the drag's commit sent again got no answer: {e}"))?;
            match answer {
                Answer::Done { .. } => {
                    row.journaled += 1;
                    self.record(What::Drag(drag), answer);
                }
                // The journal has the commit, but a commit is not answered twice after a
                // recovery (issue #235). A drag runs alone, so a revision past the last
                // acknowledged one can only be its commit.
                Answer::Refused(_) if recovered > self.max_committed => {
                    row.journaled += 1;
                    let committed = Answer::Done {
                        output: Value::Null,
                        rev: recovered,
                        committed: recovered,
                    };
                    self.record(What::Drag(drag), committed);
                }
                // The gesture died with the engine, uncommitted.
                Answer::Refused(_) => {}
            }
        }
        Ok(())
    }

    /// Gives the reference engine what the engine under test acknowledged, in revision order.
    fn sync_reference(&mut self) -> Result<(), String> {
        let mut entries = std::mem::take(&mut self.pending);
        entries.sort_by_key(|e| (e.rev, e.seq));
        for entry in entries {
            let output = match &entry.what {
                What::Plain(op) => {
                    let params = self.ids.translate(&op.params);
                    let live = &mut self.reference;
                    let answer = match op.who {
                        Who::Editor => {
                            live.editor
                                .call(op.command, &params, Some(&op.request_id), None)
                        }
                        Who::Agent => live.agent.call(op.command, &params, &op.request_id),
                    };
                    reference_output(op.command, answer)?
                }
                What::Drag(drag) => self.replay_drag(drag)?,
            };
            self.ids.pair(&entry.output, &output);
        }
        Ok(())
    }

    fn replay_drag(&mut self, drag: &Drag) -> Result<Value, String> {
        let editor = &mut self.reference.editor;
        let begin = &drag.begin;
        let answer = editor.call(begin.command, &begin.params, Some(&begin.request_id), None);
        let opened = reference_output(begin.command, answer)?;
        let tx = opened["tx"].as_str().unwrap_or_default().to_owned();
        for update in &drag.updates {
            let params = self.ids.translate(&update.params);
            let answer = editor.call(update.command, &params, Some(&update.request_id), Some(&tx));
            reference_output(update.command, answer)?;
        }
        let commit = &drag.commit;
        let answer = editor.call(
            commit.command,
            &commit.params,
            Some(&commit.request_id),
            None,
        );
        reference_output(commit.command, answer)
    }

    fn snapshot(&mut self, side: Side) -> Result<Value, String> {
        let live = match side {
            Side::Subject => self.subject()?,
            Side::Reference => &mut self.reference,
        };
        live.editor
            .query("project.snapshot", &json!({}))
            .map_err(|e| format!("no snapshot of the {side:?} document: {e}"))
    }

    /// The document, the checkpoints and the export queue of the engine under test against
    /// the reference and what was acknowledged.
    fn compare(&mut self) -> Result<(), String> {
        let subject = self.snapshot(Side::Subject)?["project"].take();
        let reference = self.snapshot(Side::Reference)?["project"].take();
        if let Err(place) = same_document(&subject, &reference) {
            for (name, doc) in [("subject", &subject), ("reference", &reference)] {
                let path = self.scratch.join(format!("mismatch-{name}.json"));
                let _ = std::fs::write(path, format!("{doc:#}"));
            }
            return Err(format!(
                "the recovered document is not the acknowledged one; first difference at {place} \
                 (both documents are in {})",
                self.scratch.display()
            ));
        }
        self.ids
            .pair_documents(&subject.to_string(), &reference.to_string());
        let names = |live: &mut Live| {
            let list = live
                .editor
                .query("edit.list_checkpoints", &json!({}))
                .map_err(|e| format!("no checkpoint list: {e}"))?;
            let names: Vec<String> = list["checkpoints"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| c["name"].to_string())
                .collect();
            Ok::<_, String>(names)
        };
        let (ours, theirs) = (names(self.subject()?)?, names(&mut self.reference)?);
        if ours != theirs {
            return Err(format!(
                "the checkpoints after the recovery are {ours:?}, acknowledged were {theirs:?}"
            ));
        }
        let queue = self
            .subject()?
            .editor
            .query("export.queue", &json!({}))
            .map_err(|e| format!("the export queue cannot be read after the crash: {e}"))?;
        if queue["paused"] != self.paused {
            return Err(format!(
                "the export queue's pause is {} after the crash, acknowledged was {}",
                queue["paused"], self.paused
            ));
        }
        Ok(())
    }

    fn save(&mut self) -> Result<(), String> {
        let id = self.script.request_id();
        let answer = self
            .subject()?
            .editor
            .call("project.save", &json!({}), Some(&id), None)
            .map_err(|e| format!("project.save got no answer: {e}"))?;
        if let Answer::Refused(e) = answer {
            return Err(format!("the recovered project cannot be saved: {e}"));
        }
        disk::check(&self.folder, true)
    }

    /// The end of a run: save, kill, start again. The project must open with nothing to
    /// recover and be the acknowledged document.
    pub(super) fn closing_check(&mut self) -> Result<(), String> {
        self.save()?;
        self.subject()?.engine.kill();
        let project = self.restart(&[])?;
        if !project["recovery"].is_null() {
            return Err(format!(
                "a project saved before the kill offers a recovery: {project}"
            ));
        }
        self.compare()?;
        disk::check(&self.folder, true)
    }
}

#[derive(Clone, Copy, Debug)]
enum Side {
    Subject,
    Reference,
}

/// What both engines are started with.
struct Base<'a> {
    exe: &'a Path,
    scratch: &'a Path,
    seed: u64,
    rt: &'a Arc<Runtime>,
}

impl Base<'_> {
    /// `real`: the full engine (FFmpeg, the GPU) instead of the stub prober.
    fn spawn(
        &self,
        tag: &str,
        project: &Path,
        env: Vec<(String, String)>,
        real: bool,
    ) -> Result<Live, BenchError> {
        let engine = ChildEngine::spawn(&ChildSpec {
            exe: self.exe.to_path_buf(),
            scratch: self.scratch.join(tag),
            project: Some(project.to_path_buf()),
            stub_media: !real,
            gui: false,
            deterministic_ids: Some(self.seed),
            env,
            roots: vec![self.scratch.join("imports"), self.scratch.join("exports")],
        })?;
        let editor = Editor::connect(&engine)?;
        let agent = Agent::connect(self.rt, &engine)?;
        Ok(Live {
            editor,
            agent,
            engine,
        })
    }
}

/// The answer to the agent's call; `None`: none came. With the engine `gone` the wait is
/// short (see [`AGENT_DRAIN_WAIT`]); otherwise it lasts as long as the engine runs (rmcp's
/// client does not always notice an engine that aborted), at most [`ANSWER_WAIT`].
fn agent_answer(live: &mut Live, mut pending: Pending, gone: bool) -> Option<Answer> {
    if gone {
        return live.agent.finish(pending, AGENT_DRAIN_WAIT).ok();
    }
    let deadline = Instant::now() + ANSWER_WAIT;
    loop {
        if let Some(result) = live.agent.poll(&mut pending, AGENT_DRAIN_WAIT) {
            return result.ok();
        }
        if live.engine.wait_exit(Duration::ZERO) || Instant::now() >= deadline {
            return live.agent.finish(pending, AGENT_DRAIN_WAIT).ok();
        }
    }
}

/// The environment of an engine that will be crashed at `point`: for
/// [`Point::CommitAbort`], the engine's own abort after a seeded number of commits.
fn abort_env(script: &mut Script, point: Option<Point>) -> Vec<(String, String)> {
    if point == Some(Point::CommitAbort) {
        // The engine counts from its start, and its life begins with the commands that were in
        // flight at the crash before (at most six) being sent again: it must outlast those.
        let after = 8 + script.below(10);
        vec![("CUTWEAVE_TEST_CRASH_AFTER".to_owned(), after.to_string())]
    } else {
        Vec::new()
    }
}

/// Whether `media.import`'s output says every file was already in the project.
fn is_duplicate(output: &Value) -> bool {
    output["media"]
        .as_array()
        .is_some_and(|m| !m.is_empty() && m.iter().all(|f| f["duplicate"] == true))
}

/// The output of a command on the reference engine, which must accept whatever the engine
/// under test acknowledged.
fn reference_output(command: &str, answer: Result<Answer, BenchError>) -> Result<Value, String> {
    match answer {
        Ok(Answer::Done { output, .. }) => Ok(output),
        Ok(Answer::Refused(why)) => Err(format!(
            "the reference engine refused `{command}`, which the engine under test had \
             acknowledged: {why}"
        )),
        Err(e) => Err(format!(
            "the reference engine did not answer `{command}`: {e}"
        )),
    }
}

/// What opening a drag gave.
struct Opened {
    /// The gesture's transaction; `Ok(None)`: refused, or no update was accepted (cancelled);
    /// `Err`: the connection ended.
    tx: Result<Option<String>, ()>,
    refused: usize,
}

/// Opens the gesture and sends its updates; `drag` keeps the accepted ones.
fn open_drag(editor: &mut Editor, drag: &mut Drag) -> Opened {
    let mut refused = 0;
    let begin = &drag.begin;
    let tx = match editor.call(begin.command, &begin.params, Some(&begin.request_id), None) {
        Ok(Answer::Done { output, .. }) => output["tx"].as_str().unwrap_or_default().to_owned(),
        Ok(Answer::Refused(_)) => {
            return Opened {
                tx: Ok(None),
                refused: 1,
            };
        }
        Err(_) => {
            return Opened {
                tx: Err(()),
                refused,
            };
        }
    };
    let mut accepted = Vec::new();
    for update in std::mem::take(&mut drag.updates) {
        let answer = editor.call(
            update.command,
            &update.params,
            Some(&update.request_id),
            Some(&tx),
        );
        match answer {
            Ok(Answer::Done { .. }) => accepted.push(update),
            Ok(Answer::Refused(_)) => refused += 1,
            Err(_) => {
                return Opened {
                    tx: Err(()),
                    refused,
                };
            }
        }
    }
    drag.updates = accepted;
    if drag.updates.is_empty() {
        // Under the commit's request id, which is never sent now.
        let id = drag.commit.request_id.clone();
        let _ = editor.call("edit.cancel_tx", &json!({}), Some(&id), None);
        return Opened {
            tx: Ok(None),
            refused,
        };
    }
    Opened {
        tx: Ok(Some(tx)),
        refused,
    }
}
