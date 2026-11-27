// SPDX-License-Identifier: GPL-3.0-or-later
//! Copy on import (WP-2F.04c): `media.import{copy_to_project}` copies each new file into the
//! open project's `media/` folder before registering it, with the copier of `project.collect`
//! (through `<name>.part`, compared byte for byte, then renamed) and its layout
//! (`media/<source folder name>/<file>`, ` (2)` for a taken name).
//!
//! A file that is already inside the project folder is left where it is, and so is a file
//! whose content the project already has (it is a duplicate and gets no second copy). A file
//! found inside an imported folder keeps its place in that folder's tree under `media/`.
//!
//! `media.import_sequence{copy_to_project}` copies an image sequence the way `project.collect`
//! does (ADR-0035): every picture of the run into one folder, under its own file name, so the
//! asset's pattern finds them ([`copy_run_into_project`]).

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use cutweave_commands::CmdError;
use cutweave_model::ImageSequence;

use super::{
    CollectProgress, HEADROOM, MEDIA_FOLDER, already_copied, copy_verified, file_name, inside,
    numbered, numbered_folder, parent_name, picture_names,
};

/// Reads the free bytes of the drive that holds a folder, or of its nearest existing parent
/// (`None`: cannot tell). The engine answers with [`CollectHost::free_space`](super::CollectHost).
pub type FreeSpace = Arc<dyn Fn(&Path) -> Option<u64> + Send + Sync>;

/// The open project's folder, for copying imported files into it.
#[derive(Clone)]
pub struct ProjectFolder {
    /// Absolute path of the folder that holds the project document.
    pub root: String,
    /// The free space of a drive.
    pub free_space: FreeSpace,
    /// The content hashes of the media the project already has.
    pub known_hashes: HashSet<String>,
}

impl std::fmt::Debug for ProjectFolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectFolder")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

/// One probed file of an import.
#[derive(Clone, Copy, Debug)]
pub struct ImportedFile<'a> {
    /// Where the file is.
    pub from: &'a str,
    /// Its content hash.
    pub content_hash: &'a str,
    /// For a file found inside an imported folder: the names of the folders from the imported
    /// one down to the file's. Empty for a file named on its own.
    pub dirs: &'a [String],
}

/// What copy on import did with one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CopyOutcome {
    /// Nothing to copy: the file is inside the project folder, or its content is in the
    /// project already. Register it where it is.
    Stays,
    /// The file is in the project folder now, at this path (copied by this call, or a
    /// complete copy was there already).
    At(String),
    /// The copy failed; the file must not be registered.
    Failed(String),
}

/// A synchronous copy: never cancelled, progress is not announced.
struct Quiet;

impl CollectProgress for Quiet {
    fn cancelled(&self) -> bool {
        false
    }

    fn report(&self, _done: u64, _total: u64, _file: &str) {}
}

/// A file name or folder name that is safe under `media/`: no separators, never `.` or `..`.
fn safe_part(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| if matches!(c, '/' | '\\') { '_' } else { c })
        .collect();
    let clean = clean.trim().trim_end_matches('.');
    if clean.is_empty() {
        "files".to_owned()
    } else {
        clean.to_owned()
    }
}

/// One file that needs a copy.
struct Planned {
    /// Index in the request.
    index: usize,
    to: PathBuf,
    len: u64,
    /// A complete copy is there already.
    present: bool,
}

/// Copies `files` into `<project>/media/` and says where each one is now, in request order.
///
/// # Errors
/// `Invalid` at `/copy_to_project`, before anything is copied, when the files do not fit on
/// the project's drive.
pub fn copy_into_project(
    project: &ProjectFolder,
    files: &[ImportedFile<'_>],
) -> Result<Vec<CopyOutcome>, CmdError> {
    let media = Path::new(&project.root).join(MEDIA_FOLDER);
    let mut outcomes = vec![CopyOutcome::Stays; files.len()];
    let mut planned: Vec<Planned> = Vec::new();
    // Files of this call with the same content share the first one's copy.
    let mut first_with: HashMap<&str, usize> = HashMap::new();
    let mut shared: Vec<(usize, usize)> = Vec::new();
    let mut taken: HashSet<String> = HashSet::new();
    for (index, file) in files.iter().enumerate() {
        if inside(&project.root, file.from).is_some()
            || project.known_hashes.contains(file.content_hash)
        {
            continue;
        }
        if let Some(first) = first_with.get(file.content_hash) {
            shared.push((index, *first));
            continue;
        }
        first_with.insert(file.content_hash, index);
        let len = match fs::metadata(file.from) {
            Ok(meta) if meta.is_file() => meta.len(),
            Ok(_) => {
                outcomes[index] = CopyOutcome::Failed(format!(
                    "`{}` is not a file, so it cannot be copied into the project",
                    file.from
                ));
                continue;
            }
            Err(e) => {
                outcomes[index] = CopyOutcome::Failed(format!(
                    "could not read `{}` to copy it into the project: {e}",
                    file.from
                ));
                continue;
            }
        };
        let folder = if file.dirs.is_empty() {
            media.join(parent_name(file.from))
        } else {
            file.dirs
                .iter()
                .fold(media.clone(), |p, dir| p.join(safe_part(dir)))
        };
        let name = safe_part(file_name(file.from));
        let mut n = 1;
        let (to, present) = loop {
            let candidate = folder.join(if n == 1 {
                name.clone()
            } else {
                numbered(&name, n)
            });
            n += 1;
            if !taken.insert(candidate.to_string_lossy().to_lowercase()) {
                continue;
            }
            if !candidate.exists() {
                break (candidate, false);
            }
            if already_copied(Path::new(file.from), &candidate, len) {
                break (candidate, true);
            }
        };
        planned.push(Planned {
            index,
            to,
            len,
            present,
        });
    }

    let total: u64 = planned.iter().filter(|p| !p.present).map(|p| p.len).sum();
    if total > 0
        && let Some(free) = (project.free_space)(&media)
        && free < total.saturating_add(HEADROOM)
    {
        return Err(no_space(total, free));
    }

    let mut done = 0_u64;
    for plan in planned {
        let from = Path::new(files[plan.index].from);
        let copied = plan.present
            || match copy_verified(from, &plan.to, &Quiet, &mut done, total) {
                Ok(_) => true,
                Err(e) => {
                    outcomes[plan.index] = CopyOutcome::Failed(e.to_string());
                    false
                }
            };
        if copied {
            outcomes[plan.index] = CopyOutcome::At(plan.to.to_string_lossy().into_owned());
        }
    }
    for (index, first) in shared {
        outcomes[index] = outcomes[first].clone();
    }
    Ok(outcomes)
}

/// The error of a project drive that is too full for `total` more bytes.
fn no_space(total: u64, free: u64) -> CmdError {
    CmdError::invalid(
        "/copy_to_project",
        format!(
            "not enough space in the project folder: the files take {total} bytes and the \
             drive has {free} free; free some space, or import without copying"
        ),
    )
}

/// Copies an image sequence into `<project>/media/` (ADR-0035): every picture of `run` that
/// had a file at import, from the folder of `first` (the first picture) into one folder,
/// `media/<source folder name>/`, under its own file name. A folder where one of those names
/// is taken by another file is passed over for `<folder> (2)` and so on; pictures a complete
/// copy of which is there already are not copied again. Says where the first picture is now.
///
/// The run stays where it is when it is inside the project folder, or when the project
/// already has its content (`content_hash`: it is a duplicate). When a picture cannot be
/// read or copied the answer is [`CopyOutcome::Failed`] and the run must not be registered
/// (pictures copied before the failure stay; the next import uses them).
///
/// # Errors
/// `Invalid` at `/copy_to_project`, before anything is copied, when the pictures do not fit
/// on the project's drive.
pub fn copy_run_into_project(
    project: &ProjectFolder,
    first: &str,
    run: &ImageSequence,
    content_hash: &str,
) -> Result<CopyOutcome, CmdError> {
    if inside(&project.root, first).is_some() || project.known_hashes.contains(content_hash) {
        return Ok(CopyOutcome::Stays);
    }
    let cut = first.rfind(['/', '\\']).map_or(0, |i| i + 1);
    let mut pictures: Vec<(PathBuf, String, u64)> = Vec::new();
    for name in picture_names(run) {
        let from = PathBuf::from(format!("{}{name}", &first[..cut]));
        match fs::metadata(&from) {
            Ok(meta) if meta.is_file() => pictures.push((from, name, meta.len())),
            Ok(_) => {
                return Ok(CopyOutcome::Failed(format!(
                    "`{}` is not a file, so the run cannot be copied into the project",
                    from.display()
                )));
            }
            Err(e) => {
                return Ok(CopyOutcome::Failed(format!(
                    "could not read `{}` to copy it into the project: {e}",
                    from.display()
                )));
            }
        }
    }
    let media = Path::new(&project.root).join(MEDIA_FOLDER);
    let base = parent_name(first);
    // The first folder where every name is free or holds a complete copy of its picture.
    let mut n = 1;
    let (folder, present) = loop {
        let folder = media.join(numbered_folder(&base, n));
        n += 1;
        let present: Option<Vec<bool>> = pictures
            .iter()
            .map(|(from, name, len)| {
                let to = folder.join(name);
                if to.exists() {
                    already_copied(from, &to, *len).then_some(true)
                } else {
                    Some(false)
                }
            })
            .collect();
        if let Some(present) = present {
            break (folder, present);
        }
    };
    let total: u64 = pictures
        .iter()
        .zip(&present)
        .filter(|(_, present)| !**present)
        .map(|((_, _, len), _)| *len)
        .sum();
    if total > 0
        && let Some(free) = (project.free_space)(&media)
        && free < total.saturating_add(HEADROOM)
    {
        return Err(no_space(total, free));
    }
    let mut done = 0_u64;
    for ((from, name, _), present) in pictures.iter().zip(&present) {
        if *present {
            continue;
        }
        if let Err(e) = copy_verified(from, &folder.join(name), &Quiet, &mut done, total) {
            return Ok(CopyOutcome::Failed(e.to_string()));
        }
    }
    Ok(pictures.first().map_or(CopyOutcome::Stays, |(_, name, _)| {
        CopyOutcome::At(folder.join(name).to_string_lossy().into_owned())
    }))
}
