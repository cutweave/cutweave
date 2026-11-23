// SPDX-License-Identifier: GPL-3.0-or-later
//! The `cutweave` binary: `engine | exec | query | commands | describe | render | mcp | worker`.
//! All logic lives in the `cutweave_cli` library (so `cargo xtask gen` can document it).

fn main() {
    // Exit directly: a blocked stdin read (engine mode) must not delay the exit.
    std::process::exit(cutweave_cli::run(std::env::args_os()));
}
