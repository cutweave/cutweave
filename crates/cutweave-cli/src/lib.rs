// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave` command-line interface (WP-1K.01, arch §6, ADR-0002).
//!
//! One binary, several roles:
//!
//! - `engine` runs the engine process (the launch protocol of `cutweave_engine::launch`).
//! - `exec` and `query` send commands to an engine and print JSON. They **attach** to a running
//!   engine (run-file discovery, the `cli` agent token, `/rpc`) or **spawn** an in-process
//!   one-shot engine (`--headless` / `--project`).
//! - `mcp` (WP-1J.03) serves MCP over stdio: a proxy to a running engine's `/mcp` (the `mcp`
//!   agent token) or an in-process engine.
//! - `commands` and `describe` print the registry.
//! - `render --frames` (WP-1O.01) writes still frames of a project as PNGs from a one-shot
//!   engine's `render.frame`, optionally checking them against a second render and the export
//!   pipeline's readback (the golden-frame runner, arch §20). Export to a media file and `worker`
//!   are reserved.
//!
//! The CLI never reimplements command logic: every call goes through the engine's dispatcher,
//! in process (`Connection::call`), over `/rpc` or over `/mcp`.

mod cli;
mod docs;
mod failure;
mod mcp;
mod remote;
mod render;
mod run;
mod session;

pub use cli::command;
pub use docs::reference_markdown;
pub use run::run;
