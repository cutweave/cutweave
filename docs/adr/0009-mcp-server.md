# ADR-0009: MCP server: `rmcp`, hybrid tool catalog (core tools plus search/describe/execute/query), attach and headless modes

- **Status:** Proposed
- **Date:** 2026-11-22
- **Deciders:** Cutweave maintainers (decision pending Spike S5)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §14, §14b, §5.3, §20

## Context

Decision D5 requires 100% of capabilities through MCP, with state as structured data and frames as images, never screenshots. The command surface is large and growing, including plugin commands. Hundreds of listed tools cost tens of thousands of tokens and hurt tool selection, while a single `execute_command` loses discoverability and per-tool annotations. Two MCP spec revisions are in play (2025-11-25 stateful, 2026-07-28 stateless). Research 05 recommends a hybrid design.

## Decision drivers

- Token-efficient tool lists with good annotations (readOnly, destructive, idempotent).
- Every command reachable, including plugin commands.
- Works with both spec revisions, with notifications that follow the negotiated revision.
- The agent can see (images) and wait (jobs) without screenshots.
- Parity with RPC by construction (generated from the command registry).

## Considered options

1. **`rmcp` (official Rust SDK, pinned); hybrid catalog: about 30 always-listed core tools plus 4 generic tools (`cutweave_search_commands`, `cutweave_describe_command`, `cutweave_execute`, `cutweave_query`); stdio attach proxy and headless server** (leading hypothesis).
2. **One tool per command (hundreds).** Rejected: token cost and selection quality.
3. **Only a generic execute tool.** Rejected: no per-tool annotations or approval UX, and poor discoverability.
4. **A scripting tool as the main interface** (Rhai/JS). Kept as an optional later add-on (research 05 phase 2), not the foundation.
5. **A hand-rolled MCP implementation.** Rejected: spec churn; prefer the official SDK with a pinned version.

## Decision

**Decided by: Spike S5** (WP-0H.01 schemas and snapshots, WP-0H.02 MCP Inspector, WP-0H.03 agent run) with these pass criteria: schemas generated and snapshot-tested; **MCP Inspector passes** on stdio and HTTP; and **an agent (Claude Code) completes "import, cut, render frame" with no screenshots**. On fail, adjust the macro design and pin the `rmcp` version. ADR-0009 must record **which MCP spec revision(s) are supported and the notification mechanism per revision**.

Current architectural commitments feeding the spike (R23, arch §14), subject to S5 confirmation: **both spec revisions are supported** (2025-11-25 stateful and 2026-07-28 stateless), change notifications use `resources/subscribe` or `subscriptions/listen` per negotiated revision, tool profiles are per endpoint (`/mcp`, `/mcp/minimal`, `/mcp/full`) or `--profile`, never per connection, one endpoint is one engine and one open project, and a single CI-enforced tool-list budget (minimal <= 12 tools / <= 3k tokens; standard <= 36 / <= 8k). `media.import` returns `media_ids` synchronously and `job_wait` exists (R21); `view_get`/`view_set` are in the catalog. Nothing depends on sampling, elicitation or MCP Apps; each has a tool-result fallback.

## Consequences

**Positive** (if confirmed)

- Small, well-annotated tool list with full long-tail reach; the catalog is generated from the registry, so parity is checked in CI.
- Works for stateless and stateful clients; attach mode lets the user watch the agent edit live.

**Negative / costs**

- Two spec revisions double the conformance matrix (Inspector, notification paths).
- The `rmcp` version must be pinned and upgrades need conformance runs.
- The generic `cutweave_execute` carries a discriminated union validated server-side, so its error quality matters and needs agent evals.
- The tool-list budget is a hard constraint on adding core tools.

**Follow-ups**

- WP-0H.01-0H.03; WP-0J.02 (accept, record spec revisions); Epics 1J, 2I, 6J; Agent Kit evals (6H).

## Links

- Architecture: [§14](../01-architecture.md), [§14b](../01-architecture.md), [§5.3](../01-architecture.md), [§20](../01-architecture.md)
- Research: [05-mcp-integration.md](../research/05-mcp-integration.md) (§1 spec status, §2 Rust SDKs, §4.1 granularity, §5 architecture), [08-agent-harness.md](../research/08-agent-harness.md)
- Stack: [02-tech-stack.md](../02-tech-stack.md); roadmap S5 in [04-roadmap.md](../04-roadmap.md); WBS [0H](../wbs/P0-foundations.md)
- Integrator resolutions: R21, R23
- Related: ADR-0002, ADR-0015, ADR-0016, ADR-0017
