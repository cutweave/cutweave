# ADR-0039: The standard MCP tool list: parameter docs, the three visual tools and the 8k token budget

- **Status:** Accepted (product owner, 2026-11-27): option 1; built in WP-2I.01 (#239)
- **Date:** 2026-11-27
- **Deciders:** Product owner
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §14 (tool strategy, tool-list budget); [ADR-0009](0009-mcp-server.md) (hybrid catalog); integrator resolution R36

## Context

Arch §14 and R36 gate the `/mcp` tool list at 36 tools and 8,000 tokens (`ceil(bytes / 4)` of the compact `tools/list` result). WP-2I.01 lists the P2 core set: 31 named tools, of which 28 have a command today, next to the 4 generic tools. The work package also asks for a description of up to 300 characters with one example per tool.

The numbers, measured on the full engine:

| List | Tools | Tokens |
|---|---|---|
| Before WP-2I.01 (parameter descriptions in the listed schemas, no examples) | 24 | 7,931 |
| The 28 core tools with parameter descriptions, no examples | 32 | about 10,000 |
| As built in WP-2I.01: no parameter descriptions, one example per tool | 32 | 7,694 |
| The same plus the three visual tools of WP-2I.02 (estimate) | 35 | about 8,350 |
| All 35 with parameter descriptions and examples (estimate) | 35 | about 11,500 |

So the planned set does not fit 8,000 tokens with parameter descriptions, and WP-2I.01 had to choose. It kept the budget, which is the accepted constraint, and removed every parameter description from the listed input schemas. The listed schema still validates exactly like the command's; the description and its example stand in for the parameter docs, and `cutweave_describe_command` returns them in full. The server instructions say so.

Two things are still open, and both are a decision for the product owner:

- **Parameter docs.** An agent now sees `clip_trim{item, edge, to, delta, unlink, clamp, fine, prune_keyframes}` with types and enums but no word on what `clamp` does. It must call `cutweave_describe_command` first or guess. Whether that costs more than the tokens it saves is a product judgement; no agent run has measured it yet (WP-2I.06 is the first scripted run, the LLM evals are in P6).
- **The visual tools.** `render_contact_sheet`, `media_filmstrip` and `audio_waveform` (WP-2I.02) need about 650 tokens. 306 are free. They do not fit unless something else goes.

Later phases add to this: the plan names `text_add`, `caption_generate`, `effect_add`, `transition_add`, `composition_create` and `ai_*` as core tools for P3 to P5. With them the list passes 36 tools as well as any token figure discussed here.

## Decision drivers

- Agents should use a tool correctly at the first call, without screenshots and with few extra round trips.
- The tool list is loaded into every agent session; its size is a cost every user pays in every conversation.
- One budget, in one place, enforced in CI (arch §14), so the list cannot grow unnoticed.
- The work packages after this one (2I.02, 2I.05, 2I.06) need a figure to build against.

## Considered options

1. **Keep 8,000 tokens and the validation-only schemas (as built); list one visual tool.** No budget change. WP-2I.02 lists `render_contact_sheet` if it fits the 306 tokens that are free (it should: about 240) and keeps `media_filmstrip` and `audio_waveform` in the catalog, reached through `cutweave_query`.
2. **Keep 8,000 tokens; make room for the visual tools by unlisting others.** `view_get` and `view_set` cost 621 tokens together and are the natural candidates (arch §14 only required them "in the catalog"). An agent would then show the user things through `cutweave_execute`.
3. **Raise the standard budget to 12,000 tokens and list parameter descriptions again.** All 35 tools with descriptions, examples and parameter docs fit (about 11,500). Every agent session pays about 4,000 tokens more than today.
4. **Raise the standard budget to 9,000 tokens, schemas stay validation-only.** All 35 tools fit with examples (about 8,350); parameter docs stay in `cutweave_describe_command`. About 650 tokens of room remain for P3.
5. **Drop the examples.** They cost about 310 tokens in total. This alone does not make room for the visual tools and gives up the one piece of usage documentation left in the list.

Options 1 and 2 need no new figure but leave an agent either without parameter docs and with visual tools one step further away, or without a listed way to show the user something. Option 3 restores what the list had before WP-2I.01 at the highest cost per session. Option 4 is the smallest change that lists the whole P2 plan.

## Decision

**Option 1** (product owner, 2026-11-27).

1. The standard profile's budget stays at **8,000 tokens**.
2. Listed input schemas stay **validation-only**, as WP-2I.01 built them: no parameter descriptions in `tools/list`; each listed tool has a description of at most 300 characters with one example; `cutweave_describe_command` returns the parameter docs in full, and the server instructions say so.
3. WP-2I.02 builds its three commands and their tests. It lists `render_contact_sheet` as a core tool if the list still fits 8,000 tokens with it; `media_filmstrip` and `audio_waveform` stay in the catalog, reached through `cutweave_query`.
4. The list stays capped at **36 tools**. Which tools are listed once the P3 to P5 core tools arrive is decided then, not now.

The parameter-docs question is looked at again after the first scripted-agent run (WP-2I.06), which is the first measurement of whether agents stumble without them.

**Pass criteria:** `STANDARD_BUDGET` and `[budget]` in `crates/cutweave-api/mcp_core.toml` carry 8,000 (one test fails when they differ); the `tools/list` snapshot and `docs/reference/mcp-core-tools.md` match the registry; arch §14 and §23 state the figure and the rule.

## Consequences

**Positive**

- The budget every agent session pays does not grow, and the whole core set that has commands today is listed.
- The cost of each option is measured and written down, so the question can be reopened with numbers.

**Negative / costs**

- Agents read parameter docs with an extra call (`cutweave_describe_command`) or work from the description, the example and the parameter names and types.
- Two of the three visual tools of WP-2I.02 are catalog commands, one step further from an agent than a listed tool.

**Follow-ups**

- WP-2I.02 and WP-2I.05 build against this decision.
- WP-2I.06's scripted-agent run is the first measurement of the cost; the parameter-docs question is looked at again with its results.
- The tool-count question returns with the P3 core tools.

## Links

- Architecture: `../01-architecture.md` §14
- Work packages: `../wbs/P2-core-editor.md` (WP-2I.01, WP-2I.02, WP-2I.05)
- The list and its measured size: `../reference/mcp-core-tools.md`; the table: `../../crates/cutweave-api/mcp_core.toml`
- Related: [ADR-0009](0009-mcp-server.md), [ADR-0028](0028-mcp-result-schemas.md)
