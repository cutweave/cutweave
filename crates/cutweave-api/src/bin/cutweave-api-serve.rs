// SPDX-License-Identifier: GPL-3.0-or-later
//! Skeleton launch harness speaking the spawn handshake (`cutweave_api::launch`). The real
//! `cutweave engine` command arrives with WP-1I.04; until then this lets the supervisor and
//! the integration tests exercise the protocol against the real server.

use tokio::io::BufReader;

#[tokio::main]
async fn main() {
    let args = match cutweave_api::launch::LaunchArgs::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("usage error: {message}");
            std::process::exit(cutweave_api::launch::EXIT_USAGE);
        }
    };
    let code =
        cutweave_api::launch::run(args, BufReader::new(tokio::io::stdin()), std::io::stdout())
            .await;
    // Exit directly: a blocked stdin read must not delay shutdown.
    std::process::exit(code);
}
