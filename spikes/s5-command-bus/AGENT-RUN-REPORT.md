# S5 agent run report (WP-0H.03)

Date: 2026-11-22. Host: Windows 11, Git Bash. Server: `s5-mcp` (release build, rmcp `=3.11.0`), stdio, standard profile (13 tools), server name `cutweave`.
Client: Claude Code 2.1.283, headless (`claude -p`). Model: `--model sonnet` was accepted and resolved to `claude-sonnet-5` (the `model` field in all 4 transcripts). No fallback to the default model was needed.
Prompt (all runs): `Import testdata/fixture.mp4, cut it at 2 s, and show me the frame at 3 s`.
Commands as in `MCP-AGENT-RUN.md` sections 1, 2 and 4, plus `--model sonnet`. Runs 1-3 use `--strict-mcp-config --tools "" --allowedTools "mcp__cutweave"`. Run 4 drops `--tools ""` and keeps the other flags.

## Verdict

**PASS** (3/3 restricted runs; no fallback needed).

## Per-run results

`check_run.py` was run with `--state` and `--transcript` for every run (output in `agent-run/run-N.check.txt`).

| Criterion | Run 1 | Run 2 | Run 3 | Run 4 (unrestricted) |
|---|---|---|---|---|
| Server `cutweave` connected | PASS | PASS | PASS | PASS |
| Only `mcp__cutweave__*` tools (no screenshot / computer use / shell) | PASS (4 calls) | PASS (4) | PASS (4) | FAIL by design (ToolSearch, Bash x2, Glob also used) |
| Frame at 3 s returned as image block | PASS | PASS | PASS | PASS |
| Run finished without error | PASS | PASS | PASS | PASS |
| Final state: 1 media, 1 track, 2 items cut at 2 s ([0,2 s) and [2 s,10 s)), both from the imported media | PASS | PASS | PASS | PASS |
| All changes by one actor `agent:claude-code` | PASS | PASS | PASS | PASS |
| Change log `media.import -> timeline.insert -> clip.split` | PASS | PASS | PASS | PASS |
| Check VERDICT | PASS | PASS | PASS | FAIL (1), expected |

Run 4 is not part of the 3/3 criterion. The one failing check is the foreign-tool check, which is expected when built-in tools are allowed. All state checks pass in that run too.

## Negotiated MCP revision

From `run-N.server.log`, identical in all 4 runs: `initialize (2025-11-25-style session lifecycle) client=claude-code client_version=2.1.283 requested=2025-11-25 negotiated=2025-11-25 profile="standard"`. Claude Code 2.1.283 speaks 2025-11-25 and does not use `server/discover` (2026-07-28). Its capabilities include roots (listChanged) and elicitation (form, url). The server only exercised the 2025-11-25 path against a real host. The 2026-07-28 path is still covered only by the scripted dry run and rmcp tests from WP-0H.02.

## Tool calls

Runs 1-3 (identical sequence, 4 MCP calls, 5 turns each):
`media_import {path: "testdata/fixture.mp4"}` -> `timeline_insert {media, at: "0s"}` -> `clip_split {item, at: "2s"}` -> `render_frame {at: "3s"}`.
The agent used the 13 first-class tools directly and never needed `cutweave_search_commands`, `cutweave_describe_command`, `cutweave_execute` or `cutweave_query`. It correctly placed the clip on the timeline itself and reported that the 3 s frame falls in the right-hand clip.

Run 4 (10 turns): `ToolSearch` (deferred-tool select of the cutweave tools) -> `Bash find . -iname fixture.mp4` (no output) -> `Glob **/testdata/fixture.mp4` (not found) -> `Bash find /d/projects/Vidai ...` (denied by the sandbox: outside the working directories) -> then the same four MCP calls as above, preceded by a `timeline_get` to confirm an empty project.

## Tokens and cost (from the `result` event)

| Run | Turns | Cost (USD) | Input / cache-create / cache-read / output tokens |
|---|---|---|---|
| 1 | 5 | 0.0841 | 10 / 16,616 / 63,699 / 486 |
| 2 | 5 | 0.0351 | 10 / 3,557 / 77,191 / 543 |
| 3 | 5 | 0.0347 | 10 / 3,574 / 77,390 / 490 |
| 4 | 10 | 0.1902 | 18 / 20,673 / 467,319 / 1,405 |

Run 1 pays the prompt-cache creation cost, which later runs reuse. The `/context` calibration (runbook section 3, interactive) was not done. `s5-mcp budget` figures: minimal 969 tokens (bytes/4), 7 tools; standard 2,706 (upper bound 3,608), 13 tools; full 4,976 (6,635), 13 tools.

## Confirmation: no screenshot, computer-use or shell in runs 1-3

Checked by `check_run.py` ("only Cutweave MCP tools used", 4 calls each). The session `tools` list in the init event of run 1 contains only the 13 `mcp__cutweave__*` tools. `--strict-mcp-config` excluded all other MCP servers, and `--tools ""` removed every built-in. The "screenshot" in the task came from the `render_frame` MCP tool, which returned a PNG image block.

## Failures and causes

- Runs 1-3: none.
- Run 4: no run failure. Its FAIL is the foreign-tool check. In the unrestricted session the agent did not know that `media.import` never touches the filesystem. It first tried to locate the file on disk with Bash and Glob, then fell back to the MCP tools. It did not reach for a screenshot or computer-use tool, and none was available (`--strict-mcp-config`).
- Tooling fix: `check_run.py` crashed on run 4's transcript with `AttributeError: 'str' object has no attribute 'get'`. A `system` / `permission_denied` event has a string `message`. I added a guard (`isinstance(m, dict)`) in `agent-run/check_run.py`. Results for runs 1-3 are unchanged.
- Not done: runbook section 5 (Codex and Gemini CLI are not installed, so no alternative-client run; the WBS asks for one) and section 6 (MCP Inspector, which needs a package download and execution and no exact version was pinned). The WP-0H.03 alternative-client criterion is therefore still open.

## Observations and adjustments

- Descriptions of the core tools were enough for a cold-start agent to chain import, insert, split and render without help. The Core profile (13 tools, about 2.4k model-visible tokens) works for this task.
- With built-ins available, the agent wastes turns searching the disk for the fixture. Consider stating in the `media_import` description that the path is only recorded (the spike stub does not read files). In the real product this does not apply, because the file is actually read.
- Claude Code defers MCP tool schemas behind `ToolSearch` when other tools exist, so the first call in a full session is a schema fetch, not a catalog call.
- The 3 s frame was returned as an MCP image content block and was displayed to the agent. This confirms that image results work through Claude Code stdio.

## Recommendation for ADR-009

Accept the rmcp + hybrid tool catalog (13 first-class core tools plus search/describe/execute/query catalog tools): **PASS**, no fallback needed. Record these points:

1. rmcp `=3.11.0` pinned. Real host evidence exists only for revision **2025-11-25** (Claude Code 2.1.283 negotiates it). Support for 2026-07-28 is verified only by scripted and rmcp tests, so ADR-009 should say "2025-11-25 required, 2026-07-28 supported but not yet seen from a real host".
2. Notification mechanism for 2025-11-25 is the session-based one. Claude Code advertises roots and elicitation, not sampling.
3. Revisit the client-independence claim after one run with Codex or Gemini CLI. This is the open WP-0H.03 item and should be done before ADR-009 is marked Accepted in WP-0J.02.

## Evidence in `agent-run/`

`run-{1..4}.claude.jsonl` (stream-json transcripts, user profile path replaced by `<HOME>`, no tokens present), `run-{1..4}.state.json`, `run-{1..4}.server.log`, `run-{1..4}.check.txt`. `mcp.json` is git-ignored.
