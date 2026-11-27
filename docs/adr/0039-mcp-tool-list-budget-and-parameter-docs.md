# ADR-0039: The standard MCP tool list: parameter docs, the three visual tools and the 8k token budget

- **Status:** Proposed
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

Not decided here.

**Decided by:** the product owner, on three questions:

1. What is the standard profile's token budget: 8,000 (today), 9,000 (option 4) or 12,000 (option 3)?
2. Do listed input schemas carry parameter descriptions again (only possible at about 12,000), or do they stay validation-only as WP-2I.01 built them?
3. When the P3 to P5 core tools arrive and the list passes 36 tools: raise the tool count, or keep 36 and decide then which tools are listed?

**Pass criteria for the work package that applies the answer:** `STANDARD_BUDGET` and `[budget]` in `crates/cutweave-api/mcp_core.toml` carry the figure (one test already fails when they differ); the `tools/list` snapshot and `docs/reference/mcp-core-tools.md` are regenerated; if parameter descriptions return, `lean_schema` keeps them and its unit tests say so; arch §14 and R36 state the figure.

**Until then:** the budget is 8,000 tokens and 36 tools. Listed schemas are validation-only. WP-2I.02 builds its three commands and their tests, lists one as a core tool only while the list still fits 8,000 tokens, and reports the measured size; the others stay under `[[pending]]` in `mcp_core.toml`.

## Consequences

**Positive**

- No budget changed without a decision, and the whole core set that has commands today is listed.
- The cost of each choice is measured and written down.

**Negative / costs**

- Until this is decided, agents read parameter docs with an extra call, and the visual tools of WP-2I.02 may ship as catalog commands rather than listed tools.
- WP-2I.06 (the scripted agent runs) depends on which tools are listed.

**Follow-ups**

- WP-2I.02 and WP-2I.05 build against the answer.
- The same question returns with the P3 core tools (question 3).

## Links

- Architecture: `../01-architecture.md` §14
- Work packages: `../wbs/P2-core-editor.md` (WP-2I.01, WP-2I.02, WP-2I.05)
- The list and its measured size: `../reference/mcp-core-tools.md`; the table: `../../crates/cutweave-api/mcp_core.toml`
- Related: [ADR-0009](0009-mcp-server.md), [ADR-0028](0028-mcp-result-schemas.md)
