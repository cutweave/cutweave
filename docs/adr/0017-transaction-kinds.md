# ADR-0017: Transaction kinds (`gesture`, `proposal`), connection-owned transactions and committed-only saves

- **Status:** Accepted
- **Date:** 2026-11-22
- **Deciders:** Cutweave maintainers (product owner approved)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §5.2, §14b (proposal mode), §17

## Context

ADR-0005 gives one linear, single-writer history. Two interaction patterns need an open, uncommitted transaction: continuous gestures (dragging, slider scrubbing) that must update the preview live yet collapse to one undo step, and copilot **proposals** where an agent turn is shown as a diff to accept or reject. Both can overlap with other actors editing, and a dropped connection or crash must not leave half-applied state or persist it. Review item R22 settled the semantics.

## Decision drivers

- Live preview during gestures without polluting undo history.
- Proposal mode without engine branching.
- No ghost transactions after a client disconnect; no unconfirmed edits saved.
- Predictable conflicts between actors.

## Considered options

1. **Two ephemeral transaction kinds with connection ownership, item soft locks and committed-only saves** (chosen).
2. **Branching documents for proposals.** Rejected: contradicts the single-writer linear model and complicates journaling and snapshots.
3. **Client-side preview only for gestures** (commit on drop). Rejected: the preview must render engine state, and the timeline needs live feedback from the real pipeline.
4. **Global document lock during gestures.** Rejected: blocks agents and the user from unrelated edits.

## Decision

- Transactions are grouped via `edit.begin_tx` / `edit.commit_tx` (batches) or are **ephemeral**: they update the document live, then collapse into one undo entry on commit or roll back on cancel.
- Kinds: **`gesture`** (auto-cancels after 30 s without updates) and **`proposal`** (copilot; no idle timeout, cancellable). Proposal mode is an ephemeral transaction: accept commits one undo step tagged `agent:<id>`, reject rolls back.
- A transaction is **owned by the connection that opened it**; a dropped connection rolls it back.
- An open ephemeral transaction holds a **soft lock on the items it touches**. Another actor's command touching those items fails fast with `Conflict{reason:"item_in_gesture", retry_after_ms}`; commands on other items proceed. Undoing a gesture inverts only its own ops, valid because they are on disjoint items.
- **Saves and autosaves persist only the last committed revision**, never in-flight ephemeral edits (ADR-0006).
- A minimal soft lock ships in P1; P2 adds retry semantics and UI feedback.
- `cutweave_execute` (MCP) is a transaction by default (ADR-0009).

## Consequences

**Positive**

- Proposal mode needs no second document model; accept and reject are cheap thanks to persistent snapshots (ADR-0005).
- Crash and disconnect safety: nothing uncommitted is persisted or stranded.
- Users and agents can work concurrently on disjoint items.

**Negative / costs**

- Soft locks create an observable failure mode (`item_in_gesture`) that agents and the UI must handle with retries.
- The rollback and timeout machinery needs thorough property and fault-injection tests (disconnects mid-gesture, timeouts, overlapping proposals).
- Undo semantics interact with interleaved actors; "undo the agent's last change" must stay valid when user edits sit in between.

**Follow-ups**

- Epic 1B (minimal soft lock), 2A (retry semantics), 6I (copilot proposal UI).

## Links

- Architecture: [§5.2](../01-architecture.md), [§14b](../01-architecture.md), [§17](../01-architecture.md)
- Research: [05-mcp-integration.md](../research/05-mcp-integration.md) §4.2 (transactions and dry runs), [08-agent-harness.md](../research/08-agent-harness.md) (copilot proposals)
- Integrator resolutions: R11, R16, R22 in [05-work-breakdown.md](../05-work-breakdown.md)
- Related: ADR-0005, ADR-0006, ADR-0009, ADR-0016
