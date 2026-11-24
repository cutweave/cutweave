// SPDX-License-Identifier: GPL-3.0-or-later
#![allow(clippy::expect_used, clippy::unwrap_used)]
//! Process-level tests of the `cutweave` binary (WP-1K.01): spawn mode, attach mode, exit codes.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_cutweave");

/// Runs `cutweave args...` with a clean environment of the variables the CLI reads.
fn cutweave(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .env_remove("CUTWEAVE_DETERMINISTIC_IDS")
        .env_remove("CUTWEAVE_DEV_STUB_MEDIA")
        .stdin(Stdio::null())
        .output()
        .expect("spawn cutweave")
}

fn stdout_json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}): {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn stderr_json(out: &Output) -> Value {
    serde_json::from_slice(&out.stderr).unwrap_or_else(|e| {
        panic!(
            "stderr is not JSON ({e}): {}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

const NEW_PROJECT: &str = r#"{"cmd":"project.new","args":{"name":"t"}}"#;

#[test]
fn exec_headless_prints_rev_and_output() {
    let out = cutweave(&["exec", NEW_PROJECT, "--headless"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let v = stdout_json(&out);
    assert_eq!(v["rev"], 1);
    assert_eq!(v["committed_rev"], 1);
    assert!(
        v["output"]["sequence"]
            .as_str()
            .is_some_and(|s| s.starts_with("seq_"))
    );
}

/// WP-1D.08: an agent (the CLI's credential) reads the cache status, and the three commands
/// that change the caches are the desktop app's: `forbidden`.
#[test]
fn the_cache_status_is_readable_and_changing_the_caches_is_forbidden() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let cache = cache.to_str().unwrap();
    let out = cutweave(&[
        "exec",
        r#"{"cmd":"media.cache_status"}"#,
        "--headless",
        "--cache-dir",
        cache,
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let status = stdout_json(&out)["output"].clone();
    assert_eq!(status["decode_hw"], "auto", "{status}");
    assert!(status["kinds"].is_array(), "{status}");
    // The folder is the one asked for (or none: an engine built without FFmpeg has no caches).
    assert!(
        status["folder"].is_null() || status["folder"] == cache,
        "{status}"
    );
    for request in [
        r#"{"cmd":"media.set_cache_config","args":{"frame_cache_bytes":0}}"#,
        r#"{"cmd":"media.clear_cache","args":{"kinds":["thumbnails"]}}"#,
        r#"{"cmd":"media.set_decode_policy","args":{"hw":"off"}}"#,
    ] {
        let out = cutweave(&["exec", request, "--headless", "--cache-dir", cache]);
        assert_eq!(out.status.code(), Some(1), "{request}: {out:?}");
        assert_eq!(stderr_json(&out)["code"], "forbidden", "{request}");
    }
}

#[test]
fn unknown_command_is_exit_1_not_found() {
    let out = cutweave(&["exec", r#"{"cmd":"no.such"}"#, "--headless"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(out.stdout.len(), 0);
    let e = stderr_json(&out);
    assert_eq!(e["code"], "not_found");
    assert!(e["message"].is_string());
}

#[test]
fn exec_reads_stdin_and_files() {
    let mut child = Command::new(BIN)
        .args(["exec", "-", "--headless"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(NEW_PROJECT.as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("wait");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout_json(&out)["rev"], 1);

    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("req.json");
    std::fs::write(&file, format!("[{NEW_PROJECT}]")).expect("write");
    let out = cutweave(&["exec", "--file", file.to_str().expect("utf8"), "--headless"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(stdout_json(&out)["steps"][0]["cmd"], "project.new");
}

#[test]
fn array_runs_as_one_transaction() {
    let out = cutweave(&[
        "exec",
        &format!(r#"[{NEW_PROJECT},{{"cmd":"project.rename","args":{{"name":"x"}}}}]"#),
        "--headless",
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let v = stdout_json(&out);
    assert_eq!(v["steps"].as_array().map(Vec::len), Some(2));
    // Both steps are one revision.
    assert_eq!(v["rev"], 1);
}

#[test]
fn failing_step_names_its_index() {
    let out = cutweave(&[
        "exec",
        &format!(r#"[{NEW_PROJECT},{{"cmd":"no.such"}}]"#),
        "--headless",
    ]);
    assert_eq!(out.status.code(), Some(1));
    let e = stderr_json(&out);
    assert_eq!(e["code"], "not_found");
    assert_eq!(e["failed_step"], 1);
    // A one-step batch names its step too (WP-1O.02: as `cutweave_execute` does); a lone step
    // object is not a batch and has no `failed_step`.
    for (request, failed_step) in [
        (r#"[{"cmd":"edit.redo"}]"#, Some(0)),
        (r#"[{"cmd":"no.such"}]"#, Some(0)),
        (r#"{"cmd":"edit.redo"}"#, None),
    ] {
        let out = cutweave(&["exec", request, "--headless"]);
        assert_eq!(out.status.code(), Some(1), "{request}: {out:?}");
        let e = stderr_json(&out);
        assert_eq!(e["failed_step"].as_u64(), failed_step, "{request}: {e}");
    }
}

/// WP-1O.02 parity with `cutweave_execute`: a batch step uses an earlier step's output through
/// `$ref`; a reference to a later step is refused before anything runs, one to a missing value
/// fails its step and rolls the batch back.
#[test]
fn batch_steps_reference_earlier_outputs() {
    let batch = format!(
        r#"[{NEW_PROJECT},{{"cmd":"track.add","args":{{"sequence":{{"$ref":"steps[0].output.sequence"}},"kind":"video"}}}}]"#
    );
    let out = cutweave(&["exec", &batch, "--headless"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let v = stdout_json(&out);
    assert!(
        v["steps"][1]["output"]["track"]
            .as_str()
            .is_some_and(|t| t.starts_with("trk_")),
        "{v}"
    );
    for (reference, path) in [
        ("steps[1].output.name", "/steps/1/args/name"),
        ("steps[0]", "/steps/1/args/name"),
    ] {
        let bad = format!(
            r#"[{NEW_PROJECT},{{"cmd":"project.rename","args":{{"name":{{"$ref":"{reference}"}}}}}}]"#
        );
        let out = cutweave(&["exec", &bad, "--headless"]);
        assert_eq!(out.status.code(), Some(1), "{reference}: {out:?}");
        let e = stderr_json(&out);
        assert_eq!(e["code"], "invalid", "{e}");
        assert_eq!(e["failed_step"], 1, "{e}");
        assert_eq!(e["path"], path, "{e}");
    }
    let missing = format!(
        r#"[{NEW_PROJECT},{{"cmd":"project.rename","args":{{"name":{{"$ref":"steps[0].output.nope"}}}}}}]"#
    );
    let out = cutweave(&["exec", &missing, "--headless"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let e = stderr_json(&out);
    assert_eq!(
        (e["code"].as_str(), e["failed_step"].as_u64()),
        (Some("invalid"), Some(1)),
        "{e}"
    );
}

#[test]
fn commands_json_length_is_the_registry_size() {
    let out = cutweave(&["commands", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    let list = stdout_json(&out);
    let registry = cutweave_engine::build_registry().expect("registry");
    assert_eq!(list.as_array().map(Vec::len), Some(registry.len()));
    let plain = cutweave(&["commands"]);
    assert_eq!(
        String::from_utf8_lossy(&plain.stdout).lines().count(),
        registry.len()
    );
}

#[test]
fn describe_prints_the_schema_and_rejects_unknown_names() {
    let out = cutweave(&["describe", "clip.split"]);
    assert_eq!(out.status.code(), Some(0));
    let v = stdout_json(&out);
    assert_eq!(v["name"], "clip.split");
    assert!(v["input_schema"].is_object());
    let out = cutweave(&["describe", "no.such"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr_json(&out)["code"], "not_found");
}

#[test]
fn version_has_engine_version_and_schema_hash() {
    let out = cutweave(&["--version"]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(&format!("engine {}", cutweave_engine::VERSION)),
        "{text}"
    );
    assert!(text.contains(cutweave_commands::SCHEMA_HASH), "{text}");
}

#[test]
fn deterministic_ids_give_identical_output() {
    let run = || {
        let out = cutweave(&[
            "exec",
            NEW_PROJECT,
            "--headless",
            "--deterministic-ids",
            "42",
        ]);
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        out.stdout
    };
    let first = run();
    assert_eq!(first, run());
    let other = cutweave(&["exec", NEW_PROJECT, "--deterministic-ids", "43"]);
    assert_ne!(first, other.stdout);
}

#[test]
fn the_env_var_seeds_a_spawned_engine() {
    let run = |seed: &str| {
        Command::new(BIN)
            .args(["exec", NEW_PROJECT, "--headless"])
            .env("CUTWEAVE_DETERMINISTIC_IDS", seed)
            .stdin(Stdio::null())
            .output()
            .expect("spawn")
            .stdout
    };
    let flag = cutweave(&[
        "exec",
        NEW_PROJECT,
        "--headless",
        "--deterministic-ids",
        "7",
    ]);
    assert_eq!(run("7"), flag.stdout);
}

#[test]
fn query_refuses_a_command_and_runs_a_query() {
    let out = cutweave(&["query", "project.new", "{}", "--headless"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(stderr_json(&out)["code"], "usage");
    let out = cutweave(&["query", "project.info", "--headless"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(stdout_json(&out)["output"].is_object());
}

#[test]
fn usage_errors_exit_2_as_json() {
    for args in [
        &["bogus"][..],
        &["exec"][..],
        &["exec", "{not json", "--headless"][..],
        &["exec", NEW_PROJECT, "--pid", "1", "--headless"][..],
        &["query"][..],
    ] {
        let out = cutweave(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {out:?}");
        assert_eq!(stderr_json(&out)["code"], "usage");
    }
}

#[test]
fn no_engine_is_exit_3() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = cutweave(&[
        "exec",
        NEW_PROJECT,
        "--run-dir",
        dir.path().to_str().expect("utf8"),
    ]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert_eq!(stderr_json(&out)["code"], "no_engine");
}

#[test]
fn reserved_subcommands() {
    let out = cutweave(&["worker"]);
    assert_eq!(out.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not implemented in this build"));
    // `mcp` is built (WP-1J.03): tests/mcp.rs.

    let out = cutweave(&[
        "render",
        "--project",
        "p",
        "--sequence",
        "seq_x",
        "--out",
        "o.mp4",
        "--range",
        "0:5s",
        "--preset",
        "web",
        "--sw",
    ]);
    assert_eq!(out.status.code(), Some(1));
    let e = stderr_json(&out);
    assert_eq!(e["code"], "unsupported");
    assert!(
        e["message"]
            .as_str()
            .is_some_and(|m| m.contains("media backend"))
    );
}

#[test]
fn a_gui_engine_refuses_deterministic_ids() {
    let out = cutweave(&["engine", "--deterministic-ids", "1", "--no-log"]);
    assert_eq!(out.status.code(), Some(2));
    let line = stdout_json(&out);
    assert_eq!(line["event"], "error");
    assert_eq!(line["code"], "usage");

    let out = Command::new(BIN)
        .args(["engine", "--no-log"])
        .env("CUTWEAVE_DETERMINISTIC_IDS", "5")
        .stdin(Stdio::null())
        .output()
        .expect("spawn");
    assert_eq!(
        out.status.code(),
        Some(2),
        "the env var must be honoured: {out:?}"
    );
}

/// A `cutweave engine --headless` child, killed on drop.
struct EngineProcess {
    child: Child,
    pid: u32,
}

impl EngineProcess {
    fn start(run_dir: &Path) -> EngineProcess {
        let mut child = Command::new(BIN)
            .args([
                "engine",
                "--headless",
                "--port",
                "0",
                "--no-log",
                "--run-dir",
            ])
            .arg(run_dir)
            .env_remove("CUTWEAVE_DETERMINISTIC_IDS")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn engine");
        let stdout = child.stdout.take().expect("stdout");
        let mut line = String::new();
        BufReader::new(stdout)
            .read_line(&mut line)
            .expect("ready line");
        let ready: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
        assert_eq!(ready["event"], "ready", "{line}");
        let pid = u32::try_from(ready["pid"].as_u64().expect("pid")).expect("pid fits");
        EngineProcess { child, pid }
    }
}

impl Drop for EngineProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn attach_through_the_run_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = dir.path().to_str().expect("utf8").to_owned();
    let engine = EngineProcess::start(dir.path());

    let out = cutweave(&["exec", NEW_PROJECT, "--run-dir", &run_dir]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let created = stdout_json(&out);
    assert_eq!(created["rev"], 1);
    let sequence = created["output"]["sequence"]
        .as_str()
        .expect("sequence")
        .to_owned();

    // A failing batch changes nothing; a second invocation sees the first one's edit only.
    let out = cutweave(&[
        "exec",
        r#"[{"cmd":"project.rename","args":{"name":"zzz"}},{"cmd":"no.such"}]"#,
        "--run-dir",
        &run_dir,
        "--pid",
        &engine.pid.to_string(),
    ]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert_eq!(stderr_json(&out)["failed_step"], 1);

    let out = cutweave(&["query", "project.info", "--run-dir", &run_dir]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(stdout_json(&out)["output"]["name"], "t");

    let out = cutweave(&[
        "exec",
        r#"{"cmd":"project.rename","args":{"name":"renamed"}}"#,
        "--run-dir",
        &run_dir,
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    // An edit made through one attach is visible to the next invocation.
    let add =
        serde_json::json!({"cmd": "track.add", "args": {"sequence": sequence, "kind": "video"}});
    let out = cutweave(&["exec", &add.to_string(), "--run-dir", &run_dir]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let track = stdout_json(&out)["output"]["track"]
        .as_str()
        .expect("track id")
        .to_owned();
    let params = serde_json::json!({"sequence": sequence, "depth": 1});
    let out = cutweave(&[
        "query",
        "timeline.get",
        &params.to_string(),
        "--run-dir",
        &run_dir,
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let timeline = stdout_json(&out);
    assert!(
        timeline["rev"].as_u64().is_some_and(|r| r >= 3),
        "{timeline}"
    );
    assert!(timeline.to_string().contains(&track), "{timeline}");
    let out = cutweave(&["query", "project.info", "--run-dir", &run_dir]);
    assert_eq!(stdout_json(&out)["output"]["name"], "renamed");

    // A wrong pid is exit 3.
    let out = cutweave(&["query", "project.info", "--run-dir", &run_dir, "--pid", "1"]);
    assert_eq!(out.status.code(), Some(3));
}

#[test]
fn a_spawned_engine_edits_and_saves_a_project() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_str().expect("utf8").to_owned();
    let project = dir.path().join("proj");
    let save_as = serde_json::json!({
        "cmd": "project.save_as",
        "args": {"path": project.to_str().expect("utf8")},
    });
    // Engine commands run alone; this one turns the untitled document into a project folder.
    let out = cutweave(&[
        "exec",
        &save_as.to_string(),
        "--headless",
        "--allow-root",
        &root,
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");

    let project_arg = project.to_str().expect("utf8");
    let out = cutweave(&[
        "exec",
        r#"{"cmd":"project.rename","args":{"name":"hello"}}"#,
        "--project",
        project_arg,
        "--save",
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(stdout_json(&out)["saved"].is_object());

    let out = cutweave(&["query", "project.info", "--project", project_arg]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(stdout_json(&out)["output"]["name"], "hello");
}

#[test]
fn engine_speaks_the_launch_protocol_like_the_desktop_supervisor() {
    let dir = tempfile::tempdir().expect("tempdir");
    let token = "t".repeat(48);
    let mut child = Command::new(BIN)
        .args([
            "engine",
            "--port",
            "0",
            "--token-stdin",
            "--allow-origin",
            "app://cutweave",
        ])
        .args(["--no-log", "--run-dir"])
        .arg(dir.path())
        .env_remove("CUTWEAVE_DETERMINISTIC_IDS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn engine");
    let mut stdin = child.stdin.take().expect("stdin");
    writeln!(stdin, "{token}").expect("handshake");
    let mut line = String::new();
    BufReader::new(child.stdout.take().expect("stdout"))
        .read_line(&mut line)
        .expect("ready line");
    let ready: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
    assert_eq!(ready["event"], "ready");
    assert!(
        !line.contains(&token),
        "the token never appears in the ready line"
    );
    // Closing stdin (the supervisor died or quit) stops a GUI engine cleanly.
    drop(stdin);
    let status = child.wait().expect("wait");
    assert_eq!(status.code(), Some(0));
}
