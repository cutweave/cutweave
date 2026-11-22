# ADR-0015: Wire protocol: `request_id` idempotency, flicks as JSON integers, deterministic-ID test mode

- **Status:** Accepted
- **Date:** 2026-11-22
- **Deciders:** Cutweave maintainers (product owner approved)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §4.1, §5.1, §5.3, §20

## Context

The engine is a separate process (ADR-0002), so the UI, MCP clients and CLI send commands over a network-like boundary where requests can be lost, repeated or retried after an engine restart. Time crosses JSON and TypeScript, where numbers are IEEE doubles. Parity tests must compare saved documents across CLI, MCP and RPC byte for byte, but UUIDv7 IDs are random and time-based. R10 in the integrator resolutions fixed these points.

## Decision drivers

- Safe retries after drops and restarts (no double application, no double spend).
- Lossless time on the wire and exact integer math in TS.
- Reproducible documents for parity and golden tests.
- Agent ergonomics (accept readable time forms).

## Considered options

1. **Client-generated `request_id` with server-side dedupe; integer flicks on the wire; seeded ID generation in test mode** (chosen).
2. **Rely on `expected_rev` only.** Rejected: it prevents stale writes but cannot tell a retried request from a new one, so a repeat after a lost reply either conflicts or double-applies.
3. **Time as strings or `{num, den}` rationals on the wire.** Rejected: heavier on every payload and error-prone in TS. Strings and timecodes are still accepted as input and normalized.
4. **Time as float seconds.** Rejected (see ADR-0004).
5. **Time as `bigint` or decimal strings.** Rejected: flick values for any realistic timeline fit a safe integer (< 2^53, about 147 days), so `number` is exact and ergonomic.
6. **Random IDs only, with normalization in tests.** Rejected: normalization hides real ordering and ID bugs and makes byte-comparison impossible.

## Decision

- Every command call carries a client-generated `request_id` (UUIDv7) and an optional `expected_rev`. The engine journals `request_id` with each applied transaction and deduplicates repeats within a **10-minute window**, returning the original result. After an engine restart, clients resend unacknowledged requests safely.
- `Time` serializes as a JSON **integer** (flicks) and maps to TS `number`, validated as a safe integer. Inputs may also be timecode strings (`"00:01:02:03"`, `"62.5s"`, `"f:1500"`), normalized on parse. The specta export uses a type override for the `Time` newtype.
- `--deterministic-ids <seed>` replaces UUIDv7 generation with a seeded sequence, so parity tests (CLI = MCP = RPC) and golden tests can byte-compare saved project documents.
- Errors are typed (`NotFound`, `Locked`, `Conflict{current_rev}`, `Invalid{path,msg}`, `Unsupported`) with a human message and a machine code.
- `view` commands are not undoable and not journaled (arch §4.1).
- The command schema catalog is generated and snapshot-gated (arch §5.3); changing the wire contract requires an ADR.

## Consequences

**Positive**

- Retries are safe across drops and restarts, including for paid AI submissions and exports.
- No float time bugs across Rust, JSON and TS; one integer type end to end.
- Deterministic parity and golden tests with byte-identical documents.

**Negative / costs**

- The engine must persist and expire the `request_id` window (journal storage, 10 minutes of memory, correct behavior across restarts).
- Clients must generate UUIDv7s and resend only unacknowledged requests.
- Deterministic mode must be impossible to enable accidentally in production; it is a test flag with seeded, collision-prone IDs.
- The 2^53 flick bound (about 147 days) is a hard limit that validation must enforce.

**Follow-ups**

- Epic 1B and 1I (envelope, dedupe), 1N (codegen and type override), 1O (parity tests). MCP's own idempotency surface (research/05 uses `idempotency_key`) maps to `request_id`, see ADR-0009.

## Links

- Architecture: [§4.1](../01-architecture.md), [§5.1](../01-architecture.md), [§5.3](../01-architecture.md), [§20](../01-architecture.md)
- Research: [05-mcp-integration.md](../research/05-mcp-integration.md) §4.2 (idempotency, time, errors)
- Integrator resolution: R10 in [05-work-breakdown.md](../05-work-breakdown.md)
- Related: ADR-0002, ADR-0004, ADR-0005, ADR-0017
