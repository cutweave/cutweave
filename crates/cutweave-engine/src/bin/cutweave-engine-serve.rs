// SPDX-License-Identifier: GPL-3.0-or-later
//! The engine process speaking the launch protocol (`cutweave_engine::launch`). `cutweave
//! engine` (WP-1K.01) wraps the same entry point; until then the supervisor and the
//! integration tests run this binary (`CUTWEAVE_ENGINE_BIN`).

fn main() {
    // Exit directly: a blocked stdin read must not delay the exit.
    std::process::exit(cutweave_engine::launch::main());
}
