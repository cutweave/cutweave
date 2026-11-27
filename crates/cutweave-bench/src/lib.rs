// SPDX-License-Identifier: GPL-3.0-or-later
//! Benchmark fixtures and the engine benchmark suite (WP-2J.01).
//!
//! - [`fixture`]: the seeded projects **P-400**, **P-5K** and **P-short**, built through the
//!   registered commands (`cargo xtask gen-bench-project`).
//! - [`media`]: the small synthetic source files, made with the FFmpeg bundle when a
//!   benchmark has to decode something.
//!
//! The UI benchmarks (WP-2J.02) and the playback benchmarks (WP-2J.03) open the same fixture
//! folders and read the same [`fixture::FixtureManifest`].

pub mod fixture;

use std::path::Path;

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
