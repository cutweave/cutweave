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
use cutweave_model::{FolderId, MAX_FOLDER_DEPTH, folder_name_key};

use super::{
    MediaRegister, MediaRegistered, by_hash,
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
        if let Some(folder) = self.folder {
            find_folder(ctx.doc(), folder)?;
        }
        for (i, entry) in self.entries.iter().enumerate() {
            let mut register = entry.register.clone();
            register.folder = None;
            register.validate(ctx).map_err(|e| at(i, e))?;
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<MediaTreeRegistered, CmdError> {
        self.validate(&tx.query_ctx())?;
        // The folder of each directory chain made or found so far, keyed by the chain's names.
        let mut made: HashMap<Vec<String>, Option<FolderId>> = HashMap::new();
        let mut folders = 0_u32;
        let mut results = Vec::with_capacity(self.entries.len());
        for (i, entry) in self.entries.iter().enumerate() {
            let known = by_hash(tx.doc(), &entry.register.content_hash).is_some();
            let mut register = entry.register.clone();
            register.folder = if known || entry.dirs.is_empty() {
                self.folder
            } else {
                let mut parent = self.folder;
                let mut chain = Vec::with_capacity(entry.dirs.len());
                for name in &entry.dirs {
                    chain.push(folder_name_key(name));
                    parent = match made.get(&chain) {
                        Some(folder) => *folder,
                        None => {
                            let folder = if depth_of(tx.doc(), parent) >= MAX_FOLDER_DEPTH {
                                parent
                            } else {
                                folders += 1;
                                let created = MediaCreateFolder {
                                    parent,
                                    name: Some(name.clone()),
                                    on_conflict: Some(OnConflict::Rename),
                                }
                                .apply(tx)
                                .map_err(|e| at(i, e))?;
                                Some(created.folder_id)
                            };
                            made.insert(chain.clone(), folder);
                            folder
                        }
                    };
                }
                parent
            };
            results.push(register.apply(tx).map_err(|e| at(i, e))?);
        }
        Ok(MediaTreeRegistered { results, folders })
    }
}
