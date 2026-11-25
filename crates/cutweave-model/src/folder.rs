// SPDX-License-Identifier: GPL-3.0-or-later
//! Media folders (schema v2; ADR-0020): a folder tree that organizes the project's media.
//!
//! Every [`MediaFolder`] has an optional parent (none: the top level of the bin) and every
//! [`MediaAsset`](crate::MediaAsset) lives in at most one folder (`MediaAsset::folder`, none:
//! the top level). Folders and assets are addressed by ID; the `/`-separated path
//! ([`Project::folder_path`]) is for people and is unambiguous because sibling names are unique
//! ignoring case. [`Project::validate`] checks the tree: parents exist, no cycles, at most
//! [`MAX_FOLDER_DEPTH`] levels, names per [`folder_name_problem`] and unique among siblings
//! ([`folder_name_key`]).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{Custom, Extra, FolderId, Project};

/// Deepest allowed nesting: a top-level folder is at depth 1.
pub const MAX_FOLDER_DEPTH: usize = 32;

/// Longest folder name, in characters.
pub const MAX_FOLDER_NAME_CHARS: usize = 255;

/// Longest display name of a media asset (`MediaAsset::name`), in characters.
pub const MAX_MEDIA_NAME_CHARS: usize = 255;

/// A folder of the media bin (ADR-0020).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, specta::Type)]
pub struct MediaFolder {
    /// The folder ID (also its key in `Project::folders`).
    pub id: FolderId,
    /// The name shown in the bin: 1 to 255 characters, no `/` or control characters, unique
    /// among its siblings ignoring case.
    pub name: String,
    /// The folder it is in; absent at the top level.
    pub parent: Option<FolderId>,
    /// Plugin data (arch §4.3).
    pub custom: Custom,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

impl MediaFolder {
    /// A folder named `name` in `parent` (none: the top level).
    #[must_use]
    pub fn new(id: FolderId, name: impl Into<String>, parent: Option<FolderId>) -> MediaFolder {
        MediaFolder {
            id,
            name: name.into(),
            parent,
            custom: Custom::new(),
            extra: Extra::new(),
        }
    }
}

/// Why `name` cannot name a folder, or `None` if it can: it must hold a visible character, be
/// at most [`MAX_FOLDER_NAME_CHARS`] characters and contain no `/` (the path separator) and no
/// control characters.
#[must_use]
pub fn folder_name_problem(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        Some("a folder name must not be empty")
    } else if name.chars().count() > MAX_FOLDER_NAME_CHARS {
        Some("a folder name must be at most 255 characters")
    } else if name.contains('/') {
        Some("a folder name must not contain `/`")
    } else if name.chars().any(char::is_control) {
        Some("a folder name must not contain control characters")
    } else {
        None
    }
}

/// Why `name` cannot be the display name of a media asset, or `None` if it can: it must hold a
/// visible character, be at most [`MAX_MEDIA_NAME_CHARS`] characters and contain no control
/// characters.
#[must_use]
pub fn media_name_problem(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        Some("a display name must not be empty")
    } else if name.chars().count() > MAX_MEDIA_NAME_CHARS {
        Some("a display name must be at most 255 characters")
    } else if name.chars().any(char::is_control) {
        Some("a display name must not contain control characters")
    } else {
        None
    }
}

/// The form sibling folder names are compared in: two names clash when their keys are equal
/// (Unicode lower case, so `B-roll` and `b-ROLL` clash).
#[must_use]
pub fn folder_name_key(name: &str) -> String {
    name.to_lowercase()
}

impl Project {
    /// `id` and its ancestors up to the top level, `id` first. Stops early at a parent that
    /// does not exist or that is already listed (a cycle); empty if `id` does not exist.
    #[must_use]
    pub fn folder_ancestry(&self, id: FolderId) -> Vec<FolderId> {
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        let mut cur = Some(id);
        while let Some(f) = cur {
            let Some(folder) = self.folders.get(&f) else {
                break;
            };
            if !seen.insert(f) {
                break;
            }
            out.push(f);
            cur = folder.parent;
        }
        out
    }

    /// How deep `id` is nested (1 at the top level), or `None` if it does not exist, a parent
    /// is missing or its parents form a cycle.
    #[must_use]
    pub fn folder_depth(&self, id: FolderId) -> Option<usize> {
        let chain = self.folder_ancestry(id);
        let top = self.folders.get(chain.last()?)?;
        top.parent.is_none().then_some(chain.len())
    }

    /// The `/`-separated names from the top level down to `id` (`B-roll/Drone`), or `None` if
    /// it does not exist or is not reachable from the top level.
    #[must_use]
    pub fn folder_path(&self, id: FolderId) -> Option<String> {
        self.folder_depth(id)?;
        let chain = self.folder_ancestry(id);
        let names: Vec<&str> = chain
            .iter()
            .rev()
            .filter_map(|f| self.folders.get(f).map(|f| f.name.as_str()))
            .collect();
        Some(names.join("/"))
    }

    /// Whether `id` is `ancestor` or a folder inside it (at any depth).
    #[must_use]
    pub fn folder_is_within(&self, id: FolderId, ancestor: FolderId) -> bool {
        self.folder_ancestry(id).contains(&ancestor)
    }

    /// The folders of each parent (`None`: the top level), each list in ID order. Parents
    /// without subfolders are absent.
    #[must_use]
    pub fn folder_children(&self) -> BTreeMap<Option<FolderId>, Vec<FolderId>> {
        let mut out: BTreeMap<Option<FolderId>, Vec<FolderId>> = BTreeMap::new();
        for folder in self.folders.values() {
            out.entry(folder.parent).or_default().push(folder.id);
        }
        out
    }

    /// `root` and every folder inside it, parents before their children (depth first, siblings
    /// in ID order); empty if `root` does not exist.
    #[must_use]
    pub fn folder_subtree(&self, root: FolderId) -> Vec<FolderId> {
        if !self.folders.contains_key(&root) {
            return Vec::new();
        }
        let children = self.folder_children();
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        let mut stack = vec![root];
        while let Some(f) = stack.pop() {
            if !seen.insert(f) {
                continue;
            }
            out.push(f);
            if let Some(kids) = children.get(&Some(f)) {
                stack.extend(kids.iter().rev());
            }
        }
        out
    }

    /// How many levels the subtree of `root` spans: 1 for a folder without subfolders, 0 if it
    /// does not exist.
    #[must_use]
    pub fn folder_height(&self, root: FolderId) -> usize {
        if !self.folders.contains_key(&root) {
            return 0;
        }
        let children = self.folder_children();
        let mut best = 0;
        let mut seen = BTreeSet::new();
        let mut stack = vec![(root, 1_usize)];
        while let Some((f, level)) = stack.pop() {
            if !seen.insert(f) {
                continue;
            }
            best = best.max(level);
            if let Some(kids) = children.get(&Some(f)) {
                stack.extend(kids.iter().map(|k| (*k, level + 1)));
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> (Project, [FolderId; 4]) {
        let ids = [1, 2, 3, 4].map(FolderId::from_u128);
        let mut doc = Project::new("Folders");
        for (id, name, parent) in [
            (ids[0], "B-roll", None),
            (ids[1], "Drone", Some(ids[0])),
            (ids[2], "Day 1", Some(ids[1])),
            (ids[3], "Music", None),
        ] {
            doc.folders.insert(id, MediaFolder::new(id, name, parent));
        }
        (doc, ids)
    }

    #[test]
    fn paths_depths_and_subtrees() {
        let (doc, [broll, drone, day, music]) = tree();
        assert_eq!(doc.folder_path(day).as_deref(), Some("B-roll/Drone/Day 1"));
        assert_eq!(doc.folder_depth(day), Some(3));
        assert_eq!(doc.folder_depth(music), Some(1));
        assert_eq!(doc.folder_subtree(broll), vec![broll, drone, day]);
        assert_eq!(doc.folder_height(broll), 3);
        assert_eq!(doc.folder_height(music), 1);
        assert_eq!(doc.folder_height(FolderId::from_u128(9)), 0);
        assert!(doc.folder_is_within(day, broll));
        assert!(doc.folder_is_within(broll, broll));
        assert!(!doc.folder_is_within(broll, day));
        assert_eq!(doc.folder_children()[&None], vec![broll, music]);
    }

    #[test]
    fn cycles_and_missing_parents_have_no_depth() {
        let (mut doc, [broll, drone, day, _]) = tree();
        doc.folders.get_mut(&broll).unwrap().parent = Some(day);
        assert_eq!(doc.folder_depth(drone), None);
        assert_eq!(doc.folder_path(drone), None);
        assert_eq!(doc.folder_ancestry(drone), vec![drone, broll, day]);
        assert_eq!(doc.folder_subtree(broll).len(), 3);
        doc.folders.get_mut(&broll).unwrap().parent = Some(FolderId::from_u128(99));
        assert_eq!(doc.folder_depth(day), None);
    }

    #[test]
    fn names() {
        assert_eq!(folder_name_problem("Day 1"), None);
        assert_eq!(folder_name_problem("日本語 ✂"), None);
        assert!(folder_name_problem("").is_some());
        assert!(folder_name_problem("  ").is_some());
        assert!(folder_name_problem("a/b").is_some());
        assert!(folder_name_problem("a\tb").is_some());
        assert!(folder_name_problem(&"x".repeat(256)).is_some());
        assert_eq!(folder_name_problem(&"é".repeat(255)), None);
        assert_eq!(media_name_problem("Take 1/2"), None);
        assert!(media_name_problem("a\nb").is_some());
        assert_eq!(folder_name_key("B-Roll"), folder_name_key("b-rOLL"));
        assert_ne!(folder_name_key("Café"), folder_name_key("Cafe"));
    }
}
