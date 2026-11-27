// SPDX-License-Identifier: GPL-3.0-or-later
//! The agent evaluation (WP-2I.09) in the gate, with no model and no network: recorded
//! sessions are made again on real headless engines, their checkers run and their measures
//! are computed, so a change that breaks the runner, a checker or a tool the tasks use fails
//! `just check-affected`. The run with a model is `cutweave-bench agent-eval`, on demand only
//! (`docs/reference/agent-eval.md`).
//!
//! The sessions in `tests/fixtures/agent-eval/` were written by hand, one per task, and
//! recorded on a real engine with `cutweave-bench agent-eval --record`: no model made them.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cutweave_bench::agent_eval::anthropic::{PythonTransport, Stop, Transport as _, parse_reply};
use cutweave_bench::agent_eval::run::{Bench, Replayed, replay};
use cutweave_bench::agent_eval::tasks::TASKS;
use cutweave_bench::agent_eval::transcript::{Transcript, tokens_estimate};
use cutweave_bench::workspace_root;
use serde_json::json;
use tokio::runtime::Runtime;

fn runtime() -> Arc<Runtime> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    Arc::new(rt)
}

fn recorded(task: &str) -> Transcript {
    let file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/agent-eval")
        .join(format!("{task}.json"));
    serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap()
}

fn replayed(rt: &Arc<Runtime>, tag: &str, transcript: &Transcript) -> Replayed {
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(tag);
    let bench = Bench {
        exe: Path::new(env!("CARGO_BIN_EXE_cutweave-bench")),
        scratch: &scratch,
        rt,
    };
    replay(&bench, transcript).unwrap()
}

/// The smoke test of the work package: one recorded session against a real engine, the
/// checker, and every measure.
#[test]
fn a_recorded_session_replays_on_a_real_engine_and_is_measured() {
    let record = recorded("split-and-trim");
    let Replayed {
        session,
        differences,
    } = replayed(&runtime(), "agent-eval-smoke", &record);
    assert_eq!(differences, Vec::<String>::new());
    let m = &session.measures;
    assert!(m.success, "{}", m.failure);
    // Six responses: look (with a timeline_get that misses its sequence), look again,
    // describe clip.trim, split, trim, done.
    assert_eq!((m.turns, m.tool_calls, m.tool_errors), (6, 6, 1));
    assert_eq!((m.tools_used, m.first_calls_valid), (5, 4));
    assert_eq!(m.first_calls_invalid, ["timeline_get (invalid)"]);
    assert_eq!(
        (m.describe_calls, m.search_calls, m.tool_search_calls),
        (1, 0, 0)
    );
    assert_eq!(m.batches, 0);
    // The token counts of the record (made up, since no model ran) are summed.
    let usage: Vec<_> = record.turns.iter().map(|t| t.usage).collect();
    assert_eq!(
        m.input_tokens,
        usage.iter().map(|u| u.total_input()).sum::<u64>()
    );
    assert_eq!(m.input_tokens, 73_706);
    assert_eq!(m.cache_read_tokens, 58_920);
    assert_eq!(m.output_tokens, 419);
    // The sizes are this run's: what the engine answered and what it lists.
    let bytes: u64 = session
        .transcript
        .turns
        .iter()
        .flat_map(|turn| &turn.calls)
        .map(|call| call.result_bytes)
        .sum();
    assert_eq!(m.tool_result_tokens, tokens_estimate(bytes));
    assert!(
        bytes > 4_000,
        "a depth-2 timeline is a few thousand bytes: {bytes}"
    );
    let tools = session.transcript.tools;
    assert!(tools.count >= 28, "{tools:?}");
    assert_eq!(tools.sent_bytes, tools.upfront_bytes);
    assert_eq!(m.tool_definition_tokens, tokens_estimate(tools.sent_bytes));
    assert_eq!(
        m.tool_definition_tokens,
        m.tool_definition_tokens_in_context
    );
    assert!((4_000..8_000).contains(&m.tool_definition_tokens), "{m:?}");
    assert!(
        session.transcript.system_bytes > 1_000,
        "the server instructions are in"
    );
}

/// Every task has a session that does it, and its checker accepts what the engine made of
/// it. The sessions cover the variants and client modes too.
#[test]
fn every_task_has_a_recorded_session_its_checker_accepts() {
    let rt = runtime();
    let tasks: Vec<&str> = TASKS.iter().map(|task| task.id).collect();
    let mut sessions = Vec::new();
    // Three engines at a time.
    for group in tasks.chunks(3) {
        let done: Vec<Replayed> = std::thread::scope(|scope| {
            let handles: Vec<_> = group
                .iter()
                .map(|task| {
                    let rt = &rt;
                    scope
                        .spawn(move || replayed(rt, &format!("agent-eval-{task}"), &recorded(task)))
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        sessions.extend(done);
    }
    for replayed in &sessions {
        let s = &replayed.session;
        assert!(s.measures.success, "{}: {}", s.task, s.measures.failure);
        assert_eq!(replayed.differences, Vec::<String>::new(), "{}", s.task);
    }
    let of = |task: &str| {
        &sessions
            .iter()
            .find(|r| r.session.task == task)
            .unwrap()
            .session
    };
    let standard = of("split-and-trim").transcript.tools;
    // The described list: the same tools, larger definitions, within its 16,000 tokens.
    let described = of("set-speed");
    assert_eq!(described.variant, "described");
    assert_eq!(described.transcript.endpoint, "/mcp/described");
    assert_eq!(described.transcript.tools.count, standard.count);
    assert!(described.transcript.tools.sent_bytes > standard.sent_bytes);
    assert!(described.measures.tool_definition_tokens < 16_000);
    // Through cutweave_execute: the task's core tools are not shown.
    let via = of("undo-and-checkpoint");
    assert_eq!(via.variant, "via-execute");
    assert_eq!(via.transcript.tools.count, standard.count - 3);
    assert_eq!(via.measures.tools_used, 5, "{:?}", via.measures);
    // One batch against one change per call.
    assert_eq!(of("insert-and-reorder").measures.batches, 1);
    assert_eq!(
        (
            of("add-markers").variant.as_str(),
            of("add-markers").measures.batches
        ),
        ("no-batch", 0)
    );
    // The deferred mode: the always-load tools at the start, one more found by a search.
    let deferred = of("fade-audio");
    assert_eq!(deferred.client, "deferred");
    assert_eq!(deferred.measures.tool_search_calls, 1);
    let sizes = deferred.transcript.tools;
    assert!(
        sizes.loaded_bytes > 0 && sizes.upfront_bytes * 4 < sizes.sent_bytes,
        "{sizes:?}"
    );
    assert!(
        deferred.measures.tool_definition_tokens_in_context * 4
            < deferred.measures.tool_definition_tokens
    );
    // The long-tail task goes through search, describe and execute.
    let long_tail = &of("long-tail").measures;
    assert_eq!((long_tail.search_calls, long_tail.describe_calls), (1, 1));
}

/// A session that does not do the task fails its checker, and a call that goes differently
/// than recorded is reported.
#[test]
fn a_session_that_leaves_the_task_undone_fails_and_a_changed_call_is_reported() {
    let mut record = recorded("set-speed");
    // Everything but the edit.
    record
        .turns
        .retain(|turn| turn.calls.iter().all(|call| call.tool != "clip_set_speed"));
    // And the record claims the first call failed.
    record.turns[0].calls[0].is_error = true;
    let replayed = replayed(&runtime(), "agent-eval-undone", &record);
    assert!(!replayed.session.measures.success);
    assert!(replayed.session.measures.failure.contains("not 2x"));
    assert_eq!(replayed.differences.len(), 1);
    assert!(replayed.differences[0].starts_with("turn 1 call 1: project_info works now"));
}

/// Reads one HTTP request from the next connection and answers it.
fn serve_once(
    listener: &TcpListener,
    status: &str,
    extra_header: &str,
    body: &str,
) -> (String, String) {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => panic!("the script never connected: {e}"),
        }
    };
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let mut seen = Vec::new();
    let mut chunk = [0u8; 4096];
    let (head, sent) = loop {
        let n = stream.read(&mut chunk).unwrap();
        assert!(n > 0, "the request ended early");
        seen.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&seen).into_owned();
        let Some((head, rest)) = text.split_once("\r\n\r\n") else {
            continue;
        };
        let length = head
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|v| v.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        if rest.len() >= length {
            break (head.to_ascii_lowercase(), rest.to_owned());
        }
    };
    let answer = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n{extra_header}content-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(answer.as_bytes()).unwrap();
    (head, sent)
}

/// The one piece that talks HTTP, against a listener on this machine: the script sends the
/// body as it is with the API's headers, and hands back the status, `retry-after` and body.
/// The key is a made-up one, given to the script's process only.
#[test]
fn the_post_script_sends_the_request_and_returns_the_answer() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://127.0.0.1:{}/v1/messages",
        listener.local_addr().unwrap().port()
    );
    let mut transport = PythonTransport::new(&workspace_root(), &url, Duration::from_secs(30))
        .unwrap()
        .env("ANTHROPIC_API_KEY", "made-up-key-for-the-test");
    let request = json!({"model": "claude-sonnet-5-5", "max_tokens": 16, "messages": [{"role": "user", "content": "Hi"}]});
    let reply = r#"{"content":[{"type":"text","text":"Hello"}],"stop_reason":"end_turn","usage":{"input_tokens":8,"output_tokens":2}}"#;
    let (answer, (head, sent)) = std::thread::scope(|scope| {
        let server = scope.spawn(|| serve_once(&listener, "200 OK", "", reply));
        let answer = transport.post(&request).unwrap();
        (answer, server.join().unwrap())
    });
    assert!(head.starts_with("post /v1/messages http/1.1"), "{head}");
    assert!(
        head.contains("x-api-key: made-up-key-for-the-test"),
        "{head}"
    );
    assert!(head.contains("anthropic-version: 2026-10-21"), "{head}");
    assert!(head.contains("content-type: application/json"), "{head}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&sent).unwrap(),
        request
    );
    assert_eq!(answer.status, 200);
    let parsed = parse_reply(&answer.body).unwrap();
    assert_eq!(
        (parsed.stop, parsed.usage.output_tokens),
        (Stop::EndTurn, 2)
    );

    // A rate limit comes back as a status with its retry-after, not as a failure.
    let limited = r#"{"type":"error","error":{"type":"rate_limit_error","message":"Slow down"}}"#;
    let answer = std::thread::scope(|scope| {
        let server = scope.spawn(|| {
            serve_once(
                &listener,
                "429 Too Many Requests",
                "retry-after: 7\r\n",
                limited,
            )
        });
        let answer = transport.post(&request).unwrap();
        server.join().unwrap();
        answer
    });
    assert_eq!((answer.status, answer.retry_after), (429, Some(7.0)));
    assert_eq!(answer.body["error"]["type"], "rate_limit_error");

    // Nothing listens any more: no answer, status 0, and the key is not in the message.
    drop(listener);
    let answer = transport.post(&request).unwrap();
    assert_eq!(answer.status, 0);
    assert!(
        !answer.body.to_string().contains("made-up-key"),
        "{}",
        answer.body
    );
}

/// The script refuses to run without a key, and never sends one to a plain-http address
/// that is not this machine. Neither case opens a connection.
#[test]
fn the_post_script_needs_a_key_and_an_https_address() {
    let root = workspace_root();
    let local = "http://127.0.0.1:9/v1/messages";
    let mut no_key = PythonTransport::new(&root, local, Duration::from_secs(5))
        .unwrap()
        .env("ANTHROPIC_API_KEY", "");
    let refused = no_key.post(&json!({})).unwrap_err().to_string();
    assert!(
        refused.contains("ANTHROPIC_API_KEY is not set"),
        "{refused}"
    );
    let mut plain = PythonTransport::new(
        &root,
        "http://api.example.invalid/v1/messages",
        Duration::from_secs(5),
    )
    .unwrap()
    .env("ANTHROPIC_API_KEY", "made-up-key-for-the-test");
    let refused = plain.post(&json!({})).unwrap_err().to_string();
    assert!(refused.contains("must be https"), "{refused}");
    assert!(!refused.contains("made-up-key"), "{refused}");
}
