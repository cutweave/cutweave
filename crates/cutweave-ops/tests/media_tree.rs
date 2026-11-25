// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.09: importing a folder tree with `media.import` (ADR-0020): the bin folders it makes,
//! name clashes, the 32-level limit, `keep_structure: false`, and `media.register_tree`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::PathBuf;

use common::{H, assert_invalid, id};
use cutweave_commands::Access;
use cutweave_model::{FolderId, MAX_FOLDER_DEPTH};
use cutweave_ops::media::FakeProbe;
use serde_json::{Value, json};

struct Tree {
    root: PathBuf,
    probe: FakeProbe,
}

impl Tree {
    fn new(tag: &str) -> Tree {
        let root = std::env::temp_dir().join(format!("cutweave-tree-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Tree {
            root,
            probe: FakeProbe::new(),
        }
    }

    /// A file with a probe result at `rel` (a `/`-separated path under the root).
    fn video(&mut self, rel: &str) -> &mut Tree {
        let path = self.joined(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, rel).unwrap();
        let p = path.to_string_lossy().into_owned();
        self.probe = self.probe.clone().with(&p, FakeProbe::video(&p, 5, false));
        self
    }

    /// The path as the walk spells it (the OS separator throughout).
    fn joined(&self, rel: &str) -> PathBuf {
        rel.split('/')
            .fold(self.root.clone(), |p, part| p.join(part))
    }

    fn path(&self, rel: &str) -> String {
        self.joined(rel).to_string_lossy().into_owned()
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn import(h: &H, tree: &Tree, params: Value) -> Result<Value, cutweave_commands::CmdError> {
    let mut ctx = h.import_ctx(tree.probe.clone());
    h.registry
        .invoke_engine_json("media.import", params, &mut ctx, Access::STANDARD)
}

fn paths(h: &H) -> Vec<String> {
    let out = h.query("media.list_folders", json!({})).unwrap();
    let mut v: Vec<String> = out["folders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap().to_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn directories_with_media_become_folders_inside_the_chosen_one() {
    let mut h = H::empty();
    let mut tree = Tree::new("mirror");
    tree.video("Shoot/a.mp4")
        .video("Shoot/day1/b.mp4")
        .video("Shoot/day1/deep/c.mp4");
    std::fs::create_dir_all(tree.joined("Shoot/empty")).unwrap();
    let into: FolderId = id(&h.ok("media.create_folder", json!({"name": "Into"}))["folder_id"]);
    let out = import(
        &h,
        &tree,
        json!({"paths": [tree.path("Shoot")], "folder": into}),
    )
    .unwrap();
    assert_eq!(out["media_ids"].as_array().unwrap().len(), 3);
    assert_eq!(out["folders"], 3, "Shoot, day1 and deep; not `empty`");
    assert_eq!(
        paths(&h),
        [
            "Into",
            "Into/Shoot",
            "Into/Shoot/day1",
            "Into/Shoot/day1/deep"
        ]
    );
    let doc = h.doc();
    let in_shoot = doc
        .media
        .values()
        .filter(|m| m.folder.and_then(|f| doc.folder_path(f)).as_deref() == Some("Into/Shoot"))
        .count();
    assert_eq!(in_shoot, 1);
}

#[test]
fn a_folder_name_that_is_taken_gets_the_next_free_one() {
    let mut h = H::empty();
    h.ok("media.create_folder", json!({"name": "shoot"}));
    let mut tree = Tree::new("clash");
    tree.video("Shoot/a.mp4");
    let out = import(&h, &tree, json!({"paths": [tree.path("Shoot")]})).unwrap();
    assert_eq!(out["folders"], 1);
    assert_eq!(paths(&h), ["Shoot 2", "shoot"]);
}

#[test]
fn without_keep_structure_everything_lands_in_the_folder() {
    let mut h = H::empty();
    let mut tree = Tree::new("flat");
    tree.video("Shoot/a.mp4").video("Shoot/day1/b.mp4");
    let into: FolderId = id(&h.ok("media.create_folder", json!({"name": "Into"}))["folder_id"]);
    let out = import(
        &h,
        &tree,
        json!({"paths": [tree.path("Shoot")], "folder": into, "keep_structure": false}),
    )
    .unwrap();
    assert_eq!(out["folders"], 0);
    assert_eq!(paths(&h), ["Into"]);
    assert!(h.doc().media.values().all(|m| m.folder == Some(into)));
}

#[test]
fn what_would_nest_deeper_than_the_limit_goes_into_the_deepest_folder() {
    let mut h = H::empty();
    let mut parent: Option<FolderId> = None;
    for i in 0..MAX_FOLDER_DEPTH - 1 {
        parent = Some(id(&h.ok(
            "media.create_folder",
            json!({"parent": parent, "name": format!("L{i}")}),
        )["folder_id"]));
    }
    let mut tree = Tree::new("deep");
    tree.video("Shoot/day1/a.mp4");
    let out = import(
        &h,
        &tree,
        json!({"paths": [tree.path("Shoot")], "folder": parent}),
    )
    .unwrap();
    // One level is left: `Shoot` fits, `day1` does not, so the file goes into `Shoot`.
    assert_eq!(out["folders"], 1, "{out}");
    let doc = h.doc();
    let media = doc.media.values().next().unwrap();
    assert_eq!(
        doc.folder_depth(media.folder.unwrap()).unwrap(),
        MAX_FOLDER_DEPTH
    );
}

#[test]
fn a_file_in_a_second_tree_keeps_its_first_folder_and_makes_no_new_one() {
    let h = H::empty();
    let mut tree = Tree::new("known");
    tree.video("One/a.mp4");
    import(&h, &tree, json!({"paths": [tree.path("One")]})).unwrap();
    // Its content is known now: neither the folder nor the named file adds anything.
    let again = import(
        &h,
        &tree,
        json!({"paths": [tree.path("One"), tree.path("One/a.mp4")]}),
    )
    .unwrap();
    assert_eq!(again["folders"], 0);
    assert_eq!(again["media_ids"].as_array().unwrap().len(), 1);
    assert_eq!(paths(&h), ["One"]);
}

#[test]
fn a_sink_that_cannot_make_folders_refuses_a_tree_and_the_error_names_the_path() {
    let h = H::empty();
    let mut tree = Tree::new("badpath");
    tree.video("One/a.mp4");
    let result = import(
        &h,
        &tree,
        json!({"paths": [tree.path("One"), "relative/dir"]}),
    );
    assert_invalid(result, "/paths/1");
}
