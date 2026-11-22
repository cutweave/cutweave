// SPDX-License-Identifier: GPL-3.0-or-later
//! Agent credentials (arch §6, ADR-016).
//!
//! - **Agent tokens** are random 256-bit secrets written to the run-file, which MCP/CLI clients
//!   read. Each maps to actor `agent:<label>` server-side; a client never asserts its actor.
//! - The **UI credential** is never accepted here: it is handed to Electron over the spawn pipe,
//!   never written to disk, and only it carries UI-only scopes. An agent token therefore can never
//!   run a `scope = "ui"` command (the engine refuses it with `forbidden`).

use std::{
    fs,
    io::{self, Write as _},
    net::SocketAddr,
    path::{Path, PathBuf},
};

use serde_json::json;

use crate::catalog::Profile;

/// Minimum accepted secret length (hex chars). 64 = 256 bits.
pub const MIN_SECRET_LEN: usize = 32;

/// One agent credential.
#[derive(Clone)]
pub struct AgentToken {
    /// Actor label (`agent:<label>`).
    pub label: String,
    /// The bearer secret.
    pub secret: String,
}

impl std::fmt::Debug for AgentToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentToken")
            .field("label", &self.label)
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl AgentToken {
    /// A fresh random token for `label`.
    pub fn generate(label: &str) -> io::Result<AgentToken> {
        Ok(AgentToken {
            label: label.to_owned(),
            secret: random_secret()?,
        })
    }
}

/// 32 random bytes from the OS, hex-encoded (64 chars).
pub fn random_secret() -> io::Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    }))
}

/// Constant-time comparison (no early exit on the first differing byte).
#[must_use]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0_u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Finds the token matching an `Authorization` header value (`Bearer <secret>`).
#[must_use]
pub fn authenticate<'a>(header: Option<&str>, tokens: &'a [AgentToken]) -> Option<&'a AgentToken> {
    let secret = header?.strip_prefix("Bearer ")?.trim();
    // Check every token so timing does not reveal which label matched.
    let mut found = None;
    for t in tokens {
        if constant_time_eq(secret.as_bytes(), t.secret.as_bytes()) {
            found = Some(t);
        }
    }
    found
}

/// The run-file (arch §6): how local clients find the engine. Removed when dropped.
pub struct RunFile {
    path: PathBuf,
}

impl RunFile {
    /// Default location: `~/.cutweave/run/<pid>.json`.
    #[must_use]
    pub fn default_path() -> Option<PathBuf> {
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
        Some(
            PathBuf::from(home)
                .join(".cutweave")
                .join("run")
                .join(format!("{}.json", std::process::id())),
        )
    }

    /// Writes `{pid, port, url, endpoints, agent_token, agent_label}` to `path` with user-only
    /// permissions on Unix (0600). On Windows the file inherits the ACL of the user profile
    /// directory, which is user-only by default (not hardened further in the spike).
    pub fn write(path: &Path, addr: SocketAddr, token: &AgentToken) -> io::Result<RunFile> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let base = format!("http://{addr}");
        let endpoints: serde_json::Map<String, serde_json::Value> = Profile::ALL
            .iter()
            .map(|p| {
                (
                    p.as_str().to_owned(),
                    json!(format!("{base}{}", p.http_path())),
                )
            })
            .collect();
        let body = json!({
            "pid": std::process::id(),
            "port": addr.port(),
            "url": format!("{base}{}", Profile::Standard.http_path()),
            "endpoints": endpoints,
            "agent_label": token.label,
            "agent_token": token.secret,
        });
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        file.write_all(serde_json::to_string_pretty(&body)?.as_bytes())?;
        file.write_all(b"\n")?;
        Ok(RunFile {
            path: path.to_owned(),
        })
    }

    /// Path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RunFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
