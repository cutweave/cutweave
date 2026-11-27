# Agent evaluation

How well does a model use Cutweave's tools? `cutweave-bench agent-eval` measures it (WP-2I.09; [ADR-0040](../adr/0040-mcp-at-scale.md), question 4; what to measure and why: [research 11](../research/11-mcp-at-scale.md)). It gives a model one task at a time and an MCP connection to a fresh headless engine, records every tool call, and lets a checker read the final document. The same task is given in several variants, so one run compares them.

It needs a model and an API key, so **it never runs in the gate or in CI**. It runs when someone starts it. What the gate runs is a replay of recorded sessions with no model and no network (see [What the gate runs](#what-the-gate-runs)).

## Run it

```
ANTHROPIC_API_KEY=... cargo run -p cutweave-bench -- agent-eval
```

(PowerShell: `$env:ANTHROPIC_API_KEY = '...'; cargo run -p cutweave-bench -- agent-eval`.)

That is the whole command: 57 sessions on `claude-sonnet-5-5`, one after the other; expect roughly half an hour (an estimate: about five requests per session). It needs Python 3 on the `PATH` (standard library only) and nothing else: no GPU, no FFmpeg, no audio device. Without the key it prints how to run it and exits 0.

Before spending anything:

```
cargo run -p cutweave-bench -- agent-eval --list       # the tasks, the variants, the sessions of a run
cargo run -p cutweave-bench -- agent-eval --dry-run    # the first request, key redacted; sends nothing
cargo run -p cutweave-bench -- agent-eval --tasks set-speed --variants standard --client upfront   # one session
```

| Option | What it does |
|---|---|
| `--model ID` | Any model id the API accepts: `claude-sonnet-5-5` (default), `claude-haiku-5-5`, `claude-opus-5-5`, `claude-fable-5-2`, … |
| `--tasks a,b` | Only these tasks |
| `--variants a,b` | Only these variants |
| `--client upfront\|deferred\|both` | The client mode for every chosen variant. Default: upfront for all, and deferred as well for `standard` and `described` |
| `--effort low\|medium\|high\|xhigh\|max` | `output_config.effort`. Default: the model's own default, which differs per model, so set it when comparing models |
| `--max-turns N` | Responses a session may take (20) |
| `--max-tokens N` | `max_tokens` of a request (16000) |
| `--tool-search bm25\|regex` | Which of the API's search tools the deferred mode uses (bm25) |
| `--out DIR` | Where the result goes (`docs/reference/agent-eval/`) |
| `--scratch DIR` | The engines' projects and run files (`target/tmp/agent-eval/`) |
| `--api-url URL` | Another Messages endpoint; https only |

The key is read from the environment variable `ANTHROPIC_API_KEY` and from nowhere else: no file, no option. `cutweave-bench` only checks that it is set; `scripts/agent-eval-post.py` reads it, sends it in the `x-api-key` header to an https address and never prints it.

## What it costs

A default run on `claude-sonnet-5-5` is estimated at **about 3 million input tokens and 0.1 million output tokens: 3 to 7 US dollars**. It has not been run yet, so this is an estimate, made like this:

- A session is about five requests (look at the project, read the timeline, one or two edits, the closing sentence). Every request sends the tool definitions, the system prompt (about 900 tokens with the server instructions) and the conversation so far (a timeline read is about 1,400 tokens).
- Per request that is about 10,000 tokens with the standard list upfront (6,700 of definitions), 16,000 with the described list (12,700), and 4,000 to 6,000 in the deferred mode (700 of definitions at the start, plus what a search loads).
- So a session reads about 50,000 tokens (standard, upfront), 80,000 (described, upfront), 25,000 (deferred), 65,000 (through `cutweave_execute`, which takes more turns). Over the 57 sessions: 10 × 50k + 10 × 80k + 20 × 25k + 9 × 65k + 8 × 50k ≈ 2.8 million, rounded up to 3.
- Output: about 1,500 tokens per session with thinking, so about 0.1 million.
- At 2 dollars per million input tokens and 10 per million output tokens that is 7 dollars if nothing is cached. Requests ask for prompt caching and most of every request repeats the one before, so the real figure should be near 3.

At the list prices of November 2026 the same run costs about a twentieth on `claude-haiku-5-5`, twice as much on `claude-opus-5-5` and four times as much on `claude-fable-5-2`. The result file has the API's own token counts, so the first run replaces this estimate.

## What it writes

Two files in `docs/reference/agent-eval/`, named by date and model (`2026-12-01-claude-sonnet-5-5.json`; a second run on the same day gets `-2`):

- **the result** (`.json`): for every session the task, the variant, the client mode, the measures and every tool call with its arguments (not the results);
- **the report** (`.md`): generated from the result. `cargo run -p cutweave-bench -- agent-eval --report FILE.json` writes it again, so a report is never edited by hand.

Commit both. The exit code is 0 when the run was made, whatever the model did; a session the model fails is a result, not an error.

## The tasks

Every session starts from the same seeded project (`P-eval`: four minutes, 24 clips, six each on the main track V1, on V2, A1 and A2). The task is one message in a user's words; clips are named by where they are, never by id, so the model has to look first.

| Task | The user asks for | The checker passes when |
|---|---|---|
| `split-and-trim` | Split V1's second clip 10 s in, then shorten the right part by 5 s | The clip is 10 s long and a new clip of the same source follows it, 5 s shorter than the rest was |
| `insert-and-reorder` | Insert `pattern-1.mp4` at the start of V1 and move V1's last clip to second place | V1 is: a new clip of that source at 0, the former last clip, then the others in their order |
| `set-speed` | V2's first clip at double speed | Its speed is 2 |
| `add-markers` | Markers "Review" at 12 s and "Outro" at 3:30 | The sequence has exactly those two |
| `fade-audio` | A 0.5 s fade in and a 2 s fade out on A1's first two clips | Both clips have both fades |
| `keyframe-move` | V2's second clip slides in: x = -0.5 at its start, 0 two seconds later | Its position has exactly those two keys |
| `vertical-format` | A 1080 × 1920 sequence | The format is 1080 × 1920 |
| `undo-and-checkpoint` | A restore point "before-cleanup", delete A2's last two clips one by one, undo the last deletion | The checkpoint exists, the fifth clip is gone, the last one is back where it was |
| `import-and-place` | Import a file and add it to the end of V1 | The file is in the media and its clip follows V1's former last clip |
| `long-tail` | Mute A2 and rename it "Music" (`track.set`, a command only the catalog has) | The track is muted and called "Music" |

A checker reads the document the session ended with (`project.snapshot`, and the checkpoint list) and nothing else. It does not care which tools were used or how many calls it took, and no model judges anything. It does not judge taste either: every task has one right answer.

## The variants

| Variant | What the model gets | Runs on |
|---|---|---|
| `standard` | The tools of `/mcp`: no parameter descriptions | All ten |
| `described` | The tools of `/mcp/described`: every parameter described | All ten |
| `via-execute` | The standard list without the task's own core tools, so the same commands have to go through `cutweave_execute`. The runner leaves them out of the list it shows the model; the server is not changed | The nine with a core tool |
| `batch` | The standard list; the task also says "make all the changes in a single cutweave_execute call" | The four with two changes |
| `no-batch` | The standard list; the task also says "one change per call" | The same four |
| `cli-skill` | The command line with a skill instead of MCP | Not built: WP-2I.10 adds it |

And two **client modes**, which say how the tool definitions reach the model:

- `upfront`: every definition is in the model's context from the first request, as with a client that loads the whole `tools/list`.
- `deferred`: every tool is sent with `defer_loading: true` except the five the server marks always-load (`project_info`, `timeline_get` and the three generic tools), and the API's tool search tool is added. The model sees those five and searches for the rest, as with a client that defers definitions (Claude Code). The API supports this for tools the client defines itself: "You provide every tool definition in the `tools` array and set `defer_loading: true` on the tools that shouldn't load up front." ([tool search tool](https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-search-tool)).

In both modes the system prompt is two sentences of the runner's ("you are operating Cutweave for a user…") followed by the server's own instructions, as an MCP client passes them on. Nothing else is told to the model: no hints about the tools, no examples.

## The measures

Per session:

| Measure | What it is |
|---|---|
| Passed | The checker's verdict |
| First call valid | Of the distinct tools the model called, how many worked at their first call. A command run through `cutweave_execute` or `cutweave_query` counts as a tool of its own (`cutweave_execute:clip.split`). The report lists the first calls that failed with the engine's error code: `invalid` means the arguments did not validate, any other code that they validated and the engine refused the call |
| Turns | Responses of the model, which is requests to the API |
| Tool calls, describe, search | All calls; calls of `cutweave_describe_command`; calls of `cutweave_search_commands` |
| Tool search | Searches with the API's tool search (deferred mode) |
| Tokens in, tokens out | The API's own counts (`usage`), summed over the session. Tokens in is everything the model read, cached or not: `input_tokens + cache_creation_input_tokens + cache_read_input_tokens` |
| Tool results (est.) | Bytes of the result texts sent to the model, divided by 4 and rounded up |
| Definitions (est.) | Bytes of the tool definitions as sent to the API (name, description, input schema), divided by 4 and rounded up. "In context" counts the ones the model had loaded: all of them upfront, the always-load ones plus what its searches found when deferred |

**The two estimates are bytes / 4**, the rule [mcp-core-tools.md](mcp-core-tools.md) uses for the tool list; they are not the API's counts. The definitions come out smaller here than in that file (about 6,700 tokens against 7,700 for the standard list) because the API is not sent a tool's annotations and title.

## Reading the report

The report starts with a summary row per variant and client mode, then answers three questions, each with a table of the two variants side by side on the tasks both ran:

1. **Do parameter descriptions change first-call validity?** `standard` against `described`. Look at "First call to a tool valid" and "Describe calls", and at what the descriptions cost in "Input tokens". In the deferred mode the cost of descriptions should mostly disappear, which is the point of ADR-0040's second endpoint.
2. **What does a core tool save over `cutweave_execute`?** `via-execute` against `standard`: turns, describe and search calls, tokens.
3. **What does a batch save?** `no-batch` against `batch`: turns, tool calls, tokens.

Then every session with its numbers, why each failed session failed, and each first call that failed.

One run is one sample of a model that does not answer the same way twice. A difference of one call or a few percent between two variants is noise; run it again (it is a few dollars) before deciding anything on a small difference. Four tasks carry the batch comparison and one the long tail, so those rows are thinner than the others.

## What the gate runs

`crates/cutweave-bench/tests/agent_eval.rs`, ordinary tests (about 10 s, no GPU, no key, nothing leaves the machine):

- ten recorded sessions, one per task, are made again on real headless engines: every call goes through MCP, the task's checker reads the final document, and the measures are computed and compared. The sessions cover every variant and both client modes;
- a session that leaves its task undone must fail its checker, and a call that goes differently than recorded must be reported;
- the script that carries a request to the API is run against a listener on this machine with a made-up key;
- one whole run, as started from the command line, against a "model" that is a listener on this machine answering from a script: the binary, the Python script, the session loop, a real engine, the checker and the two files.

The recorded sessions are in `crates/cutweave-bench/tests/fixtures/agent-eval/`. They were **written by hand**, not made by a model, and recorded with `--record`; the token counts in `split-and-trim.json` are made up so that the sums are tested. Each checker also has unit tests with a passing and a failing document.

```
cargo run -p cutweave-bench -- agent-eval --replay crates/cutweave-bench/tests/fixtures/agent-eval/split-and-trim.json
cargo run -p cutweave-bench -- agent-eval --record my-session.json --out recorded.json
```

`--replay` works on a session cut out of a result file too, which is how to look closer at a session a model failed.

## Where things are

| | |
|---|---|
| The runner | `crates/cutweave-bench/src/agent_eval/` (`tasks.rs`: tasks and checkers; `run.rs`: the session loop, variants, replay; `anthropic.rs`: the API request and response shapes; `surface.rs`: the MCP connection and the engine; `transcript.rs`: measures; `report.rs`: the report) |
| The HTTPS request | `scripts/agent-eval-post.py`. The workspace has no HTTP client with TLS (reqwest is built without it, for loopback only), and the evaluation does not add one: Python's standard library sends the one request |
| Adding a task | A `Task` in `tasks.rs` with a prompt, a checker and its unit tests, and a recorded session in the fixtures folder |
| Adding the command line path (WP-2I.10) | A second `ToolSurface` in `surface.rs` (a shell tool, the skill's text as instructions) and `available: true` on the `cli-skill` variant in `run.rs` |

## Limits

- Anthropic's Messages API only. Requests set no `thinking`, no sampling parameter and no forced tool choice, and no fallback model: a session a safety classifier declines ends as `refusal` and counts as failed.
- The engine runs with stub media and no renderer: it lists 31 of the 32 standard tools (`render_frame` is not listed without a renderer), imports are probed as 10 s clips and nothing is decoded. An image a tool returns would be named to the model, not sent; no task needs to look at one. Tasks that need real pixels or sound are out of scope here.
- It measures whether the edit is right, not whether it is good (WP-2I.09, out of scope).
