// SPDX-License-Identifier: GPL-3.0-or-later
#![allow(clippy::expect_used, clippy::unwrap_used)]
//! Snapshots of `--help` for the main command and every subcommand (WP-1K.01): a change to the
//! command-line surface is a reviewed change to these files and to `docs/reference/cli.md`.

fn help(path: &[&str]) -> String {
    let mut cmd = cutweave_cli::command();
    cmd.build();
    let mut cmd = &cmd;
    for name in path {
        cmd = cmd
            .find_subcommand(name)
            .unwrap_or_else(|| panic!("no subcommand {name}"));
    }
    cmd.clone().render_long_help().to_string()
}

#[test]
fn help_cutweave() {
    insta::assert_snapshot!(help(&[]));
}

#[test]
fn help_engine() {
    insta::assert_snapshot!(help(&["engine"]));
}

#[test]
fn help_exec() {
    insta::assert_snapshot!(help(&["exec"]));
}

#[test]
fn help_query() {
    insta::assert_snapshot!(help(&["query"]));
}

#[test]
fn help_engines() {
    insta::assert_snapshot!(help(&["engines"]));
}

#[test]
fn help_commands() {
    insta::assert_snapshot!(help(&["commands"]));
}

#[test]
fn help_describe() {
    insta::assert_snapshot!(help(&["describe"]));
}

#[test]
fn help_render() {
    insta::assert_snapshot!(help(&["render"]));
}

#[test]
fn help_mcp() {
    insta::assert_snapshot!(help(&["mcp"]));
}

#[test]
fn help_worker() {
    insta::assert_snapshot!(help(&["worker"]));
}

#[test]
fn every_subcommand_has_a_snapshot_and_a_reference_section() {
    let reference = cutweave_cli::reference_markdown();
    let mut root = cutweave_cli::command();
    root.build();
    let names: Vec<_> = root
        .get_subcommands()
        .map(|s| s.get_name().to_owned())
        .filter(|n| n != "help")
        .collect();
    assert_eq!(
        names,
        [
            "engine", "exec", "query", "engines", "commands", "describe", "render", "mcp", "worker"
        ]
    );
    for name in names {
        assert!(reference.contains(&format!("`cutweave {name}`")), "{name}");
    }
}
