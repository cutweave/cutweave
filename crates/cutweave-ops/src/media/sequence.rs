// SPDX-License-Identifier: GPL-3.0-or-later
//! `media.import_sequence` (WP-2F.05c, ADR-0035): a numbered run of pictures imported as one
//! asset that plays like video.
//!
//! - **Detection** ([`detect`]). Given one file of a run, the run is every file of the same
//!   directory whose name differs only in the number at the end (same extension). Given a
//!   directory, it is the directory's one run of pictures. Fewer than two files is `Invalid`.
//!   `media.import` never does this: one picture stays one still.
//! - **Gaps.** Numbers without a file are kept in the asset's `missing` and named in
//!   `warnings`; a missing frame shows the picture before it.
//! - **Probe only.** One file is read: the first. The media backend gets the run as a locator
//!   (`ImageSequence::locator`), computes the frame index and decodes any frame directly.
//! - **Frame rate.** `fps`, else the frame rate of the sequence the editor shows
//!   ([`ImportCtx::sequence_fps`]), else 30. It is stored on the asset.

use std::collections::BTreeMap;

use cutweave_commands::{CmdError, EngineCommand, command, wire};
use cutweave_jobs::JobId;
use cutweave_model::{
    Extra, FolderId, FrameRate, ImageSequence, MAX_IMAGE_SEQUENCE_FRAMES, MediaId, MediaKind,
};

use super::{
    AnalysisInput, ImportCtx, ImportProxy, MediaProbe, MediaRegister, OUTSIDE_ROOTS_HINT,
    ProbeError, policy,
};
use crate::util::check_abs_path;

/// The extensions a directory is searched for (a file you name yourself may have any).
const PICTURE_EXTENSIONS: [&str; 10] = [
    "png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp", "exr", "dpx", "tga",
];

/// A run of numbered files found on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DetectedRun {
    /// Absolute path of the first file.
    pub first_file: String,
    /// The file name with the counter as `%0Nd` or `%d`.
    pub pattern: String,
    /// The first number.
    pub first: u32,
    /// The numbers covered.
    pub count: u32,
    /// The numbers in between that have no file.
    pub missing: Vec<u32>,
}

/// A file name split at the number its stem ends with: `shot_0098.png` gives
/// `("shot_", "0098", ".png")`. `None` when the stem does not end with a digit.
fn split_name(name: &str) -> Option<(&str, &str, &str)> {
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    };
    let digits_at = stem
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_ascii_digit())
        .last()
        .map(|(i, _)| i)?;
    Some((&stem[..digits_at], &stem[digits_at..], ext))
}

/// How a counter is written: zero-padded to a width, or plain.
fn written(number: u32, width: Option<usize>) -> String {
    match width {
        Some(width) => format!("{number:0width$}"),
        None => number.to_string(),
    }
}

fn is_padded(digits: &str) -> bool {
    digits.len() > 1 && digits.starts_with('0')
}

/// The numbers of one run: the counters in `digits` that are written the same way (all
/// zero-padded to one width, or all plain), ascending without repeats, with that width.
/// `anchor` is the counter of the file the caller named; it is always in the result.
fn numbers_of(digits: &[&str], anchor: Option<&str>) -> (Vec<u32>, Option<usize>) {
    let parsed: Vec<(&str, u32)> = digits
        .iter()
        .filter_map(|d| Some((*d, d.parse::<u32>().ok()?)))
        .filter(|(_, n)| *n < i32::MAX.unsigned_abs())
        .collect();
    let keep = |width: Option<usize>| -> Vec<u32> {
        let mut numbers: Vec<u32> = parsed
            .iter()
            .filter(|(d, n)| written(*n, width) == *d)
            .map(|(_, n)| *n)
            .collect();
        numbers.sort_unstable();
        numbers.dedup();
        numbers
    };
    let padded_width = match anchor {
        Some(a) if is_padded(a) => Some(a.len()),
        _ => parsed
            .iter()
            .filter(|(d, _)| is_padded(d))
            .min_by_key(|(_, n)| *n)
            .map(|(d, _)| d.len()),
    };
    if let Some(width) = padded_width {
        let numbers = keep(Some(width));
        let has_anchor = anchor.is_none_or(|a| {
            a.parse::<u32>()
                .is_ok_and(|n| numbers.binary_search(&n).is_ok() && written(n, Some(width)) == a)
        });
        if has_anchor {
            return (numbers, Some(width));
        }
    }
    (keep(None), None)
}

fn parent_and_name(path: &str) -> (&str, &str) {
    let cut = path.rfind(['/', '\\']).map_or(0, |i| i + 1);
    path.split_at(cut)
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() || dir.ends_with(['/', '\\']) {
        format!("{dir}{name}")
    } else if dir.contains('\\') && !dir.contains('/') {
        format!("{dir}\\{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// The run `path` stands for: `path` is one file of it, or the directory that holds it.
pub(crate) fn detect(prober: &dyn MediaProbe, path: &str) -> Result<DetectedRun, CmdError> {
    let invalid = |msg: String| CmdError::invalid("/path", msg);
    let (dir, prefix, ext, digits, anchor) = if let Some(names) = prober.list_files(path) {
        // A directory: its one run of pictures.
        let mut runs: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
        for name in &names {
            let Some((prefix, digits, ext)) = split_name(name) else {
                continue;
            };
            let picture = ext
                .strip_prefix('.')
                .is_some_and(|e| PICTURE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()));
            if picture {
                runs.entry((prefix.to_owned(), ext.to_owned()))
                    .or_default()
                    .push(digits.to_owned());
            }
        }
        runs.retain(|_, digits| digits.len() >= 2);
        let mut runs = runs.into_iter();
        let Some(((prefix, ext), digits)) = runs.next() else {
            return Err(invalid(format!(
                "`{path}` holds no numbered run of pictures (two or more files such as \
                 shot_0001.png, shot_0002.png)"
            )));
        };
        if let Some(((other, other_ext), _)) = runs.next() {
            return Err(invalid(format!(
                "`{path}` holds more than one numbered run (`{prefix}…{ext}` and \
                 `{other}…{other_ext}`); give one file of the run you want instead"
            )));
        }
        (path.to_owned(), prefix, ext, digits, None)
    } else {
        // One file of the run.
        let (dir, name) = parent_and_name(path);
        let names = prober
            .list_files(dir)
            .filter(|names| names.iter().any(|n| n == name))
            .ok_or_else(|| invalid(format!("no readable file at `{path}`")))?;
        let Some((prefix, anchor, ext)) = split_name(name) else {
            return Err(invalid(format!(
                "`{name}` has no number at the end of its name, so it is not part of a numbered \
                 run (such as shot_0001.png)"
            )));
        };
        let digits: Vec<String> = names
            .iter()
            .filter_map(|n| split_name(n))
            .filter(|(p, _, e)| *p == prefix && *e == ext)
            .map(|(_, d, _)| d.to_owned())
            .collect();
        (
            dir.to_owned(),
            prefix.to_owned(),
            ext.to_owned(),
            digits,
            Some(anchor.to_owned()),
        )
    };
    let digits: Vec<&str> = digits.iter().map(String::as_str).collect();
    let (numbers, width) = numbers_of(&digits, anchor.as_deref());
    let (Some(&first), Some(&last)) = (numbers.first(), numbers.last()) else {
        return Err(invalid(format!("`{path}` is not part of a numbered run")));
    };
    if numbers.len() < 2 {
        return Err(invalid(format!(
            "an image sequence needs two or more numbered pictures, and `{path}` is the only one \
             of its run; import it as a still with media.import instead"
        )));
    }
    let count = last - first + 1;
    if count > MAX_IMAGE_SEQUENCE_FRAMES {
        return Err(invalid(format!(
            "the run covers {count} numbers; one image sequence takes at most \
             {MAX_IMAGE_SEQUENCE_FRAMES}"
        )));
    }
    let present = u32::try_from(numbers.len()).unwrap_or(u32::MAX);
    if count - present > present {
        return Err(invalid(format!(
            "more numbers are missing than present between {first} and {last} ({present} files \
             for {count} numbers), so this is not one run; import the pictures as stills, or \
             renumber them"
        )));
    }
    let mut missing = Vec::with_capacity((count - present) as usize);
    let mut expected = first;
    for &n in &numbers {
        missing.extend(expected..n);
        expected = n + 1;
    }
    let first_digits = written(first, width);
    Ok(DetectedRun {
        first_file: join(&dir, &format!("{prefix}{first_digits}{ext}")),
        pattern: ImageSequence::make_pattern(&prefix, &first_digits, &ext),
        first,
        count,
        missing,
    })
}

/// "Pictures 120 to 121 are missing": one sentence per range of missing numbers.
fn gap_warnings(run: &ImageSequence) -> Vec<String> {
    run.missing_ranges()
        .into_iter()
        .map(|(a, b)| {
            let what = if a == b {
                format!("Picture {a} is")
            } else {
                format!("Pictures {a} to {b} are")
            };
            format!("{what} missing: the picture before shows instead.")
        })
        .collect()
}

/// Result of `media.import_sequence`.
#[wire]
pub struct MediaImportSequenceOutput {
    /// The asset; an existing one if the same run was imported before.
    pub media_id: MediaId,
    /// `image_sequence`.
    pub kind: MediaKind,
    /// `true` when the run was already in the project and nothing was added.
    pub duplicate: bool,
    /// Absolute path of the first picture of the run.
    pub path: String,
    /// The run as stored on the asset: pattern, first number, count, frame rate and the
    /// numbers that have no file.
    pub image_sequence: ImageSequence,
    /// One sentence per stretch of missing numbers; empty when the run has no gaps.
    pub warnings: Vec<String>,
    /// Background analysis jobs queued for the new asset (wait with `job.wait`).
    pub job_ids: Vec<JobId>,
    /// Whether a proxy was queued for it, or is already in the cache.
    pub proxy: ImportProxy,
}

/// Imports a numbered run of pictures (`shot_0001.png`, `shot_0002.png`, …) as one asset that
/// plays like video (ADR-0035). `path` is one file of the run, or the folder that holds it:
/// the run is every file of that folder whose name differs only in the number at its end.
/// Fewer than two pictures is `Invalid` (use `media.import` for a still; it never guesses a
/// sequence). Only the first picture is read, so a long run imports at once; the files are
/// referenced, not copied.
///
/// Numbers without a file are allowed: they are listed in `image_sequence.missing`, named in
/// `warnings`, and each shows the picture before it. The frame rate is `fps`, else the frame
/// rate of the sequence the editor shows, else 30; it is stored on the asset. The asset goes
/// on video tracks only (it has no sound). One undo removes it. Agents and headless callers
/// may only import from the project folder and user-approved folders.
#[command(
    name = "media.import_sequence",
    summary = "Import a numbered run of pictures as one image sequence that plays like video",
    engine,
    undoable = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaImportSequence {
    /// Absolute path of one picture of the run, or of the folder that holds the run.
    pub path: String,
    /// Frames per second, as a fraction (`{"num": 24000, "den": 1001}`). Default: the frame
    /// rate of the sequence the editor shows, 30 without one.
    pub fps: Option<FrameRate>,
    /// Bin folder (default: top level).
    pub folder: Option<FolderId>,
}

fn probe_failure(error: &ProbeError) -> CmdError {
    CmdError::invalid("/path", format!("the run cannot be imported: {error}"))
}

impl EngineCommand for MediaImportSequence {
    type Context = ImportCtx;
    type Output = MediaImportSequenceOutput;

    fn run(&self, ctx: &mut ImportCtx) -> Result<MediaImportSequenceOutput, CmdError> {
        check_abs_path("/path", &self.path)?;
        if let Some(roots) = &ctx.policy.allowed_roots
            && !roots.allows(&self.path)
        {
            return Err(CmdError::invalid(
                "/path",
                format!(
                    "`{}` is outside the folders you may import from (the project folder and \
                     the folders the user allowed); {OUTSIDE_ROOTS_HINT}",
                    self.path
                ),
            ));
        }
        let found = detect(ctx.prober.as_ref(), &self.path)?;
        let run = ImageSequence {
            pattern: found.pattern,
            first: found.first,
            count: found.count,
            fps: self.fps.or(ctx.sequence_fps).unwrap_or(FrameRate::FPS_30),
            missing: found.missing,
            extra: Extra::new(),
        };
        if let Some(problem) = run.problem() {
            return Err(CmdError::invalid("/path", problem));
        }
        // One probe, of the first picture, through the run's locator: the prober answers with
        // the run's duration and its own content hash.
        let locator = run.locator(&found.first_file);
        let probed = policy::probe_all(&ctx.prober, &[locator.clone()], &ctx.policy, &mut || {})
            .pop()
            .unwrap_or_else(|| Err(ProbeError::NotFound(found.first_file.clone())))
            .map_err(|e| probe_failure(&e))?;
        let register = MediaRegister {
            media_id: None,
            path: found.first_file.clone(),
            probe: probed.probe,
            content_hash: probed.content_hash,
            folder: self.folder,
            image_sequence: Some(run.clone()),
        };
        register.kind()?;
        let registered = ctx
            .sink
            .register(vec![register.clone()])
            .map_err(|e| match e {
                // The sink points at `/paths/0/...` (a batch of one).
                CmdError::Invalid { path, msg } => CmdError::Invalid {
                    path: match path.strip_prefix("/paths/0") {
                        Some("/folder") => "/folder".to_owned(),
                        _ => "/path".to_owned(),
                    },
                    msg,
                },
                other => other,
            })?
            .pop()
            .ok_or_else(|| CmdError::internal("the media sink returned no result"))?;
        let mut job_ids = Vec::new();
        let mut index_job = Vec::new();
        if registered.created
            && let Some(analyzer) = &ctx.analysis
        {
            let stages = analyzer.spawn_stages(AnalysisInput {
                media_id: registered.media_id,
                kind: registered.kind,
                path: locator,
                content_hash: register.content_hash,
                probe: register.probe,
            });
            index_job.extend(
                stages
                    .iter()
                    .filter(|(stage, _)| *stage == "index")
                    .map(|(_, id)| *id),
            );
            job_ids.extend(stages.into_iter().map(|(_, id)| id));
        }
        let proxy = ctx.proxies.as_ref().map_or(ImportProxy::None, |p| {
            p.imported(registered.media_id, &index_job)
        });
        Ok(MediaImportSequenceOutput {
            media_id: registered.media_id,
            kind: registered.kind,
            duplicate: !registered.created,
            path: found.first_file,
            warnings: gap_warnings(&run),
            image_sequence: run,
            job_ids,
            proxy,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::FakeProbe;

    fn files(dir: &str, names: &[&str]) -> FakeProbe {
        names.iter().fold(FakeProbe::new(), |probe, name| {
            let path = format!("{dir}/{name}");
            probe.with(&path, FakeProbe::image(&path))
        })
    }

    fn found(probe: &FakeProbe, path: &str) -> DetectedRun {
        detect(probe, path).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    fn refused(probe: &FakeProbe, path: &str) -> String {
        match detect(probe, path) {
            Err(e) => e.message().to_owned(),
            Ok(run) => panic!("{path} was detected as {run:?}"),
        }
    }

    #[test]
    fn names_split_at_the_number_their_stem_ends_with() {
        assert_eq!(split_name("shot_0098.png"), Some(("shot_", "0098", ".png")));
        assert_eq!(split_name("0098.png"), Some(("", "0098", ".png")));
        assert_eq!(split_name("v2_take7"), Some(("v2_take", "7", "")));
        assert_eq!(split_name("a.b.12.exr"), Some(("a.b.", "12", ".exr")));
        assert_eq!(split_name("日本 7.png"), Some(("日本 ", "7", ".png")));
        assert_eq!(split_name("still.png"), None);
        assert_eq!(split_name("shot_01_final.png"), None);
        assert_eq!(split_name(".5"), Some((".", "5", "")));
    }

    #[test]
    fn one_file_of_a_run_finds_the_whole_run() {
        let probe = files(
            "/r",
            &[
                "shot_0098.png",
                "shot_0099.png",
                "shot_0100.png",
                "shot_0101.png",
                // Not of the run: another extension, another prefix, no number, another width.
                "shot_0102.jpg",
                "shots_0102.png",
                "shot_.png",
                "shot_102.png",
            ],
        );
        for path in ["/r/shot_0100.png", "/r/shot_0098.png", "/r/shot_0101.png"] {
            assert_eq!(
                found(&probe, path),
                DetectedRun {
                    first_file: "/r/shot_0098.png".into(),
                    pattern: "shot_%04d.png".into(),
                    first: 98,
                    count: 4,
                    missing: vec![],
                },
                "{path}"
            );
        }
    }

    #[test]
    fn plain_counters_and_counters_that_outgrow_their_padding() {
        let plain = files("/r", &["f8.png", "f9.png", "f10.png", "f11.png"]);
        let run = found(&plain, "/r/f10.png");
        assert_eq!(
            (run.pattern.as_str(), run.first, run.count),
            ("f%d.png", 8, 4)
        );
        assert_eq!(run.first_file, "/r/f8.png");
        let grown = files("/r", &["f998.png", "f999.png", "f1000.png"]);
        let run = found(&grown, "/r/f1000.png");
        assert_eq!(
            (run.pattern.as_str(), run.first, run.count),
            ("f%d.png", 998, 3)
        );
        // A counter that outgrew its padding belongs to the padded run (`%03d` writes 100).
        let padded = files("/r", &["f098.png", "f099.png", "f100.png", "f0101.png"]);
        let run = found(&padded, "/r/f100.png");
        assert_eq!(run.pattern, "f%03d.png");
        assert_eq!((run.first, run.count), (98, 3));
        assert_eq!(written(1000, Some(3)), "1000");
        // A percent sign in the name is escaped in the pattern.
        let percent = files("/r", &["50%_1.png", "50%_2.png"]);
        assert_eq!(found(&percent, "/r/50%_2.png").pattern, "50%%_%d.png");
    }

    #[test]
    fn gaps_are_reported_as_missing_numbers() {
        let probe = files(
            "/r",
            &["f_01.png", "f_02.png", "f_05.png", "f_06.png", "f_08.png"],
        );
        let run = found(&probe, "/r/f_05.png");
        assert_eq!((run.first, run.count), (1, 8));
        assert_eq!(run.missing, [3, 4, 7]);
        let seq = ImageSequence {
            pattern: run.pattern,
            first: run.first,
            count: run.count,
            fps: FrameRate::FPS_30,
            missing: run.missing,
            extra: Extra::new(),
        };
        assert_eq!(
            gap_warnings(&seq),
            [
                "Pictures 3 to 4 are missing: the picture before shows instead.",
                "Picture 7 is missing: the picture before shows instead.",
            ]
        );
    }

    #[test]
    fn a_folder_stands_for_its_one_run_of_pictures() {
        let probe = files(
            "/r",
            &[
                "a_1.png",
                "a_2.png",
                "a_3.png",
                "notes1.txt",
                "notes2.txt",
                "cover.png",
            ],
        );
        let run = found(&probe, "/r");
        assert_eq!(
            (run.pattern.as_str(), run.first, run.count),
            ("a_%d.png", 1, 3)
        );
        assert_eq!(run.first_file, "/r/a_1.png");
        let two = files("/r", &["a_1.png", "a_2.png", "b_1.png", "b_2.png"]);
        assert!(refused(&two, "/r").contains("more than one numbered run"));
        let none = files("/r", &["a.png", "b_1.png", "notes1.txt", "notes2.txt"]);
        assert!(refused(&none, "/r").contains("no numbered run"));
    }

    #[test]
    fn fewer_than_two_pictures_and_files_without_a_number_are_refused() {
        let probe = files("/r", &["f_01.png", "still.png", "g_1.png", "g_900.png"]);
        assert!(refused(&probe, "/r/f_01.png").contains("two or more"));
        assert!(refused(&probe, "/r/still.png").contains("no number at the end"));
        assert!(refused(&probe, "/r/f_02.png").contains("no readable file"));
        assert!(refused(&probe, "/nowhere/f_01.png").contains("no readable file"));
        assert!(refused(&probe, "/r/g_1.png").contains("more numbers are missing than present"));
    }

    #[test]
    fn windows_paths_keep_their_separator() {
        let probe = FakeProbe::new()
            .with("D:\\r\\f1.png", FakeProbe::image("a"))
            .with("D:\\r\\f2.png", FakeProbe::image("b"));
        assert_eq!(found(&probe, "D:\\r\\f2.png").first_file, "D:\\r\\f1.png");
        assert_eq!(found(&probe, "D:\\r").first_file, "D:\\r\\f1.png");
    }
}
