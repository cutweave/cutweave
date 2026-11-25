# ADR-0023: Timeline edit semantics for Phase 2: magnetic main track, placement modes, attachments, edit reports and replacing gesture updates

- **Status:** Proposed
- **Date:** 2026-11-25
- **Deciders:** Cutweave maintainers (decision); drafted by the WP-2A.01 agent
- **Supersedes:** none (extends the P1 edit semantics of WP-1B.05/1B.06 recorded in arch §4.3)
- **Architecture refs:** `docs/01-architecture.md` §4.2, §4.3, §5.1, §5.2, §14; spec [`docs/spec/timeline-edit-semantics.md`](../spec/timeline-edit-semantics.md)

## Context

Phase 2 turns the P1 walking-skeleton timeline into an everyday editor: a CapCut-style magnetic main track, NLE ripple, roll, slip and slide, linked and attached clips, lift and extract, replace clip, snapping and gestures that stay one undo step. WP-2A.01 writes the normative spec that WP-2A.02 to 2A.14 implement. Several of its decisions change contracts that only an ADR may change (AGENTS.md): command schemas, the project format, the request envelope and the MCP surface. P1 decided the opposite in two places that matter here: "edits never ripple" and "`timeline.insert` rejects overlaps by default" (arch §4.3).

## Decision drivers

- Agents and the editor drive the same commands (ADR-0002): every rule must be explicit, deterministic and reported, never inferred from the pointer.
- P1 agent scripts and the five core MCP edit tools must keep working (R33: no new core tools).
- One user action is one undo step (ADR-0017, ADR-0019), including drags that reorder the main track.
- Property tests must be able to state every invariant (arch §20).

## Considered options

1. **Extend the P1 commands compatibly and add the P2 vocabulary, with a separate attachment relation and replacing gesture updates** (leading; specified in full by the spec).
2. **Reuse `linked_group` for CapCut attachments** (arch §4.3's wording). Rejected: linked groups are symmetric and aligned (trimmed, split and deleted together); an attached overlay must follow its parent's moves but not its trims or deletion.
3. **Change the defaults to the CapCut ones** (`new_track` on free tracks, clamping trims). Rejected: silently changes the behaviour of core MCP tools; the editor can pass these modes explicitly.
4. **Keep accumulating gesture updates (P1) and make every command path-independent.** Rejected: magnetic reorders, ripples and auto-created tracks do not compose; drags would leave stray tracks and drift.

## Decision

Proposed, not decided. **Decided by:** a maintainer's approval of the WP-2A.01 pull request (spec review), before WP-2A.02 and WP-2A.03 start. Pass criteria: the spec's fixtures pass `cargo xtask check-fixtures`, and the reviewer accepts each change below. If a change is rejected, the spec is amended and the affected WPs follow the amended spec; the fallback for every item is the P1 behaviour.

The contract changes, each detailed in the spec section named:

1. **Command schemas (additive).**
   - New commands: `timeline.overwrite`, `timeline.ripple_delete`, `timeline.lift`, `timeline.extract`, `timeline.delete_gap`, `item.duplicate`, `clip.ripple_trim`, `clip.roll`, `clip.slip`, `clip.slide`, `clip.replace`, `item.link`, `item.unlink`, `item.attach`, `item.detach`, `track.set_magnetic` (spec §4). All are MCP catalog tools.
   - New optional params on P1 commands: `timeline.insert{mode, audio_track, ripple_all_tracks}`, `item.move{items, delta, mode, ripple_all_tracks, fine}`, `clip.split{items, fine}`, `clip.trim{clamp, fine}`. The P1 `overlap` param stays as an alias of `mode` (spec §3.6, §11).
   - Every edit command's output gains `report: EditReport` (spec §3.1), and the trim family gains `applied_delta` and `clamped`.
2. **Behaviour on magnetic tracks.** No P1 document has a magnetic track, so P1 behaviour is unchanged on every existing document. On the magnetic track: placements insert by default, deletes and trims close the space (ripple), moves lift and insert (spec §1.2, §3.4, §3.6). The ripple reaches the tracks of linked partners of shifted items; others only with `ripple_all_tracks`.
3. **Default durations snap to whole frames** (floor for the media remainder, nearest for the 5 s still default), where P1 left them unsnapped (spec INV-25).
4. **Project format v2** (with WP-2A.02's planned bump): `Item.attached_to: Option<ItemId>`; `Project::validate` enforces at most one magnetic track per sequence (a video track), a gap-free magnetic track and aligned linked groups. The migration floors off-grid clip durations, dissolves misaligned groups and keeps the first of several magnetic flags (spec §1.5).
5. **Request envelope:** `_meta.replace: true` on a command inside an open `gesture` transaction rolls the transaction's live changes back before applying it, atomically (spec §5, INV-301/302). WP-2A.12 implements it in `cutweave-commands`.
6. **Error shape:** `Conflict{reason:"item_in_gesture"}` gains `holder` (the actor holding the lock), as WP-2A.12 already plans; arch §5.2 notes this needs an ADR.

## Consequences

**Positive**

- One placement algebra and one ripple rule for every command, so magnetic, locked and linked behaviour lives in one place (WP-2A.03's `PlacementPlan`).
- Agents get exact, reported results (`EditReport`) and never a CapCut heuristic they did not ask for.
- A drag of any length is one command applied once, so undo, soft locks and auto-created tracks behave.

**Negative / costs**

- Two relations (links and attachments) to teach users and agents; the UI needs badges for both (WP-2A.09/2A.10).
- Ripple collisions refuse some edits that CapCut would allow by rearranging tracks; the error names the blocking item.
- P1 tests that assert unsnapped default durations change in WP-2A.03.
- The v2 migration may shorten clips by less than one frame.

**Follow-ups**

- WP-2A.02 (model, migration, `track.set_magnetic`), 2A.03, 2A.04, 2A.05, 2A.06, 2A.07, 2A.08, 2A.11, 2A.12, 2A.14 implement the spec.
- Arch §4.3 and §5.2 point to the spec now; when this ADR is accepted, the integrator replaces the P1 "edits never ripple" bullet with a pointer to the spec and lists ADR-0023 in §23.
- `docs/user-flows.md` F37–F39 cover the user-visible behaviour (blocked by WP-2A.10).

## Links

- Spec: [timeline-edit-semantics.md](../spec/timeline-edit-semantics.md), fixtures in [`docs/spec/fixtures/`](../spec/fixtures/)
- Architecture: [§4.3](../01-architecture.md), [§5.2](../01-architecture.md)
- Work packages: [P2-core-editor.md](../wbs/P2-core-editor.md) epic 2A
- Related ADRs: ADR-0002, ADR-0004, ADR-0005, ADR-0017, ADR-0019
