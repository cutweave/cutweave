// SPDX-License-Identifier: GPL-3.0-or-later
//! The ten tasks of the evaluation and their checkers (WP-2I.09).
//!
//! A task is what a user would ask for, in a user's words: clips are named by where they are
//! ("the second clip on V1"), never by id, so the model has to read the timeline first. A
//! checker reads the document the session ended with ([`FinalState`]: `project.snapshot` and
//! the checkpoint list) and says pass or fail with a reason. No model judges anything, and a
//! checker does not care how the model got there: which tools, how many calls, in what order.

use serde_json::Value;

use super::surface::{FinalState, IMPORT_FILE};
use crate::BenchError;
use crate::fixture::{FLICKS_PER_SECOND, Fixture};

/// What a task and its checker know about the project before the session.
#[derive(Clone, Debug)]
pub struct Scene {
    /// The sequence id.
    pub sequence: String,
    /// The magnetic main track V1.
    pub v1: Lane,
    /// The second video track V2.
    pub v2: Lane,
    /// The first audio track A1.
    pub a1: Lane,
    /// The second audio track A2.
    pub a2: Lane,
    /// The document before the session.
    pub base: Value,
    /// The id of the source `pattern-1.mp4`.
    pub pattern_1: String,
    /// The folder of the file to import, written with `/`.
    pub imports: String,
}

/// A track and its clips in time order.
#[derive(Clone, Debug)]
pub struct Lane {
    /// The track id.
    pub id: String,
    /// The ids of its clips, first to last.
    pub items: Vec<String>,
}

fn secs(seconds: f64) -> i64 {
    // Every time a task names is a whole number of milliseconds.
    #[allow(clippy::cast_possible_truncation)]
    let millis = (seconds * 1000.0).round() as i64;
    millis * FLICKS_PER_SECOND / 1000
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

/// The clips of a track of the document, in time order.
fn clips(track: &Value) -> Vec<&Value> {
    let mut items: Vec<&Value> = track["items"]
        .as_object()
        .into_iter()
        .flat_map(|items| items.values())
        .collect();
    items.sort_by_key(|item| item["start"].as_i64().unwrap_or(i64::MAX));
    items
}

impl Scene {
    /// The scene of `fixture`, with the import folder at `imports`.
    pub fn new(fixture: &Fixture, imports: &str) -> Result<Scene, BenchError> {
        let bad = |what: &str| BenchError::Other(format!("the evaluation project has no {what}"));
        let base: Value = serde_json::from_str(&fixture.document()?)
            .map_err(|e| BenchError::Other(format!("the evaluation project: {e}")))?;
        let sequence = fixture.manifest.sequence.clone();
        let lane = |name: &str| {
            let tracks = base["sequences"][&sequence]["tracks"].as_object();
            let track = tracks
                .into_iter()
                .flat_map(|tracks| tracks.values())
                .find(|track| track["name"] == name)
                .ok_or_else(|| bad(&format!("track {name}")))?;
            Ok::<Lane, BenchError>(Lane {
                id: text(&track["id"]).to_owned(),
                items: clips(track)
                    .iter()
                    .map(|item| text(&item["id"]).to_owned())
                    .collect(),
            })
        };
        let pattern_1 = fixture
            .manifest
            .media
            .iter()
            .find(|media| media.file == "pattern-1.mp4")
            .ok_or_else(|| bad("pattern-1.mp4"))?;
        let scene = Scene {
            v1: lane("V1")?,
            v2: lane("V2")?,
            a1: lane("A1")?,
            a2: lane("A2")?,
            sequence,
            pattern_1: pattern_1.id.clone(),
            imports: imports.to_owned(),
            base,
        };
        let lanes = [&scene.v1, &scene.v2, &scene.a1, &scene.a2];
        if lanes.iter().any(|lane| lane.items.len() != 6) {
            return Err(bad("six clips on every track"));
        }
        Ok(scene)
    }

    fn tracks<'a>(&self, doc: &'a Value) -> &'a Value {
        &doc["sequences"][&self.sequence]["tracks"]
    }

    /// The clip `id` of `doc`, on whatever track it is.
    fn item<'a>(&self, doc: &'a Value, id: &str) -> Option<&'a Value> {
        self.tracks(doc)
            .as_object()?
            .values()
            .find_map(|track| track["items"].get(id))
    }

    /// The clips of track `lane` of `doc`, in time order.
    fn lane_clips<'a>(&self, doc: &'a Value, lane: &Lane) -> Vec<&'a Value> {
        clips(&self.tracks(doc)[&lane.id])
    }

    /// The path of the file to import.
    #[must_use]
    pub fn import_path(&self) -> String {
        format!("{}/{IMPORT_FILE}", self.imports)
    }
}

/// One task.
pub struct Task {
    /// Its name on the command line and in the report.
    pub id: &'static str,
    /// What it is about, for the report.
    pub title: &'static str,
    /// The listed tools that do the task's edits. The `via-execute` variant hides them, so
    /// the model has to run the same commands through `cutweave_execute`. Empty: the task
    /// needs a catalog command anyway.
    pub core_tools: &'static [&'static str],
    /// The task makes at least two changes that one batch can carry, so the `batch` and
    /// `no-batch` variants apply.
    pub batchable: bool,
    /// What the user asks for.
    pub prompt: fn(&Scene) -> String,
    /// Whether the final document is what was asked for; the error says what is wrong.
    pub check: fn(&Scene, &FinalState) -> Result<(), String>,
}

fn expect(holds: bool, why: impl FnOnce() -> String) -> Result<(), String> {
    if holds { Ok(()) } else { Err(why()) }
}

fn ids(items: &[&Value]) -> Vec<String> {
    items
        .iter()
        .map(|item| text(&item["id"]).to_owned())
        .collect()
}

fn check_split_and_trim(scene: &Scene, state: &FinalState) -> Result<(), String> {
    let target = &scene.v1.items[1];
    let before = scene.item(&scene.base, target).ok_or("no base clip")?;
    let (start, length) = (
        before["start"].as_i64().unwrap_or(0),
        before["duration"].as_i64().unwrap_or(0),
    );
    let now = scene.lane_clips(&state.document, &scene.v1);
    expect(now.len() == 7, || {
        format!("V1 has {} clips, not 7", now.len())
    })?;
    let left = now[1];
    expect(left["id"] == target.as_str(), || {
        "the second clip of V1 is not the clip that was split".to_owned()
    })?;
    expect(left["duration"] == secs(10.0), || {
        format!(
            "the left part is {} flicks long, not 10 s",
            left["duration"]
        )
    })?;
    let right = now[2];
    expect(
        !scene.v1.items.contains(&text(&right["id"]).to_owned()),
        || "no new clip follows the split clip".to_owned(),
    )?;
    expect(right["media"] == before["media"], || {
        "the right part plays another source".to_owned()
    })?;
    expect(right["start"] == start + secs(10.0), || {
        "the right part does not start at the split".to_owned()
    })?;
    expect(right["duration"] == length - secs(15.0), || {
        format!(
            "the right part is {} flicks long, not {} (5 s shorter)",
            right["duration"],
            length - secs(15.0)
        )
    })
}

fn check_insert_and_reorder(scene: &Scene, state: &FinalState) -> Result<(), String> {
    let now = scene.lane_clips(&state.document, &scene.v1);
    expect(now.len() == 7, || {
        format!("V1 has {} clips, not 7", now.len())
    })?;
    let first = now[0];
    expect(
        first["media"] == scene.pattern_1.as_str()
            && !scene.v1.items.contains(&text(&first["id"]).to_owned()),
        || "the first clip of V1 is not a new clip of pattern-1.mp4".to_owned(),
    )?;
    expect(first["start"] == 0, || "V1 does not start at 0".to_owned())?;
    let mut wanted = vec![scene.v1.items[5].clone()];
    wanted.extend_from_slice(&scene.v1.items[..5]);
    let rest = ids(&now[1..]);
    expect(rest == wanted, || {
        "after the new clip V1 is not: the former last clip, then the others in their order"
            .to_owned()
    })
}

fn check_set_speed(scene: &Scene, state: &FinalState) -> Result<(), String> {
    let item = scene
        .item(&state.document, &scene.v2.items[0])
        .ok_or("the first clip of V2 is gone")?;
    let speed = &item["speed"];
    let (num, den) = (speed["num"].as_i64(), speed["den"].as_i64());
    expect(
        matches!((num, den), (Some(n), Some(d)) if d > 0 && n == 2 * d),
        || format!("its speed is {speed}, not 2x"),
    )
}

fn check_add_markers(scene: &Scene, state: &FinalState) -> Result<(), String> {
    let markers = state.document["sequences"][&scene.sequence]["markers"].as_object();
    let markers: Vec<&Value> = markers.into_iter().flat_map(|m| m.values()).collect();
    expect(markers.len() == 2, || {
        format!("the sequence has {} markers, not 2", markers.len())
    })?;
    for (label, at) in [("Review", 12.0), ("Outro", 210.0)] {
        expect(
            markers
                .iter()
                .any(|m| m["label"] == label && m["t"] == secs(at)),
            || format!("no marker \"{label}\" at {at} s"),
        )?;
    }
    Ok(())
}

fn check_fade_audio(scene: &Scene, state: &FinalState) -> Result<(), String> {
    for (n, id) in scene.a1.items[..2].iter().enumerate() {
        let item = scene
            .item(&state.document, id)
            .ok_or_else(|| format!("clip {} of A1 is gone", n + 1))?;
        let audio = &item["audio"];
        expect(audio["fade_in"]["duration"] == secs(0.5), || {
            format!("clip {} of A1 has no 0.5 s fade in", n + 1)
        })?;
        expect(audio["fade_out"]["duration"] == secs(2.0), || {
            format!("clip {} of A1 has no 2 s fade out", n + 1)
        })?;
    }
    Ok(())
}

fn check_keyframe_move(scene: &Scene, state: &FinalState) -> Result<(), String> {
    let item = scene
        .item(&state.document, &scene.v2.items[1])
        .ok_or("the second clip of V2 is gone")?;
    let keys = item["visual"]["transform"]["position"]["keyframes"].as_array();
    let keys = keys.ok_or("its position is not animated")?;
    expect(keys.len() == 2, || {
        format!("its position has {} keys, not 2", keys.len())
    })?;
    for (key, (t, x)) in keys.iter().zip([(0.0, -0.5), (2.0, 0.0)]) {
        let near =
            |value: &Value, want: f64| value.as_f64().is_some_and(|v| (v - want).abs() < 1e-6);
        expect(
            key["t"] == secs(t) && near(&key["value"]["x"], x) && near(&key["value"]["y"], 0.0),
            || format!("no key x = {x}, y = 0 at {t} s of the clip (found {key})"),
        )?;
    }
    Ok(())
}

fn check_vertical(scene: &Scene, state: &FinalState) -> Result<(), String> {
    let format = &state.document["sequences"][&scene.sequence]["format"];
    expect(format["width"] == 1080 && format["height"] == 1920, || {
        format!("the sequence is {} x {}", format["width"], format["height"])
    })
}

fn check_undo_and_checkpoint(scene: &Scene, state: &FinalState) -> Result<(), String> {
    expect(
        state
            .checkpoints
            .iter()
            .any(|name| name == "before-cleanup"),
        || {
            format!(
                "no checkpoint \"before-cleanup\" (found {:?})",
                state.checkpoints
            )
        },
    )?;
    let now = ids(&scene.lane_clips(&state.document, &scene.a2));
    let mut wanted = scene.a2.items[..4].to_vec();
    wanted.push(scene.a2.items[5].clone());
    expect(now == wanted, || {
        "A2 is not its first four clips and its last one (the fifth deleted)".to_owned()
    })?;
    let last = scene.item(&state.document, &scene.a2.items[5]);
    let before = scene.item(&scene.base, &scene.a2.items[5]);
    expect(
        last.map(|item| &item["start"]) == before.map(|item| &item["start"]),
        || "the last clip of A2 is not where it was".to_owned(),
    )
}

fn check_import_and_place(scene: &Scene, state: &FinalState) -> Result<(), String> {
    let wanted = format!("/{IMPORT_FILE}");
    let media = state.document["media"].as_object();
    let imported = media
        .into_iter()
        .flat_map(|media| media.values())
        .find(|asset| {
            text(&asset["path_abs"])
                .replace('\\', "/")
                .ends_with(&wanted)
        })
        .ok_or_else(|| format!("{IMPORT_FILE} is not in the project's media"))?;
    let now = scene.lane_clips(&state.document, &scene.v1);
    expect(now.len() == 7, || {
        format!("V1 has {} clips, not 7", now.len())
    })?;
    expect(ids(&now[..6]) == scene.v1.items, || {
        "the first six clips of V1 are not the ones that were there".to_owned()
    })?;
    let (last, previous) = (now[6], now[5]);
    expect(last["media"] == imported["id"], || {
        format!("the last clip of V1 does not play {IMPORT_FILE}")
    })?;
    let end = previous["start"].as_i64().unwrap_or(0) + previous["duration"].as_i64().unwrap_or(0);
    expect(last["start"] == end, || {
        "the new clip does not start where the former last clip ends".to_owned()
    })
}

fn check_long_tail(scene: &Scene, state: &FinalState) -> Result<(), String> {
    let track = &scene.tracks(&state.document)[&scene.a2.id];
    expect(track["muted"] == true, || "A2 is not muted".to_owned())?;
    expect(track["name"] == "Music", || {
        format!("the track is called {}, not \"Music\"", track["name"])
    })
}

/// The ten tasks, in the order of the work package.
pub static TASKS: [Task; 10] = [
    Task {
        id: "split-and-trim",
        title: "Split a clip and trim the new part",
        core_tools: &["clip_split", "clip_trim"],
        batchable: true,
        prompt: |_| {
            "On the main video track V1, split the second clip 10 seconds after its start. \
             Then shorten the new right-hand part by 5 seconds at its end."
                .to_owned()
        },
        check: check_split_and_trim,
    },
    Task {
        id: "insert-and-reorder",
        title: "Insert a source and reorder the main track",
        core_tools: &["timeline_insert", "item_move"],
        batchable: true,
        prompt: |_| {
            "Insert the video pattern-1.mp4 at the very start of the main video track V1, so \
             that it plays first. Then move the clip that is last on V1 so that it comes right \
             after the new clip, as the second clip of the track."
                .to_owned()
        },
        check: check_insert_and_reorder,
    },
    Task {
        id: "set-speed",
        title: "Set a clip's speed",
        core_tools: &["clip_set_speed"],
        batchable: false,
        prompt: |_| "Make the first clip on video track V2 play at double speed.".to_owned(),
        check: check_set_speed,
    },
    Task {
        id: "add-markers",
        title: "Add markers",
        core_tools: &["timeline_add_marker"],
        batchable: true,
        prompt: |_| {
            "Add two markers to the timeline: one labelled \"Review\" at 12 seconds and one \
             labelled \"Outro\" at 3 minutes 30 seconds."
                .to_owned()
        },
        check: check_add_markers,
    },
    Task {
        id: "fade-audio",
        title: "Fade audio in and out",
        core_tools: &["audio_set"],
        batchable: true,
        prompt: |_| {
            "Give each of the first two clips on audio track A1 a half-second fade in and a \
             2 second fade out."
                .to_owned()
        },
        check: check_fade_audio,
    },
    Task {
        id: "keyframe-move",
        title: "Keyframe a move",
        core_tools: &["keyframe_set"],
        batchable: false,
        prompt: |_| {
            "Animate the second clip on video track V2 so that it slides in from the left: its \
             position x is -0.5 at the start of the clip and 0 two seconds later, and y stays 0."
                .to_owned()
        },
        check: check_keyframe_move,
    },
    Task {
        id: "vertical-format",
        title: "Set a 9:16 format",
        core_tools: &["sequence_set_format"],
        batchable: false,
        prompt: |_| {
            "Change the sequence to a vertical 9:16 format, 1080 by 1920 pixels.".to_owned()
        },
        check: check_vertical,
    },
    Task {
        id: "undo-and-checkpoint",
        title: "Checkpoint, delete, undo",
        core_tools: &["edit_checkpoint", "item_delete", "edit_undo"],
        batchable: false,
        prompt: |_| {
            "Save a restore point named \"before-cleanup\". Then delete the second-to-last clip \
             on audio track A2. Then delete the last clip on A2. I changed my mind about that \
             last one: undo only the last deletion."
                .to_owned()
        },
        check: check_undo_and_checkpoint,
    },
    Task {
        id: "import-and-place",
        title: "Import a file and place it",
        core_tools: &["media_import", "timeline_insert"],
        batchable: false,
        prompt: |scene| {
            format!(
                "Import the video file {} into the project and add it to the end of the main \
                 video track V1, right after the last clip.",
                scene.import_path()
            )
        },
        check: check_import_and_place,
    },
    Task {
        id: "long-tail",
        title: "A command only the catalog has (track settings)",
        core_tools: &[],
        batchable: false,
        prompt: |_| "Mute audio track A2 and rename it to \"Music\".".to_owned(),
        check: check_long_tail,
    },
];

/// The task called `id`.
#[must_use]
pub fn task(id: &str) -> Option<&'static Task> {
    TASKS.iter().find(|task| task.id == id)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::agent_eval::surface::eval_spec;
    use crate::fixture::generate;

    fn scene() -> Scene {
        Scene::new(&generate(&eval_spec()).unwrap(), "C:/work/imports").unwrap()
    }

    fn state(document: Value) -> FinalState {
        FinalState {
            document,
            checkpoints: Vec::new(),
        }
    }

    fn run(id: &str, scene: &Scene, state: &FinalState) -> Result<(), String> {
        (task(id).unwrap().check)(scene, state)
    }

    /// The clip `id` of `doc`, to change in place.
    fn item_mut<'a>(scene: &Scene, doc: &'a mut Value, id: &str) -> &'a mut Value {
        let tracks = doc["sequences"][&scene.sequence]["tracks"]
            .as_object_mut()
            .unwrap();
        tracks
            .values_mut()
            .find_map(|track| track["items"].get_mut(id))
            .unwrap()
    }

    fn items_mut<'a>(scene: &Scene, doc: &'a mut Value, lane: &Lane) -> &'a mut Value {
        &mut doc["sequences"][&scene.sequence]["tracks"][&lane.id]["items"]
    }

    /// A new clip of `media` on `lane`.
    fn add_clip(
        scene: &Scene,
        doc: &mut Value,
        lane: &Lane,
        id: &str,
        media: &str,
        at: i64,
        len: i64,
    ) {
        let mut clip = scene.item(&scene.base, &lane.items[0]).unwrap().clone();
        clip["id"] = json!(id);
        clip["media"] = json!(media);
        clip["start"] = json!(at);
        clip["duration"] = json!(len);
        items_mut(scene, doc, lane)[id] = clip;
    }

    #[test]
    fn the_project_has_what_the_tasks_name() {
        let scene = scene();
        assert_eq!(TASKS.len(), 10);
        let mut names: Vec<&str> = TASKS.iter().map(|t| t.id).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 10, "task names are unique");
        // V1 is the main track: six clips back to back from 0, the second longer than the
        // 15 s the split task takes off it.
        let v1 = scene.lane_clips(&scene.base, &scene.v1);
        let mut end = 0;
        for clip in &v1 {
            assert_eq!(clip["start"], end);
            end += clip["duration"].as_i64().unwrap();
        }
        assert!(v1[1]["duration"].as_i64().unwrap() > secs(20.0));
        assert!(end > secs(210.0), "the Outro marker is inside the timeline");
        // The clip the keyframe task animates has no position keys yet, and the clips of the
        // fade task no fades.
        let moved = scene.item(&scene.base, &scene.v2.items[1]).unwrap();
        assert!(
            moved["visual"]["transform"]["position"]
                .get("keyframes")
                .is_none()
        );
        let faded = scene.item(&scene.base, &scene.a1.items[0]).unwrap();
        assert_eq!(faded["audio"]["fade_in"]["duration"], 0);
        assert!(scene.import_path().ends_with("/imports/beach.mp4"));
        for task in &TASKS {
            assert!(
                !(task.prompt)(&scene).contains("itm_"),
                "{}: no ids in a prompt",
                task.id
            );
        }
    }

    #[test]
    fn the_untouched_project_fails_every_task() {
        let scene = scene();
        let untouched = state(scene.base.clone());
        for task in &TASKS {
            let verdict = (task.check)(&scene, &untouched);
            assert!(
                verdict.is_err(),
                "{} passes on the untouched project",
                task.id
            );
        }
    }

    #[test]
    fn split_and_trim() {
        let scene = scene();
        let target = scene.v1.items[1].clone();
        let before = scene.item(&scene.base, &target).unwrap().clone();
        let (start, length) = (
            before["start"].as_i64().unwrap(),
            before["duration"].as_i64().unwrap(),
        );
        let mut doc = scene.base.clone();
        item_mut(&scene, &mut doc, &target)["duration"] = json!(secs(10.0));
        let right = (start + secs(10.0), length - secs(15.0));
        add_clip(
            &scene,
            &mut doc,
            &scene.v1,
            "itm_new",
            text(&before["media"]),
            right.0,
            right.1,
        );
        assert_eq!(run("split-and-trim", &scene, &state(doc.clone())), Ok(()));
        // Split but not trimmed.
        item_mut(&scene, &mut doc, "itm_new")["duration"] = json!(length - secs(10.0));
        let verdict = run("split-and-trim", &scene, &state(doc)).unwrap_err();
        assert!(verdict.contains("5 s shorter"), "{verdict}");
    }

    #[test]
    fn insert_and_reorder() {
        let scene = scene();
        let mut doc = scene.base.clone();
        let last = scene.v1.items[5].clone();
        // The new clip first, the former last clip second, the rest after them.
        add_clip(
            &scene,
            &mut doc,
            &scene.v1,
            "itm_new",
            &scene.pattern_1,
            0,
            secs(120.0),
        );
        item_mut(&scene, &mut doc, &last)["start"] = json!(secs(120.0));
        for id in &scene.v1.items[..5] {
            let start = scene.item(&scene.base, id).unwrap()["start"]
                .as_i64()
                .unwrap();
            item_mut(&scene, &mut doc, id)["start"] = json!(start + secs(200.0));
        }
        assert_eq!(
            run("insert-and-reorder", &scene, &state(doc.clone())),
            Ok(())
        );
        // Inserted, but the last clip was left at the end.
        item_mut(&scene, &mut doc, &last)["start"] = json!(secs(900.0));
        assert!(run("insert-and-reorder", &scene, &state(doc)).is_err());
    }

    #[test]
    fn set_speed() {
        let scene = scene();
        let mut doc = scene.base.clone();
        item_mut(&scene, &mut doc, &scene.v2.items[0])["speed"] = json!({"num": 2, "den": 1});
        assert_eq!(run("set-speed", &scene, &state(doc.clone())), Ok(()));
        item_mut(&scene, &mut doc, &scene.v2.items[0])["speed"] = json!({"num": 1, "den": 2});
        let verdict = run("set-speed", &scene, &state(doc)).unwrap_err();
        assert!(verdict.contains("not 2x"), "{verdict}");
    }

    #[test]
    fn add_markers() {
        let scene = scene();
        let mut doc = scene.base.clone();
        let marker = |t: f64, label: &str| json!({"id": label, "t": secs(t), "label": label});
        doc["sequences"][&scene.sequence]["markers"] =
            json!({"mrk_1": marker(12.0, "Review"), "mrk_2": marker(210.0, "Outro")});
        assert_eq!(run("add-markers", &scene, &state(doc.clone())), Ok(()));
        // 3:30 read as 3.5 s.
        doc["sequences"][&scene.sequence]["markers"]["mrk_2"] = marker(3.5, "Outro");
        let verdict = run("add-markers", &scene, &state(doc)).unwrap_err();
        assert!(verdict.contains("Outro"), "{verdict}");
    }

    #[test]
    fn fade_audio() {
        let scene = scene();
        let mut doc = scene.base.clone();
        for id in &scene.a1.items[..2] {
            let audio = &mut item_mut(&scene, &mut doc, id)["audio"];
            audio["fade_in"]["duration"] = json!(secs(0.5));
            audio["fade_out"]["duration"] = json!(secs(2.0));
        }
        assert_eq!(run("fade-audio", &scene, &state(doc.clone())), Ok(()));
        // Only the first clip was faded.
        item_mut(&scene, &mut doc, &scene.a1.items[1])["audio"]["fade_out"]["duration"] = json!(0);
        let verdict = run("fade-audio", &scene, &state(doc)).unwrap_err();
        assert!(verdict.contains("clip 2 of A1"), "{verdict}");
    }

    #[test]
    fn keyframe_move() {
        let scene = scene();
        let mut doc = scene.base.clone();
        let key = |t: f64, x: f64| json!({"t": secs(t), "value": {"x": x, "y": 0.0}});
        let id = &scene.v2.items[1];
        item_mut(&scene, &mut doc, id)["visual"]["transform"]["position"] =
            json!({"keyframes": [key(0.0, -0.5), key(2.0, 0.0)]});
        assert_eq!(run("keyframe-move", &scene, &state(doc.clone())), Ok(()));
        // The second key at 2 s of the sequence, not of the clip.
        item_mut(&scene, &mut doc, id)["visual"]["transform"]["position"] =
            json!({"keyframes": [key(0.0, -0.5), key(-40.0, 0.0)]});
        assert!(run("keyframe-move", &scene, &state(doc)).is_err());
    }

    #[test]
    fn vertical_format() {
        let scene = scene();
        let mut doc = scene.base.clone();
        doc["sequences"][&scene.sequence]["format"]["width"] = json!(1080);
        doc["sequences"][&scene.sequence]["format"]["height"] = json!(1920);
        assert_eq!(run("vertical-format", &scene, &state(doc.clone())), Ok(()));
        doc["sequences"][&scene.sequence]["format"]["height"] = json!(1080);
        let verdict = run("vertical-format", &scene, &state(doc)).unwrap_err();
        assert_eq!(verdict, "the sequence is 1080 x 1080");
    }

    #[test]
    fn undo_and_checkpoint() {
        let scene = scene();
        let mut doc = scene.base.clone();
        let fifth = scene.a2.items[4].clone();
        items_mut(&scene, &mut doc, &scene.a2)
            .as_object_mut()
            .unwrap()
            .remove(&fifth);
        let mut done = state(doc.clone());
        // Right clips, but no restore point.
        assert!(
            run("undo-and-checkpoint", &scene, &done)
                .unwrap_err()
                .contains("checkpoint")
        );
        done.checkpoints.push("before-cleanup".into());
        assert_eq!(run("undo-and-checkpoint", &scene, &done), Ok(()));
        // Both deleted and nothing undone.
        let last = scene.a2.items[5].clone();
        items_mut(&scene, &mut doc, &scene.a2)
            .as_object_mut()
            .unwrap()
            .remove(&last);
        done.document = doc;
        assert!(run("undo-and-checkpoint", &scene, &done).is_err());
    }

    #[test]
    fn import_and_place() {
        let scene = scene();
        let mut doc = scene.base.clone();
        doc["media"]["med_new"] =
            json!({"id": "med_new", "path_abs": "C:\\work\\imports\\beach.mp4"});
        let last = scene.item(&scene.base, &scene.v1.items[5]).unwrap();
        let end = last["start"].as_i64().unwrap() + last["duration"].as_i64().unwrap();
        add_clip(
            &scene,
            &mut doc,
            &scene.v1,
            "itm_new",
            "med_new",
            end,
            secs(10.0),
        );
        assert_eq!(run("import-and-place", &scene, &state(doc.clone())), Ok(()));
        // Imported and placed, but on V2.
        items_mut(&scene, &mut doc, &scene.v1)
            .as_object_mut()
            .unwrap()
            .remove("itm_new");
        add_clip(
            &scene,
            &mut doc,
            &scene.v2,
            "itm_new",
            "med_new",
            end,
            secs(10.0),
        );
        assert!(run("import-and-place", &scene, &state(doc)).is_err());
    }

    #[test]
    fn long_tail() {
        let scene = scene();
        let mut doc = scene.base.clone();
        let track = &mut doc["sequences"][&scene.sequence]["tracks"][&scene.a2.id];
        track["muted"] = json!(true);
        track["name"] = json!("Music");
        assert_eq!(run("long-tail", &scene, &state(doc.clone())), Ok(()));
        doc["sequences"][&scene.sequence]["tracks"][&scene.a2.id]["name"] = json!("A2");
        let verdict = run("long-tail", &scene, &state(doc)).unwrap_err();
        assert!(verdict.contains("not \"Music\""), "{verdict}");
    }
}
