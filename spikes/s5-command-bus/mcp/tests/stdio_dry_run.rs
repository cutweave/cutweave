// SPDX-License-Identifier: GPL-3.0-or-later
//! Scripted dry run of the WP-0H.03 agent task through the real stdio transport, without rmcp on
//! the client side: spawn `s5-mcp stdio` and exchange newline-delimited JSON-RPC exactly as an
//! agent host (Claude Code, Codex CLI, ...) would, for the task "Import <fixture>, cut it at 2 s,
//! and show me the frame at 3 s". Once with the 2025-11-25 handshake, once with 2026-07-28
//! `server/discover` + per-request `_meta`.
//!
//! `S5_RECORD_DRY_RUN=1 cargo test -p s5-mcp --test stdio_dry_run` also writes the transcripts
//! to `agent-run/stdio-dry-run-<revision>.jsonl` (committed as evidence; see MCP-AGENT-RUN.md).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{path::PathBuf, process::Stdio, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout},
};

const FLICKS: i64 = 705_600_000;

struct Session {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
    meta: Option<Value>,
    transcript: Vec<Value>,
    dump: PathBuf,
}

impl Session {
    fn spawn(name: &str) -> Session {
        let dump =
            std::env::temp_dir().join(format!("s5-dry-run-{name}-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&dump);
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_s5-mcp"))
            .args([
                "stdio",
                "--agent-label",
                "dry-run",
                "--deterministic-ids",
                "2026",
                "--state-file",
            ])
            .arg(&dump)
            .env("S5_MCP_LOG", "warn")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn s5-mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap()).lines();
        Session {
            child,
            stdin,
            stdout,
            next_id: 1,
            meta: None,
            transcript: Vec::new(),
            dump,
        }
    }

    async fn send(&mut self, message: Value) {
        let mut line = message.to_string();
        assert!(!line.contains('\n'), "stdio messages are single lines");
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await.unwrap();
        self.stdin.flush().await.unwrap();
        self.transcript
            .push(json!({ "dir": "client->server", "msg": message }));
    }

    /// Sends a request and returns its `result` (panics on a JSON-RPC error).
    async fn request(&mut self, method: &str, mut params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        if let Some(meta) = &self.meta {
            params["_meta"] = meta.clone();
        }
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await;
        loop {
            let line = tokio::time::timeout(Duration::from_secs(10), self.stdout.next_line())
                .await
                .expect("reply within 10 s")
                .unwrap()
                .expect("server alive");
            let msg: Value = serde_json::from_str(&line).expect("stdout carries only JSON-RPC");
            self.transcript
                .push(json!({ "dir": "server->client", "msg": msg }));
            if msg["id"] == id {
                assert!(msg.get("error").is_none(), "{method}: {msg}");
                return msg["result"].clone();
            }
        }
    }

    async fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let result = self
            .request(
                "tools/call",
                json!({ "name": tool, "arguments": arguments }),
            )
            .await;
        assert_ne!(result["isError"], true, "{tool}: {result}");
        result
    }

    async fn finish(mut self, name: &str) {
        drop(self.stdin); // EOF ends the stdio server
        let status = tokio::time::timeout(Duration::from_secs(10), self.child.wait())
            .await
            .expect("server exits on EOF")
            .unwrap();
        assert!(status.success(), "{status}");

        // The server's own record of the final state (`--state-file`), independent of what the
        // client saw: what WP-0H.03 checks after each agent run.
        let dump = std::fs::read_to_string(&self.dump).expect("state file written");
        let _ = std::fs::remove_file(&self.dump);
        let state: Value = serde_json::from_str(&dump).unwrap();
        assert_eq!(state["rev"], 3);
        let labels: Vec<(&str, &str)> = state["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| (c["actor"].as_str().unwrap(), c["label"].as_str().unwrap()))
            .collect();
        assert_eq!(
            labels,
            [
                ("agent:dry-run", "media.import"),
                ("agent:dry-run", "timeline.insert"),
                ("agent:dry-run", "clip.split"),
            ]
        );
        assert_eq!(
            state["timeline"]["tracks"][0]["items"]
                .as_array()
                .map(Vec::len),
            Some(2)
        );

        if std::env::var_os("S5_RECORD_DRY_RUN").is_some() {
            let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../agent-run");
            std::fs::create_dir_all(&dir).unwrap();
            let mut body = self
                .transcript
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            body.push('\n');
            std::fs::write(dir.join(format!("stdio-dry-run-{name}.jsonl")), body).unwrap();
            std::fs::write(dir.join(format!("stdio-dry-run-{name}.state.json")), dump).unwrap();
        }
    }
}

/// The agent's side of the task, given an open session.
async fn import_cut_show(s: &mut Session) {
    let tools = s.request("tools/list", json!({})).await;
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"media_import") && names.contains(&"render_frame"));

    // A model would look first.
    let empty = s.call("timeline_get", json!({})).await;
    assert_eq!(empty["structuredContent"]["result"]["media"], json!([]));

    let imported = s
        .call("media_import", json!({ "path": "testdata/fixture.mp4" }))
        .await;
    let media = imported["structuredContent"]["result"]["media"].clone();
    let inserted = s
        .call("timeline_insert", json!({ "media": media, "at": 0 }))
        .await;
    let item = inserted["structuredContent"]["result"]["item"].clone();
    let split = s
        .call("clip_split", json!({ "item": item, "at": "2s" }))
        .await;
    let right = split["structuredContent"]["result"]["right"].clone();

    let frame = s.call("render_frame", json!({ "at": "3s" })).await;
    let content = frame["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "image");
    assert_eq!(content[0]["mimeType"], "image/png");
    assert_ne!(content[0]["data"].as_str().unwrap(), "");
    assert_eq!(content[1]["type"], "text");
    assert_eq!(
        frame["structuredContent"]["result"]["visible"],
        json!([right])
    );

    // Verify the final state the way WP-0H.03 asks: query timeline.get.
    let timeline = s.call("timeline_get", json!({})).await;
    let items = timeline["structuredContent"]["result"]["tracks"][0]["items"]
        .as_array()
        .unwrap();
    let spans: Vec<(i64, i64)> = items
        .iter()
        .map(|i| (i["start"].as_i64().unwrap(), i["end"].as_i64().unwrap()))
        .collect();
    assert_eq!(spans, [(0, 2 * FLICKS), (2 * FLICKS, 10 * FLICKS)]);
    assert_eq!(timeline["structuredContent"]["rev"], 3);
}

#[tokio::test]
async fn stdio_dry_run_2025_11_25() {
    let mut s = Session::spawn("2025-11-25");
    let init = s
        .request(
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "s5-dry-run", "version": "0" }
            }),
        )
        .await;
    assert_eq!(init["protocolVersion"], "2025-11-25");
    assert_eq!(init["serverInfo"]["name"], "cutweave-s5");
    assert!(
        init["instructions"]
            .as_str()
            .unwrap()
            .contains("timeline_get")
    );
    s.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;
    import_cut_show(&mut s).await;
    s.finish("2025-11-25").await;
}

#[tokio::test]
async fn stdio_dry_run_2026_07_28() {
    let mut s = Session::spawn("2026-07-28");
    s.meta = Some(json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": { "name": "s5-dry-run", "version": "0" }
    }));
    let discovered = s.request("server/discover", json!({})).await;
    let versions = discovered["supportedVersions"].as_array().unwrap();
    assert!(versions.contains(&json!("2026-07-28")) && versions.contains(&json!("2025-11-25")));
    import_cut_show(&mut s).await;
    s.finish("2026-07-28").await;
}
