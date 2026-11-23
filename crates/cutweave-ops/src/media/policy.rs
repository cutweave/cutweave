// SPDX-License-Identifier: GPL-3.0-or-later
//! How `media.import` reads files (WP-1D.07; arch §7, §14, §18): the caller's path allowlist
//! ([`AllowedRoots`]), the per-file probe timeout and the probe parallelism
//! ([`ImportPolicy`]), and the bounded, timed probing itself.

use std::{
    collections::BTreeMap,
    panic::{self, AssertUnwindSafe},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant},
};

use super::probe::{MediaProbe, ProbeError, Probed};

/// Default per-file probe timeout (arch §7).
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// Default number of files probed at once.
pub const DEFAULT_PROBE_PARALLELISM: usize = 4;

/// The folders a caller may import from (arch §14 "filesystem allowlist", §18): the project
/// folder plus user-approved roots. The engine applies it to agent and headless callers
/// (MCP, CLI); the editor UI imports anything the user picks.
///
/// A path is allowed when it lies inside a root, compared component by component (ASCII
/// case-insensitively on Windows). Paths with `..` are refused outright. When the path and
/// the root both exist they are also compared after resolving symlinks, so a link inside a
/// root cannot reach outside it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AllowedRoots {
    roots: Vec<PathBuf>,
}

impl AllowedRoots {
    /// The given roots (absolute folders).
    #[must_use]
    pub fn new<P: Into<PathBuf>>(roots: impl IntoIterator<Item = P>) -> AllowedRoots {
        AllowedRoots {
            roots: roots.into_iter().map(Into::into).collect(),
        }
    }

    /// The roots.
    #[must_use]
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Whether `path` lies inside one of the roots.
    #[must_use]
    pub fn allows(&self, path: &str) -> bool {
        let path = Path::new(path);
        let Some(lexical) = components(path) else {
            return false;
        };
        let canonical = std::fs::canonicalize(path).ok();
        self.roots.iter().any(|root| {
            let Some(root_parts) = components(root) else {
                return false;
            };
            if !starts_with(&lexical, &root_parts) {
                return false;
            }
            match (&canonical, std::fs::canonicalize(root).ok()) {
                (Some(real), Some(real_root)) => match (components(real), components(&real_root)) {
                    (Some(r), Some(rr)) => starts_with(&r, &rr),
                    _ => false,
                },
                _ => true,
            }
        })
    }
}

/// The path's components without `.`; `None` if it has a `..`.
fn components(path: &Path) -> Option<Vec<Component<'_>>> {
    let mut out = Vec::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => return None,
            other => out.push(other),
        }
    }
    Some(out)
}

fn same(a: &Component<'_>, b: &Component<'_>) -> bool {
    if cfg!(windows) {
        a.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
    } else {
        a == b
    }
}

fn starts_with(path: &[Component<'_>], root: &[Component<'_>]) -> bool {
    !root.is_empty() && path.len() >= root.len() && path.iter().zip(root).all(|(a, b)| same(a, b))
}

/// How `media.import` treats one caller's request (part of [`ImportCtx`](super::ImportCtx)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportPolicy {
    /// The folders the caller may import from: `Some` for agent and headless callers (a path
    /// elsewhere fails the call with `Invalid` at `/paths/<i>`), `None` for the editor UI.
    pub allowed_roots: Option<AllowedRoots>,
    /// How long one file's probe may take before it is reported in `failed[]` as timed out.
    pub probe_timeout: Duration,
    /// How many files are probed at once (at least one).
    pub probe_parallelism: usize,
}

impl Default for ImportPolicy {
    fn default() -> ImportPolicy {
        ImportPolicy {
            allowed_roots: None,
            probe_timeout: DEFAULT_PROBE_TIMEOUT,
            probe_parallelism: DEFAULT_PROBE_PARALLELISM,
        }
    }
}

impl ImportPolicy {
    /// The default policy restricted to `roots` (agent and headless callers).
    #[must_use]
    pub fn restricted(roots: AllowedRoots) -> ImportPolicy {
        ImportPolicy {
            allowed_roots: Some(roots),
            ..ImportPolicy::default()
        }
    }
}

/// Probes every path with `prober`, at most `policy.probe_parallelism` at once, each bounded
/// by `policy.probe_timeout`; results come back in `paths` order.
///
/// Each probe runs on its own thread, so a prober that hangs past its timeout is reported as
/// [`ProbeError::TimedOut`] and abandoned (its thread ends whenever the prober returns; the
/// late result is dropped). A panicking prober is reported as [`ProbeError::Io`].
pub(crate) fn probe_all(
    prober: &Arc<dyn MediaProbe>,
    paths: &[String],
    policy: &ImportPolicy,
) -> Vec<Result<Probed, ProbeError>> {
    let parallel = policy.probe_parallelism.max(1);
    let mut results: Vec<Option<Result<Probed, ProbeError>>> = paths.iter().map(|_| None).collect();
    let (tx, rx) = mpsc::channel();
    let mut pending: BTreeMap<usize, Instant> = BTreeMap::new();
    let mut next = 0;
    loop {
        while pending.len() < parallel && next < paths.len() {
            let i = next;
            next += 1;
            let (prober, path, tx) = (Arc::clone(prober), paths[i].clone(), tx.clone());
            let spawned = thread::Builder::new()
                .name("cutweave-probe".into())
                .spawn(move || {
                    let result = panic::catch_unwind(AssertUnwindSafe(|| prober.probe(&path)))
                        .unwrap_or_else(|_| {
                            Err(ProbeError::Io {
                                path: path.clone(),
                                reason: "the prober crashed".into(),
                            })
                        });
                    // The import may have given up on this file already.
                    let _ = tx.send((i, result));
                });
            match spawned {
                Ok(_) => {
                    pending.insert(i, deadline(policy.probe_timeout));
                }
                Err(e) => {
                    results[i] = Some(Err(ProbeError::Io {
                        path: paths[i].clone(),
                        reason: format!("could not start probing: {e}"),
                    }));
                }
            }
        }
        let Some(&first) = pending.values().min() else {
            break;
        };
        match rx.recv_timeout(first.saturating_duration_since(Instant::now())) {
            Ok((i, result)) => {
                if pending.remove(&i).is_some() {
                    results[i] = Some(result);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                let now = Instant::now();
                let expired: Vec<usize> = pending
                    .iter()
                    .filter(|&(_, d)| *d <= now)
                    .map(|(&i, _)| i)
                    .collect();
                for i in expired {
                    pending.remove(&i);
                    results[i] = Some(Err(ProbeError::TimedOut(paths[i].clone())));
                }
            }
            // Unreachable while `tx` is alive; never loop forever.
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    results
        .into_iter()
        .zip(paths)
        .map(|(r, path)| {
            r.unwrap_or_else(|| {
                Err(ProbeError::Io {
                    path: path.clone(),
                    reason: "the probe did not report back".into(),
                })
            })
        })
        .collect()
}

fn deadline(timeout: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(timeout)
        .unwrap_or_else(|| now + Duration::from_hours(24))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_inside_a_root_are_allowed() {
        let roots = AllowedRoots::new(["/projects/film", "/media/shared"]);
        assert!(roots.allows("/projects/film/a.mp4"));
        assert!(roots.allows("/projects/film/sub/./b.mp4"));
        assert!(roots.allows("/media/shared/c.wav"));
        assert!(
            !roots.allows("/projects/filmography/a.mp4"),
            "prefix of a name"
        );
        assert!(!roots.allows("/projects/a.mp4"));
        assert!(
            !roots.allows("/projects/film/../secret.mp4"),
            "`..` is refused"
        );
        assert!(!roots.allows("relative/a.mp4"));
        assert!(!AllowedRoots::default().allows("/projects/film/a.mp4"));
        assert_eq!(roots.roots().len(), 2);
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_compare_case_insensitively_with_either_separator() {
        let roots = AllowedRoots::new(["C:\\Users\\me\\Videos"]);
        assert!(roots.allows("c:\\users\\ME\\videos\\a.mp4"));
        assert!(roots.allows("C:/Users/me/Videos/b.mp4"));
        assert!(!roots.allows("D:\\Users\\me\\Videos\\a.mp4"));
    }

    #[test]
    fn existing_files_are_checked_after_resolving_the_real_path() {
        let dir = std::env::temp_dir().join(format!("cutweave-roots-{}", std::process::id()));
        let inside = dir.join("inside");
        std::fs::create_dir_all(&inside).unwrap_or_default();
        let file = inside.join("clip.mp4");
        std::fs::write(&file, b"x").unwrap_or_default();
        let roots = AllowedRoots::new([inside.clone()]);
        assert!(roots.allows(&file.to_string_lossy()));
        let other = AllowedRoots::new([dir.join("elsewhere")]);
        assert!(!other.allows(&file.to_string_lossy()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
