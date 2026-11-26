// SPDX-License-Identifier: GPL-3.0-or-later
//! The engine's part of `project.collect` (WP-2F.04; arch §17): the disk facts and the project
//! format that `cutweave-ops` does not know ([`CollectHost`]).
//!
//! The copy's document is written with [`ProjectStore::create`], the same code that makes any
//! new project (so its relative media paths are computed for the new folder), and the store is
//! closed at once: the collected project is not open anywhere.

use std::path::{Path, PathBuf};

use cutweave_model::Project;
use cutweave_ops::project::collect::CollectHost;

use crate::export::FreeSpaceProbe;
use crate::project_io::{DOCUMENT_EXTENSION, ProjectStore};

/// The engine's [`CollectHost`].
pub(crate) struct EngineCollectHost {
    /// Reads the free bytes of a drive (the probe exports use).
    pub(crate) free_space: FreeSpaceProbe,
}

impl CollectHost for EngineCollectHost {
    fn free_space(&self, folder: &Path) -> Option<u64> {
        // The folder may not exist yet: ask about its nearest existing parent.
        let mut at = Some(folder);
        while let Some(path) = at {
            if path.exists() {
                return (self.free_space)(path);
            }
            at = path.parent();
        }
        None
    }

    fn holds_project(&self, folder: &Path) -> bool {
        std::fs::read_dir(folder).is_ok_and(|entries| {
            entries.filter_map(Result::ok).any(|e| {
                e.path()
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case(DOCUMENT_EXTENSION))
            })
        })
    }

    fn write_project(&self, folder: &Path, doc: &Project) -> Result<PathBuf, String> {
        let store = ProjectStore::create(folder, doc).map_err(|e| e.to_string())?;
        let document = store.document_path().to_path_buf();
        store.close().map_err(|e| e.to_string())?;
        Ok(document)
    }
}
