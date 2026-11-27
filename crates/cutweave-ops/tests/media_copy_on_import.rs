// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.04c: `media.import{copy_to_project}` copies new files into `<project>/media/` before it
//! registers them. The files are a few bytes made at test time in a temp folder; the prober is
//! the fake one (its content hash is the path unless a test says otherwise).
//!
//! #215: `media.import_sequence{copy_to_project}` copies every picture of a run into one folder
//! under `media/`, under its own file name.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use common::{H, assert_invalid};
use cutweave_commands::{Access, CmdError};
use cutweave_ops::{
    media::{FakeProbe, Probed},
    project::collect::on_import::ProjectFolder,
};
use serde_json::{Value, json};

/// A temp folder with a project folder (`p`) and a folder of originals next to it, removed
/// when dropped. Names are short: the fake hash is the path, and a hash has a length limit.
struct World {
    root: PathBuf,
    probe: FakeProbe,
    free: Option<u64>,
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

impl World {
    fn new(tag: &str) -> World {
        let root = std::env::temp_dir().join(format!("cw-coi-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("p")).unwrap();
        World {
            root,
            probe: FakeProbe::new(),
            free: Some(u64::MAX),
        }
    }

    fn at(&self, rel: &str) -> PathBuf {
        rel.split('/')
            .fold(self.root.clone(), |p, part| p.join(part))
    }

    fn path(&self, rel: &str) -> String {
        s(&self.at(rel))
    }

    /// Writes `bytes` at `rel` without telling the prober.
    fn write(&self, rel: &str, bytes: &[u8]) -> String {
        let path = self.at(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        s(&path)
    }

    /// A video file at `rel` holding `bytes`.
    fn video(&mut self, rel: &str, bytes: &[u8]) -> String {
        let path = self.write(rel, bytes);
        self.probe = self
            .probe
            .clone()
            .with(&path, FakeProbe::video(&path, 5, false));
        path
    }

    /// A video file at `rel` whose content hash is `hash`.
    fn video_hashed(&mut self, rel: &str, bytes: &[u8], hash: &str) -> String {
        let path = self.write(rel, bytes);
        let probed = Probed {
            content_hash: hash.to_owned(),
            ..FakeProbe::video(&path, 5, false)
        };
        self.probe = self.probe.clone().with(&path, probed);
        path
    }

    fn project(&self, h: &H) -> ProjectFolder {
        let free = self.free;
        ProjectFolder {
            root: self.path("p"),
            free_space: Arc::new(move |_| free),
            known_hashes: h
                .doc()
                .media
                .values()
                .filter_map(|m| m.content_hash.clone())
                .collect::<HashSet<_>>(),
        }
    }

    /// `media.import` against a saved project in `p`.
    fn import(&self, h: &H, params: Value) -> Result<Value, CmdError> {
        self.call(h, "media.import", params)
    }

    /// An import command against a saved project in `p`.
    fn call(&self, h: &H, name: &str, params: Value) -> Result<Value, CmdError> {
        let mut ctx = h
            .import_ctx(self.probe.clone())
            .with_project(Some(self.project(h)));
        h.registry
            .invoke_engine_json(name, params, &mut ctx, Access::STANDARD)
    }

    /// `media.import_sequence` with `copy_to_project` against a saved project in `p`.
    fn import_run(&self, h: &H, path: &str) -> Result<Value, CmdError> {
        self.call(
            h,
            "media.import_sequence",
            json!({"path": path, "copy_to_project": true}),
        )
    }

    /// The pictures `<dir>/f_<n>.png` for `numbers`, each holding its own few bytes; returns
    /// the path of the first.
    fn pictures(&mut self, dir: &str, numbers: &[u32]) -> String {
        for n in numbers {
            let path = self.write(
                &format!("{dir}/f_{n:03}.png"),
                format!("picture {n}").as_bytes(),
            );
            self.probe = self.probe.clone().with(&path, FakeProbe::image(&path));
        }
        self.path(&format!("{dir}/f_{:03}.png", numbers[0]))
    }

    /// Every file below `rel`, as `/`-separated paths relative to it, sorted.
    fn files(&self, rel: &str) -> Vec<String> {
        fn visit(dir: &Path, prefix: &str, out: &mut Vec<String>) {
            let Ok(read) = fs::read_dir(dir) else { return };
            for entry in read.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let rel = if prefix.is_empty() {
                    name
                } else {
                    format!("{prefix}/{name}")
                };
                if entry.path().is_dir() {
                    visit(&entry.path(), &rel, out);
                } else {
                    out.push(rel);
                }
            }
        }
        let mut out = Vec::new();
        visit(&self.at(rel), "", &mut out);
        out.sort();
        out
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// The path each asset of the document points at, sorted.
fn asset_paths(h: &H) -> Vec<String> {
    let mut paths: Vec<String> = h
        .doc()
        .media
        .values()
        .filter_map(|m| m.path_abs.clone())
        .collect();
    paths.sort();
    paths
}

#[test]
fn a_new_file_is_copied_into_the_media_folder_and_the_asset_points_at_the_copy() {
    let mut w = World::new("one");
    let original = w.video("src/Footage/clip.mp4", b"the bytes of the clip");
    let h = H::empty();

    let out = w
        .import(&h, json!({"paths": [original], "copy_to_project": true}))
        .unwrap();

    assert_eq!(w.files("p"), ["media/Footage/clip.mp4"]);
    let copy = w.at("p/media/Footage/clip.mp4");
    assert_eq!(fs::read(&copy).unwrap(), b"the bytes of the clip");
    assert_eq!(asset_paths(&h), [s(&copy)]);
    // The result names the path that was asked for, and the file is new to the project.
    assert_eq!(out["media"][0]["path"], json!(original));
    assert_eq!(out["media"][0]["duplicate"], json!(false));
    assert_eq!(out["failed"], json!([]));
    // The original is untouched.
    assert_eq!(fs::read(&original).unwrap(), b"the bytes of the clip");
}

#[test]
fn without_the_flag_nothing_is_copied() {
    let mut w = World::new("off");
    let original = w.video("src/a.mp4", b"a");
    let h = H::empty();

    let other = w.video("src/b.mp4", b"b");

    w.import(&h, json!({"paths": [original]})).unwrap();
    w.import(&h, json!({"paths": [other], "copy_to_project": false}))
        .unwrap();

    assert_eq!(w.files("p"), Vec::<String>::new());
    assert_eq!(asset_paths(&h), [w.path("src/a.mp4"), w.path("src/b.mp4")]);
}

#[test]
fn an_untitled_document_is_told_to_save_first_and_nothing_is_imported() {
    let mut w = World::new("untitled");
    let original = w.video("src/a.mp4", b"a");
    let h = H::empty();
    // No project folder: the context of an untitled document.
    let mut ctx = h.import_ctx(w.probe.clone());

    let refused = h.registry.invoke_engine_json(
        "media.import",
        json!({"paths": [original], "copy_to_project": true}),
        &mut ctx,
        Access::STANDARD,
    );

    let message = refused.as_ref().unwrap_err().message().clone();
    assert!(
        message.contains("save the project first"),
        "the message says what to do: {message}"
    );
    assert_invalid(refused, "/copy_to_project");
    assert!(h.doc().media.is_empty());
    assert_eq!(w.files("p"), Vec::<String>::new());
}

#[test]
fn files_that_do_not_fit_are_refused_before_anything_is_copied() {
    let mut w = World::new("full");
    let a = w.video("src/a.mp4", b"aaaa");
    let b = w.video("src/b.mp4", b"bbbb");
    w.free = Some(1 << 20);
    let h = H::empty();

    let refused = w.import(&h, json!({"paths": [a, b], "copy_to_project": true}));

    let message = refused.as_ref().unwrap_err().message().clone();
    assert!(message.contains("not enough space"), "{message}");
    assert_invalid(refused, "/copy_to_project");
    assert_eq!(w.files("p"), Vec::<String>::new(), "nothing was copied");
    assert!(h.doc().media.is_empty(), "nothing was imported");
}

#[test]
fn a_drive_that_cannot_say_its_free_space_does_not_stop_the_import() {
    let mut w = World::new("unknown");
    let a = w.video("src/a.mp4", b"aaaa");
    w.free = None;
    let h = H::empty();

    w.import(&h, json!({"paths": [a], "copy_to_project": true}))
        .unwrap();

    assert_eq!(w.files("p"), ["media/src/a.mp4"]);
}

#[test]
fn a_file_inside_the_project_folder_stays_where_it_is() {
    let mut w = World::new("inside");
    let own = w.video("p/shots/own.mp4", b"own");
    let h = H::empty();

    let out = w
        .import(&h, json!({"paths": [own], "copy_to_project": true}))
        .unwrap();

    assert_eq!(w.files("p"), ["shots/own.mp4"]);
    assert_eq!(asset_paths(&h), [own]);
    assert_eq!(out["media"][0]["duplicate"], json!(false));
}

#[test]
fn content_the_project_already_has_is_a_duplicate_and_gets_no_second_copy() {
    let mut w = World::new("dup");
    let first = w.video_hashed("src/a/clip.mp4", b"same", "h:same");
    let second = w.video_hashed("src/b/again.mp4", b"same", "h:same");
    let h = H::empty();
    let imported = w
        .import(&h, json!({"paths": [first], "copy_to_project": true}))
        .unwrap();

    let out = w
        .import(&h, json!({"paths": [second], "copy_to_project": true}))
        .unwrap();

    assert_eq!(out["media"][0]["duplicate"], json!(true));
    assert_eq!(out["media_ids"], imported["media_ids"]);
    assert_eq!(w.files("p"), ["media/a/clip.mp4"]);
    assert_eq!(h.doc().media.len(), 1);
}

#[test]
fn two_files_with_the_same_content_in_one_call_share_one_copy() {
    let mut w = World::new("twins");
    let first = w.video_hashed("src/a/clip.mp4", b"same", "h:twin");
    let second = w.video_hashed("src/b/again.mp4", b"same", "h:twin");
    let h = H::empty();

    let out = w
        .import(
            &h,
            json!({"paths": [first, second], "copy_to_project": true}),
        )
        .unwrap();

    assert_eq!(w.files("p"), ["media/a/clip.mp4"]);
    assert_eq!(out["media"][0]["duplicate"], json!(false));
    assert_eq!(out["media"][1]["duplicate"], json!(true));
    assert_eq!(out["media"][1]["path"], json!(second));
    assert_eq!(h.doc().media.len(), 1);
}

#[test]
fn a_taken_name_gets_a_number_and_a_complete_copy_is_used_as_it_is() {
    let mut w = World::new("names");
    // Two different files that both want `media/day/clip.mp4`.
    let one = w.video("src/x/day/clip.mp4", b"first take");
    let two = w.video("src/y/day/clip.mp4", b"second take, longer");
    let h = H::empty();

    w.import(&h, json!({"paths": [one, two], "copy_to_project": true}))
        .unwrap();

    assert_eq!(
        w.files("p"),
        ["media/day/clip (2).mp4", "media/day/clip.mp4"]
    );
    assert_eq!(
        fs::read(w.at("p/media/day/clip.mp4")).unwrap(),
        b"first take"
    );
    assert_eq!(
        fs::read(w.at("p/media/day/clip (2).mp4")).unwrap(),
        b"second take, longer"
    );

    // A file whose complete copy is in the folder already (from an import that was undone,
    // say) is not copied a second time: the asset points at the copy that is there.
    let h = H::empty();
    let out = w
        .import(&h, json!({"paths": [two], "copy_to_project": true}))
        .unwrap();
    assert_eq!(out["media"][0]["duplicate"], json!(false));
    assert_eq!(asset_paths(&h), [w.path("p/media/day/clip (2).mp4")]);
    assert_eq!(w.files("p").len(), 2, "no third file");
}

#[test]
fn a_folder_imported_with_its_structure_keeps_it_under_media() {
    let mut w = World::new("tree");
    w.video("src/Shoot/day1/a.mp4", b"a");
    w.video("src/Shoot/day1/cam/b.mp4", b"b");
    w.video("src/Shoot/c.mp4", b"c");
    let h = H::empty();

    let out = w
        .import(
            &h,
            json!({"paths": [w.path("src/Shoot")], "copy_to_project": true}),
        )
        .unwrap();

    assert_eq!(
        w.files("p"),
        [
            "media/Shoot/c.mp4",
            "media/Shoot/day1/a.mp4",
            "media/Shoot/day1/cam/b.mp4"
        ]
    );
    assert_eq!(
        out["folders"],
        json!(3),
        "the bin mirrors the tree as before"
    );
    assert_eq!(
        asset_paths(&h),
        [
            w.path("p/media/Shoot/c.mp4"),
            w.path("p/media/Shoot/day1/a.mp4"),
            w.path("p/media/Shoot/day1/cam/b.mp4")
        ]
    );
    // Each result names the original it stands for.
    let asked: Vec<&str> = out["media"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["path"].as_str().unwrap())
        .collect();
    assert!(asked.iter().all(|p| p.starts_with(&w.path("src"))));
}

#[test]
fn a_file_that_cannot_be_copied_is_reported_and_the_rest_is_imported() {
    let mut w = World::new("gone");
    let good = w.video("src/good.mp4", b"good");
    // The prober knows this one, but it is not on the disk (it went away after the probe).
    let gone = w.path("src/gone.mp4");
    w.probe = w
        .probe
        .clone()
        .with(&gone, FakeProbe::video(&gone, 5, false));
    let h = H::empty();

    let out = w
        .import(&h, json!({"paths": [gone, good], "copy_to_project": true}))
        .unwrap();

    assert_eq!(out["failed"].as_array().unwrap().len(), 1);
    assert_eq!(out["failed"][0]["path"], json!(gone));
    assert_eq!(out["failed"][0]["code"], json!("io"));
    assert_eq!(out["media"].as_array().unwrap().len(), 1);
    assert_eq!(out["media"][0]["path"], json!(good));
    assert_eq!(w.files("p"), ["media/src/good.mp4"]);
    assert_eq!(asset_paths(&h), [w.path("p/media/src/good.mp4")]);
}

#[test]
fn no_part_file_is_left_behind() {
    let mut w = World::new("part");
    let original = w.video("src/big.mp4", &vec![7_u8; 3 << 20]);
    let h = H::empty();

    w.import(&h, json!({"paths": [original], "copy_to_project": true}))
        .unwrap();

    assert_eq!(w.files("p"), ["media/src/big.mp4"]);
    assert_eq!(
        fs::metadata(w.at("p/media/src/big.mp4")).unwrap().len(),
        3 << 20
    );
}

// ----- image sequences (#215) -----------------------------------------------------------------

#[test]
fn a_run_is_copied_whole_into_one_folder_and_the_asset_points_at_the_copy() {
    let mut w = World::new("run");
    // Number 3 has no file: a gap.
    let first = w.pictures("src/r", &[1, 2, 4, 5]);
    let h = H::empty();

    // Any picture of the run stands for it.
    let out = w.import_run(&h, &w.path("src/r/f_004.png")).unwrap();

    assert_eq!(
        w.files("p"),
        [
            "media/r/f_001.png",
            "media/r/f_002.png",
            "media/r/f_004.png",
            "media/r/f_005.png"
        ]
    );
    assert_eq!(
        fs::read(w.at("p/media/r/f_004.png")).unwrap(),
        b"picture 4",
        "each picture keeps its own content"
    );
    assert_eq!(asset_paths(&h), [w.path("p/media/r/f_001.png")]);
    let doc = h.doc();
    let run = doc.media.values().next().unwrap().image_sequence.clone();
    let run = run.unwrap();
    assert_eq!(
        (run.pattern.as_str(), run.first, run.count, run.missing),
        ("f_%03d.png", 1, 5, vec![3])
    );
    // The result names the run where it was found, and it is new to the project.
    assert_eq!(out["path"], json!(first));
    assert_eq!(out["duplicate"], json!(false));
    assert_eq!(out["kind"], json!("image_sequence"));
    // The originals are untouched.
    assert_eq!(w.files("src").len(), 4);
    assert_eq!(h.commits(), 1, "one undo step");
}

#[test]
fn a_run_is_not_copied_without_the_flag() {
    let mut w = World::new("runoff");
    let first = w.pictures("src/r", &[1, 2, 3]);
    let h = H::empty();

    w.call(&h, "media.import_sequence", json!({"path": first}))
        .unwrap();

    assert_eq!(w.files("p"), Vec::<String>::new());
    assert_eq!(asset_paths(&h), [first]);
}

#[test]
fn a_run_in_an_untitled_document_is_told_to_save_first() {
    let mut w = World::new("rununt");
    let first = w.pictures("src/r", &[1, 2]);
    let h = H::empty();
    let mut ctx = h.import_ctx(w.probe.clone());

    let refused = h.registry.invoke_engine_json(
        "media.import_sequence",
        json!({"path": first, "copy_to_project": true}),
        &mut ctx,
        Access::STANDARD,
    );

    let message = refused.as_ref().unwrap_err().message().clone();
    assert!(message.contains("save the project first"), "{message}");
    assert_invalid(refused, "/copy_to_project");
    assert!(h.doc().media.is_empty());
}

#[test]
fn a_run_that_does_not_fit_is_refused_before_anything_is_copied() {
    let mut w = World::new("runfull");
    let first = w.pictures("src/r", &[1, 2, 3]);
    w.free = Some(1 << 20);
    let h = H::empty();

    let refused = w.import_run(&h, &first);

    let message = refused.as_ref().unwrap_err().message().clone();
    assert!(message.contains("not enough space"), "{message}");
    assert_invalid(refused, "/copy_to_project");
    assert_eq!(w.files("p"), Vec::<String>::new(), "nothing was copied");
    assert!(h.doc().media.is_empty(), "nothing was imported");
}

#[test]
fn a_run_inside_the_project_folder_stays_where_it_is() {
    let mut w = World::new("runin");
    let first = w.pictures("p/shots", &[1, 2]);
    let h = H::empty();

    w.import_run(&h, &first).unwrap();

    assert_eq!(w.files("p"), ["shots/f_001.png", "shots/f_002.png"]);
    assert_eq!(asset_paths(&h), [first]);
}

#[test]
fn a_folder_with_a_taken_name_is_passed_over_and_the_run_stays_together() {
    let mut w = World::new("runtaken");
    let first = w.pictures("src/r", &[1, 2, 3]);
    // Another file already has the name of the second picture in `media/r`.
    w.write("p/media/r/f_002.png", b"something else");
    let h = H::empty();

    w.import_run(&h, &first).unwrap();

    assert_eq!(
        w.files("p"),
        [
            "media/r (2)/f_001.png",
            "media/r (2)/f_002.png",
            "media/r (2)/f_003.png",
            "media/r/f_002.png"
        ]
    );
    assert_eq!(asset_paths(&h), [w.path("p/media/r (2)/f_001.png")]);
    assert_eq!(
        fs::read(w.at("p/media/r/f_002.png")).unwrap(),
        b"something else",
        "the other file is untouched"
    );
}

#[test]
fn a_run_the_project_has_is_a_duplicate_and_a_complete_copy_is_used_as_it_is() {
    let mut w = World::new("rundup");
    let first = w.pictures("src/r", &[1, 2, 3]);
    let h = H::empty();
    let imported = w.import_run(&h, &first).unwrap();

    // Again, into the same project: the same asset, and no second copy.
    let again = w.import_run(&h, &first).unwrap();
    assert_eq!(again["duplicate"], json!(true));
    assert_eq!(again["media_id"], imported["media_id"]);
    assert_eq!(w.files("p").len(), 3);
    assert_eq!(h.doc().media.len(), 1);

    // After an undo (a document without the asset) the pictures that are there are used, and
    // one that went missing is copied again: no `r (2)`.
    fs::remove_file(w.at("p/media/r/f_003.png")).unwrap();
    let h = H::empty();
    let out = w.import_run(&h, &first).unwrap();
    assert_eq!(out["duplicate"], json!(false));
    assert_eq!(
        w.files("p"),
        [
            "media/r/f_001.png",
            "media/r/f_002.png",
            "media/r/f_003.png"
        ]
    );
    assert_eq!(asset_paths(&h), [w.path("p/media/r/f_001.png")]);
}

#[test]
fn a_run_with_a_picture_that_cannot_be_copied_is_not_imported() {
    let mut w = World::new("rungone");
    let first = w.pictures("src/r", &[1, 2, 3]);
    // The prober listed this one, but it went away before the copy.
    fs::remove_file(w.at("src/r/f_003.png")).unwrap();
    let h = H::empty();

    let refused = w.import_run(&h, &first);

    let message = refused.as_ref().unwrap_err().message().clone();
    assert!(
        message.contains("could not be copied into the project") && message.contains("f_003.png"),
        "{message}"
    );
    assert_invalid(refused, "/path");
    assert!(h.doc().media.is_empty(), "nothing was imported");
}
