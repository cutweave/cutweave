# ADR-0043: What a commit or cancel sent again after an engine restart gets

- **Status:** Proposed (drafted for issue #251; waits for the product owner)
- **Date:** 2026-11-28
- **Deciders:** Product owner
- **Supersedes:** none. It extends the idempotency rule of [ADR-0015](0015-wire-protocol.md) and the transaction kinds of [ADR-0017](0017-transaction-kinds.md)
- **Architecture refs:** `docs/01-architecture.md` §5.1 (request ids, transactions), §5.2 (the journal), §17 (crash recovery)

## Context

A request carries a `request_id`. While the engine runs, a repeat of a request that succeeded gets its original result back. After a restart the engine knows only what the journal holds, and the journal holds one record per **committed change**: the transaction's commands, the `edit.begin_tx` that opened it and the `edit.commit_tx` that committed it (issue #235).

Three things a client can send again after a restart are not covered. Nothing is lost in any of them.

1. **`edit.commit_tx` of a transaction that changed nothing.** It succeeds (`changed: false`) and writes no record: a journal record carries a new committed revision, and there is none. Sent again after a restart it is refused: `not_found` when it names the transaction, `invalid` ("this connection has no open transaction") when it does not.
2. **`edit.cancel_tx`.** A cancel commits nothing, so nothing is journaled. Sent again it is refused the same way.
3. **A commit or cancel that names no transaction.** `tx` is optional and means "this connection's open transaction". If the client opens a new transaction before it sends an old close again, the old close reaches the new transaction. The request holds nothing that says it is a repeat: the same bytes are a valid first request for the new transaction.

Issue #251 fixed the neighbouring hazard without a contract change: transaction ids no longer repeat across engine runs (a run numbers its transactions from its start time), so a close that **names** an old transaction can never reach a new one and gets `not_found`. The editor, the CLI and the MCP server always name the transaction. The crash harness and several tests do not.

Tests pin today's answers for all three (`crates/cutweave-commands/tests/doc_dedup.rs`), so they change only with this decision.

Two limits of the per-run ids also belong here, because the same journal change would close them:

- Under `--deterministic-ids` the clock is frozen, so ids restart as before. Runs stay reproducible; the reuse stays possible there (tests and the crash harness, not the desktop app).
- If the wall clock steps back across a restart to before the earlier run started, and the journal holds no later transaction, ids can repeat.

## Decision drivers

- A repeat of a request that succeeded should get its result, or a refusal that cannot be mistaken for "your work was lost".
- A close must never act on a transaction other than the one the client meant.
- The journal format and the wire shapes are contracts: no new entry, field or meaning without a decision.
- A cancelled drag is the common case in the editor: whatever is added must cost little per cancel.

## Considered options

### A. The empty commit and the cancel (points 1 and 2)

1. **Keep the refusals (as built).** No cost. The client gets `not_found` or `invalid` for a request that had succeeded. Both first-party clients cope: the editor ignores a failed cancel, and a refused empty commit leaves nothing to redo. An agent that reads the refusal literally may think its (empty) work failed.
2. **Journal a marker for every close that commits nothing.** A new journal entry type beside `commit` and `checkpoint`: the transaction id, how it closed (`committed` with no change, or `cancelled`), and the dedup record of the closing request and of its `edit.begin_tx`. It carries no revision and changes no document, like the checkpoint marker. Recovery restores its requests into the dedup table, so the repeat gets the original result. The same entry records the highest transaction id issued, which closes both limits of the per-run ids. Cost: a journal format addition (a reader that does not know the entry type skips it, so an older engine still recovers the document); one small append per cancel and per empty commit (a journaled rename measures 77 µs at p50 from submit to acknowledgement with group commit; see the journal module's notes); a few hundred bytes per cancelled drag in the segment until the next save; the save's carry-over must keep these requests inside the 10-minute window like the others.
3. **Answer a close of a transaction from an earlier run as "already closed" without a record.** With per-run ids the engine can tell that a named id belongs to an earlier run. It cannot tell how that transaction ended (committed long ago, cancelled, or died with the engine), and `edit.cancel_tx` must answer with the transaction's `kind`, which it does not know. This needs a new output shape or a made-up value, and would report "cancelled" for a transaction that died with unsaved changes. Works only for closes that name the transaction.

### B. The close that names no transaction (point 3)

1. **Keep `tx` optional and say who must name it (as built).** A client that may send a close again names the transaction; a close without `tx` is always "the open one, now". No cost; the hazard stays for a client that does both things the rule forbids (omits `tx` and opens a transaction before repeating an old close).
2. **Require `tx` on `edit.commit_tx` and `edit.cancel_tx`.** The hazard is gone by construction. A wire change that breaks every caller that omits it (the crash harness, tests, any agent that learned the short form), for a mistake no first-party client makes.
3. **Refuse a close without `tx` whose request id is older than the transaction it would close.** Request ids are UUIDv7, which start with a timestamp: compare the close's with the `edit.begin_tx` request id of the open transaction and refuse when the close is older. No wire change, but the engine starts reading meaning into an idempotency key that ADR-0015 treats as opaque. It refuses wrongly when a client mints ids ahead of time or its clock steps back during a drag, does nothing for ids that are not UUIDv7 or that the engine generated, and seeded test ids carry arbitrary timestamps.

## Decision

**Proposed: A2 and B1.**

1. Build the close marker (A2) as its own work package: entry type, writer, replay into the dedup table, carry-over on save, the highest issued transaction id, and the crash harness sending a cancel and an empty commit again after a kill.
2. Keep `tx` optional (B1). The command descriptions of `edit.commit_tx` and `edit.cancel_tx` say that a client which may send the request again must name the transaction.
3. Until A2 is built the refusals stay, pinned by tests.

**What waits on the answer:** only the answers to a repeated cancel or empty commit after a restart, and whether `tx` stays optional. The id reuse fix of #251 is built and does not depend on it.

**Pass criteria for A2:** after a kill, a repeated cancel and a repeated empty commit return the original result with `replayed` set, on the crash harness for at least five seeds; a segment written with markers is recovered by the previous engine build to the same document; the round trip of a cancel stays inside the 16 ms budget (arch §19); with `--deterministic-ids`, a transaction id issued before a kill is not issued again after it.

## Consequences

**Positive**

- Every request that succeeded gets its result back after a restart, not only the ones that changed the document.
- The remaining ways a transaction id could repeat are closed by a record on disk instead of by the clock.
- No caller breaks: `tx` stays optional.

**Negative / costs**

- The journal gains an entry type and grows by one small record per cancelled or empty transaction.
- Cancel, today a pure in-memory operation, waits for a journal append.
- With B1 the engine still cannot protect a client that omits `tx` and repeats an old close after opening a new transaction; it is documented, not prevented.

**Follow-ups**

- If B2 is ever wanted, the generated clients and the MCP instructions can start by always sending `tx`, so the requirement arrives with nothing left to break.
