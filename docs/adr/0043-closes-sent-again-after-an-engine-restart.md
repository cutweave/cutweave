# ADR-0043: What a commit or cancel sent again after an engine restart gets

- **Status:** Accepted (product owner, 2026-11-28): A2 and B1; built under issue #251
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

When this was drafted, tests pinned those answers for all three (`crates/cutweave-commands/tests/doc_dedup.rs`), so that they would change only with this decision. The Decision section says what they are now.

Two limits of the per-run ids also belong here, because the same journal change closes them:

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

**Accepted (product owner, 2026-11-28): A2 and B1.** Built under issue #251.

### A2: the journal keeps a marker for what commits no change

1. **The entry.** Journal format 3 adds one entry type, `tx`, beside `commit` and `checkpoint`. It has no revision and changes no document. It holds `issued` (every transaction id up to this number may have been handed out), `at`, and, when it marks a close, `closed` (`tx_id`, `kind`, `outcome`: `committed` with no change, or `cancelled`) and `steps` (the requests a restarted engine keeps answering, each with its dedup record). The carry-over of a save gains `tx_issued`.
2. **When it is written.**
   - *A close that commits nothing* (an `edit.cancel_tx`, or an `edit.commit_tx` of a transaction that changed nothing), before the request is acknowledged. `steps` is what the running engine remembers of that transaction: the `edit.begin_tx` that opened it, the commands of an empty commit, and the closing request. The commands of a cancelled transaction are not kept, as on a running engine.
   - *A reservation of transaction ids*, before a transaction is opened with an id past the last reservation: `issued` alone, 64 ids at a time. This was not in the draft: a marker written at the close cannot protect the id of a transaction that is still open when the engine dies, which is the case issue #251 is about. The id has to be on disk before a client holds it. One small append per 64 transactions buys that.
   - *Again after the journal's tail was cut* (the unsaved work was discarded, or the last record was torn), so the ids stay taken if that engine dies before it opens a transaction.
3. **What a failed write does.** A reservation that cannot be written refuses `edit.begin_tx` (the journal is in the state in which it refuses commits too). A close marker that cannot be written is logged and the close stands: nothing is lost, and its repeat after a restart gets the refusal it got before this ADR.
4. **Recovery.** Replay takes a marker's requests into the dedup table (inside the 10-minute window, as for every request) and numbers transactions past the highest `issued` and the highest committed id it can read anywhere in the chain, also in the part a discard or a damaged record leaves out. A marker cut short by a crash is a torn tail like any other record.
5. **Compatibility.** Nothing reads the format number to refuse a segment. An engine of format 2 reads a format 3 segment: it skips `tx` entries (an entry of an unknown type is skipped, as the format has promised since version 1) and the unknown `tx_issued` field, recovers the same document, and answers as it did before this ADR. A format 3 engine reads formats 1 and 2 unchanged: no markers, ids continue past the last committed transaction.
6. **Per-run ids stay** (`DocConfig::with_tx_ids_per_run`, issue #251 part 1) as the second line. The marker covers a journaled project on any clock, `--deterministic-ids` included. The per-run numbering covers what has no journal record to rely on: an untitled document, and a reservation lost with the last 50 ms of writes in a power cut. The wire shape stays `tx_<n>`.

### B1: `tx` stays optional

`edit.commit_tx` and `edit.cancel_tx` without `tx` mean the connection's open transaction when the request runs. A client that may send a close again names the transaction. The rule is stated in [the RPC reference](../reference/rpc.md) (`_meta` and the request envelope). A close without `tx` that is sent again after its client opened another transaction still closes that one; a test pins it.

### What a repeat gets after a restart

| Sent again, same `request_id` and params | Answer |
|---|---|
| `edit.cancel_tx` that was acknowledged | its original result, `replayed` |
| `edit.commit_tx` of a transaction that changed nothing, acknowledged | its original result (`changed: false`), `replayed` |
| the `edit.begin_tx` of either | its original result, `replayed` (the transaction is closed; nothing opens) |
| a close the journal never had (not acknowledged, or its marker was torn or could not be written) | `not_found` when it names the transaction, `invalid` when it does not, as before |
| a close naming a transaction the engine died with, after another was opened | `not_found`; the new one is untouched |
| a close without `tx`, after another transaction was opened | closes the open one (B1) |

**Pass criteria, as met:** after a real kill a repeated cancel and a repeated empty commit return the original results marked `replayed` (`crates/cutweave-engine/tests/engine_crash.rs`), and on the crash harness, which now sends both again after every crash and checks the id of a gesture the engine was killed with; a format 2 reader's view of a format 3 segment and a format 3 reader's view of a segment without markers are tested (`journal/format.rs`, `tests/journal.rs`); on a frozen clock a transaction id issued before a restart is not issued again, across a restore, a discard and a save.

## Consequences

**Positive**

- Every request that succeeded gets its result back after a restart, not only the ones that changed the document.
- The remaining ways a transaction id could repeat are closed by a record on disk instead of by the clock.
- No caller breaks: `tx` stays optional.

**Negative / costs**

- The journal gains an entry type and grows by one small record per cancelled or empty transaction.
- A cancel that came as a request waits for a journal append; so does opening a transaction, once per 64.
- After a restart transaction ids skip to the end of the last reserved block, so they are no longer consecutive across runs.
- With B1 the engine still cannot protect a client that omits `tx` and repeats an old close after opening a new transaction; it is documented, not prevented.

**Follow-ups**

- If B2 is ever wanted, the generated clients and the MCP instructions can start by always sending `tx`, so the requirement arrives with nothing left to break.
