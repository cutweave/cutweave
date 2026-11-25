# Timeline edit semantics (normative spec for Phase 2)

> **Status:** draft for review (WP-2A.01). It becomes the contract when a maintainer approves the PR. The contract changes it implies (new commands and params, project format v2 additions, the gesture `replace` flag) are drafted in [ADR-0023](../adr/0023-timeline-edit-semantics.md) (**Proposed**); until that ADR is accepted, the P1 rules of [architecture §4.3](../01-architecture.md) stay in force.
> **Implemented by:** WP-2A.02 (model), 2A.03 (placement), 2A.04 (trims), 2A.05 (links), 2A.06 (clipboard), 2A.07 (snapping), 2A.08 (interaction), 2A.11 (replace), 2A.12 (gesture locks), 2A.14 (three-point editing).
> **Executable part:** the scenario fixtures in [`fixtures/`](fixtures/) and the checker `cargo xtask check-fixtures` (§9).

The words MUST, MUST NOT and MAY are normative. Every rule a test can check has an id `INV-n`. Ids are stable: a retired id is never reused, and new ones take the next free number of their block.

## Contents

0. [Reading this spec](#0-reading-this-spec)
1. [Model and terms](#1-model-and-terms)
2. [Document invariants](#2-document-invariants)
3. [Rules every edit command follows](#3-rules-every-edit-command-follows)
4. [Commands](#4-commands)
5. [Gestures and soft locks](#5-gestures-and-soft-locks)
6. [Snapping](#6-snapping)
7. [Selection](#7-selection)
8. [Not supported in P2](#8-not-supported-in-p2)
9. [Fixtures](#9-fixtures)
10. [Decisions and alternatives](#10-decisions-and-alternatives)
11. [Compatibility with the P1 commands](#11-compatibility-with-the-p1-commands)
12. [Hand-off to later work packages](#12-hand-off-to-later-work-packages)

---

## 0. Reading this spec

**Diagrams.** One line per track, top to bottom in display order. `*` after a track name marks the magnetic (main) track, `#` a locked track. `[B 30-60]` is item `B` from frame 30 to frame 60 (half-open: it ends where frame 60 starts). A lowercase item (`b`) is the linked audio of the uppercase one (`B`): same media, same start and duration, same linked group. All diagrams use frames at 25 fps, like the fixtures.

```text
V2      [T 35-45]                    T is an overlay attached to B (T -> B)
V1*  [A 0-30][B 30-60][C 60-90]      the main track: gap-free from 0
A1   [a 0-30][b 30-60][c 60-90]      linked audio of A, B, C
```

**Times.** Every time is a `Time` in flicks (ADR-0004). Params accept the P1 `TimeInput` forms (flicks, `"1.2s"`, `"f:30"`, timecode). Relative params (`delta`) may be negative: write them in flicks or seconds (`"-0.4s"`), since `"f:N"` takes no sign.

**Relation to P1.** P1 (WP-1B.05/1B.06, arch §4.3) shipped `timeline.insert`, `item.move`, `item.delete`, `clip.split`, `clip.trim` and `item.set_props` with no ripple and no magnetic track (the `Track.magnetic` flag exists but nothing sets it). This spec keeps every P1 behaviour on tracks that are not magnetic, extends the P1 commands compatibly and adds the rest of the editing vocabulary. §11 lists every change to a P1 command.

## 1. Model and terms

### 1.1 Sequence, tracks and items

- A **sequence** has a frame rate (rational, exact in flicks), a sample rate and **tracks** in display order (top to bottom). Upper video tracks composite over lower ones.
- A **track** has a kind (`video`, `audio`; `text`, `caption` and `adjustment` stay `Unsupported` for editing until their epics), the flags `locked`, `hidden`, `muted`, `solo` and `magnetic`, and its items.
- An **item** occupies the half-open range `[start, end)` with `end = start + duration`. A **clip** also plays the media range that starts at `source_in` and lasts `duration × speed` (speed is 1 until WP-2D.01 adds it; §3.7 gives the speed rule).
- **Free track:** any track that is not magnetic. **Gap:** a maximal empty range between two items of a track (or between 0 and its first item). The empty range after the last item is not a gap.
- **Media bound.** A clip's source range MUST lie inside its media when the media has a known length and is not a still image (§2, INV-4). Stills, generators and media of unknown length are **unbounded**: trims may extend them freely and slip has nothing to move.

### 1.2 The main (magnetic) track

The **main track** is the sequence's magnetic track: a video track with `magnetic: true`. There is at most one. It behaves like CapCut's main track with Track Magnet on: it never holds a gap and starts at 0, so placements insert by default, and deletes and trims close the space they leave. Every other track is free and behaves like an NLE track.

A sequence MAY have no main track (P1 documents, or the user turned the magnet off). New sequences get one when the editor creates them with the Timeline preference "Main track magnetic in new sequences" on (WP-2A.13, WP-2B.01).

### 1.3 Linked groups

Items sharing a `linked_group` (`grp_…`) are **linked**: they move, trim, split, are deleted and are duplicated together. In P2 a linked group is always **aligned**: every member has the same `start` and `duration`, each on its own track. The usual group is a video clip and its audio (`timeline.insert` creates it), and `item.link` can link any aligned items. An edit on some members only (`unlink: true`) takes those members out of the group. A group never keeps a single member (P1 rule).

### 1.4 Attachments

An item MAY be **attached** to a parent item (`attached_to: ItemId`, new in schema v2): CapCut's "linked to the main clip". A child follows its parent when the parent is translated (moved, shifted by a ripple, slid) and keeps its own time when the parent is trimmed, unless it would fall outside the parent (then it is clamped, INV-36). Attachments are one level deep: a parent is never attached itself and a child is never a parent. Typical children are overlays, titles, stickers and sound effects that belong to a moment of a main-track clip.

### 1.5 Model additions this spec needs (WP-2A.02, schema v2)

| Addition | Rule |
|---|---|
| `Item.attached_to: Option<ItemId>` (every item kind) | INV-9. Serialized as `null` when absent, like `linked_group`. |
| `Sequence.markers`, `Item.markers` (`Marker {id, t, duration?, label, color, kind: user\|beat\|chapter}`) | As planned in WP-2A.02. Item markers use item-relative time. Analysis beats (P5) are item markers on the analysed clip, so they move with it. |
| `Sequence.in_out: Option<{start, end}>` | Both ends or neither. |
| `Track.magnetic` | INV-6 and INV-7 are validated by `Project::validate`. |
| Linked groups aligned | INV-8 is validated by `Project::validate`. |

**Migration v1 → v2** (in addition to WP-2A.02's planned steps): every clip duration that is not a whole number of frames of its sequence is floored to whole frames (at least one frame); a linked group whose members are not aligned is dissolved (all members unlinked); if a document has several magnetic tracks, only the first in display order keeps the flag. P1 never produces the last two, and the first only for clips placed with their default length (the rest of the media, or a 5 s still at an NTSC rate).

## 2. Document invariants

These hold for every committed document and after every successful command. `Project::validate` checks the ones marked *(validate)*; the property tests of WP-2A.03/2A.04/2A.05 check all of them after every command.

- **INV-1** *(validate)* No overlap: on every track the items' ranges are pairwise disjoint, on every track kind.
- **INV-2** *(validate)* `start >= 0` and `duration >= 1` frame for every item (on audio tracks, at least one sample after a `fine` edit, INV-26).
- **INV-3** Frame grid: on video tracks every `start` and `duration` is a whole number of frames of the sequence rate. On audio tracks the same holds except for edit points a `fine` edit set (INV-26).
- **INV-4** *(validate)* Source bounds: for a clip of bounded media, `source_in >= 0` and `source_in + duration × speed <= media length`.
- **INV-5** *(validate)* Every item sits on a track whose kind accepts it (P1 `TrackKind::accepts`).
- **INV-6** *(validate)* A sequence has at most one magnetic track, and it is a video track.
- **INV-7** *(validate)* The magnetic track is gap-free: its first item starts at 0 and each item starts where the previous one ends. An empty magnetic track is valid.
- **INV-8** *(validate)* Every linked group has at least two members, all in one sequence, with identical `start` and `duration`.
- **INV-9** *(validate)* If `child.attached_to = P`: `P` exists in the same sequence; `P.attached_to` is empty; no item is attached to `child`; `child` is on another track than `P` and not in `P`'s linked group; `P.start <= child.start < P.end`; and every member of `child`'s linked group is attached to `P` too.
- **INV-10** IDs are unique across the project, and a sequence's duration is the largest item end on any of its tracks (0 when empty).

## 3. Rules every edit command follows

### 3.1 Atomicity and the edit report

- **INV-20** All or nothing. A command either applies completely or fails with a typed error and changes nothing: the document, `rev` and history stay byte-identical. Validation and application compute the same plan (P1 `plan()` pattern), so `validate` reports every error `apply` would.
- **INV-21** Every successful edit command returns `report: EditReport` next to its own output fields. The report is exactly the difference between the document before and after (one sequence):

  ```text
  EditReport {
    created:        [ItemId]                       new items, other than split right halves
    removed:        [ItemId]                       items that no longer exist
    split:          [{left, right, at}]            right is new; left keeps its id; at = cut time before the edit
    moved:          [{item, track, start}]         track or start changed, duration and source_in did not
    trimmed:        [{item, start, duration, source_in}]  duration or source_in changed (start may too)
    relinked:       [{item, linked_group, attached_to}]   either link field changed (new values)
    replaced:       [{item, media}]                media changed (clip.replace)
    toggled:        [{item, enabled}]              enabled changed (clip.replace muting old audio)
    created_tracks: [TrackId]                      in creation order
  }
  ```

  For every item id: present only after ⇒ in `created` or a `split[].right` (exactly one of them); present only before ⇒ in `removed`; present in both ⇒ in each list whose condition its fields meet, and in none otherwise. A split's left half is also in `trimmed` (its duration changed). Lists are sorted by item id, `split` by `(left, at)`. No other item field changes in any command of this spec (WP-2C.04 adds keyframe retiming to the report additively).
- **INV-22** `undo(apply(doc)) == doc` and `redo` reapplies it exactly (ADR-0005). A command that changes nothing succeeds with an empty report, does not bump `rev` and adds no history entry (P1 no-op rule).
- **INV-23** Deterministic: the same document, params and ID seed give the same result, report and new IDs. Each command draws its new IDs in a fixed order that depends only on the document and the params (P1 commands keep their current order; each new command states its order in its module docs). Fixture tests bind new entities by structure, not by ID order (§9.1).

### 3.2 Time and the frame grid

- **INV-24** Every time param is resolved against the sequence's frame rate and snapped to the nearest frame boundary (ties go later); every relative param (`delta`, `offset`) to a whole number of frames the same way. Outputs state the values used. `source_in` is a media position and is not snapped (P1).
- **INV-25** A default duration is a whole number of frames: the rest of the media is floored to whole frames (never reading past its end), at least one; a still's 5 s default is snapped to the nearest frame (150 frames at 29.97 fps). Media shorter than one frame cannot be placed (`Invalid{path:"/source_in"}`).
- **INV-26** `fine: true` (accepted by `item.move`, `clip.split`, `clip.trim`, `clip.ripple_trim` and `clip.slip`) snaps to the sequence's audio sample grid instead of frames. It is valid only when every item the command changes is on an audio track and not linked to an item on a video track; otherwise `Invalid{path:"/fine"}`. A fine edit point stays where it is when a later ripple shifts it by whole frames.

### 3.3 Locks

- **INV-27** A command fails with `Locked{kind:"track", id}` exactly when its plan, computed as if nothing were locked, would add, remove or change an item of a locked track (any field: times, source, links, attachment) or remove a locked track. Creating a track next to a locked one does not change it. `id` is the first such track in display order. The check comes after `NotFound` and parameter errors and before every other semantic check, so a locked edit is reported as `Locked` even if it would also be out of range.
- **INV-28** Consequently, after a successful command the items of every locked track are unchanged. Ripples that would shift a locked track fail, partners and children on locked tracks block the edit of their group or parent, and a ripple does not "skip" a locked track.
- **INV-29** `hidden`, `muted` and `solo` never change what an edit command does.

### 3.4 Ripple

A **ripple** shifts the items that come after an edit point. Every rippling command does it in phases (lift, cut, close, open, place) and each phase has at most one ripple, given by:

- the **ripple track**: the track the edit happens on;
- a **threshold** `θ` (or several, for multi-range edits) and a **shift** `f(t)`: the amount added to an item that starts at `t` (0 before the first threshold);
- the **rippled tracks** `R`.

- **INV-30** Every item on a track of `R` that the phase does not itself remove or place, and whose start `s >= θ`, moves to `s + f(s)`. Nothing else in `R` changes: a ripple never splits or trims an item on a track other than the ripple track.
- **INV-31** `R` is the smallest set that contains the ripple track and every track holding a linked partner of an item that shifts. With `ripple_all_tracks: true`, `R` is every track of the sequence. Tracks holding only attached children are not in `R` (children follow item by item, INV-36).
- **INV-32** Ripple collision: if a shift would make an item of a track in `R` overlap an item of that track that does not shift (one that starts before `θ` and ends after its shifted neighbour's new start), the command fails with `Invalid` naming both items and the track (under `clamp: true` the collision bounds the delta instead, §3.7).
- **INV-33** A ripple whose ripple track is not the magnetic track MUST NOT move items of the magnetic track: if the main track is in `R` and any of its items would shift, the command fails with `Invalid` ("the main track cannot open or close a gap here; edit on the main track instead").
- **INV-34** Sequence markers and the sequence in/out never move with a ripple. Item markers belong to their item and move with it.

### 3.5 Who follows an edit

- **INV-35** Linked groups. Every member of a group receives the same edit as the item named in the command: the same translation (on its own track), the same edge delta, the same split time, the same removal, the same duplication. With `unlink: true` (`item.move`, `clip.trim`, `clip.ripple_trim`) only the named items are edited and they leave their groups; a group left with one member is dissolved in the same transaction (P1). A member's constraints (bounds, neighbours, locks) constrain the whole group.
- **INV-36** Attachments. When a parent is translated by `δ` (moved, shifted by a ripple, slid), each child not otherwise edited by the command is translated by `δ` on its own track. When a parent's extent changes (trim, ripple trim, roll, replace), children keep their start unless it now lies outside `[parent.start, parent.end − 1 frame]`, in which case it is clamped into that range. An item gets one shift per phase: if it is on a track of `R`, the ripple rule (INV-30) decides, not its parent.
- **INV-37** A child that would overlap a non-moving item of its track after following goes to a new track of the same kind directly above its track (reported in `created_tracks`); it never blocks its parent's edit, unless its track is locked (INV-27).
- **INV-38** A child is detached (its `attached_to` cleared, reported in `relinked`) when its parent is removed (the child keeps its place), when it ends up on its parent's track, or when after the edit its start lies outside its parent's range for any other reason than a change of the parent's extent (which clamps it, INV-36): the ripple rule moved one and not the other, or the child itself was moved. Children of a removed parent are never removed with it.
- **INV-39** Splitting a parent re-attaches every child whose start is at or after the split time to the right half (reported in `relinked`).

### 3.6 Placement

Placing items on a track (inserting media, moving, duplicating, pasting) uses one **placement mode**:

| Mode | At the target range `[t, t+d)` on the target track |
|---|---|
| `insert` | If `t` is strictly inside an item, split it at `t` (INV-43). Ripple the target track at `θ = t` by `+d` (INV-30/31), then place. |
| `overwrite` | Cut `[t, t+d)` out of the target track (INV-44), then place. No ripple. |
| `new_track` | Place on the target track if the range is free; otherwise on a new track of the same kind created directly above it. |
| `reject` | Place if the range is free; otherwise `Invalid` at the time param, naming the item in the way. |

- **INV-40** Default mode: `insert` when the target track is magnetic, `reject` otherwise (P1's agent-safe default, arch §4.3). The editor always passes the mode its gesture and the Editing preferences mean (WP-2A.08, WP-2A.13): `new_track` when a dragged clip overlaps on a free track; for media dropped onto a clip outside the main track, the preference "dropping media onto a clip outside the main track" (`overwrite` by default, or `insert`); nothing special for a drop on free space. The P1 param `overlap: "reject"|"new_track"` stays as an alias of `mode`; giving both with different values is `Invalid{path:"/mode"}`.
- **INV-41** On the magnetic track, a placement time is clamped into `[0, end]`, where `end` is the track's end after any lift of the same command. Placing on an empty magnetic track always places at 0. `reject` on the magnetic track therefore only succeeds at its end (append).
- **INV-42** `new_track` (and INV-37) creates a track of the same kind directly above the track in the way, named like P1 (`V3`, `A2`, …), never magnetic. Tracks are never removed by an edit command when they become empty (agents keep stable track IDs).
- **INV-43** Insert inside an item splits only items of the target track (and, by INV-35, their linked partners). It never splits items of other rippled tracks.
- **INV-44** Overwrite cut of `[t, u)` on a track: an item inside it is removed; an item that straddles `t` has its end trimmed to `t`; an item that straddles `u` has its start trimmed to `u` (its `source_in` advances accordingly); an item that spans both is split into a left part `[start, t)` that keeps its id and a right part `[u, end)` with a new id. Every member of an affected item's linked group gets the same cut on its own track, so groups stay aligned; right parts of a group form a new group.
- **INV-45** Linked audio of inserted media (`link`, default `true`, a video file with sound placed on a video track): the audio goes to `audio_track` when given (it MUST then be free over the range after the command's own changes, else `Invalid{path:"/audio_track"}`); otherwise to the first unlocked audio track in display order that is free over `[t, t+d)` after the command's own ripple and cuts; otherwise, in `insert`, `overwrite` and `new_track` modes, to a new audio track above the first unlocked audio track; otherwise (in `reject` mode, or when there is no unlocked audio track) the picture is placed alone (P1).

### 3.7 Clamping

Trims, rolls, slips, slides and ripple trims change an edge or a source position by a delta `δ`. Each constraint (minimum duration of one frame, media bounds, neighbours on free tracks, ripple collisions, time 0) allows an interval of deltas that contains 0, so together they allow an interval `[lo, hi]` with `lo <= 0 <= hi`.

- **INV-46** With `clamp: true` the command applies `δ` clamped into `[lo, hi]` and rounded toward 0 to a whole number of frames, and returns `applied_delta` and `clamped` (true when it differs from the request). With `clamp: false` (default, P1-compatible) a request outside `[lo, hi]` fails with `Invalid` at the time param, and the message names the limiting constraint and the largest delta allowed. Gestures pass `clamp: true` (§5).
- **INV-47** Speed. With speed `s` (WP-2D.01), a timeline delta `δ` moves a source position by `round(δ × s)`, where the rounding is odd-symmetric (half away from zero), so trimming by `δ` then by `−δ` restores `source_in` exactly. For a reversed clip the timeline start edge moves the source *end*. Every source bound in this spec reads "the source range `[source_in, source_in + duration × s)` stays inside the media".

### 3.8 Errors

| Error | When |
|---|---|
| `Invalid{path, msg}` | Bad params (shape, sign, both or neither of an exactly-one pair), out-of-range requests under `clamp: false`, overlaps under `reject`, ripple collisions (INV-32/33), rule violations named in §4. `path` is a JSON pointer into the params. |
| `NotFound{kind, id}` | An item, track, media or sequence that does not exist. |
| `Locked{kind:"track", id}` | INV-27. |
| `Unsupported` | Item or track kinds that are not editable yet; the P2 limitations of §8 that a request can hit. |
| `Conflict{reason:"item_in_gesture", retry_after_ms}` | Another actor's open gesture or proposal holds an item the command would change (§5). |

When several apply, the order is: parameter shape, `NotFound`, `Unsupported`, `Locked`, then the semantic `Invalid` checks. Fixtures exercise one failure at a time.

For overlaps under `reject`, ripple collisions and limits, `path` points at the param that sets the edit point: the time param the request gave (`/at`, `/start`, `/delta`, `/to`, `/offset`), otherwise the item param (`/items`, `/item`). The message names the blocking item and its track.

### 3.9 One undo step per user action

- **INV-48** Each user action is one command, or one transaction of commands, and therefore exactly one history entry, labelled after the action. Effects a command needs (splits made by insert or overwrite, auto-created tracks, partners, children, dissolved groups) happen inside that command, never as separate entries.

| User action | Engine call | Transaction | History entries |
|---|---|---|---|
| Drag a clip (move, change track, reorder on the main track) | `item.move` updates | `gesture` | 1 (0 if dropped where it started) |
| Drag an edge with the select tool | `clip.trim` updates | `gesture` | 1 |
| Drag an edge with the ripple tool | `clip.ripple_trim` updates | `gesture` | 1 |
| Roll, slip, slide tools | `clip.roll`, `clip.slip`, `clip.slide` updates | `gesture` | 1 |
| Drop media from the bin | `timeline.insert` updates while hovering, the last one kept | `gesture` | 1 |
| Alt-drag duplicate | `item.duplicate` updates | `gesture` | 1 |
| Razor click, Split at playhead (S) | `clip.split` | implicit | 1 |
| Delete / Backspace | `item.delete` | implicit | 1 |
| Ripple delete (Shift+Delete) | `timeline.ripple_delete` | implicit | 1 |
| Delete a selected gap | `timeline.delete_gap` | implicit | 1 |
| Lift / Extract the in–out range | `timeline.lift` / `timeline.extract` | implicit | 1 |
| Insert (`,`) / Overwrite (`.`) from the Source view | `timeline.insert` / `timeline.overwrite` | implicit | 1 |
| Nudge by a frame (`,` `.` with a timeline selection) | `item.move{delta}` | implicit | 1 per press |
| Duplicate (Ctrl+D), Paste | `item.duplicate`, `timeline.paste` (WP-2A.06) | implicit | 1 |
| Replace clip | `clip.replace` | implicit | 1 |
| Link, unlink, attach, detach | `item.link`, `item.unlink`, `item.attach`, `item.detach` | implicit | 1 |
| Main track magnet toggle | `track.set_magnetic` | implicit | 1 |
| Several of the above as one agent step | any | `batch` (`cutweave_execute`) | 1 |

## 4. Commands

Each section gives the signature (params → output; the output always also has `report: EditReport`), the semantics, a diagram, the edge cases and the invariants tests cite. MCP exposure follows the WBS: `timeline_insert`, `item_move`, `item_delete`, `clip_split` and `clip_trim` are core; everything else is catalog.

### 4.1 `timeline.insert` and `timeline.overwrite`

```text
timeline.insert {
  sequence, track, media, at, source_in?, duration?,        # P1
  overlap?: "reject"|"new_track",  link?: bool,               # P1 (overlap = alias of mode)
  mode?: "insert"|"overwrite"|"new_track"|"reject",           # new
  audio_track?: TrackId,  ripple_all_tracks?: bool            # new
} -> { at, duration, clips, linked_group, created_tracks, report }   # P1 fields + report

timeline.overwrite { sequence, track, media, at, source_in?, duration?, link?, audio_track? }
  = timeline.insert with mode "overwrite" (same output)
```

Semantics: resolve `at`, `source_in` and `duration` (INV-24/25, P1 bounds), clamp `at` on the magnetic track (INV-41), then place the clip with the mode (§3.6) and its linked audio (INV-45).

```text
before                               insert m (20 frames, with sound) at 30
V1*  [A 0-30][B 30-60][C 60-90]      V1*  [A 0-30][N 30-50][B 50-80][C 80-110]
A1   [a 0-30][b 30-60][c 60-90]      A1   [a 0-30][n 30-50][b 50-80][c 80-110]

insert at 40 (inside B)              V1*  [A 0-30][B 30-40][N 40-60][B2 60-80][C 80-110]
                                     A1   [a 0-30][b 30-40][n 40-60][b2 60-80][c 80-110]

overwrite m (30, no sound) at 20     V1*  [A 0-20][N 20-50][B 50-60][C 60-90]
                                     A1   [a 0-20]        [b 50-60][c 60-90]
```

| Edge case | Result |
|---|---|
| `at` past the end of the magnetic track | Clamped to its end (append); the output's `at` says so. |
| Empty magnetic track | Placed at 0 whatever `at` is. |
| `at` strictly inside an item, `insert` | That item and its partners split at `at`; the right halves shift with the ripple and form a new group. |
| `at` on a free track overlapping, no mode | `Invalid{path:"/at"}` (P1 default `reject`). |
| `insert` on a free track | NLE insert: the track and the tracks of linked partners of shifted items ripple by `+d`. Gaps after `at` are kept, shifted. |
| `insert` with a rippled partner track holding an item that spans `at` | The spanning item stays; shifted items move away from it (no collision is possible for `+d`). The new audio then goes to the next free audio track (INV-45). |
| `overwrite` past the track end | The magnetic track grows; nothing else ripples. |
| `overwrite` over a linked item with picture-only media | The partner audio is cut over the same range (INV-44): a silent hole stays on its track. |
| Target track or a track the ripple or cut reaches is locked | `Locked` (INV-27). |
| Media kind the track does not accept, sequence-ref media | `Invalid{path:"/track"}`, `Unsupported` (P1; compound clips arrive in WP-2B.04). |
| `source_in` + duration past the media end | `Invalid{path:"/duration"}` (P1). |

Invariants:

- **INV-101** The new clip (and its linked audio) occupies exactly `[at_used, at_used + duration_used)`; `clips[0]` is on the target track or, under `new_track`, on the track created above it.
- **INV-102** `insert`: every item that started at or after `at_used` on a track of `R` starts exactly `duration_used` later; nothing before `at_used` changes except the split item, whose left part keeps its id, start and `source_in`.
- **INV-103** `overwrite`: no item outside `[at_used, at_used + duration_used)` changes except by the cut rule INV-44; the sequence never ripples.
- **INV-104** `reject` and `new_track` keep the P1 behaviour on free tracks exactly (same output fields, same created tracks).

### 4.2 `item.move`

```text
item.move {
  item? | items?: [ItemId],          # exactly one of; P1 has item
  start? | delta?,                   # exactly one of; P1 has start
  to_track?: TrackId, unlink?: bool, overlap?,        # P1
  mode?, ripple_all_tracks?: bool, fine?: bool        # new
} -> { items: [{item, track, start}], created_tracks, report }   # P1 fields + report
```

The **movers** are the given items plus, unless `unlink`, their linked partners. The **primary** is the first given item. A move is a **lift** followed by a **placement**, in one transaction:

1. **Lift.** The movers leave their tracks. On the magnetic track the space they leave closes: a ripple with the magnetic track as ripple track and `f(t) = −(total duration of lifted items that ended at or before t)`. Free tracks keep the gaps.
2. **Target.** The primary goes to `to_track` (default: its own track) at `start`, or at its old start plus `delta`. Given items on the primary's track keep their offsets to it; linked partners keep their own tracks and stay aligned with their group (INV-35). Times on the magnetic target are read in the **lifted** layout and clamped (INV-41); on the magnetic track the movers that land there are packed in their original order, without the gaps that separated them.
3. **Place.** With the mode (§3.6; default by the target track, INV-40). Movers that do not land on the magnetic target are translated by `δ = used primary start − old primary start`. Linked partners are placed on their own tracks with the same mode, except that `insert` does not ripple a partner's track again: it relies on the ripple of the target track reaching it (INV-31), and a partner that still overlaps fails the move as under `reject`. Attached children of the movers that are not movers themselves translate with their parent (INV-36/37).

```text
reorder on the main track: item.move{item: A, start: 30}      (30 is read in the lifted layout)
V1*  [A 0-30][B 30-60][C 60-90]      lift A:  [B 0-30][C 30-60]      place:  [B 0-30][A 30-60][C 60-90]
A1   [a 0-30][b 30-60][c 60-90]               [b 0-30][c 30-60]              [b 0-30][a 30-60][c 60-90]

out of the main track: item.move{item: A (no sound), start: 0, to_track: V2}
V2                                   V2   [A 0-30]
V1*  [A 0-30][B 30-60][C 60-90]      V1*  [B 0-30][C 30-60]
```

| Edge case | Result |
|---|---|
| A given item on another track than the primary, with `to_track` | `Unsupported` in P2 (§8: moving a multi-track selection to other tracks). Without `to_track` every mover stays on its track. |
| `start` past the lifted end of the magnetic target | Clamped to it (INV-41); the output states the used start. |
| `start` strictly inside an item of the lifted magnetic track | That item is split there and the movers go in between (`insert`), or it is overwritten (`overwrite`). The editor rounds a CapCut-style drop to the nearest cut itself (WP-2A.08); the engine never guesses. |
| Moving a clip with linked audio out of the main track | The audio partner keeps its time on its own track while the lift ripples that track; under `reject` it then usually overlaps and the move fails naming it; `new_track` puts it on a new audio track. |
| A mover or its partner would start before 0 | `Invalid{path:"/start"}` or `/delta` (P1). |
| Overlap on a free target | `reject`: `Invalid`; `new_track`: the overlapping mover (or partner) goes to a new track above its target (P1). |
| A child whose parent moves onto the child's track | The child is detached and stays (INV-38). |
| Moving an attached child alone, outside its parent's range | The child is detached (INV-38). Inside the range it stays attached. |
| Called repeatedly in a gesture | Each update replaces the previous one (§5): the result equals one move from where the gesture began. |

Invariants:

- **INV-121** Free tracks: every mover is translated by the same `δ = new primary start − old primary start` and nothing else changes except auto-created tracks and following children (P1 plus INV-36).
- **INV-122** Magnetic source: after the lift the magnetic track is gap-free; items of the magnetic track that were not moved keep their order.
- **INV-123** Magnetic target: the movers that land on the magnetic track are contiguous, in their original order, starting at the used start; the track stays gap-free.
- **INV-124** `unlink: true` moves exactly the given items, which leave their groups (P1 orphan rule).

### 4.3 `item.delete`

```text
item.delete { items: [ItemId] } -> { deleted, report }     # P1 + report
```

Removes the items and their linked partners. On a free track the space stays (P1). On the magnetic track it closes: one ripple with the magnetic track as ripple track, `f(t) = −(total duration of removed magnetic items that ended at or before t)`, and `R` by INV-31. Children of removed items are detached and keep their place (INV-38).

```text
V2         [T 35-45]                       V2   [T 35-45]   (T was attached to B: now detached)
V1*  [A 0-30][B 30-60][C 60-90]   delete B   V1*  [A 0-30][C 30-60]
A1   [a 0-30][b 30-60][c 60-90]              A1   [a 0-30][c 30-60]
```

| Edge case | Result |
|---|---|
| Deleting the first main-track item | The next one starts at 0. |
| Deleting the last main-track item | Nothing shifts. |
| Deleting items on the main track and on free tracks in one call | Only the main track's spaces close (and the tracks `R` adds); free-track spaces stay. |
| A partner, or an item of a rippled track, on a locked track | `Locked` (INV-27). |
| A child of a removed item on a locked track | `Locked` (detaching changes it). |

Invariants:

- **INV-141** Every given item and every partner is gone; nothing else is removed.
- **INV-142** Free tracks that are not in `R` are unchanged except for the removed items.
- **INV-143** On the magnetic track, the remaining items keep their order and the track stays gap-free.

### 4.4 `timeline.ripple_delete`

```text
timeline.ripple_delete { items: [ItemId], ripple_all_tracks?: bool } -> { deleted, report }
```

Removes the items and their partners and closes the space each given item leaves on its track. All given items MUST be on one track, the ripple track (`Invalid{path:"/items"}` otherwise: ripple deletes across tracks are `timeline.extract`). `f(t) = −(total duration of removed items of the ripple track that ended at or before t)`; `R` by INV-31. Pre-existing gaps next to the removed items stay. On the magnetic track it equals `item.delete`.

```text
V2   [X 0-20]   [Y 40-60]   [Z 80-100]     ripple delete Y    V2   [X 0-20]   [Z 60-80]

collision: Z's audio z is on A2 where music M ends after Z's new start
V2   [X 0-20]   [Y 40-60]   [Z 80-100]
A2   [M 0-70]               [z 80-100]     ripple delete Y -> Invalid (z would overlap M on A2)
```

Invariants:

- **INV-151** On the ripple track, every remaining item after a removed one moves left by exactly the total duration removed before it; items before the first removed one do not move.
- **INV-152** The command fails rather than leaving any track of `R` overlapping (INV-32).

### 4.5 `timeline.lift` and `timeline.extract`

```text
timeline.lift    { sequence, ranges?: [{start, end}], tracks?: [TrackId] } -> { ranges, report }
timeline.extract { sequence, ranges?: [{start, end}], tracks?: [TrackId] } -> { ranges, report }
```

`ranges` default to the sequence in/out (`Invalid{path:"/ranges"}` if neither is set); they are snapped (INV-24), each must end after it starts, and overlapping or touching ranges merge. `tracks` default to every track of the sequence. On each target track the ranges are cut out by INV-44 (partners get the same cut, even on tracks not listed).

- **Lift** leaves the cut ranges empty, except on the magnetic track, where they close (one ripple, ripple track = magnetic track, `R` by INV-31).
- **Extract** closes the cut ranges on every target track and on the tracks `R` adds: one ripple whose ripple tracks are all target tracks, `f(t) = −(total length of ranges that ended at or before t)`. The magnetic track, if targeted, closes like the others.

```text
extract [10, 20) on all tracks
V2      [T 15-25]                    V2      [T 10-15]          (T cut: its part before 20 is gone)
V1*  [A 0-30][B 30-60]               V1*  [A 0-10][A2 10-20][B 20-50]
A1   [a 0-30][b 30-60]               A1   [a 0-10][a2 10-20][b 20-50]
```

| Edge case | Result |
|---|---|
| A range that cuts nothing on a track | That track still closes the range when it is a target of `extract` (items after it shift). |
| A locked track among the targets with anything at or after the first range start | `Locked`. Pass `tracks` to leave it out. |
| Lift on a free track | Gaps stay; nothing shifts. |
| A spanning item | Split by INV-44: the left part keeps the id; the right part gets a new id, and for `extract` it moves left to the range start. |

Invariants:

- **INV-161** `lift` never moves an item of a free track that is not in `R`.
- **INV-162** `extract`: after the edit, no target track holds anything that was inside the ranges, and every item that started at or after a range end moved left by the total length of the ranges before it.
- **INV-163** Both report the merged, snapped `ranges` they used.

### 4.6 `timeline.delete_gap`

```text
timeline.delete_gap { track, at, ripple_all_tracks?: bool } -> { gap: {start, end}, report }
```

Closes the gap of `track` that contains `at`: items at or after the gap's end ripple left by its length, with `R` by INV-31. `Invalid{path:"/at"}` if `at` is inside an item or after the last item (no gap there). The magnetic track has no gaps, so it is always `Invalid` there.

```text
V2   [X 0-20]        [Y 40-60]     delete_gap at 30     V2   [X 0-20][Y 20-40]
```

Invariants:

- **INV-171** The gap is gone, items before it do not move and items after it move left by exactly its length.

### 4.7 `clip.split`

```text
clip.split { item? | items?: [ItemId], at, fine? } -> { left, right, at, linked, right_group, report }   # P1 + items + report
```

P1 semantics: the left half keeps the id and `source_in`; the right half gets a new id, starts at `at` and reads on from where the left half ends (stills keep `source_in`). Partners split at the same time, and the right halves share a new group. With `items`, each given item (and its group) is split at the same `at`; every given item MUST contain `at` strictly. Children at or after `at` move to the right half (INV-39). Item markers at or after the cut move to the right half with their time rebased. Nothing ripples, on any track.

```text
V2               [T 50-55]                         V2               [T 50-55]   (T -> B2)
V1*  [A 0-30][B 30-60][C 60-90]   split B at 45    V1*  [A 0-30][B 30-45][B2 45-60][C 60-90]
```

Invariants:

- **INV-181** Durations of the halves sum to the original; the right half's `source_in` = left `source_in` + left duration × speed (bounded media); the timeline is otherwise unchanged.
- **INV-182** `at` equal to an item's start or end, or outside it, is `Invalid{path:"/at"}` (P1).

### 4.8 `item.duplicate`

```text
item.duplicate { items: [ItemId], offset?, mode? } -> { copies: [{source, copy}], created_tracks, report }
```

Copies the items and their partners (each copied group becomes a new group; a copied child stays attached to the copy of its parent if that was copied too, to its original parent if the copy still starts inside it, and is detached otherwise). Each copy goes to its original track at `original start + offset`; the default `offset` is the selection's span (largest end − smallest start), so the copies follow the originals. On the magnetic track the copies are inserted as one block right after the last copied main-track item (default mode `insert`, INV-40); `offset` is not allowed there (`Invalid{path:"/offset"}`). Copies get new ids (INV-23).

```text
V1*  [A 0-30][B 30-60][C 60-90]   duplicate B   V1*  [A 0-30][B 30-60][B2 60-90][C 90-120]
```

Invariants:

- **INV-191** Each copy equals its source except id, group, attachment, start and (under `new_track`) track.
- **INV-192** The originals never change, except by the ripple of an `insert` placement.

### 4.9 `clip.trim`

```text
clip.trim { item, edge: "start"|"end", to? | delta?, unlink?, clamp?, fine? }
  -> { items, applied_delta, clamped, report }            # P1 items + new fields
```

Moves one edge of the clip and of each linked partner by the same delta `δ` (for `to`, `δ = to − current edge time` of the named item). Start edge: `start += δ`, `source_in += δ × speed`, `duration −= δ`. End edge: `duration += δ`. On a free track nothing else moves (P1). **On the magnetic track a trim is a ripple trim** (§4.10): the track stays gap-free.

Constraints (§3.7): duration ≥ 1 frame; source bounds of every member (bounded media only); on a free track the edge stops at the neighbour (or at time 0 for the start edge).

```text
free track, end edge to 50 with a neighbour at 40
V2   [X 0-30]   [Y 40-60]      clamp: true  ->  V2   [X 0-40][Y 40-60]     applied_delta 10, clamped
                               clamp: false ->  Invalid{path:"/to"} ("at most +10 frames: item Y")
main track, end edge of A by -10
V1*  [A 0-30][B 30-60]         ->               V1*  [A 0-20][B 20-50]
```

Invariants:

- **INV-201** Free track: only the named clip and its partners change, and only at the trimmed edge.
- **INV-202** The trimmed edge of every member moves by exactly `applied_delta`; the other edge does not move (free track).
- **INV-203** Magnetic track: the result equals `clip.ripple_trim` with the same params.

### 4.10 `clip.ripple_trim`

```text
clip.ripple_trim { item, edge, to? | delta?, unlink?, clamp?, fine?, ripple_all_tracks? }
  -> { applied_delta, clamped, report }
```

Trims like §4.9, then ripples so the clip's other neighbours keep touching it:

- **End edge** by `δ`: `duration += δ`; ripple at `θ = old end` by `+δ`.
- **Start edge** by `δ` (positive removes frames from the head): `source_in += δ × speed`, `duration −= δ`, the clip's `start` does not move; ripple at `θ = old end` by `−δ`. With `to`, `δ = to − start` (the head is cut as if the clip did not move).

`R` by INV-31. Constraints: duration ≥ 1 frame, source bounds, ripple collisions on `R` (INV-32), INV-33. No neighbour limit on the clip's own track: the downstream items move with the edge.

```text
V2   [X 0-20]   [Y 40-60]      end of X by +10       V2   [X 0-30]   [Y 50-70]
V2   [X 0-20]   [Y 40-60]      start of X by +5      V2   [X 0-15]   [Y 35-55]    (X's source_in +5)
```

Invariants:

- **INV-211** Every item of `R` at or after the old end moves by exactly the change of the clip's end (`+δ` or `−δ`).
- **INV-212** The clip's start does not move.

### 4.11 `clip.roll`

```text
clip.roll { left, right, to? | delta?, clamp? } -> { applied_delta, clamped, report }
```

Moves the cut between two adjacent clips of one track (`right.start == left.end`, else `Invalid{path:"/right"}`) by `δ` (for `to`, `δ = to − cut`): `left` and its partners get `end += δ`, `right` and its partners get `start += δ` (start-edge trim). Nothing else moves; the track's length never changes. Constraints: both keep ≥ 1 frame, both source bounds, partner tracks' neighbours (a partner whose new edge would overlap an item that is not the other side's partner bounds `δ`).

```text
V1*  [A 0-30][B 30-60]      roll the A|B cut by +10      V1*  [A 0-40][B 40-60]   (B's source_in +10)
```

Invariants:

- **INV-221** `left.duration + right.duration`, `left.start` and `right.end` are unchanged, for the named pair and for every partner pair.

### 4.12 `clip.slip`

```text
clip.slip { item, delta, clamp?, fine? } -> { applied_delta, clamped, report }
```

Changes only the media range: `source_in += δ` for the clip and its partners, where `δ` is a media-time offset in whole sequence frames (positive shows later material). Timeline positions never change. Constraint: the source range stays inside the media. An unbounded item (still, generator) has nothing to slip: `Invalid{path:"/item"}`.

```text
V1*  [A 0-30][B 30-60 src 30-60][C 60-90]   slip B by +100   V1*  [A 0-30][B 30-60 src 130-160][C 60-90]
```

Invariants:

- **INV-231** `start`, `duration` and `track` of every item are unchanged; only `source_in` of the clip and its partners changes, by `applied_delta`.

### 4.13 `clip.slide`

```text
clip.slide { item, delta, clamp? } -> { applied_delta, clamped, report }
```

Moves the clip by `δ` between its neighbours, keeping its content: `start += δ` for the clip and its partners; the adjacent item before it (if `left.end == clip.start`) and its partners get `end += δ`; the adjacent item after it (if `right.start == clip.end`) and its partners get `start += δ`. A side without an adjacent item is a gap (or time 0): `δ` is bounded by the gap (the clip never covers another item), and an empty end of the track allows any positive `δ`. Children of the clip translate with it (INV-36). Constraints: neighbours keep ≥ 1 frame and their source bounds; `start + δ >= 0`.

On the magnetic track the first item cannot slide (that would open a gap at 0): its interval is `[0, 0]`. The last item has no right neighbour; sliding it changes the track's end.

```text
V1*  [A 0-30][B 30-60][C 60-90]     slide B by +10     V1*  [A 0-40][B 40-70][C 70-90]   (C's source_in +10)
```

Invariants:

- **INV-241** The clip's duration and `source_in` are unchanged; the left neighbour's start and the right neighbour's end are unchanged; the sum of the three durations is unchanged when both neighbours exist.

### 4.14 `clip.replace`

```text
clip.replace { item, media, mode?: "keep_duration"|"keep_source_in"|"fit_to_source",
               source_in?, old_audio?: "mute"|"keep"|"remove" } -> { item, shortened, report }
```

Swaps the clip's media and keeps every other attribute: `id`, track, `start`, name (a clip without its own name shows the new media's name), `enabled`, transform, crop, opacity, blend, flip, all keyframes (item-relative times unchanged), effects, masks, audio settings, color label, linked group, attachment and children. Speed and reverse are kept; replacing with a still drops them (reported). A freeze frame stays a freeze of the new media at the new `source_in`.

| Mode | New `source_in` | New duration |
|---|---|---|
| `keep_duration` (default) | `source_in` param, else 0 | The old one; if the new media is too short, the clip is shortened to what it has (`shortened: true`). |
| `keep_source_in` | The old one (`source_in` param not allowed) | As `keep_duration`. |
| `fit_to_source` | `source_in` param, else 0 | The rest of the new media (floored, INV-25). |

A change of duration is a ripple trim of the end edge (§4.10) on every track kind, so the magnetic track stays gap-free and downstream items keep their sync; a bounded-media limit on a partner shortens the group together.

Linked audio: if the clip has an audio partner and the new media has sound, the partner's media is replaced too, with the same mode. If the new media has no sound, `old_audio` decides what happens to the old audio partner: `"mute"` (default) unlinks it and keeps it disabled (`enabled: false`, reported in `toggled`), trimmed to the clip's new duration when the clip was shortened; `"keep"` does the same but leaves it audible (the cutaway case); `"remove"` deletes it. Media the track does not accept is `Invalid{path:"/media"}` (an audio file onto a video track, a still onto an audio track).

Invariants:

- **INV-251** `item.get` before and after differ only in `media`, `source_in`, `duration` (and the speed fields when replacing with a still).
- **INV-252** `shortened` is true exactly when the new duration is less than the old one under `keep_duration` or `keep_source_in`.

### 4.15 `item.link` and `item.unlink`

```text
item.link   { items: [ItemId] } -> { group, members, report }
item.unlink { items: [ItemId] } -> { report }
```

`item.link` links at least two distinct items of one sequence, which MUST be aligned (identical `start` and `duration`; `Invalid{path:"/items"}` otherwise, suggesting a trim first or `item.attach`). The given items and every member of their current groups form one new group. They MUST all be attached to the same parent or to none (INV-9). `item.unlink` takes the items out of their groups; a group left with one member dissolves. Neither moves anything.

Invariants:

- **INV-261** After `item.link` all given items share one new `linked_group`, and their old groups no longer exist.
- **INV-262** After `item.unlink` none of the given items has a group, and no group has a single member.

### 4.16 `item.attach` and `item.detach`

```text
item.attach { items: [ItemId], parent: ItemId } -> { report }
item.detach { items: [ItemId] } -> { report }
```

`item.attach` sets `attached_to = parent` on the items and their linked partners. Every INV-9 condition is checked up front (`Invalid{path:"/items"}` or `/parent`); a child is never moved to make it fit. `item.detach` clears `attached_to` on the items and their linked partners.

Invariants:

- **INV-271** Attach and detach change only `attached_to`.
- **INV-272** After any command, INV-9 holds for every child; a child that would break it is detached in the same command (INV-38).

### 4.17 `track.set_magnetic` (implemented in WP-2A.02)

```text
track.set_magnetic { track, magnetic: bool, close_gaps?: bool } -> { track, magnetic, previous?, report }
```

`magnetic: true` makes a video track the main track (`Invalid{path:"/track"}` for other kinds). The sequence's previous main track, if any, becomes free in the same command (`previous`). If the track has gaps or does not start at 0: with `close_gaps: false` (default) the command fails with `Invalid{path:"/track"}` naming the first gap; with `close_gaps: true` it closes every gap, left to right, as `timeline.delete_gap` would (partners ripple; INV-32 applies). `magnetic: false` only clears the flag. The flag alone may change on a locked track (like P1 `track.set`); closing gaps on a locked track, or on a track whose ripple reaches one, is `Locked`.

Invariants:

- **INV-281** After the command the sequence has at most one magnetic track, and it is gap-free.

### 4.18 Three-point placement (used by WP-2A.14)

The Source view turns marks into `timeline.insert`/`timeline.overwrite` params with this pure function. Times are sequence frames; the source marks are media positions.

| Source marks | Timeline in and out | `at` | `source_in` | `duration` |
|---|---|---|---|---|
| none | none | playhead | 0 | the media length (INV-25) |
| in and/or out | none | playhead | source in, else 0 | (source out, else media end) − source in |
| none | in and out | timeline in | 0 | timeline out − timeline in |
| in and/or out | in and out | timeline in (it wins) | source in, else 0 | from the source marks; the timeline out is ignored (no fit-to-fill, §8) |

- **INV-291** The duration never reads past the media: it is shortened to the media remainder when needed, and the function reports `shortened: true`.
- **INV-292** A source out at or before the source in is `Invalid`; a still without marks lasts 5 s (INV-25).

## 5. Gestures and soft locks

Continuous gestures run in a `gesture` transaction (ADR-0017, arch §5.2). P2 changes how the updates of a gesture compose.

- **INV-301** **Updates replace.** Each update of a P2 gesture is a command sent with `_meta.replace: true`: the engine rolls the transaction's live changes back to the committed document, then applies the command. The committed effect of a gesture therefore equals its last command applied once to the committed document at commit time, whatever path the pointer took. Without the flag commands accumulate (P1 behaviour, still valid for P1 gestures without ripple).
- **INV-302** A failing update leaves the previous update in place (the replace is atomic), and the gesture stays open. The editor shows the clamped or last good state (WP-2A.08).
- **INV-303** Tracks an update created and the next update no longer needs disappear with it; they are never left behind by a gesture.
- **INV-304** **Lock set.** The items a gesture soft-locks are the union, over all its updates so far, of the items named in their `EditReport`s (created, removed, split halves, moved, trimmed, relinked, replaced): ripple-shifted items, linked partners and following children included. The set only grows until commit or cancel.
- **INV-305** Another actor's command that would change a locked item fails before any mutation with `Conflict{reason:"item_in_gesture", holder, retry_after_ms}` (WP-2A.12); commands that change only other items proceed. An update of the holder that would change an item locked by *another* actor's gesture fails the same way and, by INV-302, keeps the previous state.
- **INV-306** Commands that support gestures: `item.move`, `clip.trim`, `clip.ripple_trim`, `clip.roll`, `clip.slip`, `clip.slide`, `timeline.insert` (dropping media) and `item.duplicate` (Alt-drag). Gestures pass `clamp: true` where the command has it.

## 6. Snapping

Snapping is a pure function the UI runs (WP-2A.07, through the wasm module of WP-2C.06). The engine never snaps to candidates; it only snaps times to the frame grid (INV-24).

**Inputs:** the raw time `r` (flicks, from the pointer), the probe offsets `o₁ … oₙ` (whole frames, relative to `r`: `[0]` for an edge, the playhead or a marker; `[0, duration]` for a clip; every edge of every moving item, relative to the primary's start, for a multi-item move), the candidates, the zoom `px_per_second`, `threshold_px` (8 by default, the Timeline preference "Snap distance"), the enabled kinds and the frame rate.

**Candidates** and their rank:

| Rank | Kind | Times |
|---|---|---|
| 0 | `playhead` | the playhead |
| 1 | `edge` | the start and end of every item on every track, and the timeline start 0 |
| 2 | `marker` | sequence markers and item markers (projected to timeline time while inside their item), both ends of range markers |
| 3 | `beat` | markers of kind `beat` |
| 4 | `in_out` | the sequence in and out |

Items that move with the gesture (the movers, their partners and following children) are not candidates.

**Algorithm:**

1. Snap every candidate time to the nearest frame (ties later).
2. `tolerance = threshold_px × 705 600 000 / px_per_second` flicks (an `f64`).
3. A pair (probe `oᵢ`, candidate `c`) is eligible when its kind is enabled, `d = |c − (r + oᵢ)| <= tolerance` and `c − oᵢ >= 0`.
4. The winner minimises `(d, rank, i, c)` lexicographically: the nearest wins; ties go to the higher-ranked kind, then the earlier probe, then the earlier time.
5. The result is `t = c − oᵢ` with that candidate as `target`; with no eligible pair (or snapping off, or Ctrl/Cmd held), `t` is `r` snapped to the nearest frame and there is no target.

- **INV-321** The result is always on the frame grid.
- **INV-322** With a target, the snapped probe lands exactly on it, and `|t − r| <= tolerance`.
- **INV-323** The tolerance scales with zoom: doubling `px_per_second` halves it.
- **INV-324** With only playhead and edge candidates on the frame grid, the result equals the P1 `snapSpan` of `packages/timeline` whenever that result is not negative (nearest, then earlier probe, then earlier time).

## 7. Selection

Selection is view state (`view.selection`, not undoable, not journaled; agents set it with `view_set`).

- **INV-341** Clicking an item selects its whole linked group; Alt+click selects only that item. Shift+click adds or removes the clicked item's group (Alt+Shift: the item). A marquee selects every item it touches and their groups (with Alt: only the touched items). Attached children are never selected with their parent.
- **INV-342** Commands receive the selected ids; the engine expands linked groups itself (INV-35). An edit of a selection that holds only some members of a group (Alt, or the toolbar's link toggle off) passes `unlink: true` where the command has it, so those members leave their group, and is otherwise refused by the editor.
- **INV-343** After a command the selection keeps the ids that still exist. Insert, paste and duplicate select what they created; split keeps the left halves selected.

## 8. Not supported in P2

Each of these is refused (`Unsupported` or `Invalid` as noted) or simply absent; none is silently approximated.

- Multicam editing (post-1.0).
- Nested compound clips: a compound clip inside a compound clip (WP-2B.04 supports one level; deeper nesting is `Unsupported`).
- Track targeting UI (source/record patching); the engine's `tracks` and `audio_track` params exist, the editor uses defaults.
- Linked members out of sync (offset links, sync badges) and J/L cuts inside a link: trim one member with `unlink: true` instead.
- Transitions (P4, epic 4B): no overlaps at cuts in P2.
- Four-point editing and fit-to-fill.
- Moving a multi-track selection to other tracks in one command (`item.move` with `to_track` and given items on several tracks is `Unsupported`).
- Rolling or trimming several edit points at once (multi-roll, asymmetric trim).
- Ripple of sequence markers or of the in/out range.
- Removing empty tracks automatically, and attaching items automatically on insert (CapCut's linkage default); both are explicit commands.
- A magnetic audio track, and more than one magnetic track per sequence.
- Ripple across sequences.
- Item-level locks (track locks only).
- Speed curves (P4, 4H); constant speed arrives in WP-2D.01 under INV-47.
- Editing text, caption and adjustment tracks before their epics (3A, 3E, 4A).
- Replace-all-instances and generative replace (P5).

## 9. Fixtures

`docs/spec/fixtures/*.json` are executable examples of this spec. WP-2A.03, 2A.04, 2A.05, 2A.11 and 2A.02 run the `edit` fixtures of their commands as table-driven tests; WP-2A.07 runs the `snap` fixtures and WP-2A.14 the `three_point` ones. `cargo xtask check-fixtures` validates all of them against [`fixture.schema.json`](fixture.schema.json) and this spec, and the xtask's tests run it in `just check`.

### 9.1 Format

Every fixture has `kind`, `id` (equal to its file name), `title`, `cites` (the `INV-n` ids it illustrates, each defined in this file) and `fps` (sequence frame rate; all fixtures use 25). Times in documents are whole frames.

**`edit`** fixtures:

| Field | Meaning |
|---|---|
| `command`, `tags` | The command, and which of `magnetic`, `free`, `locked`, `linked`, `clamp` it exercises. |
| `media` | Name → `{kind: video\|audio\|image, frames?, audio?}`; `frames` is the length in sequence frames (absent: unbounded). |
| `before` | The sequence's tracks in display order: `{track, kind, magnetic?, locked?, items: [{id, media, start, dur, in?, group?, parent?, enabled?}]}`. `in` is `source_in` in frames (default 0); `group` names a linked group, `parent` the item it is attached to; `enabled` defaults to true. |
| `params` | The command's params with names in place of ids (`"sequence": "S"`; tracks, items and media by name) and times as `TimeInput` (`"f:30"`, or `"-0.4s"`/flicks for negative deltas). |
| `after` | The expected tracks, same shape. Names starting with `+` are new (items, groups, tracks); a test binds each to the id the engine created and compares the structure. |
| `error` | Instead of `after`: `{code: invalid\|locked\|not_found\|unsupported\|conflict, path?, id?}`; the document must be unchanged. |
| `output`, `report` | Optional: command-specific output fields to compare (`at`, `applied_delta`, `clamped`, `shortened`, …), and the expected `EditReport` (names, unordered). |

**`snap`** fixtures give `candidates` (`{kind, t}` in flicks), `offsets`, `r`, `px_per_second`, `threshold_px`, optional `enabled` kinds, and the expected `{t, target}`. **`three_point`** fixtures give the marks and the expected `{at, source_in, duration, shortened}` or an error.

### 9.2 What the checker verifies

1. Every fixture matches the schema, and its `id` matches its file name and is unique.
2. Every cited `INV-n` is defined in this spec.
3. `edit`: names resolve; `before` and `after` satisfy INV-1, 2, 4, 5, 6, 7, 8 and 9; every item of a locked track is the same before and after (INV-28); new names appear only in `after` and existing names keep their media (except `clip.replace`); a stated `report` equals the one derived from `before` and `after` (INV-21).
4. `snap` and `three_point`: the expected result equals the reference implementation of §6 and §4.18 in the checker.
5. Coverage: every command of §4 has, for each tag, at least one fixture or an entry with a reason in [`fixture-coverage.json`](fixture-coverage.json); at least 40 `edit` fixtures exist.

## 10. Decisions and alternatives

The product owner confirmed D1, D2b, D7 and D16 on 2026-11-25, when accepting ADR-0023.

| # | Decision | Alternatives considered | Why |
|---|---|---|---|
| D1 | At most one magnetic track; a sequence may have none. | Exactly one, always (WBS wording). | P1 documents have none, and CapCut's magnet can be turned off. "Exactly one per sequence that has one" is what the property tests assert. |
| D2 | Default mode on free tracks stays `reject`; the editor passes the mode (`new_track` for overlapping drags, the WP-2A.13 preference for media dropped onto a clip). | Default `new_track` (WBS 2A.01 wording). | Arch §4.3 decided `reject` for agents in WP-1B.05; changing a core tool's default silently would break agent scripts. The user-visible behaviour is the same, and it also reconciles WP-2A.01's `new_track` with WP-2A.13's "drops onto a clip overwrite by default". |
| D2b | Replacing with media that has no sound mutes the old audio by default (`old_audio: "mute"`). | Keep it audible; remove it. | WP-2A.11's "unlinked and muted per option"; CapCut users expect the old clip's sound to go with it, and muting keeps it recoverable. |
| D3 | Ripple reaches the tracks of linked partners of shifted items (sync lock by linkage); other tracks only with `ripple_all_tracks`. | Ripple every track (NLE sync lock everywhere); ripple only the partner items, not their tracks. | Keeps a clip's sound and the sounds placed after it in sync without moving unrelated music; item-only following collides with any unlinked item on the audio track. |
| D4 | A ripple collision fails (or bounds a clamped delta). | Push the blocker to a new track; split it. | Premiere refuses the same edit; failing names the blocker and never rearranges tracks the user did not touch. |
| D5 | Attachment is a separate, one-way relation (`attached_to`), not a linked group. | Use `linked_group` for attachments (arch §4.3 wording). | Linked groups are symmetric and aligned; "follow the parent's start but not its trims" is a different rule, and children must not be deleted or trimmed with their parent. |
| D6 | Children that collide go to a new track; they never block the parent. | Fail like a ripple collision. | Overlays follow the main clip in CapCut and auto-created overlay tracks are the product's overlap rule (arch §4.3). |
| D7 | Linked groups are always aligned; editing one member alone unlinks it. | Allow offset links with sync indicators. | Keeps every group edit uniform (same edges, same ripple threshold); offset links are listed in §8. |
| D8 | Insert and move at a time inside an item split it there; the editor rounds CapCut drops to the nearest cut. | The engine rounds to the nearest cut on the magnetic track. | Agents get exactly the time they ask for; the CapCut feel lives in the UI where the pointer is. |
| D9 | Magnetic target times are read in the lifted layout. | Read them in the document before the move. | For a single clip both give the same number (the clip's left edge where it is dropped), and the lifted layout is what the user sees while dragging. |
| D10 | Trim-family commands fail by default when a request exceeds a limit; `clamp: true` clamps and reports. | Clamp by default (WBS 2A.04 wording). | P1's `clip.trim` (an MCP core tool) fails; clamping silently would change its contract. Gestures always pass `clamp: true`. |
| D11 | Gesture updates replace (`_meta.replace`). | Accumulate updates (P1); make every command path-independent. | Magnetic reorders, ripples and auto-created tracks do not compose: accumulated updates leave stray tracks and drift. Replace makes a gesture equal one command. |
| D12 | Default durations floor to whole frames; the v2 migration floors existing ones. | Keep unsnapped media-end durations (P1). | A gap-free main track on the frame grid needs whole-frame durations; at most one frame of the media's last partial frame is lost. |
| D13 | `timeline.overwrite` is a thin alias of `timeline.insert{mode:"overwrite"}`. | Only the mode; only a separate command. | Both names are what users and agents look for; one implementation. |
| D14 | `ripple_delete` takes items on one track; ranges across tracks are `extract` (which also covers WP-3F's `ripple_delete_ranges`). | `ripple_delete{items | range}` (WBS wording). | One command per shape of input; multi-track item deletes would need one shift function per track. |
| D15 | Snapping picks the nearest candidate; rank only breaks ties. | Strict priority (any playhead candidate beats any edge). | Matches P1's `snapSpan`, NLE behaviour and the pointer's intent. |
| D16 | Deleting a parent keeps and detaches its children. | Delete the children too. | Never destroy content the user did not select; the children's tracks may be locked or shared. |
| D17 | Tracks are never removed automatically when they become empty. | Remove empty overlay tracks (CapCut). | Stable track ids for agents; a gesture's temporary tracks still vanish (INV-303). |

## 11. Compatibility with the P1 commands

| P1 command | Unchanged | Changed or added |
|---|---|---|
| `timeline.insert` | Params, output fields, `reject`/`new_track` behaviour on free tracks, linked-audio rule under `reject`/`new_track`. | `mode`, `audio_track`, `ripple_all_tracks`; `insert` default on the magnetic track; default durations floor to frames (INV-25, P1 test `insert` "defaults derived from the media are not snapped" changes); `report`. |
| `item.move` | `item` + `start` + `to_track` + `unlink` + `overlap` on free tracks. | `items`, `delta`, `mode`, `ripple_all_tracks`, `fine`; lift/insert on the magnetic track; children follow; `report`. |
| `item.delete` | Free tracks: no ripple. | The magnetic track closes; children detach; `report`. |
| `clip.split` | All. | `items`, `fine`; children re-attach; `report`. |
| `clip.trim` | Free tracks with `clamp: false`. For aligned groups "same edge time" and "same delta" are the same. | `clamp`, `fine`, `applied_delta`, `clamped`; ripple on the magnetic track; `report`. |
| `item.set_props` | All. | Nothing (not an edit of timing). |
| `track.set` | All. | WP-2A.02 adds `track.set_magnetic` instead of a `magnetic` field (it has its own rules); renaming and flags stay in `track.set`. |

Every output change is additive. Contract changes are listed in ADR-0023.

## 12. Hand-off to later work packages

| WP | Takes from this spec |
|---|---|
| 2A.02 | §1.5 (`attached_to`, aligned groups, migration steps), §4.17 `track.set_magnetic`, INV-6/7/8/9 in `Project::validate`, track commands as P1 `track.set` plus `track.set_magnetic`. |
| 2A.03 | §3 (all), §4.1–§4.8, `EditReport`, the `edit` fixtures of those commands, the property tests listed with INV-1…INV-48. |
| 2A.04 | §3.7, §4.9–§4.13, INV-47 hooks for 2D. |
| 2A.05 | §1.3, §1.4, §4.15, §4.16, INV-35…INV-39. |
| 2A.06 | Placement modes for paste (§3.6), INV-48, children and groups on copy (§4.8). |
| 2A.07 | §6 and the `snap` fixtures. |
| 2A.08 | §5, §6, §7, the table of §3.9; rounding CapCut drops to the nearest cut (D8). |
| 2A.11 | §4.14. |
| 2A.12 | §5: `_meta.replace`, the lock set (INV-304), `Conflict` with `holder`. |
| 2A.14 | §4.18 and the `three_point` fixtures. |
| 2D.01, 2C.04, 2B.04 | INV-47 (speed), keyframe retiming fields in `EditReport`, compound clips as bounded media. |
