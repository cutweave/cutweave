// SPDX-License-Identifier: GPL-3.0-or-later
//! `media.register_tree` (WP-2F.09, ADR-0020): registers the files of an imported OS folder
//! together with the bin folders that mirror its directories, in one transaction (one undo).
//!
//! It is the plumbing of `media.import` with `keep_structure`, hidden from agents like
//! `media.register`. A bin folder is created only when a file that is new to the project goes
//! into it, so a directory with no media gets none and importing the same tree again creates
//! nothing. A folder whose name is taken where it goes takes the next free one ("Day 1 2");
//! folders nest at most [`MAX_FOLDER_DEPTH`] levels, so what is deeper goes into the deepest
//! one that fits.

use std::collections::HashMap;

use cutweave_commands::{CmdError, Command, QueryCtx, Tx, command, wire};
use cutweave_model::{FolderId, MAX_FOLDER_DEPTH, folder_name_key, folder_name_problem};

use super::{
    MAX_IMPORT_FILES, MediaRegister, MediaRegistered, by_hash,
    folders::{MediaCreateFolder, OnConflict, depth_of, find_folder},
};

/// One file of a tree and the directories above it.
#[wire]
pub struct TreeEntry {
    /// The names of the bin folders to put the file in, outermost first, inside `folder`; empty:
    /// straight into `folder`.
    pub dirs: Vec<String>,
    /// The file. Its `folder` is ignored.
    pub register: MediaRegister,
}

/// Result of `media.register_tree`.
#[wire]
pub struct MediaTreeRegistered {
    /// One result per entry, in order.
    pub results: Vec<MediaRegistered>,
    /// How many bin folders were created.
    pub folders: u32,
}

/// Registers `entries` like `media.register` and puts each new asset into the bin folders named
/// by its `dirs`, created inside `folder` as needed. Internal plumbing of `media.import`.
#[command(
    name = "media.register_tree",
    summary = "Register probed media files into bin folders that mirror a directory tree (used by import)",
    undoable,
    mcp = "hidden",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaRegisterTree {
    /// The folder the tree goes into; absent: the top level.
    pub folder: Option<FolderId>,
    /// The files (1..=2000).
    pub entries: Vec<TreeEntry>,
}

fn at(index: usize, error: CmdError) -> CmdError {
    match error {
        CmdError::Invalid { path, msg } => CmdError::invalid(format!("/paths/{index}{path}"), msg),
        other => other,
    }
}

impl Command for MediaRegisterTree {
    type Output = MediaTreeRegistered;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        if self.entries.is_empty() || self.entries.len() > MAX_IMPORT_FILES {
            return Err(CmdError::invalid(
                "/entries",
                format!("give 1..={MAX_IMPORT_FILES} entries"),
            ));
        }
        if let Some(folder) = self.folder {
            find_folder(ctx.doc(), folder)?;
        }
        for (i, entry) in self.entries.iter().enumerate() {
            for (d, name) in entry.dirs.iter().enumerate() {
                if let Some(problem) = folder_name_problem(name.trim()) {
                    return Err(at(i, CmdError::invalid(format!("/dirs/{d}"), problem)));
                }
            }
            let mut register = entry.register.clone();
            register.folder = None;
            register.validate(ctx).map_err(|e| at(i, e))?;
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<MediaTreeRegistered, CmdError> {
        self.validate(&tx.query_ctx())?;
        let mut made = Made::default();
        let mut results = Vec::with_capacity(self.entries.len());
        for (i, entry) in self.entries.iter().enumerate() {
            let known = by_hash(tx.doc(), &entry.register.content_hash).is_some();
            let mut register = entry.register.clone();
            register.folder = if known || entry.dirs.is_empty() {
                self.folder
            } else {
                made.folder_for(tx, self.folder, &entry.dirs)
                    .map_err(|e| at(i, e))?
            };
            results.push(register.apply(tx).map_err(|e| at(i, e))?);
        }
        Ok(MediaTreeRegistered {
            results,
            folders: made.created,
        })
    }
}

/// The bin folders made so far for the directory chains of the tree.
#[derive(Default)]
struct Made {
    /// The folder of each chain (its names, case folded); `None`: the top level.
    by_chain: HashMap<Vec<String>, Option<FolderId>>,
    created: u32,
}

impl Made {
    /// The folder for the chain `dirs` below `base`, making the folders that are missing. At the
    /// deepest level a folder can have, the files stay in the folder they reached.
    fn folder_for(
        &mut self,
        tx: &mut Tx<'_>,
        base: Option<FolderId>,
        dirs: &[String],
    ) -> Result<Option<FolderId>, CmdError> {
        let mut parent = base;
        let mut chain = Vec::with_capacity(dirs.len());
        for name in dirs {
            chain.push(folder_name_key(name));
            if let Some(folder) = self.by_chain.get(&chain) {
                parent = *folder;
                continue;
            }
            if depth_of(tx.doc(), parent) < MAX_FOLDER_DEPTH {
                let created = MediaCreateFolder {
                    parent,
                    name: Some(name.clone()),
                    on_conflict: Some(OnConflict::Rename),
                }
                .apply(tx)?;
                self.created += 1;
                parent = Some(created.folder_id);
            }
            self.by_chain.insert(chain.clone(), parent);
        }
        Ok(parent)
    }
}
