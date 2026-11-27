// SPDX-License-Identifier: GPL-3.0-or-later
//! The mixed editing commands the round-trip probes send on a fixture: property changes,
//! keyframes, moves, track switches and markers, in a seeded order. Every command changes the
//! document (so each one produces a `doc.patched`) and every one succeeds: a move stays inside
//! the stretch of track the generator left to its item.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::fixture::{FixtureItem, FixtureManifest, Rng};

/// One command of the workload.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    /// The command name.
    pub command: &'static str,
    /// Its params.
    pub params: Value,
}

/// A seeded stream of commands for one fixture.
#[derive(Clone, Debug)]
pub struct Workload {
    sequence: String,
    frame: i64,
    /// Video items anywhere.
    video: Vec<FixtureItem>,
    /// Items off the main track with room to move, and where each one is now.
    movable: Vec<(FixtureItem, i64)>,
    /// Tracks and whether each is muted now.
    tracks: Vec<(String, bool)>,
    length: i64,
    rng: Rng,
    /// The last value written per target, so the next one differs.
    last: BTreeMap<String, i64>,
}

impl Workload {
    /// The workload for the fixture `manifest` describes.
    #[must_use]
    pub fn new(manifest: &FixtureManifest, seed: u64) -> Workload {
        let mut video = Vec::new();
        let mut movable = Vec::new();
        let mut length = 1;
        for track in &manifest.tracks {
            for item in &track.items {
                length = length.max(item.start + item.duration);
                if track.video {
                    video.push(item.clone());
                }
                if !track.main && item.slot_len > item.duration {
                    movable.push((item.clone(), item.start));
                }
            }
        }
        Workload {
            sequence: manifest.sequence.clone(),
            frame: manifest.frame,
            video,
            movable,
            tracks: manifest
                .tracks
                .iter()
                .map(|t| (t.id.clone(), false))
                .collect(),
            length,
            rng: Rng::new(seed),
            last: BTreeMap::new(),
        }
    }

    fn pick(&mut self, len: usize) -> usize {
        usize::try_from(self.rng.below(i64::try_from(len).unwrap_or(1))).unwrap_or(0)
    }

    /// A value in `0..modulus` that is not the last one written to `target`.
    fn fresh(&mut self, target: String, modulus: i64) -> i64 {
        let mut v = self.rng.below(modulus);
        if self.last.get(&target) == Some(&v) {
            v = (v + 1) % modulus;
        }
        self.last.insert(target, v);
        v
    }

    fn set_props(&mut self) -> Step {
        let i = self.pick(self.video.len());
        let id = self.video[i].id.clone();
        let v = self.fresh(format!("crop:{id}"), 400);
        Step {
            command: "item.set_props",
            #[allow(clippy::cast_precision_loss)]
            params: json!({"item": id, "props": {"crop": {"left": v as f64 / 1000.0}}}),
        }
    }

    fn keyframe(&mut self) -> Step {
        let i = self.pick(self.video.len());
        let (id, duration) = (self.video[i].id.clone(), self.video[i].duration);
        let t = self.rng.below(duration);
        let v = self.fresh(format!("key:{id}:{t}"), 400);
        Step {
            command: "keyframe.set",
            #[allow(clippy::cast_precision_loss)]
            params: json!({"item": id, "prop": "crop.top",
                           "keys": [{"t": t * self.frame, "value": v as f64 / 1000.0}]}),
        }
    }

    fn move_item(&mut self) -> Step {
        let i = self.pick(self.movable.len());
        let (item, at) = self.movable[i].clone();
        let room = item.slot_len - item.duration;
        let mut start = item.slot_start + self.rng.below(room + 1);
        if start == at {
            start = item.slot_start + (start - item.slot_start + 1) % (room + 1);
        }
        self.movable[i].1 = start;
        Step {
            command: "item.move",
            params: json!({"item": item.id, "start": start * self.frame}),
        }
    }

    fn mute(&mut self) -> Step {
        let i = self.pick(self.tracks.len());
        self.tracks[i].1 = !self.tracks[i].1;
        Step {
            command: "track.set",
            params: json!({"track": self.tracks[i].0, "muted": self.tracks[i].1}),
        }
    }

    fn marker(&mut self) -> Step {
        let t = self.rng.below(self.length);
        Step {
            command: "timeline.add_marker",
            params: json!({"sequence": self.sequence, "t": t * self.frame}),
        }
    }

    /// The next command: 35% property changes, 20% keyframes, 25% moves, 10% track mutes,
    /// 10% markers.
    pub fn next_step(&mut self) -> Step {
        match self.rng.below(100) {
            0..35 => self.set_props(),
            35..55 => self.keyframe(),
            55..80 if !self.movable.is_empty() => self.move_item(),
            80..90 => self.mute(),
            90..100 => self.marker(),
            _ => self.set_props(),
        }
    }
}
