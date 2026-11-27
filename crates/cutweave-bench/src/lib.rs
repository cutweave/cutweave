// SPDX-License-Identifier: GPL-3.0-or-later
//! Benchmark fixtures, the engine benchmark suite (WP-2J.01) and the crash-injection harness
//! (WP-2J.05).
//!
//! - [`fixture`]: the seeded projects **P-400**, **P-5K** and **P-short**, built through the
//!   registered commands (`cargo xtask gen-bench-project`).
//! - [`media`]: the small synthetic source files, made with the FFmpeg bundle when a
//!   benchmark has to decode something.
//! - [`probes`]: the measurements (`cargo bench -p cutweave-bench`), over [`harness`] and
//!   [`workload`]; [`result`] holds the result format and the budget comparison, [`cli`] the
//!   commands of `cutweave-bench` and `cargo xtask gen-bench-project`.
//!
//! - [`crash`]: the crash harness (`cutweave-bench crash`): a real engine process is killed
//!   during seeded editing work, restarted, and its recovery compared with a reference
//!   engine that was never killed.
//!
//! The UI benchmarks (WP-2J.02) and the playback benchmarks (WP-2J.03) open the same fixture
//! folders and read the same [`fixture::FixtureManifest`].

pub mod cli;
pub mod crash;
pub mod fixture;
pub mod harness;
pub mod media;
pub mod probes;
pub mod result;
pub mod stats;
pub mod workload;

use std::path::{Path, PathBuf};

/// The checkout this crate was built in (`bench/budgets.toml`, `bench-results/`).
#[must_use]
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map_or_else(PathBuf::new, Path::to_path_buf)
}

/// What can go wrong generating a fixture or running a probe.
#[derive(Debug, thiserror::Error)]
pub enum BenchError {
    /// The fixture spec cannot be generated.
    #[error("bad fixture spec: {0}")]
    Spec(String),
    /// A command or query failed.
    #[error("`{command}` failed: {message}")]
    Command {
        /// The command.
        command: String,
        /// Its error.
        message: String,
    },
    /// A file operation failed.
    #[error("cannot {what} {path}: {source}")]
    Io {
        /// What was attempted.
        what: String,
        /// On which path.
        path: String,
        /// The error.
        #[source]
        source: std::io::Error,
    },
    /// Anything else.
    #[error("{0}")]
    Other(String),
}

impl BenchError {
    pub(crate) fn command(name: &str, error: &dyn std::fmt::Display) -> BenchError {
        BenchError::Command {
            command: name.to_owned(),
            message: error.to_string(),
        }
    }

    pub(crate) fn io(what: &str, path: &Path, source: std::io::Error) -> BenchError {
        BenchError::Io {
            what: what.to_owned(),
            path: path.display().to_string(),
            source,
        }
    }
}
