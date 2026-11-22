// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave` command-line binary (stub; WP-0A.01).
//!
//! Subcommands (`engine | mcp | exec | query | render | worker | plugin`) arrive in later work packages.

const HELP: &str = "cutweave: open-source video editor engine and CLI\n\n\
USAGE:\n    cutweave [--version | --help]\n\n\
Subcommands (engine, mcp, exec, query, render, worker, plugin) are not implemented yet.";

/// Builds the text printed for the given arguments (excluding the program name).
#[must_use]
pub fn respond(args: &[String]) -> String {
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => format!("cutweave {}", env!("CARGO_PKG_VERSION")),
        _ => HELP.to_owned(),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    println!("{}", respond(&args));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_flag() {
        let out = respond(&["--version".to_owned()]);
        assert!(out.starts_with("cutweave "));
    }

    #[test]
    fn default_is_help() {
        assert!(respond(&[]).contains("USAGE"));
    }
}
