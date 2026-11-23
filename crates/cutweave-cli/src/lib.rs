// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave` command-line interface (WP-1K.01, arch §6, ADR-0002).
//!
//! One binary, several roles:
//!
//! - `engine` runs the engine process (the launch protocol of `cutweave_engine::launch`).
//! - `exec` and `query` send commands to an engine and print JSON. They **attach** to a running
//!   engine (run-file discovery, the `cli` agent token, `/rpc`) or **spawn** an in-process
//!   one-shot engine (`--headless` / `--project`).
//! - `commands` and `describe` print the registry; `render`, `mcp` and `worker` are reserved.
//!
//! The CLI never reimplements command logic: every call goes through the engine's dispatcher,
//! in process (`Connection::call`) or over `/rpc`.

mod cli;
mod docs;
mod failure;
mod remote;
mod run;
mod session;

pub use cli::command;
pub use docs::reference_markdown;
pub use run::run;
