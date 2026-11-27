// SPDX-License-Identifier: GPL-3.0-or-later
//! The seeded benchmark projects (WP-2J.01): **P-400** (1 hour, 400 items, 8 tracks), **P-5K**
//! (5,000 items, 50,000 keyframes) and **P-short** (9:16, 3 layers with animation).
//!
//! A fixture is built by running the registered commands (`project.new`, `track.add`,
//! `media.register`, `timeline.insert`, `timeline.overwrite`, `keyframe.set`) in transactions,
//! with IDs from [`SeededGen`] and every choice from a seeded generator: the same
//! [`FixtureSpec`] always gives the same document, byte for byte ([`Fixture::sha256`]), and
//! the document stays valid as the model changes because nothing here writes its JSON.
//!
//! Media are six small sources (four test-pattern videos, two tones) the items cut from. The
//! document names them under [`CANONICAL_MEDIA_ROOT`]; [`Fixture::write`] points them at the
//! project's own `media/` folder (with `media.set_location`) before saving, and
//! [`crate::media::synthesize`] makes the files when a benchmark needs to decode them.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use cutweave_commands::{Access, Registry, Transaction};
use cutweave_engine::project_io::{ProjectStore, document_sha256};
use cutweave_model::{Project, SeededGen, to_document_string};
use cutweave_ops::media::FakeProbe;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::BenchError;

/// Flicks per second (arch §4.1).
pub const FLICKS_PER_SECOND: i64 = 705_600_000;
/// The folder the canonical document says its media are in: the same on every machine, so
/// the fixture hash is too.
pub const CANONICAL_MEDIA_ROOT: &str = "/cutweave-bench/media";
/// Length of every source file, in seconds.
pub const SOURCE_SECS: i64 = 120;
/// The marker [`Fixture::write`] leaves in a fixture folder; only a folder with it is replaced.
pub const MARKER_FILE: &str = "bench-fixture.json";

const VIDEO_SOURCES: usize = 4;
const AUDIO_SOURCES: usize = 2;
const EASINGS: [&str; 4] = ["linear", "ease_in_out", "ease_out", "hold"];
const VIDEO_PROPS: [&str; 4] = [
    "opacity",
    "transform.position",
    "transform.scale",
    "transform.rotation",
];

/// What to generate. Two equal specs give the same document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureSpec {
    /// Project name and folder name (`P-400`).
    pub name: String,
    /// Seeds the IDs and every choice.
    pub seed: u64,
    /// Items on the timeline.
    pub items: u32,
    /// Tracks (video and audio together); at least one video track.
    pub tracks: u32,
    /// How many of `tracks` are audio; `None`: half, rounded down.
    pub audio_tracks: Option<u32>,
    /// Keyframes, spread over the items.
    pub keyframes: u32,
    /// Timeline length in seconds (the main track comes out close to it).
    pub duration_secs: u32,
    /// Canvas width.
    pub width: u32,
    /// Canvas height.
    pub height: u32,
    /// Frames per second (a whole number that divides a second of flicks).
    pub fps: u32,
    /// Items on the magnetic main track (back to back); `None`: an equal share.
    pub main_items: Option<u32>,
}

impl FixtureSpec {
    /// A 1080p30 spec with an equal share of items per track.
    #[must_use]
    pub fn new(name: &str, seed: u64, items: u32, tracks: u32, keyframes: u32) -> FixtureSpec {
        FixtureSpec {
            name: name.to_owned(),
            seed,
            items,
            tracks,
            audio_tracks: None,
            keyframes,
            duration_secs: 3600,
            width: 1920,
            height: 1080,
            fps: 30,
            main_items: None,
        }
    }

    /// **P-400**: one hour, 400 items on 8 tracks (100 back to back on the main track), 1080p30.
    #[must_use]
    pub fn p_400() -> FixtureSpec {
        FixtureSpec {
            main_items: Some(100),
            ..FixtureSpec::new("P-400", 400, 400, 8, 800)
        }
    }

    /// **P-5K**: 5,000 items and 50,000 keyframes on 10 tracks over two hours; 2,000 items are
    /// back to back on the main track, so a ripple at its start moves all of them.
    #[must_use]
    pub fn p_5k() -> FixtureSpec {
        FixtureSpec {
            duration_secs: 7200,
            main_items: Some(2000),
            ..FixtureSpec::new("P-5K", 5000, 5000, 10, 50_000)
        }
    }

    /// **P-short**: a 30-second 9:16 short, 3 video layers, every item animated.
    #[must_use]
    pub fn p_short() -> FixtureSpec {
        FixtureSpec {
            audio_tracks: Some(0),
            duration_secs: 30,
            width: 1080,
            height: 1920,
            ..FixtureSpec::new("P-short", 916, 12, 3, 96)
        }
    }

    /// A tiny project for the smoke run of the suite (an ordinary test).
    #[must_use]
    pub fn smoke() -> FixtureSpec {
        FixtureSpec {
            duration_secs: 120,
            main_items: Some(8),
            ..FixtureSpec::new("P-smoke", 1, 32, 4, 64)
        }
    }

    /// The preset called `name` (`p-400`, `p-5k`, `p-short`, `smoke`; case does not matter).
    #[must_use]
    pub fn preset(name: &str) -> Option<FixtureSpec> {
        match name.to_ascii_lowercase().as_str() {
            "p-400" => Some(FixtureSpec::p_400()),
            "p-5k" => Some(FixtureSpec::p_5k()),
            "p-short" => Some(FixtureSpec::p_short()),
            "smoke" | "p-smoke" => Some(FixtureSpec::smoke()),
            _ => None,
        }
    }

    fn audio_track_count(&self) -> u32 {
        self.audio_tracks.unwrap_or(self.tracks / 2)
    }

    fn check(&self) -> Result<(), BenchError> {
        let bad = |why: &str| Err(BenchError::Spec(format!("{}: {why}", self.name)));
        if self.tracks == 0 || self.audio_track_count() >= self.tracks {
            return bad("needs at least one video track");
        }
        if self.items < self.tracks {
            return bad("needs at least one item per track");
        }
        if self.fps == 0 || FLICKS_PER_SECOND % i64::from(self.fps) != 0 {
            return bad("the frame rate must divide a second of flicks");
        }
        if self.duration_secs == 0 || self.width == 0 || self.height == 0 {
            return bad("duration and canvas must not be zero");
        }
        if self.main_items.is_some_and(|m| m == 0 || m > self.items) {
            return bad("main_items must be between 1 and items");
        }
        if self.main_items.is_some_and(|m| m == self.items) && self.tracks > 1 {
            return bad("main_items leaves no items for the other tracks");
        }
        Ok(())
    }
}

/// One generated item, in frames of the sequence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureItem {
    /// The item id (`itm_…`).
    pub id: String,
    /// First frame.
    pub start: i64,
    /// Length in frames.
    pub duration: i64,
    /// First frame of the stretch of track this item has to itself (off the main track an
    /// item can move anywhere inside its stretch without touching a neighbour).
    pub slot_start: i64,
    /// Length of that stretch in frames.
    pub slot_len: i64,
}

/// One generated track.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureTrack {
    /// The track id (`trk_…`).
    pub id: String,
    /// Video (else audio).
    pub video: bool,
    /// The magnetic main track.
    pub main: bool,
    /// Its items in time order.
    pub items: Vec<FixtureItem>,
}

/// One source file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureMedia {
    /// The media id (`med_…`).
    pub id: String,
    /// File name inside the project's `media/` folder.
    pub file: String,
    /// Video (else audio).
    pub video: bool,
}

/// What a fixture holds, without the document: written next to it as [`MARKER_FILE`] so the UI
/// and playback benchmarks (WP-2J.02, WP-2J.03) find ids and sources without parsing the
/// project format.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureManifest {
    /// The spec it was generated from.
    pub spec: FixtureSpec,
    /// SHA-256 of the canonical document (see [`Fixture::sha256`]).
    pub sha256: String,
    /// The sequence id.
    pub sequence: String,
    /// Flicks per frame.
    pub frame: i64,
    /// Seconds of every source file.
    pub source_secs: i64,
    /// The sources.
    pub media: Vec<FixtureMedia>,
    /// The tracks, top video track first as created.
    pub tracks: Vec<FixtureTrack>,
    /// Keyframes written.
    pub keyframes: u32,
}

/// A generated project.
#[derive(Clone, Debug)]
pub struct Fixture {
    /// The canonical document (media under [`CANONICAL_MEDIA_ROOT`]).
    pub project: Project,
    /// Ids and layout.
    pub manifest: FixtureManifest,
}

/// splitmix64: a small seeded generator, the same on every platform.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// A generator for `seed`.
    #[must_use]
    pub const fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    /// The next value.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..n` (`0` when `n` is not positive).
    pub fn below(&mut self, n: i64) -> i64 {
        match u64::try_from(n) {
            Ok(n) if n > 0 => i64::try_from(self.next_u64() % n).unwrap_or(0),
            _ => 0,
        }
    }

    /// A value in `0.0..=1.0` with three decimals (short in JSON, exact in every toolchain).
    pub fn unit(&mut self) -> f64 {
        f64::from(u32::try_from(self.below(1001)).unwrap_or(0)) / 1000.0
    }
}

/// Runs commands one transaction at a time on a document, like a client would.
pub(crate) struct Runner {
    registry: Registry,
    doc: Project,
    ids: SeededGen,
}

impl Runner {
    pub(crate) fn new(doc: Project, seed: u64) -> Result<Runner, BenchError> {
        let mut registry = Registry::new();
        cutweave_ops::register(&mut registry)
            .map_err(|e| BenchError::Other(format!("command registry: {e}")))?;
        Ok(Runner {
            registry,
            doc,
            ids: SeededGen::new(seed),
        })
    }

    pub(crate) fn run(&mut self, name: &str, params: Value) -> Result<Value, BenchError> {
        let mut txn = Transaction::begin(self.doc.clone());
        let out = {
            let mut tx = txn.tx(&mut self.ids, Access::STANDARD);
            self.registry.invoke_json(name, params, &mut tx)
        };
        match out {
            Ok(out) => {
                self.doc = txn.commit().doc;
                Ok(out)
            }
            Err(e) => Err(BenchError::command(name, &e)),
        }
    }

    pub(crate) fn into_doc(self) -> Project {
        self.doc
    }
}

fn text(v: &Value, key: &str, command: &str) -> Result<String, BenchError> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| BenchError::Other(format!("`{command}` returned no `{key}`")))
}

/// How many items each track gets: the main track first.
fn shares(spec: &FixtureSpec) -> Vec<u32> {
    let others = spec.tracks - 1;
    let main = match spec.main_items {
        Some(m) => m,
        None if others == 0 => spec.items,
        None => spec.items / spec.tracks + u32::from(!spec.items.is_multiple_of(spec.tracks)),
    };
    let mut out = vec![main];
    if others > 0 {
        let rest = spec.items - main;
        out.extend((0..others).map(|i| rest / others + u32::from(i < rest % others)));
    }
    out
}

struct Builder<'a> {
    spec: &'a FixtureSpec,
    runner: Runner,
    rng: Rng,
    frame: i64,
    sequence: String,
    media: Vec<FixtureMedia>,
    /// Keys still owed by items that were too short for their share.
    owed: i64,
    keyframes: u32,
}

impl Builder<'_> {
    fn source(&mut self, video: bool) -> String {
        let pool: Vec<&FixtureMedia> = self.media.iter().filter(|m| m.video == video).collect();
        let pick =
            usize::try_from(self.rng.below(i64::try_from(pool.len()).unwrap_or(1))).unwrap_or(0);
        pool[pick].id.clone()
    }

    fn register_media(&mut self, root: &str) -> Result<(), BenchError> {
        for n in 0..VIDEO_SOURCES + AUDIO_SOURCES {
            let video = n < VIDEO_SOURCES;
            let file = if video {
                format!("pattern-{n}.mp4")
            } else {
                format!("tone-{}.wav", n - VIDEO_SOURCES)
            };
            let path = format!("{root}/{file}");
            let probed = if video {
                FakeProbe::video(&path, SOURCE_SECS, false)
            } else {
                FakeProbe::audio(&path, SOURCE_SECS)
            };
            let out = self.runner.run(
                "media.register",
                json!({
                    "path": path,
                    "probe": probed.probe,
                    "content_hash": format!("cutweave-bench-{file}"),
                }),
            )?;
            self.media.push(FixtureMedia {
                id: text(&out, "media_id", "media.register")?,
                file,
                video,
            });
        }
        Ok(())
    }

    /// Places `count` items on `track` and returns them in time order.
    fn place(&mut self, track: &FixtureTrack, count: u32) -> Result<Vec<FixtureItem>, BenchError> {
        let total = i64::from(self.spec.duration_secs) * i64::from(self.spec.fps);
        let source = SOURCE_SECS * i64::from(self.spec.fps);
        let count = i64::from(count);
        let slot = (total / count).max(2);
        let mut cursor = 0;
        let mut items = Vec::new();
        for i in 0..count {
            // The main track is gap-free: lengths vary around the slot. Elsewhere an item
            // fills 40-90% of its slot and sits somewhere inside it.
            let (start, duration) = if track.main {
                let d = (slot / 2 + self.rng.below(slot)).clamp(1, source);
                (cursor, d)
            } else {
                let d = (slot * (40 + self.rng.below(51)) / 100).clamp(1, source.min(slot));
                (i * slot + self.rng.below(slot - d + 1), d)
            };
            cursor = start + duration;
            let media = self.source(track.video);
            let params = json!({
                "sequence": self.sequence,
                "track": track.id,
                "media": media,
                "at": start * self.frame,
                "duration": duration * self.frame,
                "source_in": self.rng.below(source - duration + 1) * self.frame,
                "link": false,
            });
            let command = if track.main {
                "timeline.insert"
            } else {
                "timeline.overwrite"
            };
            let out = self.runner.run(command, params)?;
            let id = out["clips"][0]
                .as_str()
                .ok_or_else(|| BenchError::Other(format!("`{command}` returned no clip")))?
                .to_owned();
            items.push(FixtureItem {
                id,
                start,
                duration,
                slot_start: if track.main { start } else { i * slot },
                slot_len: if track.main { duration } else { slot },
            });
        }
        Ok(items)
    }

    fn key_value(&mut self, prop: &str) -> Value {
        match prop {
            "transform.position" => {
                let (w, h) = (f64::from(self.spec.width), f64::from(self.spec.height));
                json!({"x": ((self.rng.unit() - 0.5) * w / 2.0).round(),
                       "y": ((self.rng.unit() - 0.5) * h / 2.0).round()})
            }
            "transform.scale" => {
                let s = 0.5 + self.rng.unit();
                json!({"x": s, "y": s})
            }
            "transform.rotation" => json!((self.rng.unit() * 90.0 - 45.0).round()),
            "audio_gain" => json!(0.25 + self.rng.unit() * 0.75),
            _ => json!(self.rng.unit()),
        }
    }

    /// Gives `item` its share of keys (`want`, plus what shorter items still owe) on one
    /// property, at most one key per frame.
    fn animate(
        &mut self,
        item: &FixtureItem,
        video: bool,
        want: i64,
        nth: usize,
    ) -> Result<(), BenchError> {
        let count = (want + self.owed).min(item.duration);
        self.owed += want - count;
        if count <= 0 {
            return Ok(());
        }
        let prop = if video {
            VIDEO_PROPS[nth % VIDEO_PROPS.len()]
        } else {
            "audio_gain"
        };
        let mut keys = Vec::new();
        for j in 0..count {
            let easing = EASINGS[usize::try_from(self.rng.below(4)).unwrap_or(0)];
            keys.push(json!({
                "t": (j * item.duration / count) * self.frame,
                "value": self.key_value(prop),
                "easing": easing,
            }));
        }
        self.runner.run(
            "keyframe.set",
            json!({"item": item.id, "prop": prop, "keys": keys}),
        )?;
        self.keyframes += u32::try_from(count).unwrap_or(0);
        Ok(())
    }
}

/// Adds the tracks next to the `V1` and `A1` of `project.new` (its output is `new`): the video
/// tracks first, `V1` being the magnetic main track, then the audio tracks.
fn make_tracks(
    runner: &mut Runner,
    spec: &FixtureSpec,
    sequence: &str,
    new: &Value,
) -> Result<Vec<FixtureTrack>, BenchError> {
    let audio = spec.audio_track_count();
    let mut tracks = vec![FixtureTrack {
        id: text(new, "video_track", "project.new")?,
        video: true,
        main: true,
        items: Vec::new(),
    }];
    for _ in 1..spec.tracks - audio {
        let out = runner.run("track.add", json!({"sequence": sequence, "kind": "video"}))?;
        tracks.push(FixtureTrack {
            id: text(&out, "track", "track.add")?,
            video: true,
            main: false,
            items: Vec::new(),
        });
    }
    for n in 0..audio {
        // `project.new` made the first audio track.
        let id = if n == 0 {
            text(new, "audio_track", "project.new")?
        } else {
            let out = runner.run("track.add", json!({"sequence": sequence, "kind": "audio"}))?;
            text(&out, "track", "track.add")?
        };
        tracks.push(FixtureTrack {
            id,
            video: false,
            main: false,
            items: Vec::new(),
        });
    }
    if audio == 0 {
        // `project.new` always makes `A1`; a picture-only fixture drops it.
        let a1 = text(new, "audio_track", "project.new")?;
        runner.run("track.remove", json!({"track": a1}))?;
    }
    runner.run(
        "track.set_magnetic",
        json!({"track": tracks[0].id, "magnetic": true}),
    )?;
    Ok(tracks)
}

/// Generates the fixture for `spec` with its media under `media_root`.
///
/// [`CANONICAL_MEDIA_ROOT`] gives the canonical document, the one [`Fixture::sha256`] names.
pub fn generate_at(spec: &FixtureSpec, media_root: &str) -> Result<Fixture, BenchError> {
    spec.check()?;
    let frame = FLICKS_PER_SECOND / i64::from(spec.fps);
    let mut runner = Runner::new(Project::new(&spec.name), spec.seed)?;
    let new = runner.run(
        "project.new",
        json!({
            "name": spec.name,
            "format": {"width": spec.width, "height": spec.height,
                       "frame_rate": {"num": spec.fps, "den": 1}},
        }),
    )?;
    let sequence = text(&new, "sequence", "project.new")?;
    let tracks = make_tracks(&mut runner, spec, &sequence, &new)?;
    let mut b = Builder {
        spec,
        runner,
        rng: Rng::new(spec.seed),
        frame,
        sequence,
        media: Vec::new(),
        owed: 0,
        keyframes: 0,
    };
    b.register_media(media_root)?;
    let mut tracks = tracks;
    for (track, count) in tracks.iter_mut().zip(shares(spec)) {
        track.items = b.place(track, count)?;
    }
    let (want, extra) = (
        i64::from(spec.keyframes / spec.items),
        usize::try_from(spec.keyframes % spec.items).unwrap_or(0),
    );
    let mut nth = 0;
    for track in &tracks {
        for item in &track.items {
            b.animate(item, track.video, want + i64::from(nth < extra), nth)?;
            nth += 1;
        }
    }
    if b.owed > 0 {
        return Err(BenchError::Spec(format!(
            "{}: {} keyframes do not fit (items are too short)",
            spec.name, b.owed
        )));
    }
    let (media, keyframes) = (b.media, b.keyframes);
    let project = b.runner.into_doc();
    project
        .validate()
        .map_err(|e| BenchError::Other(format!("{}: invalid document: {e}", spec.name)))?;
    let mut fixture = Fixture {
        project,
        manifest: FixtureManifest {
            spec: spec.clone(),
            sha256: String::new(),
            sequence: b.sequence,
            frame,
            source_secs: SOURCE_SECS,
            media,
            tracks,
            keyframes,
        },
    };
    fixture.manifest.sha256 = document_sha256(fixture.document()?.as_bytes());
    Ok(fixture)
}

/// Generates the canonical fixture for `spec`.
pub fn generate(spec: &FixtureSpec) -> Result<Fixture, BenchError> {
    generate_at(spec, CANONICAL_MEDIA_ROOT)
}

impl Fixture {
    /// The canonical document as the project format writes it.
    pub fn document(&self) -> Result<String, BenchError> {
        to_document_string(&self.project)
            .map_err(|e| BenchError::Other(format!("serialize the document: {e}")))
    }

    /// SHA-256 of [`document`](Self::document): the identity of the fixture, the same on every
    /// machine for the same spec and the same version of the commands.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.manifest.sha256
    }

    /// Item count.
    #[must_use]
    pub fn item_count(&self) -> usize {
        self.manifest.tracks.iter().map(|t| t.items.len()).sum()
    }

    /// Writes the project folder `<parent>/<name>` (replacing a fixture already there, but
    /// keeping its source files) with its media pointed at `<folder>/media/`, and returns the
    /// folder. The media files
    /// themselves are made by [`crate::media::synthesize`].
    pub fn write(&self, parent: &Path) -> Result<PathBuf, BenchError> {
        let folder = parent.join(&self.manifest.spec.name);
        if folder.join(MARKER_FILE).is_file() {
            // Everything but `media/`: the source files take a while to make.
            let entries = fs::read_dir(&folder)
                .map_err(|e| BenchError::io("replace the fixture", &folder, e))?;
            for entry in entries.flatten() {
                let path = entry.path();
                let removed = match entry.file_type() {
                    Ok(_) if entry.file_name() == "media" => Ok(()),
                    Ok(kind) if kind.is_dir() => fs::remove_dir_all(&path),
                    _ => fs::remove_file(&path),
                };
                removed.map_err(|e| BenchError::io("replace the fixture", &path, e))?;
            }
        }
        let media_dir = folder.join("media");
        fs::create_dir_all(&media_dir)
            .map_err(|e| BenchError::io("create the fixture", &media_dir, e))?;
        let mut runner = Runner::new(self.project.clone(), self.manifest.spec.seed ^ 0xB3)?;
        for m in &self.manifest.media {
            let path = media_dir.join(&m.file).to_string_lossy().into_owned();
            runner.run("media.set_location", json!({"media": m.id, "path": path}))?;
        }
        ProjectStore::create(&folder, &runner.into_doc())
            .and_then(ProjectStore::close)
            .map_err(|e| BenchError::Other(format!("save {}: {e}", folder.display())))?;
        let mut manifest = serde_json::to_string_pretty(&self.manifest)
            .map_err(|e| BenchError::Other(format!("serialize the manifest: {e}")))?;
        let _ = writeln!(manifest);
        let marker = folder.join(MARKER_FILE);
        fs::write(&marker, manifest).map_err(|e| BenchError::io("write", &marker, e))?;
        Ok(folder)
    }
}

/// Reads the manifest of a fixture folder written by [`Fixture::write`].
pub fn read_manifest(folder: &Path) -> Result<FixtureManifest, BenchError> {
    let path = folder.join(MARKER_FILE);
    let text = fs::read_to_string(&path).map_err(|e| BenchError::io("read", &path, e))?;
    serde_json::from_str(&text).map_err(|e| BenchError::Other(format!("{}: {e}", path.display())))
}
