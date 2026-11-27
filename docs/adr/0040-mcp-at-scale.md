# ADR-0040: MCP at scale: two kinds of client, tools shaped by intent, and measuring before the next budget change

- **Status:** Proposed
- **Date:** 2026-11-27
- **Deciders:** Product owner
- **Supersedes:** none. It narrows [ADR-0039](0039-mcp-tool-list-budget-and-parameter-docs.md) (see question 2) and builds on [ADR-0009](0009-mcp-server.md)
- **Architecture refs:** `docs/01-architecture.md` §14 (tool strategy, tool-list budget); integrator resolution R36

## Context

Cutweave's catalog has about 200 commands. Phase 3 to 5 add text, captions, effects, transitions, compositions and AI features, each with core tools in the plan. WP-2I.01 already had to remove every parameter description from `tools/list` to fit 32 tools into 8,000 tokens (ADR-0039).

The product owner asked whether anyone has solved this and how to get past the limits. The research is in [`docs/research/11-mcp-at-scale.md`](../research/11-mcp-at-scale.md). In short:

- The field has converged on what Cutweave has: a small curated list plus discover, describe and execute tools.
- Major clients now defer tool definitions and search for them (Claude Code and Copilot CLI by default). For them the size of `tools/list` is not the cost. The budget matters for clients that load everything at the start.
- Lists that change during a session break prompt caching, and the 2026-07-28 MCP revision forbids lists that differ per connection.
- Products that grew past a few dozen tools moved to tools shaped by what a person wants done, not one tool per command.
- Nobody has published independent measurements. Cutweave has none either: WP-2I.06 is a fixed script, and runs with a model in the loop were planned for Phase 6.

## Decision drivers

- An agent should use a tool correctly at the first call, with few extra round trips.
- What a user pays per session should not grow with the catalog.
- One list per endpoint, stable for the whole session, in a fixed order.
- Decisions about the list should rest on Cutweave's own measurements, since the published evidence is thin.
- No tool is renamed or removed without its own decision.

## Considered options

For serving the two kinds of client:

1. **One list for everyone, as today.** Validation-only schemas at 8,000 tokens. Clients that defer definitions get nothing for the missing parameter docs.
2. **A second endpoint for clients that defer definitions.** `/mcp` stays as ADR-0039 decided. A new profile lists the same tools, in the same order, with full parameter descriptions and its own, larger budget; the Agent Kit points Claude Code and Copilot CLI at it. No detection of the client and no per-connection lists.
3. **Detect the client from its name at `initialize`** and serve it the fuller list. Fragile, and it makes the list differ per connection, which the 2026-07-28 revision forbids.
4. **Raise the standard budget to about 12,000 tokens** and list parameter descriptions for everyone (ADR-0039's option 3, already declined).

For growth past the P2 set:

- A. **Keep adding one core tool per command** until the 36-tool cap, then choose what to drop.
- B. **Add core tools by intent**: at most one or two per feature area, each covering the common case, with the area's other commands in the catalog.
- C. **Stop adding core tools** and send every new feature through `cutweave_execute`.

## Decision

Not decided here. Recommended: **option 2** and **option B**, built as WP-2I.07 to WP-2I.10, with the questions below settled by the product owner.

**Decided by:** the product owner, on five questions:

1. **Two kinds of client.** Does Cutweave treat clients that load every definition at the start and clients that defer them as two audiences, with the 8,000-token and 36-tool budget applying to the first only? (Recommended: yes.)
2. **A described profile.** Is there a second endpoint (working name `described`) with the same tools and full parameter descriptions for clients that defer definitions, with its own budget of 16,000 tokens enforced in CI? (Recommended: yes. `/mcp` and `/mcp/minimal` stay exactly as ADR-0039 left them.)
3. **Tools by intent.** From Phase 3 on, does a feature area get at most two core tools shaped by what a person wants done, with the rest in the catalog, so the list stays within 36? (Recommended: yes. Which tools each area lists is decided in that area's work package.)
4. **Measure first.** Is a small on-demand evaluation with a model in the loop (WP-2I.09) built now, in Phase 2, and are these left open until it reports: returning parameter descriptions to `/mcp`, splitting `cutweave_execute` by risk, and a scripting tool? (Recommended: yes.)
5. **Additive changes to the generic tools.** May `cutweave_search_commands` gain a detail level, `cutweave_describe_command` accept several names, and `timeline_get` and `media_list` gain a concise form, all optional and with today's behaviour as the default (WP-2I.08)? (Recommended: yes.)

**What does not change in any case:** no tool is renamed or removed; a listed schema validates exactly like its command; the list an endpoint serves is the same for every connection and keeps a fixed order; `notifications/tools/list_changed` is sent only when the catalog really changes (a plugin loads), never to switch toolsets.

**Pass criteria for the work packages that apply the answers:** the budgets live in `crates/cutweave-api/mcp_core.toml` per profile and CI fails when a list leaves its budget; the two standard lists differ only in descriptions (a test compares names, order and validation); `docs/reference/mcp-core-tools.md` shows both sizes; arch §14 and R36 state the rule.

**Until then:** ADR-0039 stands as accepted. WP-2I.07's parts that only add hints and text for clients that defer definitions (the always-load mark on a few tools, the server instructions as a map of the catalog, descriptions that use the words a user would say) change neither a schema nor the list and can be built before this ADR is accepted.

## Consequences

**Positive**

- Clients that defer definitions get parameter docs back without a cost to anyone else.
- The 36-tool cap stops being a question that returns with every phase.
- The next budget decision is made with Cutweave's own numbers.

**Negative / costs**

- A second standard list to keep in step with the first (one source, two renderings; a test guards it).
- The Agent Kit must point each client at the right endpoint, and most users never change a default, so the default per client has to be right.
- The evaluation needs a model and a key, so it cannot run in the gate. It runs on demand, and its results are a document, not a test.
- Tools shaped by intent take more design than a 1:1 mapping, for each feature area.

**Follow-ups**

- WP-2I.02, WP-2I.05 and WP-2I.06 are adjusted in the plan to build against this.
- The Phase 3 to 5 plans name their core tools; they are revisited under question 3 when those phases are planned in detail.

## Links

- Research: `../research/11-mcp-at-scale.md`
- Architecture: `../01-architecture.md` §14
- Work packages: `../wbs/P2-core-editor.md` (WP-2I.02, WP-2I.05, WP-2I.06, WP-2I.07 to WP-2I.10)
- Related: [ADR-0009](0009-mcp-server.md), [ADR-0028](0028-mcp-result-schemas.md), [ADR-0039](0039-mcp-tool-list-budget-and-parameter-docs.md)
