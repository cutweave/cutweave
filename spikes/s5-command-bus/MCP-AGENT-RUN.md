# S5 agent run (WP-0H.03): exact steps

This is the runbook for WP-0H.03: a real agent host drives the spike MCP server (`s5-mcp`, WP-0H.02) over **stdio** and completes the S5 task without screenshots or computer use:

> Import `testdata/fixture.mp4`, cut it at 2 s, and show me the frame at 3 s

Pass criteria (WBS WP-0H.03): **3/3** successful Claude Code runs, transcripts committed to `agent-run/`, no screenshot or computer-use tools, final document state as expected (checked through `timeline.get`), one more run with an alternative client if one is available, and a verdict of `PASS` or `PASS-WITH-FALLBACK`.

Run every command from `spikes/s5-command-bus/` in Git Bash (the repo's `just` shell on Windows). PowerShell works too, but you have to translate the `mkcfg` heredoc.

## 0. What has already been verified (WP-0H.02)

- `cargo test -p s5-mcp` (22 tests) runs the same task through rmcp clients on both MCP revisions over in-process, child-process stdio and Streamable HTTP transports.
- `tests/stdio_dry_run.rs` does a **scripted dry run of this exact task through the real stdio binary**, using raw newline-delimited JSON-RPC with no rmcp on the client side. It runs once with the 2025-11-25 handshake and once with 2026-07-28 `server/discover`. The transcripts and server state files are committed in `agent-run/stdio-dry-run-*.jsonl` and `agent-run/stdio-dry-run-*.state.json`, and `check_run.py --state` passes on them.
- What is still open is whether a real LLM agent host picks the right tools, and which revision that host negotiates.

## 1. Build and self-check (about 5 minutes)

```sh
cd spikes/s5-command-bus
cargo build --release -p s5-mcp
cargo test -p s5-mcp                       # expect 22 passed
./target/release/s5-mcp budget             # tools/list sizes; copy the table into the report
claude --version                           # record it (2.1.283 when this runbook was written)
```

The agent will see:

- **Server name `cutweave`**, so tools appear as `mcp__cutweave__<tool>`.
- **13 tools** (standard profile): `clip_split`, `clip_trim`, `edit_undo`, `item_delete`, `media_import`, `project_open`, `render_frame`, `timeline_get`, `timeline_insert`, `cutweave_search_commands`, `cutweave_describe_command`, `cutweave_execute`, `cutweave_query`.
- **No file I/O:** `media.import` never opens the file. Any `.mp4/.mov/.mkv/.webm` path works, and the imported clip is 10 s at 30 fps.
- **Expected final state:** 1 media and one track with two items, [0, 2 s) and [2 s, 10 s). The `render_frame` call at 3 s returns a PNG image block, and only the right part is visible.

## 2. Write the Claude Code MCP config

The config has to use absolute paths, so `agent-run/mcp.json` is git-ignored. `mkcfg N` writes it for run `N`:

```sh
SPIKE="$(pwd -W 2>/dev/null || pwd)"       # Git Bash prints D:/...; plain pwd on macOS/Linux
EXE="$SPIKE/target/release/s5-mcp"; [ "$OS" = "Windows_NT" ] && EXE="$EXE.exe"
mkcfg() {
  cat > agent-run/mcp.json <<EOF
{
  "mcpServers": {
    "cutweave": {
      "type": "stdio",
      "command": "$EXE",
      "args": ["stdio", "--agent-label", "claude-code",
               "--state-file", "$SPIKE/agent-run/run-$1.state.json",
               "--log-file", "$SPIKE/agent-run/run-$1.server.log"],
      "env": { "S5_MCP_LOG": "info" }
    }
  }
}
EOF
}
```

- **`--state-file`** keeps `{rev, changes, timeline}` current after every edit, written by the server itself. The final state is therefore recorded whether the host closes stdin or kills the process, and whatever the agent says.
- **`--log-file`** captures the lifecycle line that shows which revision the host negotiated, for example `initialize (2025-11-25-style session lifecycle) client=claude-code ... requested=... negotiated=...` or `server/discover (2026-07-28 stateless lifecycle)`. Hosts tend to swallow stderr.

## 3. Optional interactive check (calibrates the token estimate)

```sh
mkcfg 0
claude --mcp-config agent-run/mcp.json --strict-mcp-config
```

In the session:

1. Run `/mcp`. `cutweave` should be **connected** with 13 tools.
2. Run `/context` and record **"MCP tools: N tokens"**. Compare it with the `budget` estimate for `standard`: about 2.7k tokens (bytes/4) and at most 3.6k (bytes/3, the bound CI asserts).
3. Exit. Delete `agent-run/run-0.*` or keep it as an extra transcript.

## 4. The three Claude Code runs (stdio, headless)

```sh
PROMPT='Import testdata/fixture.mp4, cut it at 2 s, and show me the frame at 3 s'
for N in 1 2 3; do
  mkcfg $N
  claude -p "$PROMPT" \
    --mcp-config agent-run/mcp.json --strict-mcp-config \
    --tools "" --allowedTools "mcp__cutweave" \
    --output-format stream-json --verbose \
    > agent-run/run-$N.claude.jsonl
  python agent-run/check_run.py --state agent-run/run-$N.state.json \
    --transcript agent-run/run-$N.claude.jsonl | tee agent-run/run-$N.check.txt
done
```

What the flags do:

- **`--strict-mcp-config`** loads no MCP server except `cutweave`. No screenshot, browser or computer-use tools exist in the session.
- **`--tools ""`** disables every built-in tool (shell, file and web tools), so a success can only come from the MCP tools.
- **`--allowedTools "mcp__cutweave"`** pre-approves all `cutweave` tools for the non-interactive run.

`check_run.py` prints PASS/FAIL for each criterion and a `VERDICT`:

- the server is connected;
- only `mcp__cutweave__*` tools were called;
- a `render_frame` at 3 s returned an image block (directly or through `cutweave_query`);
- the run ended without error;
- exactly 1 media and 2 items cut at 2 s, all changed by one `agent:` actor.

It also prints the tool sequence and the server's change log. A run counts as successful when its `VERDICT` is `PASS`.

Optional fourth run to observe the host's natural behaviour: drop `--tools ""` (keep `--strict-mcp-config`) and note whether the agent tries shell or file tools first.

## 5. Alternative client (client independence)

Both configs below come from memory of each tool's docs. Check them against the installed version. The state check is the same for any client: `python agent-run/check_run.py --state agent-run/<client>-1.state.json`.

**Codex CLI** (`~/.codex/config.toml`):

```toml
[mcp_servers.cutweave]
command = "<SPIKE>/target/release/s5-mcp.exe"
args = ["stdio", "--agent-label", "codex",
        "--state-file", "<SPIKE>/agent-run/codex-1.state.json",
        "--log-file", "<SPIKE>/agent-run/codex-1.server.log"]
```

Then run `codex exec "$PROMPT" > agent-run/codex-1.transcript.txt`.

**Gemini CLI** (`.gemini/settings.json` in the working directory):

```json
{ "mcpServers": { "cutweave": { "command": "<SPIKE>/target/release/s5-mcp.exe",
  "args": ["stdio", "--agent-label", "gemini", "--state-file", "<SPIKE>/agent-run/gemini-1.state.json",
           "--log-file", "<SPIKE>/agent-run/gemini-1.server.log"] } } }
```

Then run `gemini -p "$PROMPT" > agent-run/gemini-1.transcript.txt`, approving the tool calls or using the CLI's auto-approve option.

## 6. MCP Inspector over stdio and HTTP (open WP-0H.02 acceptance item)

The WP-0H.02 agent did not run this, because it downloads and executes an npm package. Check the flags with `npx @modelcontextprotocol/inspector --help`.

```sh
# stdio: UI (proxy mode), or CLI mode with a saved transcript
npx @modelcontextprotocol/inspector ./target/release/s5-mcp stdio
npx @modelcontextprotocol/inspector --cli ./target/release/s5-mcp stdio --method tools/list > agent-run/inspector-stdio-tools.json
npx @modelcontextprotocol/inspector --cli ./target/release/s5-mcp stdio \
  --method tools/call --tool-name render_frame --tool-arg at=0 > agent-run/inspector-stdio-render.json

# Streamable HTTP: the agent token is in the run-file (git-ignored; never commit it)
./target/release/s5-mcp http --run-file agent-run/http-run.json --allow-origin http://localhost:6274 &
TOKEN=$(python -c "import json;print(json.load(open('agent-run/http-run.json'))['agent_token'])")
URL=$(python -c "import json;print(json.load(open('agent-run/http-run.json'))['url'])")
npx @modelcontextprotocol/inspector --cli "$URL" --transport http \
  --header "Authorization: Bearer $TOKEN" --method tools/list > agent-run/inspector-http-tools.json
curl -s -o /dev/null -w "%{http_code}\n" -X POST "$URL" -d '{}'                      # expect 401
curl -s -o /dev/null -w "%{http_code}\n" -X POST "$URL" -H "Host: evil.example" \
  -H "Authorization: Bearer $TOKEN" -d '{}'                                          # expect 403
kill %1
```

There is no CORS preflight handling, so use the Inspector's proxy mode or CLI mode, not a direct browser connection. The `/mcp/minimal` and `/mcp/full` endpoints use the same token.

## 7. Record in `docs/spikes/S5-report.md`

| Run | Client + version | Negotiated revision (server log) | Tool calls (sequence) | `check_run.py` | Notes |
|---|---|---|---|---|---|
| 1 | Claude Code 2.1.x | | | | |
| 2 | | | | | |
| 3 | | | | | |
| alt | Codex / Gemini | | | | |

Also record:

- **rmcp:** `=3.11.0` (latest on crates.io on 2026-11-22).
- **Per-revision results from WP-0H.02:** see the README section "WP-0H.02: MCP server".
- **Token figures:** the `budget` table, plus `/context` if you ran step 3.
- **Macro or design adjustments** the runs surface.
- **Verdict:** `PASS` (3/3 and no foreign tools) or `PASS-WITH-FALLBACK`, naming the fallback (for example "pin hosts to 2025-11-25").

Commit everything in `agent-run/` except `mcp.json` and `http-run.json`, which `.gitignore` excludes.
