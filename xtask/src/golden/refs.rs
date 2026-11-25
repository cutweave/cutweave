// SPDX-License-Identifier: GPL-3.0-or-later
//! The golden reference store and its update guard (WP-1O.01).
//!
//! References are lossless PNGs in Git LFS at
//! `testdata/golden/expected/<adapter key>/<project>/f<frame>.png`, one set per adapter class
//! (`AdapterInfo::key`: `dx12-warp`, `vulkan-llvmpipe`, `metal-apple-m1`, `<backend>-<gpu>`).
//! Each file stays under [`MAX_REF_BYTES`] and the whole store under [`MAX_TOTAL_BYTES`].
//!
//! **Update guard.** A reference may only change on purpose. Every adapter set that changed
//! relative to the base commit (the merge base with `main`, or `--base`) needs an entry in
//! [`UPDATES`], added in the same change, that names the set and carries its current
//! [`set_digest`] (a hash of every file's path and content). `cargo xtask golden --update
//! --i-know` writes that entry; a PNG edited, added or removed by hand (also after an update)
//! leaves the digest stale, so `cargo xtask golden --check-refs` (and every `cargo xtask golden`
//! run) fails. In CI the pull request must also carry the [`UPDATE_LABEL`] label
//! (`--pr-labels`).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest as _, Sha256};

/// Where the references live, relative to the repository root.
pub const EXPECTED: &str = "testdata/golden/expected";
/// The update log: one entry per reference change, naming the adapter set.
pub const UPDATES: &str = "testdata/golden/UPDATES.md";
/// Largest reference file.
pub const MAX_REF_BYTES: u64 = 150 * 1024;
/// Largest total of all references.
pub const MAX_TOTAL_BYTES: u64 = 10 * 1024 * 1024;
/// The pull-request label that approves a reference change in CI.
pub const UPDATE_LABEL: &str = "golden-update";

/// The file of `project`'s frame `frame` in `adapter`'s set.
pub fn reference_path(root: &Path, adapter: &str, project: &str, frame: u32) -> PathBuf {
    root.join(EXPECTED)
        .join(adapter)
        .join(project)
        .join(frame_file(frame))
}

/// `f00045.png` (what `cutweave render --frames` writes).
pub fn frame_file(frame: u32) -> String {
    format!("f{frame:05}.png")
}

/// Whether `path` holds a Git LFS pointer rather than the content.
pub fn is_lfs_pointer(path: &Path) -> bool {
    let mut head = [0u8; 48];
    std::fs::File::open(path)
        .and_then(|mut f| f.read(&mut head))
        .is_ok_and(|n| head[..n].starts_with(b"version https://git-lfs"))
}

/// The size of the content: the file's own, or the `size` a pointer records.
fn content_size(path: &Path) -> std::io::Result<u64> {
    if is_lfs_pointer(path) {
        let text = std::fs::read_to_string(path)?;
        if let Some(size) = text
            .lines()
            .find_map(|l| l.strip_prefix("size "))
            .and_then(|s| s.trim().parse().ok())
        {
            return Ok(size);
        }
    }
    Ok(std::fs::metadata(path)?.len())
}

/// The SHA-256 of a file's content, hex: a pointer's `oid` (Git LFS stores exactly that), else
/// the file's own.
fn content_sha256(path: &Path) -> Result<String, String> {
    if is_lfs_pointer(path) {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if let Some(oid) = text.lines().find_map(|l| l.strip_prefix("oid sha256:")) {
            return Ok(oid.trim().to_owned());
        }
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(hex(&Sha256::digest(&bytes)))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// The digest of `adapter`'s set: 16 hex digits of the SHA-256 over every file's path and
/// content hash, in path order (the same whether the files are LFS pointers or content). An
/// update entry records it; any later change to the set changes it.
pub fn set_digest(root: &Path, adapter: &str) -> Result<String, String> {
    let dir = root.join(EXPECTED).join(adapter);
    let mut hasher = Sha256::new();
    for f in files_under(&dir) {
        hasher.update(format!("{} {}\n", relative(&f, &dir), content_sha256(&f)?).as_bytes());
    }
    Ok(hex(&hasher.finalize())[..16].to_owned())
}

/// Replaces the LFS pointers of `adapter`'s set with their content from the local LFS store
/// (worktrees and LFS-less checkouts hold pointers). `Err` when some stay pointers (the objects
/// are not here: `git lfs pull`).
pub fn materialize(root: &Path, adapter: &str) -> Result<(), String> {
    let dir = root.join(EXPECTED).join(adapter);
    let pointers = |dir: &Path| -> Vec<PathBuf> {
        files_under(dir)
            .into_iter()
            .filter(|p| is_lfs_pointer(p))
            .collect()
    };
    if pointers(&dir).is_empty() {
        return Ok(());
    }
    let rel = format!("{EXPECTED}/{adapter}");
    let _ = git(root, &["lfs", "checkout", &rel]);
    let left = pointers(&dir);
    if left.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} reference(s) of `{adapter}` are Git LFS pointers whose objects are not in the \
             local LFS store; run `git lfs pull --include \"{rel}/**\"`",
            left.len()
        ))
    }
}

/// Every file under `dir`, sorted.
pub fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// `path` under `base`, with forward slashes (empty when it is not under `base`).
pub fn relative(path: &Path, base: &Path) -> String {
    path.strip_prefix(base).map_or_else(
        |_| String::new(),
        |p| p.to_string_lossy().replace('\\', "/"),
    )
}

/// The adapter sets present in the store, sorted.
pub fn stored_sets(root: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(root.join(EXPECTED)).map_or_else(
        |_| Vec::new(),
        |rd| {
            rd.filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        },
    );
    v.sort();
    v
}

/// Problems with the store: files that are not a known project frame (`known` holds
/// `<project>/f<frame>.png`), adapter directories that are not adapter keys, files over
/// [`MAX_REF_BYTES`], and a total over [`MAX_TOTAL_BYTES`]. Returns `(problems, files, total
/// bytes)`.
pub fn check_store(root: &Path, known: &BTreeSet<String>) -> (Vec<String>, usize, u64) {
    let base = root.join(EXPECTED);
    let mut problems = Vec::new();
    let mut total = 0u64;
    let files = files_under(&base);
    for f in &files {
        let rel = relative(f, &base);
        let mut parts = rel.splitn(2, '/');
        let (adapter, frame) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        if !is_adapter_key(adapter) {
            problems.push(format!(
                "{EXPECTED}/{rel}: `{adapter}` is not an adapter key (`<backend>-<adapter>`)"
            ));
        } else if !known.contains(frame) {
            problems.push(format!(
                "{EXPECTED}/{rel}: not a frame of a golden project (stale reference?)"
            ));
        }
        match content_size(f) {
            Ok(size) => {
                total += size;
                if size > MAX_REF_BYTES {
                    problems.push(format!(
                        "{EXPECTED}/{rel}: {size} bytes, over the {MAX_REF_BYTES}-byte limit"
                    ));
                }
            }
            Err(e) => problems.push(format!("{EXPECTED}/{rel}: {e}")),
        }
    }
    if total > MAX_TOTAL_BYTES {
        problems.push(format!(
            "{EXPECTED}: {total} bytes in all, over the {MAX_TOTAL_BYTES}-byte limit"
        ));
    }
    (problems, files.len(), total)
}

/// `<backend>-<adapter>`: lowercase ASCII letters, digits and dashes.
pub fn is_adapter_key(s: &str) -> bool {
    s.contains('-')
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Runs git in `root`; stdout on success.
pub fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// The commit the guard compares with: `explicit`, else (like `scripts/check-affected.py`) the
/// first parent on `main` and the merge base with `main` elsewhere.
pub fn base_rev(root: &Path, explicit: Option<&str>) -> Result<String, String> {
    if let Some(b) = explicit {
        return Ok(b.to_owned());
    }
    let branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch.trim() == "main" {
        return Ok("HEAD~1".into());
    }
    git(root, &["merge-base", "HEAD", "main"])
        .map(|s| s.trim().to_owned())
        .map_err(|e| format!("{e} (pass --base <rev>)"))
}

/// What the guard found.
#[derive(Debug, Default)]
pub struct GuardReport {
    /// The base commit.
    pub base: String,
    /// Changed reference files by adapter set (`A`dded, `M`odified, `D`eleted).
    pub changed: BTreeMap<String, Vec<(char, String)>>,
}

/// The update guard: reference changes since `base` need an [`UPDATES`] entry per adapter set,
/// and (when `labels` is given, as CI does) the [`UPDATE_LABEL`] label.
pub fn guard(
    root: &Path,
    explicit_base: Option<&str>,
    pr_labels: Option<&str>,
) -> Result<GuardReport, String> {
    let base = base_rev(root, explicit_base)?;
    let mut changes: Vec<(char, String)> = git(
        root,
        &[
            "diff",
            "--name-status",
            "--no-renames",
            &base,
            "--",
            EXPECTED,
        ],
    )?
    .lines()
    .filter_map(|l| {
        let (status, path) = l.split_once('\t')?;
        Some((status.chars().next()?, path.trim().to_owned()))
    })
    .collect();
    changes.extend(
        git(
            root,
            &["ls-files", "--others", "--exclude-standard", "--", EXPECTED],
        )?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| ('A', l.trim().to_owned())),
    );
    let untracked_log = !git(
        root,
        &["ls-files", "--others", "--exclude-standard", "--", UPDATES],
    )?
    .trim()
    .is_empty();
    let added_log: Vec<String> = if untracked_log {
        std::fs::read_to_string(root.join(UPDATES))
            .map_err(|e| format!("{UPDATES}: {e}"))?
            .lines()
            .map(str::to_owned)
            .collect()
    } else {
        git(root, &["diff", "--no-renames", "-U0", &base, "--", UPDATES])?
            .lines()
            .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
            .map(|l| l[1..].to_owned())
            .collect()
    };
    let labels: Option<Vec<String>> = pr_labels.map(|list| {
        list.split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect()
    });
    let by_set = group_by_set(&changes);
    let mut digests = BTreeMap::new();
    for set in by_set.keys() {
        digests.insert(set.clone(), set_digest(root, set)?);
    }
    judge_guard(&by_set, &digests, &added_log, labels.as_deref())?;
    Ok(GuardReport {
        base,
        changed: by_set,
    })
}

/// Changed files grouped by adapter set.
pub fn group_by_set(changes: &[(char, String)]) -> BTreeMap<String, Vec<(char, String)>> {
    let prefix = format!("{EXPECTED}/");
    let mut by_set: BTreeMap<String, Vec<(char, String)>> = BTreeMap::new();
    for (status, path) in changes {
        let rel = path.replace('\\', "/");
        let set = rel
            .strip_prefix(&prefix)
            .and_then(|r| r.split('/').next())
            .unwrap_or("")
            .to_owned();
        by_set.entry(set).or_default().push((*status, rel));
    }
    by_set
}

/// The words of an update-log line (backticks and spaces separate them).
fn words(line: &str) -> impl Iterator<Item = &str> {
    line.split(|c: char| c == '`' || c.is_whitespace())
        .filter(|w| !w.is_empty())
}

/// Whether `line` is an entry for `set` that records `digest`.
fn certifies(line: &str, set: &str, digest: &str) -> bool {
    let mut named = false;
    let mut digested = false;
    let mut previous = "";
    for w in words(line) {
        named |= w == set;
        digested |= previous == "digest" && w == digest;
        previous = w;
    }
    named && digested
}

/// The guard's rule (see [`guard`]): every changed set needs an added log line that names it
/// and records its current digest; with `labels` (CI), the [`UPDATE_LABEL`] label too.
pub fn judge_guard(
    by_set: &BTreeMap<String, Vec<(char, String)>>,
    digests: &BTreeMap<String, String>,
    added_log: &[String],
    labels: Option<&[String]>,
) -> Result<(), String> {
    if by_set.is_empty() {
        return Ok(());
    }
    let mut problems = Vec::new();
    for (set, files) in by_set {
        let digest = digests.get(set).map_or("", String::as_str);
        if added_log.iter().any(|l| certifies(l, set, digest)) {
            continue;
        }
        let named = added_log.iter().any(|l| words(l).any(|w| w == set));
        problems.push(format!(
            "{} reference(s) of `{set}` changed (e.g. {}) {}; references change only through \
             `cargo xtask golden --update --i-know` on that adapter, which records an entry with \
             the set's digest ({digest} now)",
            files.len(),
            files.first().map_or("", |f| f.1.as_str()),
            if named {
                format!("after their entry in {UPDATES} was written (its digest is stale)")
            } else {
                format!("without an entry naming `{set}` in {UPDATES}")
            }
        ));
    }
    if let Some(labels) = labels
        && !labels.iter().any(|l| l == UPDATE_LABEL)
    {
        problems.push(format!(
            "golden references changed ({}) but the pull request has no `{UPDATE_LABEL}` label",
            by_set.keys().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

/// Whether [`UPDATES`] already has an entry for `adapter`'s set as it is now.
pub fn is_recorded(root: &Path, adapter: &str) -> Result<bool, String> {
    let digest = set_digest(root, adapter)?;
    Ok(std::fs::read_to_string(root.join(UPDATES))
        .unwrap_or_default()
        .lines()
        .any(|l| certifies(l, adapter, &digest)))
}

/// Appends an [`UPDATES`] entry for `adapter` with the set's current digest (creating the log
/// when missing).
pub fn record_update(root: &Path, adapter: &str, line: &str) -> Result<(), String> {
    let digest = set_digest(root, adapter)?;
    let path = root.join(UPDATES);
    let mut text = std::fs::read_to_string(&path).unwrap_or_else(|_| UPDATES_HEADER.to_owned());
    if !text.ends_with('\n') {
        text.push('\n');
    }
    let _ = writeln!(
        text,
        "- {} · `{adapter}` · digest `{digest}` · {line}",
        today()
    );
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
}

const UPDATES_HEADER: &str = "# Golden reference updates\n\n\
Every change to `testdata/golden/expected/` comes with an entry here, in the same change, naming \
the adapter set it touches and the set's digest after the change. `cargo xtask golden --update \
--i-know` writes the entry (edit its reason if it says none, keep the digest). \
`cargo xtask golden --check-refs` fails on a changed set without a matching entry, and CI also \
wants the `golden-update` pull-request label. See [README.md](README.md).\n\n";

/// Today's date (UTC), `YYYY-MM-DD`.
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // Civil date from a day count (Howard Hinnant's algorithm).
    let z = i64::try_from(secs / 86_400).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed(paths: &[&str]) -> BTreeMap<String, Vec<(char, String)>> {
        let changes: Vec<(char, String)> = paths
            .iter()
            .map(|p| ('M', format!("{EXPECTED}/{p}")))
            .collect();
        group_by_set(&changes)
    }

    fn digests(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(s, d)| ((*s).to_owned(), (*d).to_owned()))
            .collect()
    }

    fn log(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|l| (*l).to_owned()).collect()
    }

    #[test]
    fn no_change_needs_nothing() {
        assert!(judge_guard(&changed(&[]), &digests(&[]), &[], Some(&[])).is_ok());
    }

    #[test]
    fn a_changed_set_needs_an_entry_with_its_current_digest() {
        let by_set = changed(&["dx12-warp/bars_100/f00000.png", "dx12-warp/x/f00001.png"]);
        assert_eq!(by_set["dx12-warp"].len(), 2);
        let now = digests(&[("dx12-warp", "0123456789abcdef")]);
        let err = judge_guard(&by_set, &now, &[], None).unwrap_err();
        assert!(err.contains("without an entry naming `dx12-warp`"), "{err}");
        // An entry for another set does not count.
        let other = log(&["- 2026-11-25 · `vulkan-llvmpipe` · digest `0123456789abcdef` · x"]);
        assert!(judge_guard(&by_set, &now, &other, None).is_err());
        // An entry written before a later hand edit has a stale digest.
        let stale = log(&["- 2026-11-25 · `dx12-warp` · digest `ffffffffffffffff` · 18 written"]);
        let err = judge_guard(&by_set, &now, &stale, None).unwrap_err();
        assert!(err.contains("its digest is stale"), "{err}");
        let fresh = log(&["- 2026-11-25 · `dx12-warp` · digest `0123456789abcdef` · 1 written"]);
        assert!(judge_guard(&by_set, &now, &fresh, None).is_ok());
    }

    #[test]
    fn ci_also_needs_the_label() {
        let by_set = changed(&["dx12-warp/bars_100/f00000.png"]);
        let now = digests(&[("dx12-warp", "0123456789abcdef")]);
        let entry = log(&["- 2026-11-25 · `dx12-warp` · digest `0123456789abcdef` · x"]);
        let err = judge_guard(&by_set, &now, &entry, Some(&["docs".to_owned()])).unwrap_err();
        assert!(err.contains("no `golden-update` label"), "{err}");
        assert!(judge_guard(&by_set, &now, &entry, Some(&[UPDATE_LABEL.to_owned()])).is_ok());
    }

    #[test]
    fn the_digest_is_the_same_for_a_file_and_its_lfs_pointer() {
        let root = std::env::temp_dir().join(format!("xtask-digest-{}", std::process::id()));
        let set = root.join(EXPECTED).join("dx12-warp").join("p");
        std::fs::create_dir_all(&set).unwrap();
        std::fs::write(set.join("f00000.png"), b"pixels").unwrap();
        let with_file = set_digest(&root, "dx12-warp").unwrap();
        let oid = hex(&Sha256::digest(b"pixels"));
        std::fs::write(
            set.join("f00000.png"),
            format!("version https://git-lfs.github.com/spec/v1\noid sha256:{oid}\nsize 6\n"),
        )
        .unwrap();
        assert_eq!(set_digest(&root, "dx12-warp").unwrap(), with_file);
        std::fs::write(set.join("f00000.png"), b"other pixels").unwrap();
        assert_ne!(set_digest(&root, "dx12-warp").unwrap(), with_file);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn adapter_keys() {
        for ok in [
            "dx12-warp",
            "vulkan-llvmpipe",
            "dx12-nvidia-geforce-rtx-3080-laptop-gpu",
        ] {
            assert!(is_adapter_key(ok), "{ok}");
        }
        for bad in ["warp", "DX12-warp", "-x", "dx12 warp"] {
            assert!(!is_adapter_key(bad), "{bad}");
        }
    }

    #[test]
    fn frame_files_have_five_digits() {
        assert_eq!(frame_file(45), "f00045.png");
        assert_eq!(
            reference_path(Path::new("r"), "dx12-warp", "p", 0),
            Path::new("r").join(EXPECTED).join("dx12-warp/p/f00000.png")
        );
    }

    #[test]
    fn pointer_sizes_count_as_content() {
        let dir = std::env::temp_dir().join(format!("xtask-refs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.png");
        std::fs::write(
            &p,
            "version https://git-lfs.github.com/spec/v1\noid sha256:00\nsize 97679\n",
        )
        .unwrap();
        assert!(is_lfs_pointer(&p));
        assert_eq!(content_size(&p).unwrap(), 97_679);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn today_is_a_date() {
        let d = today();
        assert_eq!(d.len(), 10);
        assert_eq!(&d[4..5], "-");
    }
}
