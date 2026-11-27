# 11. Exposing a large app over MCP: prior art and what it means for Cutweave

Researched 2026-11-27 for the product owner, after WP-2I.01 hit the tool-list budget ([ADR-0039](../adr/0039-mcp-tool-list-budget-and-parameter-docs.md)). The plan that follows from it is [ADR-0040](../adr/0040-mcp-at-scale.md) and work packages WP-2I.07 to WP-2I.10 in [the Phase 2 plan](../wbs/P2-core-editor.md).

Sources were read by a research agent. Several are secondary, and none was re-checked by a person. The list at the end says which is which. Check the client-specific details against the client's own documentation before building on them.

## Has anyone solved this?

Yes, as a pattern, and it is the one Cutweave already has. Nobody ships hundreds of first-class tools. Products with a large surface list a small curated set and reach the rest through discover, describe and execute tools.

| Product | How it exposes a large surface |
|---|---|
| Atlassian Rovo MCP | 22 primary tools; a catalog of more than 200 behind `discover` and three execute tools split by risk (read, write, destructive). The closest match to Cutweave. |
| Cloudflare | 2 tools in about 1,000 tokens for 2,500 endpoints: the model writes code against a typed API ("code mode"). |
| Sentry | About 20 tools, with search and execute. |
| DaVinci Resolve MCP (community) | 37 compound tools by default; 390 granular ones behind a flag. |
| GitHub | Let the agent switch toolsets in a session, then dropped it: each change threw away the prompt cache. |
| Notion | Dropped one tool per endpoint for fewer tools shaped around what a person wants done. |

What changed since ADR-0009: major clients now keep tool definitions out of the context themselves and search for them when needed. This is on by default in Claude Code and Copilot CLI, and available in the Anthropic and OpenAI APIs. In those clients the size of `tools/list` is not what the user pays for. The 8,000-token budget matters for clients that load every definition at the start (Cursor, VS Code, older hosts).

What is not solved: nobody has published independent measurements of meta-tools against first-class tools on current models. The evidence is vendor numbers and one paper on older models.

## Approaches

| Approach | Who uses it | Cost or saving | Client support | Fit for Cutweave |
|---|---|---|---|---|
| Curated core plus discover, describe, execute | Atlassian, Sentry, Cutweave | A fixed list; one or two extra calls for the long tail | Every client | Keep |
| Client-side tool search (deferred definitions) | Claude Code, Copilot CLI, Anthropic API, OpenAI Responses API | Anthropic reports about 85% fewer definition tokens and better accuracy on its own eval | On by default in Claude Code; Copilot CLI above about 30 tools | High. A server cannot turn it on, only cooperate |
| Toolsets switched during a session | GitHub (local server only) | Breaks the prompt cache on every switch | The 2026-07-28 MCP revision forbids lists that differ per connection | Do not build |
| Profiles by endpoint or flag | GitHub, Resolve, Cutweave | Free | Every client | Keep. Most users never change the default |
| Code mode on the server | Cloudflare; Blender and Figma servers with a code tool | About 1,000 tokens fixed; needs a sandbox; no accuracy data | Every client | Later, if batches through `cutweave_execute` prove too little |
| Command line plus a skill instead of MCP | Playwright CLI, `gh` | Reports conflict: from far cheaper to more expensive | Agents with a shell | Cutweave has the CLI: ship it in the Agent Kit and measure |
| Tools shaped by intent | Notion, GitHub (cut its defaults by 49%) | Fewer calls and tokens; more design work | Every client | The rule for Phase 3 to 5 additions |
| An example in each description | Anthropic (72% to 90% on complex parameters, its own eval) | A few tokens per tool | Every client | Done in WP-2I.01 |

## Limits that bite

- **Tool caps.** VS Code and the OpenAI API: 128 tools per request, shared with every other server (secondary sources). Cursor: 40 in the past; the current cap is unclear. Claude Code: none. Cutweave's 36 is safe.
- **Token cost.** Anthropic measured 58 tools at about 55,000 tokens, close to 950 per tool. Cutweave's list is about 240 per tool.
- **Accuracy against tool count.** One 2025 paper on older models reports 13.6% selection accuracy with everything loaded and 43% with retrieval. Treat "it degrades past 30 to 50 tools" as folklore with some vendor support.
- **Prompt caching.** A stable tool list sits in the cached part of the prompt, and any change to it throws the cache away. So a longer stable list is cheap on caching clients, and a list that changes is expensive.
- **Truncation in Claude Code.** Server instructions and descriptions loaded at the start are cut at 4,096 characters; descriptions loaded through tool search at 16,384.

## What the evidence says about Cutweave's design

Matches where the field ended up: the hybrid catalog, profiles per endpoint with no per-connection lists, an example in each description, batches as one transaction, compact results with ids and a revision.

Looks like a mistake:

- **Parameter descriptions removed for every client** (ADR-0039). Where the client defers definitions this saves nothing and removes text that the search and the model both use. Where the client caches, the saving is small.
- **`tools/list` size as the measure of what a user pays.** It is right for a shrinking group of clients.

## What to measure before deciding more

No test in the plan measured any of this: WP-2I.06 is a fixed script, and runs with a model in the loop were planned for Phase 6. The plan adds a small on-demand evaluation (WP-2I.09) that compares, per client mode:

- how often the first call to a tool is valid, with and without parameter descriptions;
- describe calls, turns and tokens per task;
- a core tool against the same command through `cutweave_execute`;
- one batch against several calls;
- MCP against the command line with a skill.

## Sources

Read directly unless marked.

- Anthropic, "Advanced tool use", 2025-11-24, <https://www.anthropic.com/engineering/advanced-tool-use>: tool search, programmatic calling and examples.
- Anthropic, "Code execution with MCP", 2025-11-04, <https://www.anthropic.com/engineering/code-execution-with-mcp>: the model writes code against tools.
- Anthropic, "Writing effective tools for agents", 2025-09-11, <https://www.anthropic.com/engineering/writing-tools-for-agents>: few tools shaped by intent, concise results.
- Claude Code documentation, MCP, read 2026-11-27, <https://code.claude.com/docs/en/mcp>: tool search by default, the always-load mark, the truncation limits, list-change handling.
- MCP specification 2026-07-28, changelog, <https://modelcontextprotocol.io/specification/2026-07-28/changelog>: lists must not differ per connection; deterministic order.
- MCP, "Client best practices", <https://modelcontextprotocol.io/docs/develop/clients/client-best-practices>: progressive discovery and its effect on caching.
- Atlassian, Rovo MCP supported tools, updated 2026-11-23, <https://developer.atlassian.com/cloud/rovo-mcp/guides/supported-tools/>: 22 primary tools, `discover`, execute by risk.
- Cloudflare, "Code Mode: give agents an entire API in 1,000 tokens", 2026-02-20, <https://blog.cloudflare.com/code-mode-mcp/>.
- GitHub documentation, Copilot CLI tool search, <https://docs.github.com/en/copilot/concepts/agents/copilot-cli/tool-search>: on by default; what it matches on.
- Notion, "Notion's hosted MCP server: an inside look", 2025-07-15, <https://www.notion.com/blog/notions-hosted-mcp-server-an-inside-look>.
- RAG-MCP, arXiv 2505.03275, 2025-05-06, <https://arxiv.org/abs/2505.03275>.
- Sam Morrow (GitHub MCP server), "MCP Doesn't Have a Context Problem", May 2026: read on a mirror; the original page was not reachable.
- Through search summaries only, not opened: Anthropic's prompt-caching and tool-reference pages, OpenAI's tool search guide, the Sentry MCP contributor docs, the DaVinci Resolve MCP repository, Epic's Unreal MCP page, Cursor's "Dynamic context discovery" post.
- Secondary and unverified: Canva's and Figma's tool counts, Unity, Adobe's local MCP preview, the VS Code 128-tool limit, Cursor's current cap, the command-line against MCP token comparisons.
- Not checked: Stripe, Linear, Zapier, Home Assistant, Ableton.
