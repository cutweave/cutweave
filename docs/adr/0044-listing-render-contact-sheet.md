# ADR-0044: Whether `render_contact_sheet` is a listed MCP tool

- **Status:** Accepted (product owner, 2026-11-28)
- **Date:** 2026-11-28
- **Deciders:** Product owner
- **Supersedes:** none. It applies point 3 of [ADR-0039](0039-mcp-tool-list-budget-and-parameter-docs.md) and meets the same budget as [ADR-0041](0041-expected-rev-in-the-listed-tool-schemas.md)
- **Architecture refs:** `docs/01-architecture.md` §14 (tool strategy, tool-list budget, seeing without screenshots)

## Context

ADR-0039 said: WP-2I.02 lists `render_contact_sheet` as a core tool if the standard list still fits 8,000 tokens with it, and expected it to fit ("about 240" tokens against 306 free). It does not fit.

Measured on the full engine (tokens are `ceil(bytes / 4)` of the compact `tools/list` result):

| | Bytes | Tokens |
|---|---|---|
| The standard list today (32 tools) | 31,489 | 7,873 |
| Free under 8,000 tokens | 511 | 127 |
| `render_contact_sheet` as listed: a 214-character description with its example, the lean schema of its 9 parameters, annotations | 1,186 | 297 |
| The list with it (33 tools) | 32,675 | 8,169 |
| Short by | 675 | 169 |

The listing is already the leanest correct one. The tool has the parameters the work package names (`sequence`, `from`, `to`, `count`, `every`, `at`, `columns`, `max_size`, `labels`); the listed schema carries validation only; the time input is defined once and referenced four times. The definition of the time input (215 bytes), the annotations (104 bytes) and the tool's name and wrapper are 530 bytes before the first parameter, so no choice of parameters fits 511.

The integrator asked for the room to be made inside WP-2I.02 by tightening the wording of the listed descriptions, without removing content. Read tool by tool, the words that can go without losing a fact come to about 95 bytes:

| Tool | Change | Bytes |
|---|---|---|
| `clip_trim` | "start or end edge ... by an amount (`delta`)" to "start or end ... by `delta`" | 17 |
| `edit_checkpoint` | "a restore point of the document" to "a restore point" | 16 |
| `timeline_add_marker` | "on one of its items (`item`; then `t` is from the item's start)" to "on an item (`item`; `t` is then from its start)" | 14 |
| `clip_set_speed` | "Change a clip's playback speed" to "Set a clip's speed" | 10 |
| `clip_split` | "at a timeline time inside it" to "at a time inside it" | 9 |
| `item_set_props` | "Set some of an item's properties" to "Set an item's properties" | 8 |
| `sequence_create` | "`main_magnetic: true` makes" to "`main_magnetic` makes" | 6 |
| `edit_restore` | "Bring the document back to" to "Return the document to" | 4 |
| the other 24 | nothing to take out | 0 |

That is a seventh of what is needed. The rest would have to come from facts (what follows a linked partner, what a tool returns, the examples), so the descriptions were left as they are and the tool is not listed.

`render.contact_sheet` is built, tested and reachable: `cutweave_query{name: "render.contact_sheet", args}` returns the picture as an image content block with the metadata, `cutweave_search_commands` finds it for "thumbnail grid", "see the edit" and "contact sheet", and the server instructions say of the `render` namespace that it shows "one frame, or a grid of frames over a range". `media.filmstrip` and `audio.waveform` are catalog commands by ADR-0039 and are not part of this question.

## Decision drivers

- An agent should be able to look at a stretch of the edit in one call, without first searching for the command.
- The standard list is loaded into every session of a client that reads all definitions; its budget was fixed at 8,000 tokens by ADR-0039 and ADR-0040, and the next change is to be decided with measurements.
- No tool is renamed, removed or made less accurate to make room.
- ADR-0041 asks for room on the same list (22 copies of `expected_rev`, about 190 to 256 tokens).

## Considered options

1. **Keep it a catalog command until the standard budget is next decided (as built).** Nothing changes for any session. An agent reaches it through `cutweave_query`, one step further than a listed tool; `render_frame` stays the listed way to see the edit.
2. **Raise the standard budget to 8,200 tokens and list it.** 8,169 tokens with the tool. Every session of a client that loads all definitions pays about 300 tokens more. With ADR-0041's option 1 as well the list is at about 8,430 tokens, so the two should be decided together (8,500 would hold both).
3. **Make room by unlisting something.** `view_get` and `view_set` cost about 620 tokens together. ADR-0039 and ADR-0041 both looked at this and did not choose it.
4. **List it on `/mcp/described` only**, which has room (14,930 of 16,000 tokens). ADR-0040 requires the described list to be the standard tools in the same order, and a test holds the two lists to the same names, so this would need that rule changed.
5. **List a smaller tool** (`sequence`, `from`, `to`, `count`, `max_size` only). An estimated 950 bytes: still does not fit, and it leaves out parameters the work package names.

## Decision

Accepted by the product owner on 2026-11-28 as recommended: option 1, not listed for now. The first agent evaluation (#247) ran on the stub engine, which has no renderer, so it did not measure this; the listing is decided again with ADR-0041 at the next change to the standard budget.

**Option 1**, which is what WP-2I.02 built, with the question returned to the product owner together with ADR-0041 and the first measurements of WP-2I.06 and WP-2I.09.

1. `render.contact_sheet` is registered with `mcp = "catalog"`. `mcp_core.toml` keeps `render_contact_sheet` under `[[pending]]` with the reason; `media_filmstrip` and `audio_waveform` leave that table (they are catalog commands by decision, not tools waiting to be listed).
2. The standard, full, minimal and described lists are byte-identical to before WP-2I.02. No budget changes.
3. If the owner chooses option 2: the query's attribute becomes `mcp = "core"`, a `[[tool]]` entry with its description and example goes into `mcp_core.toml`, the figure in `[profiles]` is raised, and the snapshots and the generated reference are regenerated. The result handling (`crates/cutweave-api/src/mcp/visual.rs`) already serves the tool by either route.

**Pass criteria:** the `tools/list` snapshots of every profile are unchanged by WP-2I.02; a test over MCP on both transports and both revisions calls the three commands through `cutweave_query` and finds them by a user's words.

## Consequences

**Positive**

- No session pays more than before, and no listed description lost a fact to make room.
- The cost of listing the tool is measured, not estimated, for the next budget decision.

**Negative / costs**

- An agent that wants an overview of the edit must know to ask `cutweave_query` (the instructions and the search say so), or uses `render_frame` several times.
- The estimate in ADR-0039 ("about 240 tokens, it should fit") was wrong by 57 tokens for the tool and by 179 for the free room, which other work has used since.

## Links

- Work package: `../wbs/P2-core-editor.md` (WP-2I.02)
- The list and its measured size: `../reference/mcp-core-tools.md`; the table: `../../crates/cutweave-api/mcp_core.toml`
- Related: [ADR-0039](0039-mcp-tool-list-budget-and-parameter-docs.md), [ADR-0040](0040-mcp-at-scale.md), [ADR-0041](0041-expected-rev-in-the-listed-tool-schemas.md)
