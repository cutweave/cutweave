// SPDX-License-Identifier: GPL-3.0-or-later
//! Where an export writes (WP-2G.03b; arch §8): the **file collision policy**, the **name
//! template** of marker-set exports and the **disk space precheck**.
//!
//! **Collision policy** (`collision`): what happens when the output file already exists or is
//! the output of another queued or running export.
//!
//! | Policy | Existing file | File of another unfinished export |
//! |---|---|---|
//! | `fail` (default) | `Invalid` at `/path` (at `/overwrite` when neither `collision` nor `overwrite` was given: the form of the call before WP-2G.03b) | `Invalid` at `/path` |
//! | `overwrite` | replaced, and only when the export succeeds (a failed or cancelled export leaves it as it was) | `Invalid` at `/path` |
//! | `rename` | a free name is picked: `name (1).mp4`, `name (2).mp4`, … (free: no such file, no other export writing it) | same |
//!
//! `overwrite: true` is `collision: "overwrite"`; naming both with different meanings is
//! `Invalid` at `/overwrite`. The name is picked when the export is queued (the output says
//! which), never later.
//!
//! **Name template** (marker sets): `path` is an existing folder (files are named
//! `{seq}_{marker_label}_{n}.mp4`) or a `.mp4` path whose file name is the template. Tokens:
//! `{seq}` (the sequence's name), `{marker_label}` (the marker's label, `marker` when empty) and
//! `{n}` (1, 2, … in time order). Values lose the characters file names cannot hold. With more
//! than one file the template needs `{n}` or `{marker_label}`.
//!
//! **Disk check.** Before anything is queued the size of each output is estimated (the preset's
//! bitrate, or a typical bitrate for the quality, plus audio) and compared with the free space of
//! its drive (all of this call's outputs on one drive together). Less free space than needed is
//! `Invalid` at `/path`. The estimate is an upper-end guess for typical footage, not a promise.

use std::path::{Path, PathBuf};

use cutweave_commands::{CmdError, wire};
use cutweave_model::FLICKS_PER_SECOND;

use super::presets::PlanRate;
use super::{ExportPlan, ExportQuality, ExportStarter, same_path};

/// What to do when the output file exists.
#[wire]
#[derive(Copy, Default, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Collision {
    /// Pick a free name (`name (1).mp4`).
    Rename,
    /// Replace the file once the export has succeeded.
    Overwrite,
    /// Refuse (the default).
    #[default]
    Fail,
}

/// The policy a call asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    /// The policy.
    pub collision: Collision,
    /// Whether the call named a policy (`collision` or `overwrite`), not the default.
    pub explicit: bool,
}

impl Policy {
    /// The policy of `collision` and the older `overwrite` flag.
    ///
    /// # Errors
    /// `Invalid` at `/overwrite` when they disagree.
    pub fn of(collision: Option<Collision>, overwrite: Option<bool>) -> Result<Policy, CmdError> {
        match (collision, overwrite) {
            (Some(c), Some(true)) if c != Collision::Overwrite => Err(CmdError::invalid(
                "/overwrite",
                "`overwrite: true` means `collision: \"overwrite\"`; do not give another \
                 `collision` with it",
            )),
            (Some(c), _) => Ok(Policy {
                collision: c,
                explicit: true,
            }),
            (None, Some(true)) => Ok(Policy {
                collision: Collision::Overwrite,
                explicit: true,
            }),
            // `overwrite: false` is the old spelling of the default.
            (None, _) => Ok(Policy {
                collision: Collision::Fail,
                explicit: false,
            }),
        }
    }
}

// ----- naming ----------------------------------------------------------------------------------

/// The default file name template of a marker set.
pub const DEFAULT_TEMPLATE: &str = "{seq}_{marker_label}_{n}.mp4";
/// Longest value of `{seq}` or `{marker_label}`, in characters.
const MAX_TOKEN_CHARS: usize = 60;

/// A marker-set output: a folder and a file name template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    /// The folder.
    pub folder: PathBuf,
    /// The file name with `{seq}`, `{marker_label}` and `{n}`.
    pub name: String,
}

impl Template {
    /// Reads `path` as an existing folder or as a `.mp4` path whose file name is the template.
    ///
    /// # Errors
    /// `Invalid` at `/path`.
    pub fn parse(path: &str) -> Result<Template, CmdError> {
        let p = Path::new(path);
        if p.is_dir() {
            return Ok(Template {
                folder: p.to_path_buf(),
                name: DEFAULT_TEMPLATE.to_owned(),
            });
        }
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| n.to_ascii_lowercase().ends_with(".mp4"))
            .ok_or_else(|| {
                CmdError::invalid(
                    "/path",
                    format!(
                        "`{path}` must be an existing folder or an .mp4 file name template such \
                         as `{{seq}}_{{marker_label}}_{{n}}.mp4`"
                    ),
                )
            })?;
        let folder = p
            .parent()
            .filter(|f| !f.as_os_str().is_empty())
            .ok_or_else(|| CmdError::invalid("/path", format!("`{path}` has no folder")))?;
        if folder.to_string_lossy().contains('{') {
            return Err(CmdError::invalid(
                "/path",
                "the folder cannot contain a `{token}`; only the file name is a template",
            ));
        }
        Ok(Template {
            folder: folder.to_path_buf(),
            name: name.to_owned(),
        })
    }

    /// Whether the name tells files apart.
    #[must_use]
    pub fn varies(&self) -> bool {
        self.name.contains("{n}") || self.name.contains("{marker_label}")
    }

    /// The file for marker region `n` (1-based).
    ///
    /// # Errors
    /// `Invalid` at `/path` for an unknown `{token}`.
    pub fn file(&self, seq_name: &str, label: &str, n: usize) -> Result<PathBuf, CmdError> {
        let expanded = self
            .name
            .replace("{seq}", &clean(seq_name, "sequence"))
            .replace("{marker_label}", &clean(label, "marker"))
            .replace("{n}", &n.to_string());
        if expanded.contains('{') || expanded.contains('}') {
            return Err(CmdError::invalid(
                "/path",
                format!(
                    "`{}` has a token this export does not know; the tokens are {{seq}}, \
                     {{marker_label}} and {{n}}",
                    self.name
                ),
            ));
        }
        Ok(self.folder.join(expanded))
    }
}

/// `value` as part of a file name: characters files cannot hold become `_`, runs of white space
/// one space, at most [`MAX_TOKEN_CHARS`]; `fallback` when nothing is left.
#[must_use]
pub fn clean(value: &str, fallback: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in value.chars() {
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(if c.is_control() || "\\/:*?\"<>|{}".contains(c) {
            '_'
        } else {
            c
        });
    }
    let out: String = out.chars().take(MAX_TOKEN_CHARS).collect();
    let out = out.trim_end_matches(['.', ' ']).to_owned();
    if out.is_empty() {
        fallback.to_owned()
    } else {
        out
    }
}

// ----- collisions ------------------------------------------------------------------------------

/// `name (n).ext` for `path`.
fn numbered(path: &Path, n: u32) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned());
    let mut name = format!("{} ({n})", stem.unwrap_or_default());
    if let Some(ext) = ext {
        name.push('.');
        name.push_str(&ext);
    }
    path.with_file_name(name)
}

/// The file an export to `out` writes under `policy`: `out`, or a free name for `rename`.
/// `taken` are the files of the same call so far.
///
/// # Errors
/// `Invalid` as the module docs say.
pub fn settle(
    out: PathBuf,
    policy: Policy,
    taken: &[PathBuf],
    starter: &dyn ExportStarter,
) -> Result<PathBuf, CmdError> {
    let exists = |p: &Path| p.exists();
    let busy = |p: &Path| starter.output_claimed(p) || taken.iter().any(|t| same_path(t, p));
    match policy.collision {
        Collision::Rename => {
            if !exists(&out) && !busy(&out) {
                return Ok(out);
            }
            for n in 1..=9999 {
                let candidate = numbered(&out, n);
                if !exists(&candidate) && !busy(&candidate) {
                    return Ok(candidate);
                }
            }
            Err(CmdError::invalid(
                "/path",
                format!("no free name next to `{}`", out.display()),
            ))
        }
        Collision::Overwrite | Collision::Fail => {
            if busy(&out) {
                return Err(CmdError::invalid(
                    "/path",
                    format!(
                        "another queued or running export is already writing `{}`; choose another file or \
                         `collision: \"rename\"`",
                        out.display()
                    ),
                ));
            }
            if policy.collision == Collision::Fail && exists(&out) {
                let (pointer, hint) = if policy.explicit {
                    (
                        "/path",
                        "choose another file or use `collision: \"rename\"`",
                    )
                } else {
                    (
                        "/overwrite",
                        "pass `overwrite: true` to replace it or `collision: \"rename\"` to keep both",
                    )
                };
                return Err(CmdError::invalid(
                    pointer,
                    format!("`{}` already exists; {hint}", out.display()),
                ));
            }
            Ok(out)
        }
    }
}

// ----- disk space ------------------------------------------------------------------------------

/// Audio bitrate assumed by the estimate (AAC stereo), bits per second.
const AUDIO_BPS: u128 = 192_000;

/// Typical bits per pixel (in thousandths) of x264 at `crf` (an upper-end figure).
const fn bpp_milli(crf: u8) -> u128 {
    match crf {
        0..=16 => 200,
        17..=20 => 150,
        21..=23 => 100,
        24..=28 => 50,
        _ => 25,
    }
}

/// Estimated size of `plan`'s file in bytes: the video bitrate (the preset's, or a typical one
/// for its quality) plus audio, over the length, plus 5 % for the container.
#[must_use]
pub fn estimate_bytes(plan: &ExportPlan) -> u64 {
    let video_bps = match plan.rate {
        Some(PlanRate::BitrateKbps(k)) => u128::from(k) * 1000,
        other => {
            let crf = match other {
                Some(PlanRate::Crf(c)) => c,
                _ => match plan.quality {
                    ExportQuality::Low => 28,
                    ExportQuality::Medium => 23,
                    ExportQuality::High => 20,
                },
            };
            let pixels_per_sec = u128::from(plan.width)
                * u128::from(plan.height)
                * u128::from(plan.frame_rate.num())
                / u128::from(plan.frame_rate.den().max(1));
            pixels_per_sec * bpp_milli(crf) / 1000
        }
    };
    let flicks = u128::from(plan.duration.flicks().max(0).unsigned_abs());
    let bits = (video_bps + AUDIO_BPS) * flicks / u128::from(FLICKS_PER_SECOND.unsigned_abs());
    u64::try_from(bits / 8 * 105 / 100).unwrap_or(u64::MAX)
}

/// `n` bytes as "512 MB" / "3.4 GB" (decimal units).
#[must_use]
pub fn human_bytes(n: u64) -> String {
    const MB: u64 = 1_000_000;
    const GB: u64 = 1_000_000_000;
    if n >= GB {
        format!("{}.{} GB", n / GB, n % GB / (GB / 10))
    } else {
        format!("{} MB", n.div_ceil(MB))
    }
}

/// Checks that the drives of `plans` have room for them (see the module docs); a drive whose
/// free space the starter cannot tell is not checked.
///
/// # Errors
/// `Invalid` at `/path`.
pub fn check_disk(plans: &[ExportPlan], starter: &dyn ExportStarter) -> Result<(), CmdError> {
    let mut folders: Vec<(&Path, u64)> = Vec::new();
    for plan in plans {
        let Some(folder) = plan.path.parent() else {
            continue;
        };
        let need = estimate_bytes(plan);
        if let Some(entry) = folders.iter_mut().find(|(f, _)| same_path(f, folder)) {
            entry.1 = entry.1.saturating_add(need);
        } else {
            folders.push((folder, need));
        }
    }
    for (folder, need) in folders {
        if let Some(free) = starter.free_space(folder)
            && free < need
        {
            return Err(CmdError::invalid(
                "/path",
                format!(
                    "not enough disk space in `{}`: the export needs about {} and {} is free; \
                     free some space or choose another drive",
                    folder.display(),
                    human_bytes(need),
                    human_bytes(free)
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_lose_what_files_cannot_hold() {
        assert_eq!(clean("Intro: part 1/2", "x"), "Intro_ part 1_2");
        assert_eq!(clean("  a   b  ", "x"), "a b");
        assert_eq!(clean("", "marker"), "marker");
        assert_eq!(clean("...", "marker"), "marker");
        assert_eq!(clean("{n}", "x"), "_n_");
        assert_eq!(clean(&"a".repeat(200), "x").len(), MAX_TOKEN_CHARS);
    }

    #[test]
    fn numbered_names_keep_the_extension() {
        assert_eq!(
            numbered(Path::new("/o/a.mp4"), 2),
            PathBuf::from("/o/a (2).mp4")
        );
    }

    #[test]
    fn policies_combine_collision_and_overwrite() {
        let p = Policy::of(None, None).unwrap_or_else(|_| unreachable!());
        assert_eq!(p.collision, Collision::Fail);
        assert!(!p.explicit);
        assert_eq!(
            Policy::of(None, Some(true)).map(|p| p.collision),
            Ok(Collision::Overwrite)
        );
        assert!(Policy::of(Some(Collision::Rename), Some(true)).is_err());
        assert_eq!(
            Policy::of(Some(Collision::Rename), Some(false)).map(|p| p.collision),
            Ok(Collision::Rename)
        );
    }

    #[test]
    fn sizes_read_like_a_person() {
        assert_eq!(human_bytes(1_500_000_000), "1.5 GB");
        assert_eq!(human_bytes(12_000_001), "13 MB");
    }
}
