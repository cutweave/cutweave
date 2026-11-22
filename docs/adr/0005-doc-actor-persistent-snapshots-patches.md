# ADR-0005: Single-writer document actor, `imbl` snapshots and invertible patches (CRDT later)

- **Status:** Accepted
- **Date:** 2026-11-22
- **Deciders:** Cutweave maintainers (product owner approved)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §5.2, §5.3, §8, §22; see also ADR-0017

## Context

The project document is mutated by the user, agents and plugins at the same time, while the render thread, audio engine and queries read it. Every edit must be undoable and replayable (brief principle 3), per-actor history must work ("undo the agent's last change"), and the render and audio hot paths cannot take locks.

## Decision drivers

- No locks or allocations on real-time paths; readers need consistent snapshots.
- Linear, deterministic history that supports undo, redo, checkpoints and journal replay.
- Per-actor undo and optimistic concurrency (`expected_rev`).
- Keep a future collaboration path open without paying for it in v1.

## Considered options

1. **Single-writer doc actor + persistent (`imbl`) snapshots + forward/inverse patch ops** (chosen).
2. **Mutex- or RwLock-guarded mutable document.** Rejected: readers block writers on hot paths, and undo needs a separate mechanism.
3. **CRDT-backed document (Loro, Yrs, Automerge) from day one.** Rejected for v1: memory and size overhead, a model constrained to CRDT types, and ripple edits are global operations that merge poorly. Research 03 §5.3 recommends commands now and ops designed so a Loro-backed model can follow.
4. **Snapshot-only undo (store a full copy per step).** Rejected: too heavy for large projects. Persistent structures give O(1) snapshots, but a patch log is still needed for the journal and per-actor history.

## Decision

All commands run serially on one document actor. Readers receive immutable snapshots (`imbl` persistent collections, `Arc` swap per revision). `Tx` records forward and inverse patch ops while it mutates; undo applies the inverses, redo the forward ops. Transactions group commands into one undo step, and ephemeral transactions cover continuous gestures. Every transaction is tagged with an actor derived server-side from the credential (ADR-0016). Patch ops are designed to map onto a CRDT (Loro) later; **no CRDT in v1**. Transaction kinds, connection ownership, soft locks and committed-only saves are specified in ADR-0017.

## Consequences

**Positive**

- Lock-free reads on the render thread, audio and queries; O(1) snapshots also give cheap `edit.checkpoint` / `edit.restore`.
- Linear history is easy to journal, replay and reproduce (bug reports, agent sessions).
- Property tests can assert `undo(apply(doc)) == doc` for every command (arch §20).
- Per-actor undo and `expected_rev` conflicts make concurrent user and agent editing tractable.

**Negative / costs**

- One writer is a throughput ceiling; commands must stay fast, and long work belongs in jobs (arch §7, §13).
- Every command needs a correct inverse; the property-test burden is mandatory, not optional.
- Persistent collections cost some CPU and memory versus plain `Vec`; the benchmark budgets in arch §19 are the guard.
- Real-time collaboration is deferred and may require translating patch ops, not just swapping storage.

**Follow-ups**

- Epic 1B (bus, tx, undo, property tests); ADR-0017 for gesture and proposal semantics.

## Links

- Architecture: [§5.2](../01-architecture.md), [§5.3](../01-architecture.md), [§22](../01-architecture.md)
- Research: [03-tech-stack.md](../research/03-tech-stack.md) §5.3 (undo/redo and collaboration), [05-mcp-integration.md](../research/05-mcp-integration.md) §4.2 (transactions, optimistic concurrency)
- Related: ADR-0002, ADR-0006, ADR-0015, ADR-0016, ADR-0017; R11, R22 in [05-work-breakdown.md](../05-work-breakdown.md)
