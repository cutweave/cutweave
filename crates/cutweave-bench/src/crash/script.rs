// SPDX-License-Identifier: GPL-3.0-or-later
//! The seeded editing work the crash harness kills the engine under: what a person does in
//! the editor (trims, ripple trims, splits, speed, audio, moves, keyframes, markers, sequences
//! and tracks, imports, checkpoints, undo and redo, drag gestures) and what an agent does over
//! MCP at the same time (properties, keyframes, pan, its own sequences and inserts).
//!
//! The script only picks; it keeps no copy of the document. A command the engine refuses (a
//! trim past a limit after an earlier ripple, an undo with nothing to undo) is part of the
//! work: the harness skips it on the reference engine too.

use serde_json::{Value, json};

use crate::fixture::{FixtureItem, FixtureManifest, Rng};
use crate::workload::Workload;

/// Who sends a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    /// The editor's WebSocket connection.
    Editor,
    /// The agent on `/mcp`.
    Agent,
}

/// One command with its idempotency key.
#[derive(Clone, Debug, PartialEq)]
pub struct Op {
    /// Who sends it.
    pub who: Who,
    /// The registry name (`clip.split`).
    pub command: &'static str,
    /// Its params.
    pub params: Value,
    /// The request id it is sent, and after a crash sent again, with.
    pub request_id: String,
}

/// A drag: a gesture transaction, some live updates and (unless the engine dies first) the
/// commit.
#[derive(Clone, Debug, PartialEq)]
pub struct Drag {
    /// `edit.begin_tx`.
    pub begin: Op,
    /// The updates, each replacing the one before (`_meta.replace`).
    pub updates: Vec<Op>,
    /// `edit.commit_tx`.
    pub commit: Op,
}

/// What the editor does next.
#[derive(Clone, Debug, PartialEq)]
pub enum Move {
    /// A command.
    Plain(Op),
    /// A command on a main-track clip that needs the clip's place first (`item.get`).
    Placed(Placed, String),
    /// A drag.
    Drag(Drag),
}

/// The commands that need a clip's current start and length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placed {
    /// `clip.split` in the middle.
    Split,
    /// `clip.ripple_trim` of the end, two frames shorter.
    RippleTrim,
    /// `clip.trim` of the end, one frame shorter.
    Trim,
}

/// The seeded source of commands.
#[derive(Clone, Debug)]
pub struct Script {
    seed: u64,
    rng: Rng,
    light: Workload,
    sequence: String,
    frame: i64,
    /// Clips of the main track (splits add their right halves).
    main: Vec<String>,
    /// Video clips off the main track.
    free: Vec<String>,
    /// Clips off the main track with room to move in.
    movable: Vec<FixtureItem>,
    /// Audio clips.
    audio: Vec<String>,
    /// Video sources.
    media: Vec<String>,
    /// Sequences the editor made: `(sequence, video track)`.
    made: Vec<(String, String)>,
    /// Sequences the agent made.
    agent_made: Vec<(String, String)>,
    /// Request ids issued.
    ids: u64,
    /// Things named so far (sequences, checkpoints, import files).
    names: u64,
}

impl Script {
    /// The script for the fixture `manifest` describes.
    #[must_use]
    pub fn new(manifest: &FixtureManifest, seed: u64) -> Script {
        let (mut main, mut free, mut audio) = (Vec::new(), Vec::new(), Vec::new());
        let mut movable = Vec::new();
        for track in &manifest.tracks {
            if !track.main {
                let roomy = track.items.iter().filter(|i| i.slot_len > i.duration);
                movable.extend(roomy.cloned());
            }
            let ids = track.items.iter().map(|i| i.id.clone());
            match (track.video, track.main) {
                (true, true) => main.extend(ids),
                (true, false) => free.extend(ids),
                (false, _) => audio.extend(ids),
            }
        }
        Script {
            seed,
            rng: Rng::new(seed ^ 0x2105),
            light: Workload::new(manifest, seed),
            sequence: manifest.sequence.clone(),
            frame: manifest.frame,
            main,
            free,
            movable,
            audio,
            media: manifest
                .media
                .iter()
                .filter(|m| m.video)
                .map(|m| m.id.clone())
                .collect(),
            made: Vec::new(),
            agent_made: Vec::new(),
            ids: 0,
            names: 0,
        }
    }

    /// A value in `0..n`.
    pub fn below(&mut self, n: i64) -> i64 {
        self.rng.below(n)
    }

    /// Flicks per frame.
    #[must_use]
    pub fn frame(&self) -> i64 {
        self.frame
    }

    /// The next request id: UUIDv7-shaped, unique in the run, the same for the same seed.
    pub fn request_id(&mut self) -> String {
        self.ids += 1;
        format!(
            "0190f5c2-{:04x}-7000-8000-{:012x}",
            self.seed & 0xFFFF,
            self.ids
        )
    }

    fn name(&mut self, what: &str) -> String {
        self.names += 1;
        format!("{what} {}", self.names)
    }

    fn op(&mut self, who: Who, command: &'static str, params: Value) -> Op {
        Op {
            who,
            command,
            params,
            request_id: self.request_id(),
        }
    }

    fn pick(&mut self, from: &[String]) -> Option<String> {
        let n = i64::try_from(from.len()).unwrap_or(0);
        let i = usize::try_from(self.rng.below(n)).unwrap_or(0);
        from.get(i).cloned()
    }

    /// One of the light commands of the benchmark workload (property, keyframe, move, track
    /// mute, marker).
    pub fn light(&mut self) -> Op {
        let step = self.light.next_step();
        self.op(Who::Editor, step.command, step.params)
    }

    /// A command that always commits a change: a marker.
    pub fn marker(&mut self) -> Op {
        let t = self.rng.below(3000) * self.frame;
        let params = json!({"sequence": self.sequence, "t": t});
        self.op(Who::Editor, "timeline.add_marker", params)
    }

    /// `media.import` of `path`.
    pub fn import(&mut self, path: &std::path::Path) -> Op {
        self.op(Who::Editor, "media.import", json!({"paths": [path]}))
    }

    /// The file name of the next export.
    pub fn export_name(&mut self) -> String {
        self.names += 1;
        format!("export-{}.mp4", self.names)
    }

    /// The file name of the next import.
    pub fn import_name(&mut self) -> String {
        self.names += 1;
        format!("import-{}.mp4", self.names)
    }

    fn speed(&mut self) -> Option<Op> {
        let item = self.pick(&self.main.clone())?;
        let speed = ["2x", "1/2", "1x", "3/2"][usize::try_from(self.rng.below(4)).unwrap_or(0)];
        Some(self.op(
            Who::Editor,
            "clip.set_speed",
            json!({"item": item, "speed": speed}),
        ))
    }

    fn audio(&mut self) -> Option<Op> {
        let item = self.pick(&self.audio.clone())?;
        let set = if self.rng.below(2) == 0 {
            json!({"kind": "volume", "db": -self.rng.below(18)})
        } else {
            json!({"kind": "fade", "in": self.rng.below(8) * self.frame,
                   "out": self.rng.below(8) * self.frame, "curve": "equal_power"})
        };
        Some(self.op(Who::Editor, "audio.set", json!({"item": item, "set": set})))
    }

    fn sequence_op(&mut self) -> Op {
        let made = self.made.clone();
        let n = i64::try_from(made.len()).unwrap_or(0);
        let chosen = usize::try_from(self.rng.below(n))
            .ok()
            .and_then(|i| made.get(i));
        match (self.rng.below(5), chosen) {
            (1, Some((seq, _))) => {
                let name = self.name("Renamed");
                self.op(
                    Who::Editor,
                    "sequence.rename",
                    json!({"sequence": seq, "name": name}),
                )
            }
            (2, Some((seq, _))) => self.op(
                Who::Editor,
                "track.add",
                json!({"sequence": seq, "kind": "video"}),
            ),
            (3 | 4, Some((seq, track))) => match self.pick(&self.media.clone()) {
                Some(media) => self.op(
                    Who::Editor,
                    "timeline.insert",
                    json!({"sequence": seq, "track": track, "media": media, "at": 0,
                           "duration": 20 * self.frame}),
                ),
                None => self.marker(),
            },
            _ => {
                let name = self.name("Scene");
                self.op(Who::Editor, "sequence.create", json!({"name": name}))
            }
        }
    }

    fn placed(&mut self) -> Option<Move> {
        let (kind, pool) = match self.rng.below(3) {
            0 => (Placed::Split, &self.main),
            1 => (Placed::RippleTrim, &self.main),
            _ => (Placed::Trim, &self.free),
        };
        let item = self.pick(&pool.clone())?;
        Some(Move::Placed(kind, item))
    }

    /// The command of a [`Move::Placed`] for a clip at `start` lasting `duration` (flicks);
    /// `None`: the clip is too short for it.
    pub fn place(&mut self, kind: Placed, item: &str, start: i64, duration: i64) -> Option<Op> {
        let frames = duration / self.frame;
        if frames < 6 {
            return None;
        }
        Some(match kind {
            Placed::Split => {
                let at = start + (frames / 2) * self.frame;
                self.op(Who::Editor, "clip.split", json!({"item": item, "at": at}))
            }
            Placed::RippleTrim => self.op(
                Who::Editor,
                "clip.ripple_trim",
                json!({"item": item, "edge": "end", "delta": -2 * self.frame}),
            ),
            Placed::Trim => self.op(
                Who::Editor,
                "clip.trim",
                json!({"item": item, "edge": "end", "delta": -self.frame}),
            ),
        })
    }

    /// A drag of a clip off the main track: two to four live positions, then the commit.
    pub fn drag(&mut self) -> Drag {
        let begin = self.op(
            Who::Editor,
            "edit.begin_tx",
            json!({"label": "Move clip", "kind": "gesture"}),
        );
        // One clip for the whole drag, moved about inside the stretch of track it has to
        // itself.
        let n = i64::try_from(self.movable.len()).unwrap_or(0);
        let clip = usize::try_from(self.rng.below(n))
            .ok()
            .and_then(|i| self.movable.get(i))
            .cloned();
        let mut updates: Vec<Op> = Vec::new();
        if let Some(clip) = clip {
            let room = clip.slot_len - clip.duration;
            for _ in 0..2 + self.rng.below(3) {
                let start = (clip.slot_start + self.rng.below(room + 1)) * self.frame;
                let op = self.op(
                    Who::Editor,
                    "item.move",
                    json!({"item": clip.id, "start": start}),
                );
                updates.push(op);
            }
        }
        let commit = self.op(Who::Editor, "edit.commit_tx", json!({}));
        Drag {
            begin,
            updates,
            commit,
        }
    }

    /// What the editor does next: mostly light commands, with the structural ones, sequence
    /// work, checkpoints, undo and redo, and drags mixed in.
    pub fn editor(&mut self) -> Move {
        let roll = self.rng.below(100);
        let op = match roll {
            0..40 => Some(self.light()),
            40..58 => return self.placed().unwrap_or_else(|| Move::Plain(self.light())),
            58..64 => self.speed(),
            64..72 => self.audio(),
            72..82 => Some(self.sequence_op()),
            82..87 => {
                let name = self.name("cp");
                Some(self.op(Who::Editor, "edit.checkpoint", json!({"name": name})))
            }
            87..91 => Some(self.op(Who::Editor, "edit.undo", json!({}))),
            91..93 => Some(self.op(Who::Editor, "edit.redo", json!({}))),
            _ => return Move::Drag(self.drag()),
        };
        Move::Plain(op.unwrap_or_else(|| self.light()))
    }

    /// A command of the agent that makes nothing new (safe next to anything in flight).
    pub fn agent_touch(&mut self) -> Op {
        let item = self.pick(&self.free.clone()).unwrap_or_default();
        let v = f64::from(u32::try_from(1 + self.rng.below(300)).unwrap_or(1)) / 1000.0;
        let t = self.rng.below(4) * self.frame;
        match self.rng.below(3) {
            0 => self.op(
                Who::Agent,
                "keyframe.set",
                json!({"item": item, "prop": "crop.bottom",
                       "keys": [{"t": t, "value": v}]}),
            ),
            1 => match self.pick(&self.audio.clone()) {
                Some(clip) => self.op(
                    Who::Agent,
                    "audio.set",
                    json!({"item": clip, "set": {"kind": "pan", "pan": v}}),
                ),
                None => self.op(
                    Who::Agent,
                    "item.set_props",
                    json!({"item": item, "props": {"crop": {"right": v}}}),
                ),
            },
            _ => self.op(
                Who::Agent,
                "item.set_props",
                json!({"item": item, "props": {"crop": {"right": v}}}),
            ),
        }
    }

    /// What the agent does next.
    pub fn agent(&mut self) -> Op {
        let made = self.agent_made.first().cloned();
        match (self.rng.below(10), made) {
            (0, _) => {
                let name = self.name("Agent cut");
                self.op(Who::Agent, "sequence.create", json!({"name": name}))
            }
            (1 | 2, Some((seq, track))) => match self.pick(&self.media.clone()) {
                Some(media) => self.op(
                    Who::Agent,
                    "timeline.insert",
                    json!({"sequence": seq, "track": track, "media": media, "at": 0,
                           "duration": 15 * self.frame}),
                ),
                None => self.agent_touch(),
            },
            _ => self.agent_touch(),
        }
    }

    /// Learns from an acknowledged command what it made (a split's right half, a sequence).
    pub fn learn(&mut self, op: &Op, output: &Value) {
        let text = |key: &str| output[key].as_str().map(str::to_owned);
        match op.command {
            "clip.split" => {
                if let Some(right) = text("right") {
                    self.main.push(right);
                }
            }
            "sequence.create" => {
                if let (Some(seq), Some(track)) = (text("sequence"), text("video_track")) {
                    match op.who {
                        Who::Editor => self.made.push((seq, track)),
                        Who::Agent => self.agent_made.push((seq, track)),
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::fixture::{FixtureSpec, generate};

    fn script(seed: u64) -> Script {
        let fixture = generate(&FixtureSpec::smoke()).expect("fixture");
        Script::new(&fixture.manifest, seed)
    }

    #[test]
    fn the_same_seed_gives_the_same_work() {
        let (mut a, mut b) = (script(5), script(5));
        for _ in 0..300 {
            assert_eq!(a.editor(), b.editor());
            assert_eq!(a.agent(), b.agent());
        }
        let mut c = script(6);
        assert!((0..50).any(|_| a.editor() != c.editor()));
    }

    #[test]
    fn the_work_is_mixed_and_request_ids_are_unique() {
        let mut s = script(9);
        let mut commands = std::collections::BTreeSet::new();
        let mut ids = std::collections::BTreeSet::new();
        let (mut drags, mut placed) = (0, 0);
        for n in 0..600 {
            match s.editor() {
                Move::Plain(op) => {
                    if op.command == "sequence.create" {
                        let out = json!({"sequence": format!("seq_{n:032x}"),
                                         "video_track": format!("trk_{n:032x}")});
                        s.learn(&op, &out);
                    }
                    commands.insert(op.command);
                    assert!(ids.insert(op.request_id));
                }
                Move::Placed(kind, item) => {
                    placed += 1;
                    let op = s
                        .place(kind, &item, 0, 30 * s.frame())
                        .expect("long enough");
                    commands.insert(op.command);
                    assert!(s.place(kind, &item, 0, 2 * s.frame()).is_none());
                }
                Move::Drag(drag) => {
                    drags += 1;
                    assert!(drag.updates.len() >= 2);
                    let item = &drag.updates[0].params["item"];
                    assert!(drag.updates.iter().all(|u| &u.params["item"] == item));
                }
            }
            let op = s.agent();
            assert_eq!(op.who, Who::Agent);
            if op.command == "sequence.create" {
                let out = json!({"sequence": format!("seq_{:032x}", n + 1000),
                                 "video_track": format!("trk_{:032x}", n + 1000)});
                s.learn(&op, &out);
            }
            commands.insert(op.command);
            assert!(ids.insert(op.request_id));
        }
        for command in [
            "clip.split",
            "clip.ripple_trim",
            "clip.trim",
            "clip.set_speed",
            "audio.set",
            "keyframe.set",
            "item.move",
            "item.set_props",
            "sequence.create",
            "sequence.rename",
            "track.add",
            "timeline.insert",
            "timeline.add_marker",
            "edit.checkpoint",
            "edit.undo",
            "edit.redo",
        ] {
            assert!(commands.contains(command), "no `{command}` in {commands:?}");
        }
        assert!(drags > 10 && placed > 50, "{drags} drags, {placed} placed");
    }
}
