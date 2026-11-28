# ADR-0041: Where `expected_rev` is listed on the MCP tools

- **Status:** Accepted (product owner, 2026-11-28)
- **Date:** 2026-11-28
- **Deciders:** Product owner
- **Supersedes:** none. It applies the budgets of [ADR-0039](0039-mcp-tool-list-budget-and-parameter-docs.md) and the two audiences of [ADR-0040](0040-mcp-at-scale.md)
- **Architecture refs:** `docs/01-architecture.md` §5.1 (optimistic concurrency), §14 (tool strategy, tool-list budget)

## Context

WP-2I.04 gives every mutating command an optional `expected_rev` in the transport envelope: an edit made against a revision the document has left fails with `Conflict{current_rev, expected_rev, changed_since}` and changes nothing. Over JSON-RPC it is `_meta.expected_rev`, on the command line `cutweave exec --expected-rev`, and `cutweave_execute` already lists it. The work package also asks for it to be "injected into every mutating tool's schema through the generator", so that a model can send it with `clip_split` as it does with `cutweave_execute`.

The standard list (`/mcp`) cannot take that. It is at 31,489 bytes, 7,873 of 8,000 tokens, and 22 of its 28 core tools change something. A listed schema is one self-contained JSON Schema per tool: a property cannot be defined once and shared between tools, so each tool carries its own copy.

Measured on the full engine (tokens are `ceil(bytes / 4)` of the compact `tools/list` result):

| List | Before | With `expected_rev` on each mutating core tool | Budget |
|---|---|---|---|
| `/mcp` and `/mcp/full` (standard) | 7,873 | 8,129 as `{"type":"integer","minimum":0}` (+1,027 bytes); 8,063 as `{"type":"integer"}` (+763 bytes) | 8,000 |
| `/mcp/minimal` | 1,959 | 1,970 (one tool, `edit_undo`) | 3,000 |
| `/mcp/described` | 14,145 | 14,930, with a one-line description on each | 16,000 |

511 bytes are free in the standard list. The shortest form costs 34 bytes per tool, so 15 tools fit and 22 do not. Even the 17 tools whose command runs on the document actor (leaving out `media_import`, `project_open`, `export_start`, `view_get` and `view_set`) need 578 bytes: 8,017 tokens. The 127 free tokens are also the only room left for anything else on that list; ADR-0039 hoped to list `render_contact_sheet` (about 240 tokens) there.

Every listed input schema says `additionalProperties: false`. The engine does not mind an unlisted `expected_rev`: the handler takes the argument out of every mutating core tool call before the command validates its params, on every endpoint. A client is another matter. One that checks arguments against the listed schema before it sends them, or that holds the model's output to that schema (strict tool use), cannot send a property the list does not show.

## Decision drivers

- A model should be able to say "only if nothing changed since I looked" on the edit it is about to make.
- The standard list is loaded into every session of a client that reads all definitions; ADR-0039 and ADR-0040 fixed its budget at 8,000 tokens and said the next change is decided with measurements.
- One generator, one property: no command's schema may drift from the others.
- No tool is renamed, removed or shortened to make room.

## Considered options

1. **List it on every mutating tool of every profile and raise the standard budget.** 8,129 tokens with the bound (`minimum: 0`), 8,063 without: the budget becomes 8,200 (or 8,100). The property is visible wherever a model looks. Every session of a client that loads all definitions pays about 250 (or 190) tokens more, and the list has no room left under the new figure either.
2. **List it where the list has room: the described profile; the standard, full and minimal lists stay as they are (as built).** `/mcp/described` shows the property, with a description, on its 22 mutating tools (14,930 of 16,000). On `/mcp` an agent uses `cutweave_execute{steps, expected_rev}`, which is listed, reaches every command and is the tool the server instructions already name for this. A core tool call that carries the argument is honoured on every endpoint.
3. **Keep it out of every listed schema; `_meta` only.** A programmatic client or a proxy sets `io.cutweave/expected_rev` in the request `_meta` (this existed before WP-2I.04). No list grows. A model cannot set `_meta`, so on a core tool the check would be out of its reach everywhere, and the described list would lose what it has room for.
4. **Make room on the standard list by unlisting something.** `view_get` and `view_set` cost about 620 tokens. ADR-0039 looked at this for the visual tools and did not choose it; nothing has changed since.

## Decision

Accepted by the product owner on 2026-11-28 as recommended: option 2, as built, until the standard budget is next decided. The first agent evaluation (#247, [agent evaluation](../reference/agent-eval.md)) had no concurrent editors and did not measure this; the question returns with the next change to the standard budget.

**Option 2**, which is what WP-2I.04 built, until the standard budget is next decided.

1. `Profile::lists_expected_rev` (in `crates/cutweave-api/src/mcp/catalog.rs`) is true for the described profile only. One generator adds the same property to every mutating core tool of a list that shows it.
2. The standard, full and minimal lists are byte-identical to before WP-2I.04. Their budgets do not change.
3. On every endpoint the handler accepts `expected_rev` as an argument of every mutating core tool, and `cutweave_execute` lists it.
4. The question returns with the next change to the standard budget (the visual tools of WP-2I.02, the Phase 3 core tools; ADR-0039 point 4, ADR-0040). If the owner prefers option 1 now, the change is one line (`lists_expected_rev` returns true for every profile), the new figure in `[profiles]` of `mcp_core.toml`, and regenerated snapshots.

**What waits on the answer:** only whether `/mcp`, `/mcp/full` and `/mcp/minimal` show the property. Everything else in WP-2I.04 is built and does not depend on it.

**Pass criteria:** the `tools/list` snapshots of the standard and minimal profiles are unchanged by WP-2I.04; a test holds that every tool of a list that shows the property shows the same subschema and that the other lists show none; the two-client conflict test passes on `/mcp` and on `/mcp/described`.

## Consequences

**Positive**

- No session pays more than before; the budgets ADR-0039 and ADR-0040 set are untouched.
- Clients that defer tool definitions (the audience of `/mcp/described`) see the property and its meaning on each tool.
- The engine's behaviour is the same on every endpoint, so a later choice of option 1 changes a list and nothing else.

**Negative / costs**

- On `/mcp` a model that wants the check on a single edit goes through `cutweave_execute` with one step. That is one listed tool it already knows, but not the tool it would otherwise pick.
- The described and standard lists no longer show exactly the same properties on a mutating tool: the described one has `expected_rev` in addition. Both endpoints accept the same calls.
- A client that validates arguments against the standard list refuses `expected_rev` on a core tool, though the engine would take it.

**Follow-ups**

- If WP-2I.09's evaluation shows agents on `/mcp` skipping the check because it is not on the tool, that is the measurement for option 1.
- The server instructions (4,096 characters, 3,344 used) could add a sentence saying that tools which change the document take `expected_rev` too. It is left out for now: it would describe an argument the standard list does not show.

## Links

- Architecture: `../01-architecture.md` §5.1, §14
- Work package: `../wbs/P2-core-editor.md` (WP-2I.04)
- The lists and their measured sizes: `../reference/mcp-core-tools.md`; the budgets: `../../crates/cutweave-api/mcp_core.toml`
- Related: [ADR-0039](0039-mcp-tool-list-budget-and-parameter-docs.md), [ADR-0040](0040-mcp-at-scale.md), [ADR-0015](0015-wire-protocol.md), [ADR-0028](0028-mcp-result-schemas.md)
