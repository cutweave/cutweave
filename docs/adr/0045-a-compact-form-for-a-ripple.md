# ADR-0045: A compact form for a ripple in patches and edit reports

- **Status:** Accepted (product owner, 2026-11-28): not now; option 1 (issues #231 and #230)
- **Date:** 2026-11-28
- **Deciders:** Product owner
- **Supersedes:** none. It would extend the patch format of [ADR-0005](0005-doc-actor-persistent-snapshots-patches.md) and the edit report of [ADR-0023](0023-timeline-edit-semantics.md) (timeline spec INV-21)
- **Architecture refs:** `docs/01-architecture.md` §5.2 (patches, the journal, `doc.patched`), §19 (budgets)

## Context

A ripple moves every clip after a point by the same amount. The engine describes it one clip at a time, in three places:

- **the patch**: one `replace …/items/<id>/start` op per shifted clip, about 260 bytes each. It is what the journal records, what `doc.patched` carries and what the undo history keeps;
- **the edit report** in the command's response: one `moved` entry per shifted clip, about 110 bytes each (INV-21: the report names every item whose track or start changed);
- **the soft locks** of a gesture: one entry per shifted clip (INV-304).

Issue #231 measured what that cost on the 5,000-clip benchmark project (P-5K, 2,000 clips on the magnetic main track): 440 ms for an insert at the head of the track, and a drag onto the main track that updated every 0.6 to 2 seconds with 234 to 933 KB of `doc.patched` per update.

The work for #231 and #230 removed the costs that were not part of a contract (the measurements are in the pull request and in `bench/README.md`'s metrics):

| On P-5K (engine suite, three alternated runs each, dev laptop) | Before | After |
|---|---|---|
| Insert at the head of the main track (2,000 clips shift), `placement_ripple_p5k_ms` | 442 to 463 ms | 23 to 25 ms |
| Drag update along the main track, round trip over the WebSocket, p50 / p95 | 36 to 39 / 66 to 67 ms | 5 to 6 / 7 to 8 ms |
| Drag update onto the main track (1,000 clips make room), p50 / p95 | 243 to 254 / 272 to 297 ms | 13 to 14 / 16 to 17 ms |
| Received per update of the drag along / onto the main track (the response and `doc.patched`) | 81 / 606 KB | 20 / 138 KB |

What is left grows with the number of shifted clips because the contracts say so:

1. **The first update and the commit of a ripple** still carry one op per shifted clip: about 520 KB for 2,000 clips, written to the journal, sent to every subscriber and applied by the editor (about 15 ms for 2,000 ops after the fix to `applyPatch`). Of the 24 ms of the head insert, roughly half is building, applying, journaling and logging those 2,000 ops; the rest is planning the edit (twice: validation and application) and its report.
2. **Every response of a ripple command** lists every shifted clip in `report.moved`: this is nearly all of the 138 KB per update of a drag onto the main track with 1,000 clips after the drop point (the `doc.patched` of such an update is now a few ops), although the editor reads only whether the move was clamped.
3. **A gesture keeps the ops of each of its updates** for request deduplication until it closes, so a long drag that ripples holds updates × shifted clips ops in memory (500 updates over 1,000 clips: some 150 MB). This is in-memory only.

## Decision drivers

- A drag stays interactive on a long timeline: the engine's answer well under 100 ms and the editor's work under a frame or two, at 5,000 clips today and at several times that later.
- The patch is the one description of a change: the journal, undo, the UI mirror, the soft locks, plugins and agents all read it. A new op kind touches each of them.
- An agent reads the edit report to learn what its edit did; INV-21 promises it is exact.
- No format changes without need: projects, journals and recorded sessions outlive a build.

## Considered options

1. **Leave the formats as they are** (what this branch ships). Ripples cost what the table says; the cost grows linearly with the clips after the edit point.
2. **A `shift` patch op**: `{op: "shift", path: "…/tracks/<id>/items", from, by}` ("every item of this track that starts at or after `from` moves by `by`"), invertible by negating `by` at the shifted `from`. One op instead of N in the journal, in `doc.patched` and in the undo history; the engine applies it as a typed range update and the editor's mirror as one pass over the track. Needs: the op in the patch schema and in `apply_patch_json` and its TypeScript twin, an inverse, the soft locks and the revision log reading the items it covers from the document (the op no longer names them), the journal's format version, the patch fixtures, and every reader of patches (plugins, agents that follow `doc.patched`) to learn it. Estimated effect on the head insert: the 2,000-op half goes away, leaving the plan and the report (about half of the 24 ms); `doc.patched` and the journal record shrink from about 520 KB to under 1 KB.
3. **Option 2, and a range form in the edit report**: `report.shifted: [{track, from, by, count}]` for the clips a ripple moved, with `moved` naming only the clips the command itself placed. The response of a ripple shrinks from about 110 bytes per shifted clip to a constant. It changes INV-21 (the report would no longer name every moved item by ID) and what agents read today.
4. **Keep the report exact but optional**: a `report: "summary"` request option that returns counts instead of lists; the editor asks for the summary during a drag. No change for agents that do not ask; the patch stays as it is.

## Decision

**Option 1, not now** (product owner, 2026-11-28).

A ripple keeps one patch op and one report entry per shifted clip. No compact `shift` op is built, and the patch, event, journal and report formats stay as they are.

The reason: after the fixes for #231 and #230 a ripple on P-5K is interactive without a format change, and the cost that remains matters at project sizes the product does not target yet.

The question comes back when drags on timelines of 10,000 clips and more become a goal. Option 2 is then the one that removes the linear cost at its source, and option 4 is the cheap way to shrink the responses of a drag.

## Consequences

- Option 1: nothing to migrate. The linear cost stays and is measured on every benchmark run (`placement_ripple_p5k_ms`, `main_insert_drag_p5k_*`, `ui_main_drag_*`), so a regression or a larger target shows.
- Options 2 and 3: a new op kind and a journal format version; every patch reader changes once; recorded patch fixtures gain cases. Old journals stay readable (they hold only the old ops).
- Option 4: an additive request option on the placement commands; no reader changes unless it asks for it.
