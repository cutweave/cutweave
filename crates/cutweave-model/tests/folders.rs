// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.07 (ADR-0020): the media folder invariants of `Project::validate` (references, cycles,
//! depth, names, sibling uniqueness, asset display names) and the folder tree's wire form.

#![allow(clippy::unwrap_used)]

use cutweave_model::{
    EntityKind, FolderId, IdGen, Invariant, MAX_FOLDER_DEPTH, MediaAsset, MediaFolder, MediaKind,
    ProbeInfo, Project, SeededGen,
};
use serde_json::json;

fn fid(n: u128) -> FolderId {
    FolderId::from_u128(n)
}

fn with_folders(folders: &[(u128, &str, Option<u128>)]) -> Project {
    let mut doc = Project::new("Folders");
    for (n, name, parent) in folders {
        let folder = MediaFolder::new(fid(*n), *name, parent.map(fid));
        doc.folders.insert(folder.id, folder);
    }
    doc
}

fn asset(doc: &mut Project, n: u128) -> cutweave_model::MediaId {
    let id = cutweave_model::MediaId::from_u128(n);
    doc.media.insert(
        id,
        MediaAsset::file(id, MediaKind::Image, "/m/x.png", ProbeInfo::default()),
    );
    id
}

#[test]
fn a_tree_with_equal_names_in_different_parents_is_valid() {
    let doc = with_folders(&[
        (1, "Day 1", None),
        (2, "B-roll", None),
        (3, "Day 1", Some(2)),
        (4, "day 2", Some(2)),
        (5, "Day 1", Some(3)),
    ]);
    assert_eq!(doc.violations(), vec![]);
}

#[test]
fn dangling_parents_and_asset_folders_are_reported() {
    let mut doc = with_folders(&[(1, "A", Some(9))]);
    let media = asset(&mut doc, 1);
    doc.media.get_mut(&media).unwrap().folder = Some(fid(8));
    let all = doc.violations();
    assert_eq!(all.len(), 2, "{all:#?}");
    assert!(all.iter().any(|v| matches!(v,
        Invariant::DanglingReference { path, kind: EntityKind::Folder, .. }
            if path.ends_with("/folder"))));
    assert!(all.iter().any(|v| matches!(v,
        Invariant::DanglingReference { path, kind: EntityKind::Folder, id }
            if path.ends_with("/parent") && *id == fid(9).to_string())));
}

#[test]
fn cycles_are_reported_once_with_their_members() {
    let doc = with_folders(&[(1, "Self", Some(1))]);
    let Err(Invariant::FolderCycle { cycle }) = doc.validate() else {
        panic!("expected a cycle: {:?}", doc.violations());
    };
    assert_eq!(cycle, vec![fid(1), fid(1)]);

    // A three-folder loop with a folder hanging below it: one report.
    let doc = with_folders(&[
        (1, "A", Some(3)),
        (2, "B", Some(1)),
        (3, "C", Some(2)),
        (4, "Below", Some(2)),
    ]);
    let all = doc.violations();
    assert_eq!(all.len(), 1, "{all:#?}");
    let Invariant::FolderCycle { cycle } = &all[0] else {
        panic!("{all:?}");
    };
    assert_eq!(cycle.len(), 4);
    assert_eq!(cycle.first(), cycle.last());
    assert!(all[0].to_string().contains(" -> "));
}

#[test]
fn depth_is_limited_to_32_levels() {
    let limit = u128::try_from(MAX_FOLDER_DEPTH).unwrap();
    let chain: Vec<(u128, String, Option<u128>)> = (1..=limit + 2)
        .map(|n| (n, format!("L{n}"), (n > 1).then(|| n - 1)))
        .collect();
    let as_refs = |upto: u128| -> Vec<(u128, &str, Option<u128>)> {
        chain
            .iter()
            .take(usize::try_from(upto).unwrap())
            .map(|(n, name, p)| (*n, name.as_str(), *p))
            .collect()
    };
    assert_eq!(with_folders(&as_refs(limit)).violations(), vec![]);
    let doc = with_folders(&as_refs(limit + 2));
    let all = doc.violations();
    assert_eq!(
        all,
        vec![
            Invariant::FolderTooDeep {
                folder: fid(limit + 1),
                depth: 33
            },
            Invariant::FolderTooDeep {
                folder: fid(limit + 2),
                depth: 34
            },
        ]
    );
    assert_eq!(doc.folder_depth(fid(limit + 2)), Some(34));
}

#[test]
fn names_are_checked_and_unique_among_siblings_ignoring_case() {
    for bad in ["", "   ", "a/b", "tab\there", &"x".repeat(256)] {
        let doc = with_folders(&[(1, bad, None)]);
        assert!(
            matches!(
                doc.violations().as_slice(),
                [Invariant::InvalidFolderName { .. }]
            ),
            "{bad:?}: {:?}",
            doc.violations()
        );
    }
    let doc = with_folders(&[(1, "Interviews", None), (2, "INTERVIEWS", None)]);
    assert_eq!(
        doc.violations(),
        vec![Invariant::DuplicateFolderName {
            parent: None,
            first: fid(1),
            second: fid(2),
            name: "INTERVIEWS".into(),
        }]
    );
    let value = serde_json::to_value(doc.validate().unwrap_err()).unwrap();
    assert_eq!(value["invariant"], "duplicate_folder_name");
}

#[test]
fn asset_display_names_are_checked() {
    let mut doc = Project::new("Names");
    let id = asset(&mut doc, 1);
    doc.media.get_mut(&id).unwrap().name = Some("Take 1/2 ✂".into());
    assert_eq!(doc.violations(), vec![]);
    for bad in ["", " ", "line\nbreak"] {
        doc.media.get_mut(&id).unwrap().name = Some(bad.into());
        assert!(
            matches!(
                doc.violations().as_slice(),
                [Invariant::InvalidMedia { .. }]
            ),
            "{bad:?}"
        );
    }
}

#[test]
fn folders_round_trip_keep_unknown_fields_and_count_for_the_id_high_water() {
    let mut ids = SeededGen::new(11);
    let mut doc = Project::new("Wire");
    let mut folder = MediaFolder::new(ids.mint(), "B-roll", None);
    folder.extra.insert("x_color", json!("teal"));
    let media = MediaAsset::file(
        ids.mint(),
        MediaKind::Image,
        "/m/a.png",
        ProbeInfo::default(),
    );
    let folder_id = folder.id;
    doc.folders.insert(folder.id, folder);
    let mut media = media;
    media.folder = Some(folder_id);
    media.name = Some("Logo".into());
    doc.media.insert(media.id, media);
    let value = serde_json::to_value(&doc).unwrap();
    let key = folder_id.to_string();
    assert!(key.starts_with("fld_"));
    assert_eq!(value["folders"][&key]["x_color"], "teal");
    assert_eq!(value["folders"][&key]["parent"], json!(null));
    let back: Project = serde_json::from_value(value).unwrap();
    assert_eq!(back, doc);
    // A new engine resumes the ID stream past folder IDs too.
    assert_eq!(doc.seeded_id_high_water(11), 2);
}
