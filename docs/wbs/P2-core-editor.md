# Cutweave — Phase 2 (Core Editor) Work Breakdown

> Status: baseline 2026-11-22. Source documents: [04-roadmap.md](../04-roadmap.md) (Phase 2), [01-architecture.md](../01-architecture.md) (cited as **§n**), [05-work-breakdown.md](../05-work-breakdown.md). Phase 1 is referenced by epic code only (E1A domain model, E1B command bus/tx/undo, E1C persistence, E1D media, E1E export, E1F render v0, E1G audio v0, E1H playback/SHM, E1I API, E1J MCP v0, E1K CLI, E1L shell, E1M editor UI v0, E1N codegen, E1O CI gates).
> Sizing: S = 1 agent-day, M = 3 agent-days (used for the totals). Default executor `sonnet`.

## Phase goal

A genuinely usable everyday editor for both long-form and short-form (M2 Dogfood): rich multi-track timeline editing, multiple sequences with aspect presets, animatable clip properties, speed, audio mixing, media management, export presets and queue, a dockable and keyboard-driven UI, a complete core MCP tool set, and CI-enforced performance budgets.

## Exit criteria (copied from roadmap)

**Exit (M2):** two dogfood edits are completed by a non-developer: a 10-min multi-cam-free YouTube video with B-roll, and a 9:16 short with manual reframe, music and fades. Performance budgets for timeline, scrub and playback are met on reference machines. No data loss across 20 forced crashes.

## Epics (one-line goals)

| Epic | Goal |
|---|---|
| **2A Timeline editing** | Unlimited tracks with auto-created overlay tracks, magnetic main track, linked clips, ripple/roll/slip/slide, replace clip, snapping, markers, clipboard, multi-select, gesture soft-locks; a specified, property-tested edit-semantics core and a fast Canvas2D timeline UI; the Editing and Timeline settings pages; source marks and three-point editing from the Source view. |
| **2B Sequences & canvas** | Many sequences per project, aspect presets (16:9, 9:16, 1:1, 4:5, 21:9), sequence settings, background fill (color, blur, image), compound clips (nested sequences as media). |
| **2C Clip properties & keyframes** | Transform/crop/opacity/flip/blend on clips, keyframes on every animatable property with easing, keyframe lanes, curve editor and bezier graph editor; keyframe/easing/time/snapping math shared with the UI as wasm; layouts for several clips (side by side, top and bottom, picture in picture, grids). |
| **2D Speed basics** | Constant speed with pitch preservation, reverse, freeze frame. |
| **2E Audio features** | Fades, keyframed volume, pan, track buses, master limiter, loudness normalization, auto-ducking, basic voice effects, voiceover recording, extract audio, audio UI, the Audio settings page. |
| **2F Media management** | Proxy pipeline (auto + manual), relink, collect & archive, image and image-sequence import, bin tags/smart collections, media folders (ADR-0020) and importing folders with their structure. |
| **2G Export presets & queue** | Presets (YouTube, Shorts, Instagram Reels/Stories/feed, TikTok, X with dated platform limits and safe areas; GIF, audio-only, ProRes), HEVC/AV1, advanced encode controls (CBR/VBR/CRF, GOP, color tags, audio codec/bitrate), background queue, range/marker-set export. |
| **2H Workspace UI** | Dockable Short-form/Long-form workspaces, schema-driven inspector, command palette, shortcut editor with NLE and CapCut presets, preview overlays, i18n scaffold, the Project settings dialog, the Appearance and Playback settings pages, closing and reopening panels (View > Panels), a History panel, context menus on every surface (framework WP-2H.12, today's surfaces WP-2H.13). |
| **2I MCP core tool set** | ~30 core tools complete, `view_get`/`view_set`, contact sheet, filmstrip, waveform image, checkpoints, `expected_rev`, per-endpoint tool profiles (`/mcp/minimal`, `/mcp`, `/mcp/full`), scripted-agent e2e. |
| **2J Performance, reliability & dogfood** | Benchmarks in CI against §19, background render cache, crash-injection harness, and the dogfood exit review (human). |

## Critical path

`2A.01 (spec)` → `2A.02` → `2A.03` → `2A.04` → `2A.12 (gesture soft-lock)` → `2A.08 (interaction state machine)` → `2A.10 (timeline UI wiring)` → `2J.06/2J.07 (dogfood edits)` → `2J.08 (exit review)`.

Parallel tracks that must land before the dogfood edits:
- Keyframes: `2C.01` → `2C.02` → `2C.06 (wasm module)` → `2A.07` / `2C.05` / `2C.07`; and `2C.02` → `2C.04`. Also gates the 9:16 reframe in the short.
- Audio: `2E.01` → `2E.02` → `2E.05`.
- Media: `2F.01` → `2F.02`.
- Export: `2G.01` → `2G.03a` → `2G.03b` → `2G.04`.
- UI shell: `2H.01` → `2H.02` → `2H.03a` → `2H.03b`.
- Gates: `2J.01` → `2J.04` (benchmarks) and `2J.05` (crash harness) run in parallel with the dogfood prep.

Schedule note: the roadmap gives 9 weeks. Total effort here is about 162 agent-days. This fits only with 6–8 agents running in parallel across the tracks above and 1–2 human reviewers acting as the merge bottleneck. Staff the 2A opus WPs first.

## Conventions used by all WPs in this phase

- Namespaces (§5.1): `item.*` is generic (delete, move, set_props, duplicate, link/unlink/attach); `clip.*` is for media-clip specifics (split, trim family, speed, replace, freeze).
- Math shared with the UI (time conversion, keyframe/easing eval, snapping) lives in wasm-safe Rust crates and ships as a wasm module in `packages/protocol` (2C.06). The UI never re-implements it and never calls an RPC per frame.
- Every new editing capability is a `#[command]` in `cutweave-ops` with `undoable`, JSON Schema, an `mcp = core|catalog|hidden` tag, and a CLI/MCP/RPC parity test through E1O. Nothing mutates the doc outside `Tx`.
- Continuous gestures (drag, trim, scrub of a slider) use `gesture`-kind transactions (§5.2; 30 s idle auto-cancel; owned by the opening connection, rolled back if it drops). Commands that take a gesture-friendly form say so.
- Time is `Time` (flicks, i64). All snapping to frames goes through the `FrameRate` helpers (§4.1). No float time in model or ops.
- Performance claims cite §19. CI benchmarks arrive in 2J but each WP states its own local threshold.
- "Core" MCP = always listed in `tools/list`. "Catalog" = reachable only via `cutweave_search_commands/describe/execute`.

---

# Epic 2A — Timeline editing

### WP-2A.01 — Spec: timeline edit semantics
- **Area:** docs/spec/timeline-edit-semantics.md (new), docs/spec/fixtures/*.json (edit-scenario goldens)
- **Depends on:** E1A, E1B
- **Size:** M
- **Executor:** opus
- **Scope:** write the normative spec that 2A.02–2A.08 implement. It must define, each with ASCII before/after diagrams and a table of edge cases:
  - **Track and placement rules:**
    - No two items overlap on one track, on any track kind (§4.3). Overlays are additional video tracks. Dropping or moving an item where it would overlap **auto-creates a new track above** (default `new_track`).
    - The main track is the first video track with `magnetic: true`. Exactly one magnetic track per sequence, which may be changed.
    - Placement has three modes: `insert` (ripple the rest of the track; default on the main track), `overwrite` (trim or split what is underneath) and `new_track` (default for any overlap on a non-main track).
  - **Magnetic track:** gap-free after every command. Deleting or trimming closes the gap. Moving an item out of it ripples the tail left. Defined behaviour for the first item (always starts at `0`) and for an empty track.
  - **Ripple scope:** ripple affects the target track plus tracks that contain items linked to shifted items. Other tracks are untouched unless the command takes `ripple_all_tracks: true` (NLE behaviour).
  - **Linked clips:** `linked_group` semantics for move, trim, split, delete, copy. This covers both video+audio of one source and CapCut "attached to main clip" (child follows parent's start; parent trim does not move children unless they now fall outside, then clamp).
  - **Trim family:**
    - Trim: change one edge, moves `source_in` accordingly, clamped to media bounds.
    - Ripple trim: trim, then shift the downstream items.
    - Roll: move the cut between two adjacent items, total duration is constant.
    - Slip: change `source_in` only.
    - Slide: move an item between neighbours by changing neighbours' trims.
    - Each has clamping rules and what it returns when clamped.
  - **Locks:** locked track rejects mutation with `Locked`. A ripple that would shift a locked track fails atomically; list the exact rule. Hidden/muted/solo are non-structural.
  - **Frame alignment:** every edit point snaps to the sequence frame grid. Audio-only tracks use sample-accurate times rounded to the nearest frame unless `fine: true`.
  - **Gesture soft-locks (§5.2):** which items a gesture locks (all items the command may touch, including ripple-shifted downstream items and linked members), and the `Conflict{reason:"item_in_gesture", retry_after_ms}` rule for other actors.
  - **Replace clip:** attributes kept (transform, keyframes, effects, audio settings, links), duration/in-point policy (`keep_duration` trims or pads with a warning; `keep_source_in`), what happens to speed/freeze.
  - **Atomicity:** every command is all-or-nothing and returns the list of items it moved, trimmed, split or removed (`EditReport`). Agents depend on this.
  - **Snapping** (pure function, specified here, used by UI only): candidates are playhead, item edges, markers, beat markers. Threshold is 8 px converted to time. Priority order and tie-break rules.
  - **Selection** semantics: multi-select, linked-group selection (selecting a linked group selects members; `alt` selects one member).
- **Interfaces:** the spec defines the signatures of the commands implemented later: `timeline.insert`, `timeline.overwrite`, `item.move`, `timeline.lift`, `timeline.extract`, `timeline.ripple_delete`, `item.delete`, `item.duplicate`, `clip.trim`, `clip.ripple_trim`, `clip.roll`, `clip.slip`, `clip.slide`, `clip.split`, `clip.replace`, `item.link`/`item.unlink`, and the `EditReport` type.
- **Acceptance criteria:**
  - Document reviewed and approved by a human maintainer (PR approval).
  - ≥ 40 scenario fixtures in `docs/spec/fixtures/` as JSON `{before, command, after_expected | error}`, covering every command × {magnetic, free track, locked track, linked group, clamp}.
  - Every command in the spec lists its invariants as numbered statements `INV-n` that tests cite by id.
  - Includes an explicit "NOT supported in P2" list (multicam, nested compound clips, track targeting UI).
- **Tests:** fixtures are consumed later by 2A.03/04 as table-driven tests; this WP adds a JSON-schema validation script for the fixtures (`cargo xtask check-fixtures`).
- **Out of scope:** implementation, UI gestures (2A.08).

### WP-2A.02 — Model additions and track/marker/range commands
- **Area:** crates/cutweave-model, crates/cutweave-ops (`track`, `timeline` marker/range modules)
- **Depends on:** E1A, E1B, WP-2A.01
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Model: `Marker {id, t, duration?, label, color, kind(user|beat|chapter)}` on sequence and on item. `Sequence.in_out`. `Track.magnetic` constraint (one per sequence). `Item.linked_group: Option<GroupId>`. Track kinds as in §4.2. Unlimited track counts; no hard cap, soft warn above 64.
  - Schema version bump (v2) with migration and golden fixture from a v1 project.
  - Track commands: `track.add{kind, index?}`, `track.remove`, `track.reorder`, `track.rename`, `track.set_flags{muted,hidden,locked,solo}`, `track.set_magnetic`, `track.set_height` (UI hint, `view`-like but persisted).
  - Marker/range commands: `timeline.add_marker`, `timeline.move_marker`, `timeline.remove_marker`, `timeline.set_in_out`, `timeline.clear_in_out`.
  - MCP: all catalog (ADR-0023 R33: no new core MCP tools).
  - **As built (spec §11, arch §5.1):** renaming and the flags stay in the P1 `track.set` (it is the "set_flags" core tool; `track.rename` and `track.set_flags` do not exist), `track.set_magnetic` follows spec §4.17 and returns an `EditReport`, and the marker query is `timeline.list_markers` because `marker` is not a command namespace (arch §5.1). The model additions share one schema v2 with WP-2F.07 (arch §17, "One v2").
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** adds the two commands the timeline menus need and nothing else: `timeline.update_marker{id, label?, color?, duration?}` (the marker menu's Edit) and `item.set_label_color{items, color}` (the clip menu's Color label). The menus themselves are WP-2A.10's. *(Not built in WP-2A.02, which merged first: the timeline menus' WP adds these two commands.)* *(Built in issue #222, split from WP-2A.10: `timeline.update_marker` also takes `kind?` for Convert to chapter, a `duration` of 0 makes a point marker, and the clip's colour is `Item.color_label` of arch §4.2, an optional `#rrggbb` left out of the file while absent.)*
- **Interfaces:** commands above; queries `track.list`, `timeline.list_markers`.
- **Acceptance criteria:**
  - A v1 fixture migrates to v2 and round-trips (insta snapshot); newer-schema projects still open read-only.
  - Property test: for 1,000 random sequences of track/marker commands, `undo(apply(doc)) == doc` and serialize round-trip holds.
  - `track.set_magnetic` on a track with gaps fails with `Invalid{path:"track", msg}` or closes gaps; behaviour per 2A.01. Exactly one magnetic track invariant is asserted after each command.
  - Solo/mute flags affect only `audio`/render evaluation, never item positions.
  - `cargo test -p cutweave-model -p cutweave-ops` and schema snapshot gate pass.
  - Both commands have undo identity in the property harness and an MCP catalog entry.
- **Tests:** unit, proptest, migration golden, insta schema snapshots.
- **Out of scope:** audio-engine honouring solo (2E.02), UI.

### WP-2A.03 — Placement core: insert, overwrite, move, lift, extract, ripple-delete
- **Area:** crates/cutweave-ops/src/timeline/{placement.rs, magnetic.rs}
- **Depends on:** WP-2A.01, WP-2A.02
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Implement the placement algebra from the spec as one internal module (`PlacementPlan` computed purely from a snapshot, then applied in a `Tx`). All commands share it so that magnetic/locked/linked rules live in one place.
  - Commands: `timeline.insert{media|item, track, at, mode}`, `timeline.overwrite`, `item.move{items[], delta | to_track+at, mode}`, `timeline.lift`, `timeline.extract`, `timeline.ripple_delete{items[] | range}`, `timeline.delete_gap`, `clip.split{item, at}` (extends P1 command with linked-group split and returns `EditReport`), `item.delete`, `item.duplicate_in_place` (the clipboard in 2A.06 reuses it). A move/insert whose target range overlaps on a non-main track creates a new track above (`created_tracks` in the report).
  - `mode = insert|overwrite|new_track`.
  - Gesture support: `item.move` accepts being called repeatedly under a `gesture` tx.
  - MCP core: `timeline_insert`, `clip_split`, `item_move`, `item_delete`. Others catalog.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** Ripple delete, Overwrite, Lift, Extract and Delete gap are items of the `timeline.clip`, `timeline.lane` and in/out range bar surfaces (tables owned by WP-2A.10); each runs its command with arguments from the menu context, no UI-only variants.
- **Interfaces:** `EditReport {moved, trimmed, split, removed, created_tracks}`. Typed errors `Locked`, `Conflict`, `Invalid`.
- **Acceptance criteria:**
  - All spec fixtures for these commands pass (`cargo test -p cutweave-ops timeline::fixtures`).
  - Property tests (`proptest`, ≥ 10k cases in CI nightly, 500 on PR) assert: **no overlap on any track (including after auto-created tracks and random overlapping drops); magnetic track gapless and starting at 0 after every command; items on a locked track never change; sequence total duration equals max item end; undo identity; redo identity; `EditReport` matches the diff of before/after exactly**.
  - Ripple on a 5,000-item sequence completes in < 5 ms (criterion bench `placement_ripple_5k`); command RTT contribution stays within §19 p95 < 16 ms.
  - Every failing command leaves the doc byte-identical (`Result::Err` ⇒ no patch).
- **Tests:** table-driven fixtures, proptest generators for sequences (shared in `cutweave-ops::testutil`), criterion bench.
- **Out of scope:** trim family (2A.04), linked-group creation commands beyond the split/delete/move semantics (2A.05).

### WP-2A.04 — Trim family: trim, ripple trim, roll, slip, slide
- **Area:** crates/cutweave-ops/src/clip/trim.rs
- **Depends on:** WP-2A.03
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Commands: `clip.trim{item, edge(start|end), to | delta}`, `clip.ripple_trim`, `clip.roll{left, right, to}`, `clip.slip{item, delta}`, `clip.slide{item, delta}`. All gesture-friendly via `gesture` tx.
  - Clamping at media bounds, neighbour bounds (non-ripple), speed (source range = duration × speed) and zero-duration. Min item duration is 1 frame. Return clamped value in output (`applied_delta`).
  - Still images and generators have no source bound. Freeze frames behave like stills.
  - Linked members trim together (video+audio) unless `unlink: true`.
  - Magnetic track: ripple_trim and plain trim both close gaps; roll/slip never change track length.
  - MCP core: `clip_trim`. Others catalog.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** the `timeline.edge` menu (Trim to playhead, Ripple trim to playhead, Roll to playhead) runs `clip.trim`, `clip.ripple_trim` and `clip.roll` with `to` set to the playhead. If "Extend to next edit" cannot be said with `to`, add `clip.extend_to_next{item}` here.
- **Interfaces:** `TrimOutput {applied_delta, report: EditReport}`.
- **Acceptance criteria:**
  - Spec fixtures for the trim family pass.
  - Property tests: slip never changes `start/duration`; roll preserves the sum of the two durations and the outer edges; slide preserves the item duration and the outer edges of the neighbours; ripple trim shifts all downstream items by exactly the delta; source range never exceeds media duration; INV set from the spec all hold; undo/redo identity.
  - Trim with `speed=2.0` at a 10 s source maps `source_in`/`source_out` exactly in flicks (no rounding drift over 1,000 random trims; asserted).
- **Tests:** fixtures, proptest, unit for clamps.
- **Out of scope:** keyframe retiming on trim (2C.03), UI gestures.

### WP-2A.05 — Linked clips and groups
- **Area:** crates/cutweave-ops/src/item/link.rs; packages/editor (stores)
- **Depends on:** WP-2A.03
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - Commands: `item.link{items[]}`, `item.unlink`, `item.group_select` (query: members of a group), `timeline.insert` auto-links video+audio when the source has both (option `link_av`, default true).
  - "Attach to main clip": `item.attach{child, parent}` sets a parent relationship (child follows parent start; per spec).
  - Link survives split (each half linked to the corresponding half), duplicate (new group id), copy/paste.
  - MCP: catalog for link/unlink/attach.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** Link, Unlink and Attach to… are items of the `timeline.clip` surface; `item.linked` is a `when` context key. Menus act on the whole linked group by default.
- **Interfaces:** `linked_group` queries via `item.get`; `LinkReport`.
- **Acceptance criteria:**
  - Moving, deleting, splitting, trimming one member applies to all members, with fixture tests from the spec.
  - Property test: members of a group always have identical `(start, duration)` for AV links; attached children's `start` stays within parent range after any parent command.
  - Unlink then move affects only one item.
  - `cargo test -p cutweave-ops link::` green.
- **Tests:** fixtures + proptest.
- **Out of scope:** audio-only extraction (2E.03), UI badges (2A.10).

### WP-2A.06 — Clipboard: copy, cut, paste, paste-attributes, duplicate
- **Area:** crates/cutweave-ops/src/timeline/clipboard.rs; packages/editor (clipboard store)
- **Depends on:** WP-2A.03, WP-2A.05
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - `timeline.copy{items[]} → ClipboardPayload` (a pure query returning serialized relative layout: items, relative offsets, relative track indices, links). The UI stores the payload in the system clipboard as `application/x-cutweave-items+json` and also in the engine, so that an MCP agent can `timeline.paste` too.
  - `timeline.paste{payload, at, track, mode}` assigns fresh IDs and remaps links.
  - `item.duplicate{items[], offset}`, `clip.paste_attributes{from, to, which[]}` (transform, effects, audio).
  - Cross-sequence paste works (IDs remapped, media refs preserved).
  - MCP: catalog.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** Cut, Copy, Paste attributes… and Duplicate are items of `timeline.clip`; Paste here is an item of `timeline.lane` (`when clipboard.hasItems`, time and track from the click). Shortcut hints come from the keymap.
- **Interfaces:** `ClipboardPayload` (versioned JSON Schema).
- **Acceptance criteria:**
  - Copy → paste in another sequence yields structurally equal items except IDs (proptest `paste(copy(x)) ≅ x`).
  - Pasting 200 items < 20 ms.
  - Pasting over a locked track fails with `Locked` and changes nothing.
  - No ID collisions across 1,000 random pastes.
  - Context-menu choices Copy then Paste here give the same document as `timeline.copy` and `timeline.paste` through the CLI (parity script).
- **Tests:** unit + proptest.
- **Out of scope:** pasting from external applications.

### WP-2A.07 — Snapping engine (Rust, shared via wasm)
- **Area:** crates/cutweave-model/src/snap.rs (wasm-safe), packages/timeline/src/snap/ (thin wrapper)
- **Depends on:** WP-2A.01 (spec), WP-2C.06, E1M
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - A pure Rust function `snap(index, t, threshold_px, px_per_second, options) → {t, target?}` in `cutweave-model::snap`, exported through the 2C.06 wasm module, implemented exactly per spec priority: playhead, item edges (including edges on other tracks), markers, beat markers, in/out. Toggle plus hold-modifier override.
  - A spatial index (sorted array + binary search) rebuilt incrementally on `doc.patched`.
  - Beat markers are `Marker.kind='beat'` and are produced here by manual tap (`timeline.add_marker`) or later by P5 analysis. See Notes N5.
- **Interfaces:** Rust `SnapIndex`, `snap`; `@cutweave/timeline` exports `createSnapIndex`, `snap` (wrappers over the wasm exports from `@cutweave/protocol/wasm`).
- **Acceptance criteria:**
  - Rust unit tests: 30+ cases including ties, disabled kinds, threshold scaling with zoom; a Vitest smoke test proves the wasm build returns identical results on the same vectors.
  - Perf: snap lookup on 50,000 candidates < 0.1 ms p95 (criterion and vitest bench through wasm).
  - Snapped result is always on the frame grid.
- **Tests:** Rust unit + criterion, Vitest wasm parity + bench.
- **Out of scope:** drawing snap guides (2A.09).

### WP-2A.08 — Timeline interaction state machine
- **Area:** packages/timeline/src/interaction/
- **Depends on:** WP-2A.04, WP-2A.07, WP-2A.12
- **Size:** M
- **Executor:** opus
- **Scope:**
  - A framework-free, explicit finite state machine (XState-style hand-rolled reducer, no renderer dependency) consuming normalized pointer/keyboard events and emitting `Intent`s (`begin_tx`, `command(name,args)`, `update_tx`, `commit_tx`, `cancel_tx`, `set_selection`, `set_cursor`).
  - States: `idle, hover, selecting(marquee), moving, trimming, ripple_trimming, rolling, slipping, sliding, razor, scrubbing, keyframe_dragging, panning, zooming`.
  - Tool modes (select, razor, ripple, slip/slide, hand).
  - Esc cancels and rolls back the `gesture` tx. Modifier semantics: Shift = additive selection / constrain axis, Alt = single member of linked group / duplicate-on-drag, Ctrl/Cmd = disable snapping.
  - Auto-scroll at edges; drag across tracks with `new_track` preview; drop-on-gap and drop-on-clip insert-vs-overwrite per spec.
  - Edge cases: pointer leaves window, command rejected mid-drag (shows clamped state, no error spam), an agent command touching a gesture's items fails with `Conflict{item_in_gesture}` (§5.2, engine side in 2A.12); the machine never retries on its own. If the engine auto-cancels the gesture after 30 s idle, the machine receives `gesture.cancelled` and returns to `idle` with the doc rolled back. A drag update rejected because another actor's gesture holds an item shows a locked cursor and does not abort.
- **Interfaces:** `createTimelineMachine(deps) → {send(event), state, subscribe}`; Intent types exported; state chart documented in `packages/timeline/README.md` with a mermaid diagram.
- **Acceptance criteria:**
  - Vitest transcript tests (≥ 60) replay recorded event streams and assert the Intent sequence and final doc via a fake engine that wraps the real `cutweave-ops` through WASM or a recorded-command stub.
  - Property test (fast-check): for random event streams, the machine never reaches an undefined state, never leaves a `gesture` tx open after `pointerup/cancel/blur`, and every `begin_tx` is paired with exactly one `commit_tx` or `cancel_tx`.
  - A drag emits `update` intents at most once per animation frame (coalesced).
  - 100% of states/transitions in the doc diagram are covered (coverage check script).
- **Tests:** Vitest unit/property.
- **Out of scope:** rendering, hit-test geometry (provided via an injected `hitTest`), keyframe lane specifics (2C.05 adds the `keyframe_dragging` handlers).

### WP-2A.09 — Timeline renderer: virtualization, thumbnails, waveforms, zoom/scroll
- **Area:** packages/timeline/src/render/
- **Depends on:** E1M, E1D, WP-2A.02
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Canvas2D virtualized renderer: ruler (timecode and frames), tracks with headers, items, selection, playhead, markers, in/out range, snap guides, linked-group badge, lock/mute/hidden styling, collapsed track heights.
  - Thumbnails and filmstrips come from `/media/{id}/filmstrip` and the waveform from `/media/{id}/waveform?lod=` via a bitmap cache (LRU, bounded to 256 MB) and LOD selection by zoom. Draws placeholders immediately and never blocks a frame on a fetch.
  - Horizontal zoom range from 1 frame/≥ 100 px up to 4 hours fit. Zoom is anchored at the cursor. Smooth wheel/pinch. Vertical scroll with sticky track headers.
  - Spatial index (interval tree) for hit-testing: `hitTest(x,y) → {item, part: body|left-edge|right-edge|fade-handle|keyframe, trackId}`.
  - Redraw is dirty-rect or full-frame only when needed (`requestAnimationFrame` coalescing).
- **Interfaces:** `<TimelineCanvas>` React wrapper in `packages/editor`; `TimelineRenderer` class API in `@cutweave/timeline`.
- **Acceptance criteria:**
  - Storybook-less demo page `packages/timeline/demo` with a 5,000-item / 50k-keyframe / 200-thumbnail fixture pans and zooms at ≥ 55 fps (§19), measured by the 2J.02 harness (a local probe in this WP prints fps).
  - Only items intersecting the viewport are drawn (instrumented counter test).
  - Hit-test p95 < 0.2 ms with 5,000 items.
  - Visual regression snapshots (Playwright) for 6 states: empty, mixed tracks, selected, locked, zoomed-out, markers.
- **Tests:** Vitest for geometry and hit-test, Playwright visual regression.
- **Out of scope:** keyframe lanes (2C.05), fade handles drawing (2E.05).

### WP-2A.10 — Timeline UI wiring: tools, selection, headers, edit gestures
- **Area:** packages/editor/src/timeline/
- **Depends on:** WP-2A.08, WP-2A.09, WP-2A.06, WP-2A.05, E1M, WP-2H.13
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Wire the state machine to the engine through `@cutweave/protocol` (Intents → `edit.begin_tx`/commands/`commit_tx`). Selection lives in `viewStore` and can be set by agents via the MCP `view_set` tool (N3).
  - Track header controls (add, lock, mute, solo, hide, rename, height, magnetic toggle). Context menus built from the command registry filtered by `when` clauses.
  - Toolbar: select, razor, ripple, slip, slide, snap toggle, link toggle, zoom-to-fit, zoom-to-selection. Markers, in/out UI (M, I, O keys, double-click marker to edit).
  - Multi-select, marquee, linked-group selection, copy/cut/paste/duplicate bindings.
  - Drag from the media bin to timeline with insert/overwrite/new_track affordances; Alt-drop (or context menu "Replace clip") onto an existing clip invokes `clip.replace`. Drop preview ghost.
  - Edits show engine errors in a non-blocking toast with machine code.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** replaces the one-line mention above: this WP owns the full content of the timeline surface tables `timeline.clip`, `timeline.edge`, `timeline.lane`, `timeline.track`, `timeline.ruler`, `timeline.playhead`, `timeline.marker` (items, order and disabled rules in section 6.4 of the research; WP-2H.13 seeds them with today's commands). Items from WPs that are not merged yet are omitted, not disabled. Right-click selects as WP-2H.12's target rules say.
- **Interfaces:** commands from 2A.02–2A.06; `view.*` UI state in `viewStore`; the MCP catalog exposes `view_get`/`view_set` (playhead, selection, in/out, zoom; non-undoable, not journaled; see 2I.01).
- **Acceptance criteria:**
  - Playwright+Electron e2e (≥ 12 scenarios): drag from bin, razor split, ripple delete on the magnetic track (no gap), roll edit, slip, slide, link toggle moves A+V together, lock prevents edit, marquee select, copy/paste across tracks, Esc cancels a drag, undo restores.
  - Command RTT UI→engine→patch→UI p95 < 16 ms measured by an in-app probe during a 100-step drag.
  - No UI mutation of `docStore` outside patch events (lint rule + test).
  - F37, F38 and F39 are extended: one edit in each is done through the context menu (ripple delete, unlink, paste here, add marker here, lock a track from its header menu); axe: 0 violations on each timeline menu.
- **Tests:** Playwright e2e, Vitest for store logic.
- **Out of scope:** keyboard shortcut editor (2H.05); keyframe lane UI (2C.05).

### WP-2A.11 — Replace clip
- **Area:** crates/cutweave-ops/src/clip/replace.rs; packages/editor (bin drop and context menu, wired in 2A.10)
- **Depends on:** WP-2A.03, WP-2A.04, WP-2A.05
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - `clip.replace{item, media, mode: keep_duration|keep_source_in|fit_to_source, source_in?}` swaps the media of a clip while keeping its attributes: transform, crop, opacity, blend, all keyframes, effects, masks, audio settings, color label, links and track position (per the 2A.01 spec).
  - Duration policy: `keep_duration` (default) trims the new media's range to the old duration or, if the new media is shorter, shortens the clip (reported as `shortened: true`); `fit_to_source` changes the duration and ripples per track rules (magnetic: gapless).
  - Kind change rules: video→image allowed (speed fields reset, reported); audio↔video blocked with `Invalid`. Linked audio is replaced together with the video when the new media has audio, otherwise unlinked and muted per option.
  - MCP: catalog (`clip_replace`).
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** "Replace clip…" is an item of `timeline.clip`; with a bin card selected it replaces with that media, otherwise it asks for one; the bin card menu offers "Replace selected clip" when one timeline clip is selected. Both run `clip.replace`.
- **Interfaces:** `clip.replace → {item, report: EditReport, shortened}`.
- **Acceptance criteria:**
  - Replacing a keyframed, effect-bearing clip: `item.get` before and after are equal except `media`, `source_in`, `duration` (fixture test).
  - Property test: replace never creates overlap, keeps a magnetic track gapless, and `undo(apply(doc)) == doc` (1,000 cases).
  - Locked track returns `Locked` and changes nothing.
- **Tests:** fixtures, proptest, Playwright smoke through Alt-drop.
- **Out of scope:** replace-all-instances, AI generative replace (P5).

### WP-2A.12 — Soft-lock extension: retry semantics, UI feedback and auto-cancel
- **Area:** crates/cutweave-commands/src/gesture.rs, crates/cutweave-api (error mapping)
- **Depends on:** E1B, WP-2A.03
- **Size:** M
- **Executor:** opus
- **Scope:**
  - **Extends** the minimal soft-lock shipped in P1 (E1B; decision B) rather than introducing it: P2 adds the full touched-set computation, retry semantics and UI feedback. An open `gesture` tx registers a soft lock on the set of items its commands touch (including items shifted by ripple and linked members, as defined in 2A.01). Another actor's command that touches a locked item fails fast, before any mutation, with `Conflict{reason:"item_in_gesture", holder, retry_after_ms}`. Commands on other items proceed.
  - The holder's own later updates may extend the lock set. Commit and cancel release it. Undo of a gesture inverts only its own ops (valid because the locks guarantee disjointness).
  - Auto-cancel applies to `gesture`-kind txs only: after 30 s without updates (timer on the doc actor, injectable clock for tests) it rolls back, releases locks, emits `gesture.cancelled{reason:"idle_timeout"}`. `proposal`-kind txs have no idle timeout and are cancellable explicitly; they soft-lock their touched items by the same rule.
  - Retry semantics: `Conflict` carries `retry_after_ms` (hint from the holder's remaining idle budget; none for proposals) and the holder; MCP clients may retry after the hint, the UI never retries on its own. UI feedback: locked items show a non-blocking "in use by <actor>" indicator and a toast with the machine code.
  - Saves persist only the last committed rev (an open gesture/proposal tx is never written to disk).
  - A tx is owned by its connection: a dropped connection or engine restart rolls back that connection's open txs (gesture and proposal) and releases their locks.
  - Same-actor commands are never blocked. MCP error text includes a retry hint. All error carries the machine code.
- **Interfaces:** error `Conflict{reason, holder, retry_after_ms}`; event `gesture.cancelled`; `edit.begin_tx{kind:"gesture"|"proposal"}` returns `tx_id`.
- **Acceptance criteria:**
  - Property test (proptest, interleaved actors A/B, random commands, random gesture begin/commit/cancel): history stays linear; B never mutates a locked item while A's gesture is open; commands on disjoint items always succeed; after A cancels, doc equals the state as if the gesture never happened plus B's disjoint edits; undo identity holds.
  - Fake-clock test: gesture idle for 30 s is cancelled and the doc is rolled back; activity every 29 s keeps it alive.
  - Rejected commands do not change the doc or the rev (byte-identical snapshot).
  - Lock check adds < 20 µs per command on a 5,000-item doc (criterion).
  - Parity test: identical error over RPC, MCP and CLI.
- **Tests:** proptest, fake-clock unit, parity.
- **Out of scope:** UI behaviour (2A.08), cross-process collaboration.

### WP-2A.13 — Settings: Editing and Timeline pages (editing defaults, snapping, playhead follow, display)
- **Area:** packages/editor (the two pages; the callers that pass the defaults to commands), packages/timeline (follow modes, thumbnail and waveform display, track height, wheel, time display), crates/cutweave-commands or crates/cutweave-engine (undo history limit), generated schema and TS types, docs/reference/settings.md, apps/desktop/e2e/flows, docs/user-flows.md
- **Depends on:** WP-1M.11, WP-2A.05, WP-2A.07, WP-2A.09, WP-2A.10
- **Size:** M
- **Executor:** sonnet
- **Scope:** (product owner request, 2026-11-24) the Editing and Timeline pages of the settings catalog (`docs/reference/settings.md`); all rows are app preferences.
  - **Editing page:** keep a clip's video and audio linked when inserted (on); dropping media onto a clip outside the main track overwrites (default) or inserts; nudge step for comma and period (1 frame, Shift ×10); undo steps kept per person or agent (1,000), applied through a new engine command (for example `edit.set_history_limit{steps}`: UI-only, not undoable, not journaled) that drops the oldest steps, with the limit reported by `project.info`. Rows other WPs add to this page: still image duration (WP-2F.05), one-click fade length (WP-2E.05), text duration (WP-3A.07), caption defaults (WP-3E.11), transition duration (WP-4A.09).
  - **Timeline page:** snapping on when a project opens (on); snap to playhead, clip edges, markers and beats (all on; the shared snapping of WP-2A.07); snap distance (8 px); follow the playhead while playing: page (default), smooth, off; main track magnetic in new sequences (on; passed when the editor creates a sequence, as `main_magnetic` on `project.new` and `sequence.create` per ADR-0037, accepted 2026-11-27; built under issue #240); video clip thumbnails: filmstrip (default), first frame, off; audio waveforms on clips (on); height of new tracks: small, medium (default), large; the mouse wheel scrolls (default; Ctrl/Cmd+wheel zooms) or zooms; show time as timecode (default), frames or seconds; drop-frame timecode at 29.97 and 59.94 fps (on).
  - Rows apply at once to the open timeline. Nothing is written to the project except through the commands the defaults feed.
- **Interfaces:** preference keys `editing.*` and `timeline.*` (catalog); `edit.set_history_limit` (UI-only engine command).
- **Acceptance criteria:**
  - Unit: the history limit drops the oldest steps per actor and undo still works up to it; an agent token calling `edit.set_history_limit` gets `forbidden`.
  - Vitest: each display row changes what the timeline renderer draws (one snapshot per value); every follow mode keeps the playhead in view during simulated playback, except Off.
  - **A user flow (ADR-0018, F25):** turn "Snapping on when a project opens" off, relaunch and open the project: snapping is off; show time as frames: the ruler and the inspector show frames; set follow to Off and play: the view stays where it was; set new tracks to Large and drop a clip that creates a track: it is large; set undo steps to 2, make three edits: two undo and the third does not.
- **Tests:** unit, Vitest, user flow.
- **Out of scope:** per-sequence overrides of these defaults; the rows other WPs own (listed above).

### WP-2A.14 — Source marks and three-point editing: in and out on a source, insert and overwrite, match frame
- **Area:** packages/editor (Source view marks, insert and overwrite actions and keys, match frame, marks on bin cards), crates/cutweave-engine (`source_marks` in the view state and `ui-state.json`), generated schema and TS types, docs/spec/edit-semantics.md (three-point rules), apps/desktop/e2e/flows, docs/user-flows.md
- **Depends on:** WP-1M.15, WP-2A.01, WP-2A.03, WP-2A.10
- **Size:** M
- **Executor:** sonnet
- **Scope:** (product owner request, 2026-11-24, the editing half of "preview without adding to timeline"; P1's WP-1M.15 plays a source and places the whole asset)
  - **Marks:** I and O set in and out on the source at its playhead (the same keys as on the timeline, by focus), "Clear in and out" clears both. Marks show on the Source view's scrub bar and as a range bar on the asset's bin card. They are kept per asset in the view state (`view.set{source_marks}`, persisted in `ui-state.json` by WP-1C.04's store; not undoable, not journaled), so they survive a reopen.
  - **Insert** (`,`) and **Overwrite** (`.`) place the marked range (or the whole asset) at the timeline playhead on the target track: `timeline.insert{source_in, duration}` and WP-2A.03's overwrite. Three-point rules follow `edit-semantics.md`: with timeline in and out set, the timeline in wins and the duration comes from the source marks. Each placement is one undo step.
  - **Drag from the Source view** places the marked range. Dragging only its picture or only its sound icon places video only or audio only.
  - **Match frame** (F) with a timeline clip under the playhead: opens the clip's media in the Source view at the matching source frame, with the clip's source range as its marks.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** the Source view's menu (`source.view`): Insert, Overwrite, Set in, Set out, Clear in and out, Match frame, Back to timeline; Overwrite at playhead joins `bin.card`, Match frame joins `timeline.clip`.
- **Interfaces:** `view.set{source_marks}` (additive view state); the Source view's Insert and Overwrite actions and keys (in `listBindings()`).
- **Acceptance criteria:**
  - Vitest: the three-point placement math agrees with the `edit-semantics.md` fixtures (source marks only, timeline marks only, both).
  - Parity: the item that Insert from the Source view creates is the one `timeline.insert{source_in, duration}` creates through the CLI on the same project (normalized structure).
  - **A user flow (ADR-0018, F31 extended):** play the interview in the Source view, mark in at 2 s and out at 5 s, press `,`: a 3 s clip lands at the playhead starting at source 2 s; press `.` over an existing clip: it is overwritten; reopen the project: the marks are still on the card; F on the placed clip opens it in the Source view at the matching frame.
  - axe: 0 violations on the Source view with marks; visual baselines of the marked scrub bar and card.
- **Tests:** Vitest, parity script, user flow.
- **Out of scope:** four-point editing and fit to fill; subclips; source and timeline viewers side by side (ADR-0022, option 2).

---

# Epic 2B — Sequences & canvas

### WP-2B.01 — Multi-sequence commands, aspect presets, sequence settings
- **Area:** crates/cutweave-ops/src/sequence/, crates/cutweave-model (aspect presets table)
- **Depends on:** E1A, E1B, WP-2A.02
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Commands: `sequence.create{name, preset|format}`, `sequence.duplicate`, `sequence.delete` (not the last one; not while referenced as a compound clip, see 2B.04), `sequence.rename`, `sequence.set_format{width,height,frame_rate,sample_rate,background}`, `sequence.set_active` (`view`; registered as `view.set_active_sequence`, since the registry reserves the `view` flag for `view.*` names), `sequence.list` query (`presets: true` adds the preset tables).
  - Aspect presets as L0 data (`16:9 1920x1080 & 3840x2160`, `9:16 1080x1920`, `1:1 1080x1080`, `4:5 1080x1350`, `21:9 2560x1080`), frame-rate presets 24/25/30/50/60.
  - Changing resolution or aspect: a `reframe` policy (`keep_scale | fit | fill | none`) applied to clip transforms. Default `fit`: preserve each clip's relative framing so it fits the new canvas; documented algorithm. Text/overlay items positions scale proportionally.
  - Changing frame rate re-snaps all item times to the new grid (rounding to nearest frame, no overlap introduced; INV tested).
  - MCP: `sequence_create`, `sequence_set_format` core. Others catalog.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** adds `sequence.create_from_media{media[], name?}`: a new sequence holding the assets in order (the bin card menu's "New timeline from selection", as in Resolve's "Create timeline using selected clips").
- **Interfaces:** `AspectPreset`, `ReframePolicy`, commands above.
- **Acceptance criteria:**
  - Duplicate sequence creates deep copy with new IDs and shared media; modifying the copy never changes the original (proptest).
  - Changing 16:9→9:16 with `fit` leaves every clip's full frame visible (unit test on computed transform bounds).
  - Frame-rate change 30→24 preserves overlap-freeness and magnetic gaplessness (proptest, 1,000 cases).
  - Undo identity for all commands.
  - The command has undo identity and a CLI/MCP parity test like the others.
- **Tests:** unit, proptest, snapshot of preset table.
- **Out of scope:** background fill rendering (2B.02), UI (2B.03).

### WP-2B.02 — Background fill: color, blur, image
- **Area:** crates/cutweave-render (background pass), crates/cutweave-model (`Background`)
- **Depends on:** E1F, WP-2B.01, WP-2C.04
- **Size:** M
- **Executor:** opus
- **Scope:**
  - `Background = Color{rgba} | Blur{source: main_track|below, radius, dim} | Image{media, fit(cover|contain|stretch)} | Transparent` (Transparent only for alpha-capable exports).
  - Rendering: Color is a clear. Blur renders the lowest visible video layer scaled to cover the canvas, blurred with a separable Gaussian (downsample + blur + upsample, single pass pair), with radius in sequence-height-relative units so preview and export match. Image is a textured quad.
  - Linear-light blend (§8), no banding (dither at the final 8-bit quantize).
  - Commands: `sequence.set_background`. Background can't be keyframed in P2.
  - Preview quality ladder consistency: blur radius scales with preview scale factor.
- **Interfaces:** `sequence.set_background{kind,...}`; render scene node `BackgroundNode`.
- **Acceptance criteria:**
  - Golden frames (WARP + lavapipe) for the 4 kinds with a 16:9 clip on a 9:16 sequence; preview (full quality) equals the pre-encode export frame bit-for-bit on the same adapter; regression vs the stored per-adapter reference SSIM ≥ 0.995 and PSNR ≥ 40 dB; blur GPU cost on iGPU < 1.5 ms at 1080×1920 (render bench, run in 2J).
  - Export of the same sequence is bit-for-bit deterministic across 2 runs.
- **Tests:** golden frames, unit for radius scaling.
- **Out of scope:** per-clip background (that is `VisualProps.background_fill`; deferred, see N10).

### WP-2B.03 — Sequence UI: tabs, settings dialog, aspect switcher
- **Area:** packages/editor/src/sequences/
- **Depends on:** WP-2B.01, WP-2B.02, WP-2H.02
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - Sequence tabs above the timeline (open, close, reorder, duplicate, rename via double-click). Aspect switcher in the preview toolbar offering the presets plus custom. A settings dialog (schema-driven form) with a reframe policy selector. Background picker (color picker, blur slider, image picker from bin).
  - Active sequence persisted in `viewStore` and the project's `ui` state.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** a menu on the sequence tab (`sequence.tab`): Rename, Duplicate, Settings…, Aspect ▸ (the presets), Close, Close others.
- **Interfaces:** consumes 2B.01/02 commands.
- **Acceptance criteria:**
  - Playwright: create a 9:16 sequence, switch aspect, the preview canvas changes aspect within one frame of the patch event; the tab state survives save/reopen.
  - All controls have accessible names (axe check on the dialog, 0 violations).
- **Tests:** Playwright, axe.
- **Out of scope:** per-sequence workspace layouts.

### WP-2B.04 — Compound clips: nested sequences as media
- **Area:** crates/cutweave-model (`sequence-ref`), crates/cutweave-ops/src/compound/, crates/cutweave-render, crates/cutweave-audio
- **Depends on:** WP-2B.01, WP-2A.03, WP-2C.04, E1F, E1G
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Model (§4.3): `MediaAsset.kind = sequence-ref` pointing to a `SequenceId`; a compound clip is a normal `Clip` referencing it. Schema migration and fixture. Duration of the reference follows the inner sequence.
  - Commands: `compound.create{items[], name}` (replaces the selection with one clip referencing a new sequence holding the items, keeping relative layout, tracks and links; undo restores exactly), `compound.break_apart{item}`, `sequence.insert_as_compound{sequence, track, at}` (drop a sequence from the project onto a timeline), `compound.open` (`view`: enter for editing).
  - **Cycle detection:** a sequence may not reference itself directly or transitively; enforced at validation for every command that creates or retargets a `sequence-ref` (`Invalid{code:"compound_cycle"}`) and at project load (load rejects with a typed error; fuzz case).
  - Recursive render: the inner sequence is evaluated as a sub-scene at the inner time (`source_time` including speed), rendered to an offscreen texture at the clip's needed resolution (not the full inner format when smaller), cached by (inner subgraph hash, t). Audio: inner mix is a bus feeding the outer clip with the outer clip's gain/pan/fades/speed. Effect-less in P2 (effects come with P4 but apply on the compound like any clip).
  - Deleting a sequence referenced by compound clips is rejected (`Invalid{refs[]}`); `sequence.duplicate` of an outer sequence keeps references to the same inner sequences (documented; option `deep_copy`).
  - MCP: `compound_create` core-eligible (listed in `full`, catalog otherwise); others catalog.
- **Interfaces:** `MediaKind::SequenceRef`; commands above; query `sequence.references{sequence}`.
- **Acceptance criteria:**
  - Cycle property test (random reference graphs up to depth 6): no command ever produces a cycle; a hand-crafted cyclic project file is rejected at load.
  - Round trip: `compound.create` then `compound.break_apart` restores the original items (structural equality ignoring IDs); undo identity for both.
  - Golden frame: a compound with 2 layers renders identically (regression vs the stored reference SSIM ≥ 0.995 and PSNR ≥ 40 dB) to the same layers flattened on the outer timeline; audio null test < −90 dBFS.
  - Edits to the inner sequence are reflected in the outer render and in the export; a 3-level nest plays 1080p30 on the reference iGPU with 0 dropped frames / 30 s.
  - Magnetic and no-overlap property tests extended with compound commands.
- **Tests:** proptest, golden, audio null, load fuzz seed corpus.
- **Out of scope:** multicam, per-instance overrides of inner content, UI (2B.05).

### WP-2B.05 — Compound clip UI: create, enter, breadcrumb
- **Area:** packages/editor/src/compound/, packages/timeline
- **Depends on:** WP-2B.04, WP-2A.10, WP-2B.03
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - Context menu and shortcut "Compound clip" / "Break apart"; compound items drawn with a distinct badge and a mini-structure preview. Double-click enters the inner sequence (opens its tab with a breadcrumb "Outer > Compound"), Esc or breadcrumb click returns. Sequence list shows reference counts and warns before deleting a referenced sequence.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** Compound clip, Break apart and Enter compound are items of `timeline.clip` (the existing mention above is built on WP-2H.12's host, not on a menu of its own).
- **Interfaces:** `view.enter_compound`, consumes 2B.04 commands.
- **Acceptance criteria:**
  - Playwright: select 3 clips, create compound, enter, trim an inner clip, exit; the outer duration updates; undo reverses; the cycle attempt (drop a sequence onto itself) shows the typed error toast.
  - axe 0 violations on the new menus and breadcrumb.
- **Tests:** Playwright.
- **Out of scope:** nested timeline view inline (expand in place).

---

# Epic 2C — Clip properties & keyframes

### WP-2C.01 — Spec: keyframe and easing model
- **Area:** docs/spec/keyframes.md (new)
- **Depends on:** E1A
- **Size:** S
- **Executor:** opus
- **Scope:** define precisely, extending §4.2 `Animatable<T>`:
  - **Time domain:** keyframe `t` is item-relative timeline time in flicks (not source time), so speed changes do not move keyframes. Behaviour on trim: keyframes outside the new range are kept (not deleted) unless a `prune` flag is set; behaviour on split (boundary value inserted into both halves so that both halves evaluate identically); on ripple (no change); on duplicate; on speed change (unchanged); on slip (unchanged, since it is item-relative).
  - **Interpolation:** `hold | linear | bezier{x1,y1,x2,y2} | spring{stiffness,damping,mass}`; interpolation belongs to the segment *starting* at a keyframe. Easing presets (ease-in/out, in-out, back, expo, bounce, elastic) are named bezier or analytic curves stored as `bezier` or `spring` after expansion, with a `preset` hint. Out-of-range handles for bezier are allowed (overshoot) with x clamped to [0,1].
  - **Value types and combine rules:** scalar, vec2 (per-component interpolation, linked handles optional), color (interpolated in linear light, premultiplied), angle (shortest-path off by default, accumulate for rotation), enum/bool (hold only).
  - **Evaluation contract:** `eval(animatable, t) → T` pure and deterministic, sample-exact at keyframes, clamped before first and after last, uses f64 internally.
  - **Animatable property registry:** the complete list for P2: position, scale (x,y linked option), rotation, anchor, crop (4 edges), opacity, flip (hold), volume, pan, and effect/composition params later.
  - **Auto-key semantics** for `item.set_props` (when a property is already animated, a set at the playhead creates/updates a keyframe; otherwise sets the static value) and an explicit `keyframe.*` API.
- **Interfaces:** definitions for `Keyframe<T>`, `Interp`, `keyframe.set/delete/move/set_interp/clear`, `item.set_props{auto_key}`.
- **Acceptance criteria:**
  - Human review approval. Test vectors file `docs/spec/keyframes-vectors.json` with ≥ 50 `(animatable, t, expected)` rows including overshoot, hold, and spring.
  - Every property in the registry names a type and its unit/range.
- **Tests:** vectors are consumed by 2C.02.
- **Out of scope:** speed ramps (P4), mask/effect params (P4).

### WP-2C.02 — Interpolation core
- **Area:** crates/cutweave-model/src/animate/ (wasm-safe)
- **Depends on:** WP-2C.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Implement `Animatable<T>` `eval`, a cubic-bezier solver (Newton with bisection fallback, error < 1e-6), analytic spring (critically/under/over-damped closed form), colour interpolation in linear light, a per-segment cache for monotonic playback queries (`Evaluator` with cursor) and batch evaluation `eval_range(t0,t1,n)` for curve drawing and the UI.
  - Property types per the registry; `Animatable::sample_times()` for UI lane drawing; JSON schema derive.
  - Stays free of non-wasm-safe dependencies so that 2C.06 can compile it into the shared wasm module (§16). The UI evaluates curves locally through that module; no per-frame RPC.
- **Interfaces:** `Animatable::eval(t)`, `Evaluator`, `Easing::{preset(name), to_bezier}`, query `keyframe.eval{item, prop, from, to, samples}` (for agents and tests only; the UI does not use it).
- **Acceptance criteria:**
  - All `docs/spec/keyframes-vectors.json` rows pass within 1e-6.
  - Proptests: continuity at keyframes; monotone linear; hold has no intermediate values; `eval` equals `Evaluator` result for random monotonic sequences; no NaN/inf for any input including degenerate (zero-duration segments, coincident keyframes rejected at construction).
  - Bench: evaluating 10 animated props × 3 layers at one `t` < 20 µs; 50k keyframes lookup (binary search) < 1 µs.
  - `cargo check --target wasm32-unknown-unknown -p cutweave-model` passes.
- **Tests:** vectors, proptest, criterion.
- **Out of scope:** UI.

### WP-2C.03 — Keyframe and property commands (with retiming rules)
- **Area:** crates/cutweave-ops/src/{keyframe,item}/, crates/cutweave-model (`VisualProps` crop, flip, blend mode; migration and goldens), generated schema and TS types
- **Depends on:** WP-2C.02, WP-2A.04
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Commands: `keyframe.set{item, prop, t, value, interp?}`, `keyframe.delete`, `keyframe.move{…, to_t}`, `keyframe.set_interp`, `keyframe.clear{prop}` (flatten to static at the value at `t` or the first), `keyframe.copy/paste` (in the same module as the clipboard payload), `item.set_props{item, props, auto_key?, at?}` (typed partial update of `VisualProps`, volume, pan; one command for UI inspector and agents), `clip.reset_props`.
  - Hook the keyframe split/trim/ripple/duplicate rules from 2C.01 into 2A commands (boundary keyframe insertion on split; optional prune).
  - Batch support for multi-select: `items[]`.
  - MCP core: `keyframe_set`, `item_set_props`. Catalog for the rest.
  - (product owner request, 2026-11-24: "place 2 videos side by side")
    - **The model gap:** the P1 model's `VisualProps` has only `transform` and `opacity` (the P1 `item.set_props` takes name, enabled, opacity, audio_gain and transform), while arch §4.2 and WP-2C.04 expect `crop`, `flip` and `blend_mode`, and no other WP adds them to `cutweave-model`. This WP adds them: crop as four edges in fractions of the source frame (animatable), flip H/V (hold) and the blend mode. Older documents migrate with the defaults (no crop, no flip, normal); the migration chains after WP-2F.07's v2 in merge order (N18). `item.set_props` takes the new fields, so agents crop with the same command the Inspector and WP-2H.06's handles use.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** adds `item.reset_props{items, group? | props?}` (reset to defaults, one undo step): the Inspector section and row resets exist as buttons with nothing behind them, and the clip menu, `inspector.section` and `inspector.row` menus need it. The row menu also offers Add and Remove keyframe (`keyframe.*`).
- **Interfaces:** commands above. `item.get` returns animatable props with keyframes (`fields` filtering). `VisualProps.{crop, flip, blend_mode}`.
- **Acceptance criteria:**
  - Splitting a clip with keyframes: evaluating each half at any matching timeline time equals the original value within 1e-6 (proptest, 1,000 cases).
  - Setting a keyframe at an occupied `t` replaces it. Moving past a neighbour is rejected with `Invalid` or reorders (per spec; one behaviour, tested).
  - Dragging a keyframe uses a `gesture` tx and yields one undo step.
  - JSON Schema describes ranges (e.g., opacity 0–1) and the parity tests pass.
  - `item.reset_props` restores the default of every field in a group (fixture) and has undo identity.
- **Tests:** unit, proptest, snapshots of schema.
- **Out of scope:** UI.

### WP-2C.04 — Render: transform, crop, opacity, flip, core blend modes
- **Area:** crates/cutweave-render (compositor, WGSL)
- **Depends on:** E1F, WP-2C.02
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Full `VisualProps` in the scene graph: position/scale/rotation/anchor, 4-edge crop (feather 0), flip H/V, opacity, evaluated via 2C.02 at `t`.
  - Core blend modes (§8; the extended set arrives in P4): `normal, multiply, screen, overlay, darken, lighten, add, soft-light`. Implemented in linear light on `Rgba16Float` with premultiplied alpha; formulas documented per mode.
  - Filtering: bilinear with mip/trilinear on downscale to avoid aliasing; bicubic option for export. Sub-pixel positioning stable under animation (no shimmer: golden test on a 1 px grid pattern at 0.25 px steps).
  - Layer ordering follows the track order, top track on top. Hidden tracks excluded. Solo does not affect video.
  - Render-graph culling of fully transparent or off-canvas layers.
- **Interfaces:** `VisualProps` evaluator; scene `LayerNode {transform, crop, opacity, blend}`.
- **Acceptance criteria:**
  - Golden frames (WARP/lavapipe) per blend mode vs. analytic CPU reference at 1e-3 max abs error in linear space on a gradient test chart.
  - Rotation 90° and flip combine with the clip's media rotation tag correctly (corpus clip with rotate=90).
  - Playback gate: 3 video layers with transform animation at 1080p60 on the iGPU reference machine: 0 dropped frames / 60 s (run in 2J.03; locally reports GPU ms ≤ 8 per frame).
  - Preview == export: preview (full quality) vs pre-encode export frame on the same adapter are bit-identical.
- **Tests:** golden, CPU reference unit, bench.
- **Out of scope:** masks, effects, adjustment layers (P4), text.

### WP-2C.05 — Timeline keyframe lanes and curve editor
- **Area:** packages/timeline/src/keyframes/, packages/ui/src/curve-editor/
- **Depends on:** WP-2C.03, WP-2C.06, WP-2A.08, WP-2A.09
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Expandable keyframe lanes under a selected item (one lane per animated property, with a diamond per keyframe, interpolation shown by glyph), drawn by the renderer from `item.get` data and kept within the 5,000-item / 50k-keyframe fps budget by LOD (collapse diamonds < 3 px apart).
  - Add via the inspector "◇" or by dragging on the lane. Select, multi-select, drag in time (snap to playhead/edges), copy/paste, delete. State machine gets `keyframe_dragging` handlers (extends 2A.08).
  - `@cutweave/ui` `<CurveEditor>`: bezier handle editing, preset picker, value/time axes, spring parameters, live preview curve evaluated locally via the 2C.06 wasm module; used in the inspector popover.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** a `timeline.keyframe` menu: Interpolation ▸ (Linear, Hold, Ease in, Ease out, Ease in-out, Bezier…), Copy, Delete keyframe, Go to keyframe.
- **Interfaces:** `<KeyframeLane>`, `<CurveEditor value onChange presets>`.
- **Acceptance criteria:**
  - Playwright: add 3 keyframes to opacity, drag one, change its interpolation to ease-in-out, undo twice, the doc is restored (via `timeline.get` assertion).
  - 50k keyframes across the 5,000-item fixture still pan/zoom ≥ 55 fps (2J.02).
  - Curve editor is keyboard-operable (arrow keys nudge handles) and passes axe.
- **Tests:** Vitest, Playwright, visual snapshots.
- **Out of scope:** multi-property graph editor (2C.07).

### WP-2C.06 — Shared wasm module: keyframe, easing, time and snapping math
- **Area:** crates/cutweave-wasm/ (new thin crate, wasm-bindgen), packages/protocol/wasm/, CI
- **Depends on:** WP-2C.02, E1N
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - A thin crate `cutweave-wasm` re-exporting from the wasm-safe crates: flicks/frame/timecode conversions (`frame_to_time`, `time_to_frame`, `snap_to_frame`, timecode parse/format), `Animatable` evaluation and easing (including bezier/spring sampling and `eval_range`), and the snapping API (2A.07 adds its exports). No new logic in this crate.
  - Build with `wasm-pack`/`wasm-bindgen` (target `web`, `--release`, `wasm-opt`), size budget ≤ 150 KB gzipped (CI check). Published inside `packages/protocol` as `@cutweave/protocol/wasm` with generated TS typings, an async `initMath()` loader (also usable in Vitest/Node) and a synchronous API after init. Float/time marshalling uses i64 as BigInt only at the boundary with typed wrappers, bulk evaluation takes `Float64Array` to avoid per-call overhead.
  - CI: build on every PR (wasm32 compile of the source crates is already gated), regenerate and diff the package typings (snapshot gate like codegen), and run the golden-vector parity test: the same vectors (`docs/spec/keyframes-vectors.json`, timecode vectors) executed natively in Rust and in Node via wasm must match bit-for-bit for time math and within 1e-9 for float eval.
  - Versioning: the module version is tied to the protocol package version. The UI refuses to start with a protocol/wasm mismatch.
- **Interfaces:** `@cutweave/protocol/wasm`: `initMath`, `time.*`, `anim.eval`, `anim.evalRange`, `easing.sample`, plus snap exports from 2A.07.
- **Acceptance criteria:**
  - `pnpm -F @cutweave/protocol test` runs the parity vectors: 0 mismatches.
  - Gzipped wasm ≤ 150 KB; init < 30 ms; `evalRange` of 1,000 samples < 0.3 ms in Chromium (vitest bench).
  - Changing a Rust signature without updating typings fails the snapshot gate.
  - No RPC is issued by any timeline/inspector code for curve drawing (lint rule banning `keyframe.eval` use in `packages/*`).
- **Tests:** parity vectors, bench, snapshot gate.
- **Out of scope:** rendering/GPU code in wasm, browser-lite engine.

### WP-2C.07 — Bezier graph editor panel
- **Area:** packages/ui/src/graph-editor/, packages/editor/src/panels/graph
- **Depends on:** WP-2C.05, WP-2C.06, WP-2H.01
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - A dockable Graph Editor panel showing value-over-time curves for the selected item's animated properties (multi-curve, per-component for vec2), with pan/zoom on both axes, fit-to-selection, value and time rulers, playhead and snapping (via wasm).
  - Keyframe point editing: drag in time and value (axis constrain with Shift), bezier tangent handles (free / aligned / mirrored), interpolation switch (hold / linear / bezier / spring), easing presets, copy/paste of keyframes, box select, numeric entry; normalized-vs-absolute value view toggle.
  - All edits are `keyframe.*` commands under `gesture` txs; the curve preview evaluates locally via the wasm module.
  - Soft-lock Conflict (2A.12) shows a non-blocking message.
- **Interfaces:** `<GraphEditor item props onCommand>`; panel id `graph-editor` in the panel registry.
- **Acceptance criteria:**
  - Playwright: on a position animation (3 keyframes) drag a tangent handle: the stored bezier values change as expected (±1e-3) and the preview curve matches `keyframe.eval` samples from the engine within 1e-6; undo restores the original.
  - 10 curves × 500 keyframes pan/zoom at ≥ 55 fps (probe).
  - Keyboard operable (arrows nudge, Delete removes); axe 0 violations.
- **Tests:** Vitest for geometry, Playwright, visual snapshots.
- **Out of scope:** expression-driven curves, motion paths in the preview (P4).

### WP-2C.08 — Layouts for several clips: side by side, top and bottom, picture in picture, grids
- **Area:** crates/cutweave-ops/src/item/ (`item.apply_layout`, the layout table as data), generated schema and TS types, packages/editor (a Layout menu in the Inspector, the timeline's clip menu and the bin), apps/desktop/e2e/flows, docs/user-flows.md
- **Depends on:** WP-2C.03, WP-2C.04, WP-2H.03a, WP-2A.10, WP-2B.01
- **Size:** M
- **Executor:** sonnet
- **Scope:** (product owner request, 2026-11-24: "if I want to place 2 videos side by side on the timeline, will it be possible?") Today this is possible by hand. A second clip dropped at the same time goes to a new track, then Scale 50 % and Position X ±480 in the Inspector. The clips are letterboxed, though, because P1 has no crop. Crop arrives with WP-2C.03, WP-2C.04 and WP-2H.03, and transform handles and snapping with WP-2H.06. This WP makes the arrangement one action.
  - **`item.apply_layout{items[], layout, fill: crop|fit, gap?, order?}`** sets each item's transform and crop as ordinary `VisualProps` values in one undoable step. MCP catalog: agents arrange clips with the same command the UI uses, and the result is plain properties that anyone can adjust afterwards.
    - `crop` fills each cell with a centred crop; `fit` scales to fit inside the cell.
    - `gap` is a spacing in sequence pixels.
    - `order` lists the items for the cells; without it, the top track comes first.
    - A property that is already animated gets a keyframe at the playhead (2C.03's auto-key rule).
    - The cells come from the sequence's canvas size (WP-2B.01).
  - **Layouts** (data, so packs can add more later):
    - two clips: side by side (left and right), top and bottom;
    - picture in picture: a small clip in any corner, 30 % of the width, inset by the title-safe margin;
    - three clips: three columns, one large and two small;
    - four clips: a 2 × 2 grid.
    - Too few or too many visual items give `Invalid`, with the counts the layout accepts.
  - **UI:**
    - With 2–4 video clips selected, the Inspector shows a Layout row of small diagrams, and the timeline's clip menu has Layout ▸.
    - In the bin, selecting 2–4 video assets offers "Place side by side…", which inserts them at the playhead on new tracks and applies the layout in one undo step.
    - "Swap" reverses the order of a two-clip layout.
- **Interfaces:** `item.apply_layout`; the layout table (`layouts.json` in the ops crate's data, schema-validated).
- **Acceptance criteria:**
  - Unit: each layout puts every clip's visible rectangle exactly on its cell (within 0.5 sequence px) on 16:9, 9:16 and 1:1 canvases with 16:9 and 9:16 sources, for `crop` and `fit`; undo restores every property; an animated scale gets a keyframe, not a static value.
  - Parity: `item.apply_layout` through the CLI, MCP and RPC gives byte-identical documents (`--deterministic-ids`).
  - Golden frame: two colour-bar clips side by side with `crop` render each half filled, with no letterbox.
  - **A user flow (ADR-0018, F36):** select two clips in the bin, "Place side by side…": both are on the timeline at the playhead on two tracks, and the preview frame shows the two pictures filling the left and right halves; Swap reverses them; Ctrl+Z undoes the layout, then the insert; select both clips, choose Picture in picture in the Inspector: the second clip sits small in a corner.
  - axe: 0 violations on the Layout row; visual baselines of the row in dark and light.
- **Tests:** unit, parity, golden, Vitest, user flow.
- **Out of scope:** animated transitions between layouts; split-screen as an effect on one clip (Split Screen Grid, WP-4C.06); templates with slots such as the picture-in-picture reaction template (WP-6D.11); borders and rounded corners on cells (P4 effects).

---

# Epic 2D — Speed basics

### WP-2D.01 — Speed model, reverse, freeze frame, source time mapping
- **Area:** crates/cutweave-model, crates/cutweave-ops/src/clip/speed.rs, crates/cutweave-render (time map)
- **Depends on:** E1A, E1F, WP-2A.04
- **Size:** M
- **Executor:** opus
- **Scope:**
  - (product owner, 2026-11-25, ADR-0024) `clip.set_speed` takes `keyframes: "keep" | "scale"`, default `keep`: keys stay at their times unless the caller asks to scale them with the speed (`scale_times`, [keyframes spec](../spec/keyframes.md) §8.1); the editor offers "Scale keyframes" in the speed control.
  - Use `Speed::Constant(r)`, r in [0.1, 100], rational stored as `{num,den}` (not f64) to keep frame math exact; `reverse: bool`; `freeze: Option<{at_source}>`.
  - Commands: `clip.set_speed{item, speed, keep: duration|in_point, ripple}`, `clip.reverse`, `clip.freeze_frame{item, at}` (splits the item and inserts a still-duration clip of the freeze frame per CapCut behaviour, default 3 s duration: configurable setting `freeze.default_duration`), `clip.reset_speed`.
  - Source-time mapping function `source_time(item, t)` is the single canonical mapping used by render, audio, thumbnails and export. Frame-nearest sampling in v1 (no blending; blending arrives P4). Reverse requires decoder support for backwards GOP traversal: use GOP-cache decode-forward then reorder; for sources without proxy and long GOP a warning plus automatic proxy request (2F.01).
  - Speed change on a magnetic track ripples; on a free track it keeps start and may be blocked by a neighbour (`Conflict`, with option `ripple`).
  - MCP core: none (catalog `clip_set_speed`, `clip_freeze_frame`); note in the MCP map (2I.01).
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** Speed ▸ (Normal, 0.5x, 2x, Custom…), Reverse and Freeze frame are items of `timeline.clip` (left out for audio-only, image and text clips as the kind rules say).
- **Interfaces:** `Speed`, `source_time`, commands above.
- **Acceptance criteria:**
  - Unit test: 2× over a 10 s clip gives duration 5 s exactly in flicks; 1/3× is exact (rational), no drift over 1,000 round trips (`set_speed(a)` then `set_speed(1/a)` restores duration±0).
  - Frame-exact: preview/export at speed 0.5 shows source frame `floor(t*0.5*fps)` verified on the corpus clip with burned-in frame counter (golden OCR-free check via pixel-coded frame numbers).
  - Reverse playback of a 1080p H.264 long-GOP clip scrubs at p95 < 150 ms (§19) with proxy, and works (slower, documented) without.
  - Freeze frame creates exactly two splits + one still and keeps the track gapless on magnetic.
  - Undo identity and magnetic-gapless property tests extended with speed commands.
- **Tests:** unit, proptest, golden frames, e2e via CLI.
- **Out of scope:** speed curves, optical-flow blending (P4), audio (2D.02).

### WP-2D.02 — Audio for speed and reverse (pitch-preserving)
- **Area:** crates/cutweave-audio (stretch node), crates/cutweave-ops
- **Depends on:** E1G, WP-2D.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Time-stretch node using `signalsmith-stretch` for 0.1×–100× (above 4× or below 0.25× quality degrades; clamp the audio-preserve path to 0.25×–4×, and outside the range switch to resample (pitch-changing) with a flag in the report).
  - `Clip.audio.preserve_pitch: bool` (default true). Reverse audio via block reversal with crossfaded joins (5 ms).
  - Stretch runs on worker threads ahead of the real-time thread into ring buffers (§9) and survives seeks with < 50 ms re-prime. Mixer uses the same `source_time` as video.
  - Offline render path (export) uses the same node deterministically (fixed block size).
- **Interfaces:** `StretchNode`, `clip.set_speed` flag `preserve_pitch`.
- **Acceptance criteria:**
  - Test tone 440 Hz at 2× with preserve-pitch: measured fundamental 440 Hz ± 2 Hz (FFT test); without: 880 Hz ± 4.
  - A/V offset at 0.5× and 2× after 60 s: < 40 ms and drift < 1 frame / 30 min equivalent (the 2J.03 sync probe; this WP asserts offset < 20 ms in an offline test).
  - 0 underruns at 256-sample buffer with 8 tracks all at 1.5× (audio soak 120 s).
  - Reverse of a 1 s sine yields the same energy (RMS ± 0.1 dB) and no clicks (max sample discontinuity < 0.05 at joins).
- **Tests:** FFT unit tests, offline render comparison, soak.
- **Out of scope:** pitch shift/voice effects (P5).

---

# Epic 2E — Audio features

### WP-2E.01 — Fades, keyframed volume, pan
- **Area:** crates/cutweave-audio, crates/cutweave-ops/src/audio/
- **Depends on:** E1G, WP-2C.02, WP-2C.03
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Per-clip `fade_in`, `fade_out` (duration + curve `linear|log|equal_power|s_curve`), keyframed `volume` (dB, −∞..+12) and `pan` (−1..1, equal-power law) evaluated sample-accurately (per-block linear ramps, block ≤ 64 samples to avoid zipper noise).
  - Commands: `audio.set_fade{item, in?, out?, curve?}`, `audio.set_volume{item, db, auto_key?}` / `audio.set_pan` (thin wrappers over `item.set_props`; same data), `audio.set_gain`. Fades clamp so that `fade_in + fade_out ≤ duration`.
  - Fades are applied post-trim so that trimming does not change the fade length unless it would exceed the duration (spec'd).
  - MCP core: `audio_set` (one tool covering fade/volume/pan via discriminated union).
- **Interfaces:** commands above; clip `audio{gain,pan,fade_in,fade_out}`.
- **Acceptance criteria:**
  - Offline render test: a 1 kHz tone with 1 s linear fade-in has amplitude at 0.5 s of 0.5 ± 0.01; equal-power pan at center gives −3 dB per channel ± 0.1 dB.
  - No clicks: for a keyframed volume jump between adjacent keyframes (hold), max sample discontinuity after smoothing < 0.02.
  - Undo identity and parity tests pass; export mix equals preview mix (null test: difference < −90 dBFS).
- **Tests:** unit with synthesized signals, offline null test.
- **Out of scope:** ducking, EQ, compressor (P3/P5).

### WP-2E.02 — Track buses, master limiter, meters
- **Area:** crates/cutweave-audio/src/{bus.rs, limiter.rs, meter.rs}
- **Depends on:** WP-2E.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Per-audio-track bus: gain, pan, mute, solo (solo is exclusive-solo-in-place: any soloed track mutes non-soloed audio tracks; video not affected), meters (peak and RMS) published at 30 Hz via `playback.meters` events. Track gain keyframable.
  - Master bus: gain plus true-peak lookahead limiter (default ceiling −1 dBTP, 5 ms lookahead, 100 ms release, configurable; on by default for export, toggle `export.limiter`), integrated LUFS-M/S/I meter via `ebur128`.
  - The real-time thread remains lock-free and allocation-free (asserted with an `assert_no_alloc` test build).
  - Commands: `track.set_gain`, `audio.set_master{gain, limiter{enabled,ceiling}}`, `audio.get_levels` query (for agents; returns peak/RMS/LUFS over a range by offline analysis).
- **Interfaces:** `BusGraph`, events `playback.meters`, queries `audio.get_levels`.
- **Acceptance criteria:**
  - Limiter: input with a +6 dBFS sine peak produces true-peak ≤ ceiling + 0.1 dB (measured with ebur128 true peak) over a 10 s test.
  - Zero underruns over 120 s at a 256-sample buffer with 8 tracks and active limiter (soak test); `assert_no_alloc` passes.
  - Solo logic unit tests (multiple solos, solo+mute interplay).
  - `audio.get_levels` for a −20 dBFS test tone returns −20 ± 0.2 dB RMS; −23 LUFS pink-noise reference returns −23 ± 0.3 LUFS.
- **Tests:** DSP unit tests, soak, no-alloc.
- **Out of scope:** sidechain ducking, EQ, reverb.

### WP-2E.03 — Loudness normalization and extract audio
- **Area:** crates/cutweave-ops/src/audio/{normalize.rs, extract.rs}, crates/cutweave-jobs
- **Depends on:** WP-2E.02, E1D, WP-2A.05
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - `audio.analyze_loudness{items|range}` job (ebur128 over decoded audio, result cached in `MediaAsset.analysis.loudness` by content hash).
  - `audio.normalize{items|master, target: -14|-16|-23|custom LUFS, mode: gain_per_clip | master_gain}` sets clip gain (non-destructive) so measured integrated loudness equals target ± 0.5 LU, never exceeding the true-peak ceiling (reduces gain if needed and reports `limited: true`). Presets are L0 data: `social -14`, `podcast -16`, `broadcast -23`, `streaming-music -14`.
  - Export option `export.normalize{target}` computes a two-pass gain over the mixed timeline.
  - `audio.extract{item, link: keep|unlink}` creates an audio item on a new/audio track from a video clip's audio, and optionally mutes the original video's audio (CapCut "extract audio"). `media.extract_audio{media, format}` writes a WAV/AAC file into `generated/` and imports it.
  - MCP: `audio_normalize` catalog (core `audio_set` per 2E.01 stays); `audio.extract` catalog.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** Extract audio and Normalize audio… are items of `timeline.clip` (`when item.hasAudio`).
- **Interfaces:** commands above; job kind `analysis.loudness`.
- **Acceptance criteria:**
  - A −30 LUFS fixture normalizes to −14 ± 0.5 LU measured on the exported file via ffmpeg `ebur128`; the true-peak stays ≤ −1 dBTP.
  - Analysis is cached: re-running returns instantly (< 50 ms) with identical values.
  - Extract: original clip audio muted/unlinked per option; the new audio item aligns to the video start within 0 flicks.
  - Undo identity.
- **Tests:** unit with synthetic LUFS fixtures, e2e through CLI + ffmpeg verification.
- **Out of scope:** auto-ducking, AI voice enhance (P5).

### WP-2E.04 — Voiceover recording
- **Area:** crates/cutweave-audio/src/input.rs, crates/cutweave-ops/src/audio/record.rs, packages/editor (record UI)
- **Depends on:** E1G, E1H, WP-2E.02, E1C
- **Size:** M
- **Executor:** opus
- **Scope:**
  - cpal input stream (device selection, sample rate negotiation, mono/stereo) writing a lossless WAV (32-bit float or 24-bit) into `generated/recordings/` with crash-safe incremental header finalization (write size on stop and every 5 s so a crash leaves a recoverable file).
  - Commands: `audio.record_start{device?, track?, count_in?}`, `audio.record_stop → {media, item}`, `audio.list_devices` query. Records while the timeline plays (monitor timeline mix to output; latency compensation: measured round-trip offset setting, default from device reported latency) and on stop inserts the clip at the record start position on a (new) audio track, overwriting per mode.
  - Level meter during recording; mic permission handling (macOS `NSMicrophoneUsageDescription`, Windows privacy toggle detection → typed error `PermissionDenied`).
  - UI: record button in timeline toolbar, countdown 3-2-1, live level meter. The input device chooser, count-in, monitoring and latency rows are on Settings > Audio (WP-2E.08).
  - MCP: catalog (agents rarely record); marked `destructive=false`.
- **Interfaces:** commands above; event `audio.recording{state, level}`.
- **Acceptance criteria:**
  - Loopback test (virtual cable or synthetic input device in CI via cpal mock): recorded clip aligns to the played click track within ±10 ms after latency compensation; 5-min recording continues with 0 dropped samples (sample count == duration × rate ± 1).
  - Kill the engine mid-recording: the WAV is playable up to the last 5 s checkpoint, and recovery offers to import it (manual check recorded in the PR + automated header-recovery unit test).
  - Permission denied path returns `PermissionDenied` and the UI shows guidance.
- **Tests:** mock-device integration, header-recovery unit, manual platform checklist on 3 OSes.
- **Out of scope:** noise reduction, multi-take comping.

### WP-2E.05 — Audio UI: waveforms, fade and volume handles, mixer panel
- **Area:** packages/timeline (audio drawing), packages/editor/src/mixer/
- **Depends on:** WP-2E.01, WP-2E.02, WP-2A.09, WP-2A.08
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Waveform rendering in audio items (peak pyramid, LOD by zoom, dB-scale option), volume line overlay with draggable keyframe points (rubber band) and fade handles at item corners (drag = `audio.set_fade` under `gesture` tx). Shows normalization/limited badges.
  - Mixer panel: per-track fader, pan knob, mute/solo, live peak meters from `playback.meters`, master fader plus limiter toggle and LUFS readout. Double-click resets.
  - Audio inspector section (volume, pan, fades, preserve pitch, normalize button) through the schema-driven inspector (2H.03b). Its "Fade in" and "Fade out" buttons add a fade of the length set in Settings > Editing ("Length of a fade added with one click", `editing.audio_fade`, 0.5 s; catalog `docs/reference/settings.md`).
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** menus for the fade handle (Linear, Logarithmic and other fade curves, Remove fade), the volume line (Reset to 0 dB, Add point here, Delete point) and the mixer fader (Reset, Mute, Solo, Bus ▸).
- **Interfaces:** consumes 2E commands; `<Mixer/>` panel registered in the dock registry (2H.01).
- **Acceptance criteria:**
  - Playwright: drag a fade handle to 1.0 s, the clip's `fade_in` equals 1.0 s ± 1 frame; drag the volume line creates two keyframes; undo reverts.
  - Meters update at ≥ 25 Hz without exceeding 3% CPU in the renderer (perf probe).
  - Waveform draws for a 60-min clip at the 1-hour zoom within the timeline frame budget (≥ 55 fps pan).
- **Tests:** Playwright, Vitest.
- **Out of scope:** EQ UI, spectral view.

### WP-2E.06 — Auto-ducking (sidechain from voice tracks)
- **Area:** crates/cutweave-audio/src/duck.rs, crates/cutweave-ops/src/audio/duck.rs, packages/editor (mixer controls)
- **Depends on:** WP-2E.02, WP-2E.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Sidechain ducker on a track bus: `audio.set_ducking{track, enabled, sources: [track_ids], threshold_db (-35), reduction_db (-12), attack_ms (50), release_ms (400), hold_ms (100), lookahead_ms (10)}`. The detector is a smoothed RMS/peak follower on the summed source tracks (post-fade, post-gain). Gain reduction is applied as a smooth dB ramp on the target bus (music/B-roll audio).
  - Deterministic in preview and export (fixed block size, lookahead via delay compensation; no wall-clock). Runs on the audio thread with no allocation. The ducking curve is also computable offline for the timeline: `audio.get_duck_curve{track, from, to}` returns breakpoints so the UI can draw the gain-reduction shading on the music track.
  - "Duck under voice" one-click: `audio.auto_duck{track}` picks sources as all tracks flagged `role=voice` (new `Track.role: voice|music|sfx|none`, settable via `track.set_role`, schema bump if not yet in 2A.02) and applies defaults.
  - Manual overrides: ducking can be disabled per clip region by volume keyframes; ducking applies on top.
  - UI: ducking section on the track in the mixer (enable, sources, amount, attack/release) with live gain-reduction meter.
  - MCP: `audio_set` gains a `ducking` variant (core), `audio.auto_duck` catalog.
- **Interfaces:** commands above; event field `playback.meters.gr_db`.
- **Acceptance criteria:**
  - Offline test: 1 kHz music at −20 dBFS under a voice burst at −18 dBFS from t=2 s to 4 s with defaults: music reduced by 12 ± 0.5 dB within 150 ms of voice onset (attack), and returns to within 0.5 dB of the original ≤ 800 ms after voice end.
  - No gain change when the sources are silent (bit-exact passthrough, null test < −120 dBFS).
  - Export mix equals preview mix (null test < −90 dBFS); 0 underruns over 120 s at 256-sample buffer with ducking active on 2 tracks; `assert_no_alloc` passes.
  - `audio.get_duck_curve` agrees with the rendered gain within 0.2 dB.
- **Tests:** DSP unit tests, null tests, soak.
- **Out of scope:** transcript/VAD-driven ducking (P3/P5), multiband ducking.

### WP-2E.07 — Basic voice effects (offline DSP)
- **Area:** crates/cutweave-audio/src/voicefx/, crates/cutweave-ops/src/audio/voicefx.rs, crates/cutweave-jobs
- **Depends on:** WP-2D.02, WP-2E.01, E1G
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Effects: `pitch` (±12 semitones, formant-preserving option, `signalsmith-stretch` pitch shift), `chipmunk` (+7 st preset) and `deep` (−6 st preset), `robot` (ring modulation at 50–150 Hz plus short comb/flanger, intensity param), `echo` (delay 50–1000 ms, feedback, mix, ≤ 8 s tail), `telephone` (band-pass 300–3400 Hz, mild soft clip, optional noise). Parameters are keyframe-free in P2.
  - "Offline DSP" means deterministic non-real-time processing: each effect clip is rendered by a job to a cached WAV (keyed by media hash, source range, effect id and params) and the mixer plays the cache. While the cache is pending the original plays and the clip shows a "processing" badge. Export always uses the same cached render (or renders inline if missing), so preview equals export.
  - Commands: `audio.set_voice_effect{item, effect: none|pitch|chipmunk|deep|robot|echo|telephone, params}`, `audio.list_voice_effects` query (with param schemas for the schema-driven inspector, `x-ui` hints). Tail handling for echo: clip duration unchanged; the tail is clipped by the fade-out or extended through an optional `tail: true` that adds an invisible extension limited by neighbours.
  - MCP: catalog.
- **Interfaces:** commands above; job kind `audio.voicefx`; schema for each effect.
- **Acceptance criteria:**
  - Pitch +12 st on a 440 Hz tone gives 880 ± 4 Hz; −12 gives 220 ± 2 Hz (FFT); duration unchanged ± 1 sample block.
  - Telephone: energy below 200 Hz and above 4 kHz attenuated by ≥ 24 dB relative to the passband (spectrum test). Echo of an impulse shows repeats at the configured delay ± 1 sample with the configured feedback ratio ± 0.5 dB.
  - A 60 s clip renders in < 10 s on the reference desktop; re-applying identical params hits the cache (< 100 ms); changing params invalidates only that clip.
  - Export of a project with voice effects equals the preview mix (null test < −90 dBFS). Undo identity.
- **Tests:** FFT/spectrum unit tests, cache test, null test.
- **Out of scope:** AI voice changer/voice cloning (P5), real-time effect preview while dragging params (cached render only), formant EQ.

### WP-2E.08 — Settings: Audio page (output and input devices, buffer, recording, loudness target)
- **Area:** crates/cutweave-audio (output device selection and switching, buffer size), crates/cutweave-engine (UI-only engine command), generated schema and TS types, packages/editor (Audio page), docs/reference/settings.md, apps/desktop/e2e/flows, docs/user-flows.md
- **Depends on:** WP-1M.11, WP-2E.03, WP-2E.04, E1G
- **Size:** M
- **Executor:** opus
- **Scope:** (product owner request, 2026-11-24) the Audio page of the settings catalog.
  - **This computer:** output device (system default, which follows the OS default); output buffer (Auto, 256, 512, 1024 samples); input device for voiceover (system default; WP-2E.04's device chooser moves here); recording latency compensation (as the device reports, adjustable in ms).
  - **Preferences:** count-in before recording (3 s; off, 1, 3, 5 s); hear the microphone while recording (off); loudness target for Normalize (−14 LUFS; −14, −16, −23 or a value; WP-2E.03's command takes it as its default); mute playback when Cutweave is in the background (off).
  - **Engine:** `audio.list_devices` (WP-2E.04) lists outputs too; `audio.set_devices{output?, input?, buffer?, latency_ms?}` is an engine command, UI-only, not undoable, not journaled, applied after each handshake. Switching the output during playback reopens the stream without stopping the transport (one gap of at most 200 ms); an unplugged device falls back to the system default with a toast ("Audio moved to <device>").
- **Interfaces:** `audio.set_devices` (UI-only); `audio.list_devices` with outputs; preference keys `audio.*`.
- **Acceptance criteria:**
  - Integration with a mock or null audio host: switching the output during playback keeps the audio clock monotonic and the A/V offset under 40 ms after the switch; removing the current device falls back to the default and emits the event the toast uses.
  - An agent token calling `audio.set_devices` gets `forbidden`; `audio.list_devices` answers agents.
  - **A user flow (ADR-0018, F27):** Settings > Audio lists the output devices (the machine's, or the null device in CI); choose another and back; set the loudness target to −16 LUFS and Normalize a clip: the document records −16 LUFS as its target; the choices survive a relaunch.
- **Tests:** audio integration with mock devices, unit, user flow.
- **Out of scope:** ASIO and exclusive modes, multichannel routing, per-track output devices, audio plugin hosting (post-1.0).

---

# Epic 2F — Media management

### WP-2F.01 — Proxy generation pipeline and policy
- **Area:** crates/cutweave-cache/src/proxy/, crates/cutweave-jobs, crates/cutweave-ops/src/media/proxy.rs
- **Depends on:** E1D, E1B, E1C
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Proxy job: transcode to an intra-frame codec at ≤ 960 px on the long edge (ProRes Proxy via `prores_ks`/videotoolbox where present, DNxHR LB fallback, MJPEG last), audio copied to PCM. Keeps frame count and timestamps identical to the source (VFR sources are conformed through the media index to CFR with a stored `pts_map` so that proxy frame *n* equals source frame *n*, essential for frame-exact edits).
  - Content-addressed in the global cache (`cache.sqlite` entry keyed by `content_hash + proxy_profile`), size-capped LRU, resumable job (partial files finalized atomically), progress events, cancel, concurrency limit (default 2; ≤ 1 while exporting), CPU/GPU niceness.
  - Policy in `project.settings.proxy_policy`: `off | auto | all`. `auto` triggers on import for long-GOP codecs (H.264/HEVC) with long edge > 1920 px or fps > 60 or HEVC 10-bit (documented). Per-media `media.set_proxy_mode{auto|force|off}`. (2026-11-25: WP-2F.01 shipped with `auto` for every project, arch §7. [ADR-0027](../adr/0027-proxy-policy-in-the-project.md) was accepted on 2026-11-26; the two project-format fields and their commands are a follow-up.)
  - Commands: `media.generate_proxy{media[]}`, `media.delete_proxy`, `media.set_proxy_policy`, `media.proxy_status` query. Jobs resumed after engine restart.
  - MCP: catalog; `media_import` reports `proxy: queued|none`.
- **Interfaces:** `ProxyRef {path, profile, status}`; `job.progress` events; queries above.
- **Acceptance criteria:**
  - For the corpus 4K HEVC 10-min clip, proxy generation ≥ 1× realtime on the reference desktop; resumes after kill -9 and completes (test).
  - Frame mapping: for 500 random frame indices on VFR phone footage, the proxy decodes the same frame index as the source (verified by embedded frame-number burn-in clip): 100% match.
  - Cache cap respected: with a cap of 1 GB and 3 × 600 MB proxies, the least recently used one is evicted and its `ProxyRef` reports `evicted` (reverts to original).
  - Scrub p95 on proxy < 80 ms, on 4K long-GOP original with proxy off < 150 ms (§19; measured in 2J.03).
- **Tests:** integration with corpus, kill/resume, cache eviction unit.
- **Out of scope:** UI beyond a status query (2F.03), preview switching logic (2F.02).

### WP-2F.02 — Proxy-aware preview/export source selection
- **Area:** crates/cutweave-playback, crates/cutweave-render, crates/cutweave-media
- **Depends on:** WP-2F.01, E1H, E1E
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - `MediaResolver::open(media, purpose)` where `purpose = Preview(quality) | Export | Thumbnail | Analysis`. Export and Analysis always use the original; Preview uses the proxy when present and ready and quality ≤ proxy resolution scaling rule (full-res preview that needs more than the proxy provides uses the original, per quality ladder §8).
  - Hot-swap when a proxy finishes while a file is open (next seek uses it; no visible glitch).
  - Setting `preview.use_proxies` (default on) toggle in the transport bar. The preview status chip shows "Proxy" or "Original".
  - Missing/evicted proxy gracefully falls back to the original. (2026-11-27, as built: the resolution rule compares the longest side a layer is drawn at with the proxy's; the toggle is the app preference `playback.proxies`, sent to the engine as the UI-only `view.set_preview_quality{use_proxies}`; `playback.state` gained `use_proxies` and `proxy` for the chip; media whose original is offline never shows its proxy; a clip that is playing keeps the proxy it reads until the next seek or pause, so the toggle and the quality ladder never cost a seek into the original in the middle of playback. Arch §7 "Selection". User flow F63.)
- **Interfaces:** `MediaResolver`; `view.set_preview_quality{use_proxies}`.
- **Acceptance criteria:**
  - Test asserts export never opens a proxy file (file-open spy in the integration test): 0 proxy opens across a 3-clip export.
  - Toggling proxies during playback causes no dropped frames > 2 within 5 s on the reference iGPU (probe).
  - Exported file of a project with proxies equals exported file without proxies (frame hash compare for 10 sample frames).
- **Tests:** integration, spy test.
- **Out of scope:** UI chips beyond status text (2F.03 adds the settings UI).

### WP-2F.03 — Relink missing media (hash-based) and media bin UI for health
- **Area:** crates/cutweave-ops/src/media/relink.rs, packages/editor/src/media/
- **Depends on:** E1D, E1C, E1M, WP-2F.01, WP-1D.08
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Detect missing media on open and on file-watch events (status `ok|missing|changed`). Offline media renders as a "media offline" slate (render pipeline fallback frame plus silent audio) and never crashes export (export refuses with a typed error listing missing assets unless `allow_offline`).
  - `media.relink{media, path}` (single), `media.relink_folder{folder, recursive}` (search the folder, match by content hash first, then by filename+size+duration), `media.find_missing` query. Hash is computed lazily with a fast partial hash (first/last/middle 1 MB + size) and full hash only on ambiguity. Path rewrite preserves `path_rel` when under the project folder.
  - UI: a relink dialog on open ("N files missing"), per-asset badges in the bin, proxy status badges, and "Regenerate proxies" in the bin's context menu.
  - Proxy settings where the settings catalog (`docs/reference/settings.md`) puts them: Settings > Media and import (proxies for new projects: off / auto / all; proxy size 540p / 720p / 960p, a machine value), Settings > Cache and storage (proxy limit, 50 GiB), Settings > Performance and hardware (proxy jobs at once, 2). The machine values are applied through UI-only engine commands as in WP-1D.08; the project's own policy is edited in File > Project settings… (WP-2H.08).
  - MCP: `media_relink` catalog. `media_list` core includes `status`.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** the bin card menu (`bin.card`) gets Relink… (when `media.offline`), Regenerate proxies and Reveal in Explorer; the existing mention of "Regenerate proxies in the bin's context menu" is satisfied through WP-2H.12's host.
- **Interfaces:** commands above; `MediaAsset.status`.
- **Acceptance criteria:**
  - Move the media folder: opening the project shows missing; `relink_folder` restores 100% of 20 test assets including renamed files (hash match) with one command; undo restores previous paths.
  - Export with missing media fails with `Invalid{path:"media.<id>"}` and a human message; with `allow_offline` it exports a slate.
  - Playwright: dialog flow end-to-end.
- **Tests:** integration, Playwright.
- **Out of scope:** network volume handling, collect & archive (2F.04).

### WP-2F.04 — Collect and archive
- **Area:** crates/cutweave-ops/src/project/collect.rs, crates/cutweave-jobs
- **Depends on:** E1C, WP-2F.03
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - `project.collect{dest, mode: copy_all|copy_external_only, include_generated, include_proxies, trim_unused: bool}` job. Copies referenced media into `<project>/media/` (or archive destination), rewrites `path_abs/path_rel` in the project doc copy, dedupes by content hash, preserves relative folder names with collision handling, verifies by hash after copy, reports progress, cancellable and resumable.
  - `project.archive{dest, format: zip|folder}` bundles `.cutweave`, media, `generated/`, fonts/LUTs, and a manifest `archive.json` (hash list). Zip64 for > 4 GB; no recompression for media (stored). Excludes caches by default.
  - Dry-run query `project.collect_estimate` returns size/free-space check before starting (fails early with `Invalid{msg: not enough space}`).
  - Copy on import: `media.import{copy_to_project?}` copies new files into `<project>/media/` with the same copier before registering them; the editor passes the preference "Copy imported files into the project folder" (`media.copy_on_import`, off; Settings > Media and import).
  - UI: "Collect files…" and "Archive project…" dialogs, with progress in the queue panel (reuses the job UI).
  - MCP: catalog; both are marked `destructive=false`, `idempotent=true`.
- **Interfaces:** `project.collect`, `project.archive`, `project.collect_estimate`.
- **Acceptance criteria:**
  - Collect then open the collected project on a "different machine" (test moves it and removes originals): 0 missing media; export of both versions yields identical frame hashes on 10 sample frames.
  - Archive zip extracts and opens; `archive.json` hashes all verify (script).
  - Interrupting at 50% then resuming completes without re-copying finished files (copy-count assertion).
  - Original project is untouched (file mtimes unchanged).
- **Tests:** integration with temp dirs, large-file test with sparse 5 GB file (nightly only).
- **Out of scope:** cloud storage, encryption.

### WP-2F.05 — Image and image-sequence import, still duration defaults
- **Area:** crates/cutweave-media (image backend), crates/cutweave-ops/src/media/, crates/cutweave-render
- **Depends on:** E1D, E1F, WP-2A.03
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Import PNG/JPEG/WebP/HEIC-if-ffmpeg/TIFF/BMP/GIF (GIF as video with its frame timing)/SVG (rasterized at import size, scale-aware re-rasterize deferred; flagged), with EXIF orientation, color profile (sRGB assumed if untagged, ICC → sRGB conversion via `lcms2`-class lib or documented skip), 16-bit PNG, alpha (premultiplied in render).
  - Image-sequence detection: pattern `name_%04d.ext` or folder selection with lexical/numeric order and gap detection (warn); frame rate prompt (default sequence fps); imported as a video-kind asset with `kind=image_sequence`.
  - Still duration defaults: `project.settings.still_duration` (default 5 s; stills inserted from the bin use it); settable; for sequences-from-images, `media.import_images_as_slideshow{files, per_image, order}` convenience command.
  - Settings (catalog `docs/reference/settings.md`): the default for new projects is the preference `editing.still_duration` (Settings > Editing; `project.new` copies it, WP-2H.08), and the frame rate for image sequences is `media.image_sequence_fps` (Settings > Media and import, default: the sequence's rate); the project's own value is a row of File > Project settings… (WP-2H.08).
  - Images are unbounded in length (no source bound; per 2A.04), cached as GPU textures with VRAM budget.
  - MCP: `media_import` handles images; `media_import_sequence` catalog.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** adds `media.import_frame{source, at}` (save a Preview or Source frame into the bin as a still; the Preview's "Add frame to media bin"), and "Duration…" for image cards.
- **Interfaces:** `MediaKind::{Image, ImageSequence}`; commands above.
- **Acceptance criteria:**
  - Corpus: rotated JPEG (EXIF 6) displays upright; 16-bit PNG with alpha composites correctly over color (golden); 4000×3000 image scrub does not exceed 80 ms p95 and holds 1080p60 (textures cached).
  - A 300-frame PNG sequence imports in < 3 s (probe only; no per-frame decode up front), plays at 30 fps, and each frame is exact (frame-number burn-in).
  - Dropping an image onto a timeline creates a clip of `still_duration`; changing the setting does not alter existing clips.
  - Importing a frame yields an image asset whose pixels match `render.frame` at that time (golden).
- **Tests:** corpus integration, golden frames.
- **Out of scope:** animated WebP/APNG, RAW stills, vector re-rasterization.

### WP-2F.06a — Media tags, ratings, notes and collections: model, commands, filter
- **Area:** crates/cutweave-model (fields), crates/cutweave-ops/src/media/{tags,collections}.rs, the shared `MediaFilter` (engine and editor predicates)
- **Depends on:** E1D, WP-2F.03, WP-1M.12, WP-2F.07, WP-2F.08
- **Size:** M
- **Executor:** sonnet
- **Scope:** (split from WP-2F.06 by [ADR-0036](../adr/0036-media-tags-ratings-notes-and-collections.md), accepted 2026-11-27; searching by name, kind, duration and use is WP-1M.12; folders are WP-2F.07 to WP-2F.09 under ADR-0020)
  - Model (ADR-0036): `MediaAsset.tags` (1–64 characters, no comma or control characters, at most 64 per asset, equal ignoring case and accents), `rating` (whole stars 1–5, absent for unrated), `note` (plain text, at most 2,000 characters); `Project.collections: Map<CollectionId, Collection {id, name, kind: manual{media[]} | smart{filter}}>`. Optional fields left out of the file while empty: **no schema bump and no migration**. A manual collection's member that names no asset is tolerated (queries skip it, edits drop it).
  - Commands in the `media` namespace (arch §5.1), each undoable as one step: `media.add_tags{media[], tags[]}`, `media.remove_tags`, `media.rename_tag{from, to}`, `media.set_rating`, `media.set_note`, `media.create_collection`, `media.rename_collection`, `media.delete_collection`, `media.add_to_collection`, `media.remove_from_collection`, `media.set_collection_filter`; queries `media.list_collections` and `media.list_tags`.
  - `MediaFilter` (WP-1M.12) gains `tags`, `min_rating` and `collection`; `text` also matches tags and notes. **No `imported_within_days`** (ADR-0036: no import time in the document, and no wall clock in queries). Smart collections are saved filters evaluated live (membership computed, not stored); a smart filter may not name a collection. `media.list` returns tags and rating, the note only with `include: ["note"]`.
  - Search stays a scan of the document shared by the engine and the editor (WP-1M.12), with no FTS5 index unless the 10,000-asset target below fails; transcript text joins through `transcript.search` in P3.
  - MCP: `media_list` (core) takes the extended filter; the rest catalog. Tags, notes and collection names are untrusted text in MCP results (ADR-0028).
- **Interfaces:** commands and queries above; `Collection` JSON Schema; extended `MediaFilter`.
- **Acceptance criteria:**
  - `media.list` with tags and text over 10,000 synthetic assets returns in < 30 ms p95; the Rust and TS predicates agree on the extended shared fixture.
  - Smart collection "video, duration > 60 s, tag:interview, unused" returns exactly the expected set on a fixture; membership updates within 100 ms after the asset is placed on a timeline.
  - Undo identity for every tag and collection command; the registry property tests pass with them.
  - A v3 document without the new fields reads the same as before; one with them keeps them through an older build's open and save (`extra`); a manual collection with a missing member opens, lists and edits without error.
- **Tests:** proptest, unit, fixtures, parity.
- **Out of scope:** the UI (WP-2F.06b, WP-2F.06c); AI auto-tagging and semantic search (P5); an FTS5 index unless the target fails.

### WP-2F.06b — Media tags, ratings and notes in the bin UI
- **Area:** packages/editor/src/media/
- **Depends on:** WP-2F.06a, WP-1M.12
- **Size:** S
- **Executor:** sonnet
- **Scope:** (split from WP-2F.06 by ADR-0036)
  - Tag editor (multi-select assign, from `media.list_tags`), rating stars and a note field in the media info; the Filters popover gains Tags and Rating.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** Add tag… and Set rating ▸ are items of `bin.card`.
- **Interfaces:** the WP-2F.06a commands and queries.
- **Acceptance criteria:**
  - Tagging, rating and a note on several selected cards are one undo step each; the Filters popover narrows the bin by tag and minimum rating.
  - **A user flow (ADR-0018):** tag and rate assets from the card menu and the media info, then filter by them.
- **Tests:** Vitest, user flow.
- **Out of scope:** collections and the bin sidebar (WP-2F.06c).

### WP-2F.06c — Collections and the bin sidebar
- **Area:** packages/editor/src/media/
- **Depends on:** WP-2F.06a, WP-2F.06b, WP-2F.07
- **Size:** S
- **Executor:** sonnet
- **Scope:** (split from WP-2F.06 by ADR-0036)
  - "Save as smart collection" from the current search and filters; Add to collection ▸; a **bin sidebar** (toggle, shown when the panel is wide enough) with the folder tree (WP-2F.07 folders, each a drop target like the breadcrumb) and the collections; a "used/unused" badge. Whether the sidebar is shown, its width and the open collection are workspace state, not document state (ADR-0036).
  - **Context menu:** Add to collection ▸ is an item of `bin.card`; Save as smart collection is an item of `bin.background` when a filter is active.
- **Interfaces:** the WP-2F.06a collection commands and queries.
- **Acceptance criteria:**
  - **A user flow (ADR-0018, F60; the sidebar's folder drop extends F22):** tag three assets, filter by tag, save as a smart collection, reopen the project and the collection is in the sidebar; drag a clip onto a folder in the sidebar's tree and it moves there.
- **Tests:** Vitest, user flow.
- **Out of scope:** collections of collections (ADR-0036).

### WP-2F.07 — Media folders: model, commands and migration (ADR-0020)
- **Area:** crates/cutweave-model (folders, asset display names, invariants, migration v1→v2 with fixtures), crates/cutweave-ops/src/media/ (folder commands, `media.list`), crates/cutweave-ops testing strategies, generated schema and TS types, docs/adr/0020-media-folders.md (status once accepted)
- **Depends on:** E1A, E1B, WP-1M.12, WP-1O.02
- **Size:** M
- **Executor:** opus
- **Scope:** (product owner request, 2026-11-24) organize the project's media into folders, as decided in ADR-0020 (Accepted by the product owner, 2026-11-25).
  - Model: `Project.folders: Map<FolderId, MediaFolder{id, name, parent?: FolderId, custom}>` with IDs `fld_…` (deterministic under `--deterministic-ids`), `MediaAsset.folder?: FolderId` (absent = top level) and `MediaAsset.name?: String` (display name; absent = the file name).
  - Invariants in `Project::validate`: parents and asset folders exist; no cycles; depth ≤ 32; folder names 1–255 characters, no `/` or control characters, unique among siblings ignoring case.
  - Commands in the `media` namespace, all undoable as one step, MCP catalog:
    - `media.create_folder{parent?, name?}` → `{folder_id, name}` (no name: "New folder", "New folder 2", …).
    - `media.rename_folder{folder, name}`; `media.rename{media, name?}` (no name: back to the file name).
    - `media.move{media?, folders?, to?, on_conflict?: fail|rename}` moves assets and folders (no `to` = top level). A folder into itself or a descendant is `Invalid`; a sibling name clash fails or renames ("Day 1 2").
    - `media.delete_folder{folder, contents: move_up|remove}`: `move_up` lifts everything one level; `remove` removes the folder, its subfolders and every asset no timeline uses, and moves the used ones to the deleted folder's parent; the result lists `removed` and `kept`. Timelines never change; files on disk are never touched.
    - `media.import` and `media.register` take an optional `folder`.
  - Queries: `media.list`'s filter gains `folder` and `recursive` (no folder: the whole project; a folder: one level unless `recursive`), and each summary carries `name` and `folder`; new `media.list_folders` returns the tree (id, name, parent, path, asset and subfolder counts). WP-1M.12's `text` matches the display name.
  - **Joins WP-2A.02's v2 (integrator resolution, 2026-11-25).** WP-2A.02 already bumped `CURRENT_SCHEMA_VERSION` to 2 and wrote `migrate/v1_to_v2.rs` as an ordered list of steps (`STEPS`); nothing is released between the two work packages, so this WP **appends a step** to that list (and its fixtures to `tests/fixtures/v2/`) and makes no v3 (arch §17, "One v2"). Its golden fixtures extend the existing v2 ones; the model types it adds go next to the timeline additions.
  - Migration v1→v2 adds `folders: {}`, `folder: null` and `name: null`; golden fixtures under `v2/` (nested folders, renamed assets); every v1 fixture migrates; v1 apps open v2 files read-only (the existing rule). Strategies for every new command in the registry property harness (`testing/strategies.rs`).
- **Interfaces:** `FolderId`, `MediaFolder`, `MediaAsset.{folder, name}`; the commands and queries above; `CURRENT_SCHEMA_VERSION` is already 2 (WP-2A.02).
- **Acceptance criteria:**
  - The registry property harness covers every new command: undo/redo identity, failure atomicity, and the invariants after random sequences of create, rename, move and delete (no cycle, no orphan, unique sibling names).
  - `media.delete_folder{remove}` on a fixture of 30 assets, 5 of them on a timeline, removes 25, keeps 5 in the parent, changes no timeline item; one undo restores the tree, the 25 assets and their folders exactly.
  - Migration: every v1 fixture migrates to its v2 golden; migrating and saving twice gives identical bytes.
  - Parity: a script that builds folders and moves media gives byte-identical documents through the CLI, MCP and RPC (WP-1O.02's harness, `--deterministic-ids`).
  - `media.list{filter:{folder, recursive:true}}` over 10,000 assets in 200 folders < 20 ms p95.
- **Tests:** unit, proptest through the registry harness, migration goldens, parity script, bench.
- **Out of scope:** the bin UI (WP-2F.08); importing OS folders (WP-2F.09); sequences in folders; a manual order inside folders; folder colors.

### WP-2F.08 — Media folders in the bin: browse, organize, drag and drop
- **Area:** packages/editor (media panel), packages/ui (breadcrumb; folder variant of `AssetTile`, with its spec in design/components/), apps/desktop/e2e/flows, docs/user-flows.md
- **Depends on:** WP-2F.07, WP-1M.12, E1M
- **Size:** M
- **Executor:** sonnet
- **Scope:** (product owner request, 2026-11-24) folders in the Media tab, built on WP-2F.07's commands.
  - **Browse:** the tab shows the open folder: its subfolders first as folder tiles (name, item count), then its assets. A breadcrumb above the grid ("Media › Interviews › Day 1"), each segment a button and a drop target. Double-click or Enter opens a folder; Backspace or Alt+↑ goes up. The open folder is kept per project with the panel's view state.
  - **Organize:** New folder (toolbar button, Ctrl/Cmd+Shift+N in the bin, context menu) creates it in the open folder and starts renaming; F2 or a slow click on the name renames a folder or an asset (WP-2F.07 display names); a context menu on tiles and on empty space: New folder, Rename, Move to… (a folder picker), Delete, Reveal in Explorer / Finder (assets).
  - **Multi-select:** Ctrl/Cmd-click, Shift-click ranges, Ctrl/Cmd+A in the bin; Delete on a selection removes the assets in one undo step (used ones refused with the engine's message, as today) and asks about folders.
  - **Drag and drop:** dragging selected tiles onto a folder tile, a breadcrumb segment or the top level moves them (`media.move{on_conflict:"rename"}`, one undo step, a toast when something was renamed). Dragging assets to the timeline works from inside folders as today. Import (button, Ctrl/Cmd+I) and OS files dropped on the bin import into the open folder; dropped on a folder tile, into that folder.
  - **Delete a folder:** a dialog that says what it holds ("Delete "B-roll" and its 30 media? 5 are used on the timeline and will move to "Interviews"."). Buttons: Delete folder and media (`remove`), Keep media (`move_up`), Cancel. An empty folder deletes without asking. Undo restores everything.
  - **Search and folders:** WP-1M.12's field gets a scope switch, All media (default) or This folder (with subfolders). In All media, results are flat with the folder path under each name, and folders whose names match come first.
  - **Accessibility:** folder tiles are options in the same listbox, announced as "folder, N items"; the breadcrumb is a `nav` with `aria-current`.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** its context menu on tiles and empty space is built on WP-2H.12's host: the items live in the `bin.card`, `bin.folder` and `bin.background` tables seeded by WP-2H.13, so F22 is extended with one folder operation done through the menu.
- **Interfaces:** consumes WP-2F.07's commands and queries; `<MediaBreadcrumb>`; the folder variant of `AssetTile`.
- **Acceptance criteria:**
  - **A user flow (ADR-0018, F22):** create "Interviews" and inside it "Day 1", rename it "Day one"; Ctrl-click three clips and drag them onto "Day one"; open it with a double-click and see them; go up with the breadcrumb; drag one clip back to the top level by dropping it on the breadcrumb; search "inter" in All media and see the path captions, then switch to This folder; delete "Interviews" with Keep media and see its contents lifted; Ctrl+Z brings the folder back; save, relaunch and reopen: the folders are there.
  - Vitest: the selection model (ranges across folders and assets), drop-target resolution, the delete dialog's wording for 0, 1 and N used media.
  - A folder of 5,000 assets scrolls at ≥ 55 fps (the existing windowing) and opens in < 50 ms.
  - axe: 0 violations; visual baselines for folder tiles, the breadcrumb and the delete dialog in dark and light.
- **Tests:** Vitest, visual, user flow.
- **Out of scope:** the folder tree sidebar (WP-2F.06c); importing OS folder structure (WP-2F.09); opening a folder in a second panel.

### WP-2F.09 — Import folders with their structure
- **Area:** crates/cutweave-ops/src/media/ (`media.import` of directories), crates/cutweave-engine if the walk lives there, generated schema and TS types, apps/desktop (folder picker IPC), packages/editor (Import folder…, folder drops, progress), docs/reference/settings.md, apps/desktop/e2e/flows, docs/user-flows.md
- **Depends on:** WP-2F.07, WP-2F.08, WP-1J.05
- **Size:** M
- **Executor:** sonnet
- **Scope:** (product owner request, 2026-11-24) importing a folder from the OS keeps its structure, as Premiere (since 23.6) and Resolve do.
  - `media.import{paths, folder?, keep_structure?}` accepts folders. The engine walks each one: no symlinked folders, hidden and system files skipped, depth ≤ 16, at most 2,000 files per call (more is `Invalid` with the count and a hint to import subfolders). It imports the files it can probe and skips the rest, counted by reason (unsupported, unreadable). With `keep_structure` (default true) it creates a folder for each directory that holds media, named after it, inside `folder`; without it everything lands in `folder`. One transaction: one undo removes the media and the folders. Files already in the project (same content hash) keep their asset and folder.
  - Progress: `media.import_progress{done, total}` events while probing; the call still returns `media_ids` when done (arch §7). Agents and headless engines are confined to the allowed roots as for files (WP-1J.05).
  - UI: "Import folder…" next to Import (a native folder picker through a new allowlisted IPC; several folders allowed); OS folders dropped on the bin or on a folder tile; a progress toast and a summary ("Imported 214 files into 12 folders; skipped 9 unsupported").
  - Settings > Media and import: "Keep folder structure when importing folders" (`media.keep_structure`, on).
- **Interfaces:** `media.import{paths, folder?, keep_structure?}` (directories accepted; `skipped` in the result); event `media.import_progress`; IPC for the folder picker; preference `media.keep_structure`.
- **Acceptance criteria:**
  - Integration: a fixture tree (3 levels, 40 media files, 5 unsupported files, a hidden file and a symlink loop) imports 40 assets into folders mirroring the directories that hold media; 5 are reported unsupported; the loop is not followed; one undo removes the 40 assets and their folders; importing the tree again creates nothing.
  - A tree of 2,001 files fails with `Invalid` and imports nothing; an agent path outside the allowed roots fails with `Invalid`.
  - A 500-file folder imports in < 30 s on the reference desktop (recorded, not gating).
  - **A user flow (ADR-0018, F22 extended):** Import folder… on the demo media folder (picker stubbed) creates its folders inside the open folder; an OS folder dropped onto a folder tile imports into it; the summary toast reports what was skipped; with the setting off, the files land flat.
- **Tests:** integration, Vitest, user flow.
- **Out of scope:** watching folders for new files; copying files into the project (WP-2F.04); tags from folder names (WP-2F.06a).

---

# Epic 2G — Export presets & queue

### WP-2G.01 — Export preset system (L0 data) and built-in presets
- **Area:** crates/cutweave-ops/src/export/presets.rs, plugins/presets-core/ (L0 pack), schema
- **Depends on:** E1E, E1B
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - Preset schema (JSON, versioned): `{id, name, container, video{codec, rate_control(crf|cbr|vbr), bitrate|crf, profile, preset, pix_fmt, hw_preferred}, audio{codec, bitrate, sample_rate, channels}, size{width,height|follow_sequence}, fps, loudness?, loop?, limits{max_duration, max_size}}`. Loaded from L0 packs; user presets saved in user config.
  - Built-in set (data): YouTube 1080p, YouTube 4K, YouTube Shorts (9:16, ≤ 60 s warning), Instagram Reels, TikTok, Instagram feed 4:5, GIF (palette, 15 fps, ≤ 480 px), audio-only (WAV, MP3, AAC, FLAC, Opus), ProRes 422 HQ (MOV), Archive (HEVC high quality), H.264 draft.
  - Commands: `export.list_presets`, `export.save_preset`, `export.delete_preset`; `export.start` accepts `preset` plus overrides.
  - Platform limits are warnings, not errors (duration, size, aspect mismatch).
  - MCP: `export_start` core takes preset id (list via `export.list_presets` in `cutweave_query`).
  - (product owner request, 2026-11-24: "Are we supporting export presets (for ex: instagram, youtube, and so on)")
    - **Platform set:** YouTube (1080p, 4K), YouTube Shorts, Instagram Reels, Instagram Stories, Instagram feed (4:5 and 1:1), TikTok, and X (landscape and portrait). Each preset gives the resolution and aspect, a frame-rate rule (the sequence's rate, capped at the platform's maximum), the video bitrate or CRF, the audio codec and bitrate, and the platform's duration and file-size limits.
    - **Dated values:** platform limits change. The Shorts "≤ 60 s" above is already out of date, so do not copy it. Every platform preset carries `platform{name, spec_url, checked: <date>}`, filled at implementation from the platform's published upload specs. Values that cannot be verified are marked `unverified`, as N7 does for shortcuts.
    - **Safe areas:** each portrait preset names its UI-safe zone from WP-2H.06's data pack (`overlays/safe-areas.json`, one shared ID per platform, Stories included). The preview can then show the safe area of the preset chosen in the export dialog or the sequence settings.
    - **Platform presets are H.264 + AAC MP4**, so they need nothing from WP-2G.02 and can ship before it.
- **Interfaces:** `ExportPreset` JSON Schema (with `platform` and `safe_area`); commands above.
- **Acceptance criteria:**
  - All presets validate against the schema in CI (`cargo test -p cutweave-ops presets`); a snapshot lists their ids.
  - Every platform preset has a `spec_url` and a `checked` date, and its `safe_area` (when present) exists in the safe-area data (CI check).
  - Resolving the "TikTok" preset on a 16:9 sequence returns a warning code `AspectMismatch` and does not change the sequence.
  - Presets referencing unavailable codecs (e.g., no HW encoder) fall back documented per the preset `hw_preferred` flag.
- **Tests:** schema validation, snapshot.
- **Out of scope:** encoding implementation (2G.02), UI.

### WP-2G.02 — Codecs: HEVC, AV1, ProRes, GIF, audio-only, sequences
- **Area:** crates/cutweave-media (encode), crates/cutweave-ops/src/export/
- **Depends on:** E1E, WP-2G.01
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Add encoders behind `MediaBackend`: HEVC (HW: NVENC/AMF/QSV/VideoToolbox/VAAPI where present, SW x265), AV1 (SW SVT-AV1, HW where present, NVENC AV1/QSV), ProRes (`prores_ks`/VideoToolbox, profiles 422 proxy…4444 with alpha), H.264 existing. Encoder capability probing at startup (`export.list_encoders` query) with a real 8-frame trial encode to confirm HW availability (not just presence of names).
  - GIF export (palettegen/paletteuse two-pass, dither option, loop), audio-only exports (WAV/MP3/AAC/FLAC/Opus), PNG/JPEG sequence, single frame export `export.frame{t, path, format}`.
  - 10-bit output for HEVC/ProRes when the sequence color pipeline is 16F; SDR tags (Rec.709) correctly written by default (color_primaries/trc/matrix/range) so players do not shift colour; overrides are 2G.05.
  - Faststart for MP4. MP4 with HEVC tagged `hvc1` for Apple compatibility. Fallback to software on HW failure mid-export with a warning event (restart encoder, not whole export).
- **Interfaces:** `export.list_encoders`, `export.frame`; extended `ExportSettings`.
- **Acceptance criteria:**
  - `ffprobe` conformance matrix test (CI, software encoders only, plus a nightly HW job on reference machines) verifies codec, profile, pix_fmt, color tags, duration (±1 frame), audio channels/rate for each built-in preset.
  - Exported H.264 1080p simple timeline HW encode ≥ 2× realtime on the reference machine (§19; asserted in nightly bench).
  - GIF 5 s 480 px < 5 MB with the preset defaults on the corpus clip; plays in Chromium (smoke).
  - HEVC output decodes frame-exact against the golden at PSNR ≥ 40 dB (CRF preset).
- **Tests:** ffprobe matrix, PSNR checks, HW nightly.
- **Out of scope:** VP9/WebM beyond what ffmpeg already exposes (listed as optional, not preset), ProRes RAW, DNxHR.
- **Built (2026-11-26, the encoder half):** HEVC (`libx265` and the hardware `hevc_*` encoders, 8 and 10 bit, tagged `hvc1`), AV1 (`libsvtav1` and hardware `av1_*`), ProRes (`prores_ks`, proxy to 4444 XQ with alpha, 24-bit PCM in MOV), `export.list_encoders` (catalog query; hardware verified by an 8-frame trial encode), the `av1-small` preset, Rec.709 tags and faststart for every codec, and the `ffprobe` matrix (software encoders, `crates/cutweave-media/tests/ffmpeg_export_codecs.rs`, `crates/cutweave-engine/tests/export_codecs_ffmpeg.rs`). An encoder that fails midway restarts the whole encode in software from frame 0 (WP-1E.02), not only the encoder: the muxed file cannot change codec partway.
- **Proposed WP-2G.02b (not built):** GIF (palette, dither, loop), audio-only exports (WAV, MP3, AAC, FLAC, Opus), PNG/JPEG sequences and `export.frame{t, path, format}`; the `gif` and `audio-*` presets keep resolving and report `supported: false` until then. 10-bit from a 16-bit pipeline waits for the ADR-0032 follow-up (the renderer produces 8-bit RGBA today).

### WP-2G.03a — Background export queue
- **Area:** crates/cutweave-jobs, crates/cutweave-ops/src/export/queue.rs, crates/cutweave-engine (export job runner, queue store), docs/reference
- **Depends on:** E1E, WP-2G.01
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - (Split from WP-2G.03 on 2026-11-25; ranges, marker sets, collisions and the disk check are WP-2G.03b.)
  - Persistent export queue (sqlite via cutweave-jobs, in `<project>/.cutweave/exports.sqlite`; in memory for an untitled project; ADR-0030, Proposed): `export.enqueue{sequence, preset, range, path, overrides}` snapshots the sequence at enqueue time (immutable snapshot reference at the current `rev`, so later edits do not alter queued jobs) and runs jobs serially by default (parallel limit `JobConfig.export_slots`, default 1 for GPU exports). Jobs survive engine restart (interrupted job restarts from the beginning, with partial output cleaned; documented). Switching projects suspends the old project's exports; they resume when it is reopened.
  - Operations: `export.enqueue` (adds; waits while the queue is paused), `export.start` (adds and resumes the queue), `export.cancel`, `export.pause_queue`, `export.reorder`, `export.retry`, `export.remove`, queries `export.queue`, `job_status` (common). Progress: percent, fps, ETA, current frame, bytes. Events `job.progress`.
  - Export doesn't block editing; the preview may degrade (GPU budget `jobs` rule: preview priority above export, with a documented throttle). Offline media rules from 2F.03.
  - MCP core: `export_start`, `job_status`; queue management catalog.
- **Interfaces:** commands above; `ExportJob` state machine `queued→running→done|failed|canceled`.
- **Acceptance criteria:**
  - Enqueue 3 jobs, kill the engine during job 2 and restart: job 1 `done`, job 2 restarts and completes, job 3 completes; outputs valid (ffprobe).
  - Editing the timeline during an export does not change the exported output (frame hash vs. snapshot render).
  - Cancel mid-export leaves no partial file and the queue proceeds.
  - Editing command RTT p95 stays < 16 ms during an export (an in-process engine test here; the gate is 2J.01's harness).
- **Tests:** integration with the kill harness, unit for the state machine.
- **Out of scope:** range kinds, marker sets, the collision policy and the disk check (WP-2G.03b); distributed export, `cutweave worker` parallel chunking (P7/post-1.0), publishing.

### WP-2G.03b — Export ranges, marker sets, file collisions and disk check
- **Area:** crates/cutweave-ops/src/export/, crates/cutweave-engine/src/export/, docs/reference
- **Depends on:** WP-2G.03a, WP-2A.02
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - (Split from WP-2G.03 on 2026-11-25.)
  - Range export: `range = {kind: sequence | in_out | custom{start,end} | markers{ids[]|all, pair_mode}}`: a marker-set range export queues one job per marker region (marker `duration` or pairs of consecutive markers, naming template `{seq}_{marker_label}_{n}`).
  - Output file collision policy (`rename|overwrite|fail`; today's `overwrite: true` keeps working), disk space precheck.
- **Interfaces:** the `range` and collision params of `export.start` / `export.enqueue`.
- **Acceptance criteria:**
  - Marker-set export of 4 markers produces 4 files with the right durations (±1 frame).
  - Each collision policy is covered: `rename` picks a free name, `fail` returns `Invalid` at `/path`, `overwrite` replaces the file only when the export succeeds.
- **Tests:** integration (ffprobe durations), unit for ranges, naming and collisions.
- **Out of scope:** the queue itself (WP-2G.03a); the dialog's range selector (WP-2G.04).

### WP-2G.04 — Export dialog and queue panel
- **Area:** packages/editor/src/export/, apps/desktop/e2e/flows, docs/user-flows.md
- **Depends on:** WP-2G.03a (queue panel), WP-2G.03b (range selector and the "file exists" choice), WP-2G.05, WP-2H.02, E1M
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Export dialog (schema-driven form from the preset schema): preset picker grouped by platform, summary (resolution/codec/estimated size from bitrate × duration), range selector (whole sequence / in-out / marker set), output path picker with template, filename tokens, advanced section. Platform warnings inline. Buttons: Export, Add to queue.
  - Queue panel (dockable): list with progress, ETA, cancel/retry/reorder (drag), reveal-in-folder, open file, errors with the machine code and human message; desktop notification on completion.
  - Save-as-preset flow.
  - Settings (catalog `docs/reference/settings.md`): Settings > Export gains the default preset (last used) and the file name pattern (`{project}`; tokens project, sequence, date, preset) next to WP-1M.13's rows; Settings > Notifications gains "Notify when an export finishes while Cutweave is in the background" (on; an OS notification from main) and "Play a sound when an export finishes" (off).
  - (product owner request, 2026-11-24) The preset picker groups platform presets under each platform with its logo-free name and a one-line summary (for example "1080 × 1920, up to the platform's limit"). It puts the presets that match the sequence's aspect first. A preset whose aspect does not match says what will happen (fit with bars) and offers "Make a 9:16 copy of the sequence" when WP-2B.01's aspect presets exist. Over-limit duration or size is an inline warning, never a block.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** a menu on each queue row (`export.row`): Cancel, Retry, Move up, Move down, Reveal file, Open file, Copy error.
- **Interfaces:** consumes `export.*` commands/queries and the `job.progress` events.
- **Acceptance criteria:**
  - Playwright: export a YouTube 1080p preset of the fixture project through the UI and verify the output with ffprobe; queue 2 exports, cancel one.
  - **A user flow (ADR-0018, F34):** on a 9:16 sequence, File > Export…: the portrait platform presets come first; choose Instagram Reels: the summary shows 1080 × 1920 and a timeline longer than the preset's limit shows the warning; export: `ffprobe` reads 1080 × 1920 H.264 + AAC; choose YouTube 1080p on the same sequence: the dialog says it will fit with bars.
  - Progress updates ≥ 4 Hz without re-rendering unrelated panels (React profiler assertion: queue panel only).
  - axe: 0 violations in the dialog.
- **Tests:** Playwright, axe.
- **Out of scope:** publishing/upload (P6).
- **Built (2026-11-26):** `packages/editor/src/export/`: the dialog (`ExportForm.tsx`, `exportModel.ts`) with the preset picker (a native select whose first group is "For this 9:16 sequence", then each platform, General and your presets; unsupported presets listed and off), the summary (size, rate, codecs, container, length, the engine's estimate or an "up to" guess for a quality preset), the warnings in words, the range (whole sequence, in to out, custom, markers by duration or marker to marker), the path (extension follows the container, a name template for marker sets), "If the file exists" (ask, keep both, replace), Advanced (size and rate, quality, hardware switch, the encoder options of `export.describe_options` in `SchemaForm`, whose new `errors` prop shows a refusal at its field), Export and Add to queue; the dialog is at most 60 % of the window and scrolls inside. The Exports panel (`ExportQueuePanel.tsx`, `exportQueue.ts`; View > Panels): rows with progress, speed and time left, icon buttons with tooltips (move up and down or drag, cancel, retry, reveal, remove), pause and resume, the `export.row` menu; rows follow `jobStore`, and the bin no longer re-renders on job progress. Settings > Export gains Default preset, Last preset used and File name (`export.preset`, `export.last_preset`, `export.filename`, a new `text` preference type). Flow F34; F14, F19, F42 and M1 open Advanced for the size and quality. Deviations: the over-limit F34 export is a 4 s custom range (a full 3:28 export is too slow for the gate); the "Make a 9:16 copy of the sequence" offer waits for WP-2B.03; the OS save dialog still filters MP4, so a `.mov` pick gets its extension in the editor.
- **Proposed WP-2G.04b (not built):** save-as-preset from the dialog (`export.save_preset`), Settings > Notifications' `notify.export_done` (an OS notification from main) and `notify.sound`, "Open file" on a queue row (a main `shell.openPath` bridge), the save dialog's filter following the preset's container, and the machine error code on a failed row (`ExportQueueJob` carries only the message).
- **Built as WP-2G.04b (2026-11-27, issue #202):** Save as preset in the dialog (an icon button beside the picker, a name row; `presetDraft.ts` folds the typed size and rate, the quality, the hardware switch and the encoder options into a preset and `export.save_preset` keeps it; a preset of the person's can be deleted with `export.delete_preset`, with Undo in the dialog); Settings > Notifications gains Finished exports (`notify.export_done`, an OS notification from main while Cutweave is in the background, `apps/desktop/src/main/export-notice.ts`) and Export sound (`notify.sound`, two notes made in the editor); Open file on a finished queue row and in its menu (`shell:openFile`: main opens only absolute paths of an export file type); the save dialog offers the preset's file type. Flow F65. **Not built:** the machine error code on a failed row, because a failed job carries only its message: ADR-0038 (accepted 2026-11-27) names the field; built under issue #226. The desktop app passes `--presets-dir` (`export-presets` in its data folder), so a saved preset is there after a restart.

### WP-2G.05 — Advanced export controls
- **Area:** crates/cutweave-ops/src/export/advanced.rs, crates/cutweave-media (encoder params), preset schema
- **Depends on:** WP-2G.01, WP-2G.02
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - `ExportSettings.advanced` exposed on `export.start`/`enqueue` and in presets: video `rate_control: crf|cbr|vbr|cq` with `crf|bitrate|max_bitrate|buffer_size|two_pass`, `gop{keyint, keyint_min, bframes, closed_gop, scenecut}`, encoder `preset`/`tune`/`profile`/`level`, `pix_fmt`, `threads`; color tags override `color{primaries, transfer, matrix, range(limited|full)}` (default Rec.709 limited, written as container and bitstream tags; flags `tag_only` vs. convert; HDR transfers rejected with `Unsupported` in P2); audio `codec(aac|mp3|opus|flac|pcm_s16|pcm_s24)`, `bitrate`, `sample_rate(44100|48000|96000)`, `channels(mono|stereo)`.
  - Validation: per-encoder capability matrix (from `export.list_encoders`) rejects unsupported combos with `Invalid{path, msg}` and a suggestion (e.g. CBR not supported by this HW encoder → "use VBR or software"). Seconds-based keyframe interval helper (`keyint_seconds`) converts via the sequence fps. Estimated output size shown from bitrate × duration.
  - Query `export.describe_options{codec}` returns the JSON Schema of valid advanced options for a codec/encoder (drives the dialog's advanced section and agents).
  - MCP: `export_start` (core) accepts `advanced`; `export.describe_options` catalog.
- **Interfaces:** `AdvancedExportOptions` JSON Schema; commands/queries above.
- **Acceptance criteria:**
  - ffprobe matrix (software encoders in CI): CBR 8 Mb/s H.264 yields measured average within ±5% and max over any 1 s window ≤ 1.2× target; CRF 18 vs 28 differ in size as expected; `keyint=60` yields keyframes exactly every 60 frames (probe packet flags); closed-GOP flag verified; color tags read back exactly as set; AAC 192 kb/s and Opus 128 kb/s read back with the right bitrates ±10%.
  - Invalid combinations (e.g. ProRes + CRF) return `Invalid` with the path and a hint; no encoder is started.
  - Presets from 2G.01 still produce identical output when `advanced` is omitted (frame-hash regression).
- **Tests:** ffprobe matrix, validation unit tests, schema snapshot.
- **Out of scope:** HDR output, per-segment bitrate, custom ffmpeg arg passthrough.
- **Built (2026-11-26):** `advanced` on `export.start` and `export.enqueue` and in presets, `export.describe_options{codec, encoder?, container?}`, the capability checks (`crates/cutweave-ops/src/export/advanced.rs`), `estimated_kib` on the start answer, and the encoder side in `cutweave-media` (`VideoAdvanced`, `RateControl::Cbr`, color conversion by matrix and range, MP3, Opus, FLAC and 16-bit PCM audio). The `ffprobe` matrix is `crates/cutweave-media/tests/ffmpeg_export_advanced.rs` and `crates/cutweave-engine/tests/export_advanced_ffmpeg.rs`. Reference: [export-quality.md](../reference/export-quality.md#advanced-controls-wp-2g05). Deviations: `two_pass` is `Unsupported` (it needs two encodes of the render; use `vbr` with `max_bitrate_kbps`); a hardware encoder takes only rate control, `keyint`, `bframes`, the peak and buffer and the speed preset (anything else needs software, and `hw: "auto"` then uses software with a warning); the hardware `cbr` and peak mappings are untested on hardware; `keyint_seconds` is whole seconds; a preset's own `audio.sample_rate`/`channels` and `video.preset` are still not applied (only `advanced` is), so the built-in presets export as before. Marker-set name templates take the preset's container extension (`.mov` for ProRes).

---

# Epic 2H — Workspace UI

### WP-2H.01 — Dockable workspace, Short-form/Long-form layouts, i18n scaffold
- **Area:** packages/editor/src/workspace/, packages/editor/src/dock/, packages/editor/src/i18n/, packages/editor/src/shell/ (View > Panels in the menu model), apps/desktop/e2e/flows, docs/user-flows.md
- **Depends on:** E1M, E1L
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - `dockview` integration: all panels (Media, Preview, Timeline, Inspector, Mixer, Export queue, Effects placeholder) registered in a typed panel registry (contribution-point shaped so L3 plugins can register later). Drag/dock/float/tab/maximize, per-panel minimum size.
  - Layout serialization to user config (`~/.cutweave/workspaces.json`) with schema version and recovery from corrupt layouts (falls back to the default and notes it).
  - Built-in workspaces: **Long-form** (wide preview, large timeline, media left, inspector right) and **Short-form** (9:16 preview centered and tall, timeline compact at the bottom, inspector right). Workspace switcher in the title bar, "Save layout as…", "Reset layout". Selecting a sequence with 9:16 aspect suggests (not forces) Short-form once per project.
  - Settings > General: "Workspace for new projects": last used (default), Long-form, Short-form (`general.workspace`, catalog `docs/reference/settings.md`).
  - i18n scaffold: `i18next` (or `lingui`), message extraction script, `en` catalog with ICU plurals, locale detection, lint rule failing on user-visible hard-coded strings in `packages/editor` and `packages/ui` (allowlist file). Command names/descriptions come from the registry (English now, translation keys reserved).
  - **Already in P1:** the workspace switcher, layouts saved per workspace, "Reset layout" in the top bar (WP-1M.01, F16) and the i18n scaffold (WP-1M.01). This WP builds on them and does not replace them.
  - (product owner request, 2026-11-24: "Option to remove and re-add panels")
    - **Close a panel:** a close button on each panel's tab (P1's tabs have none) and "Close panel" in the tab's menu.
    - **Get it back:** View > Panels lists every registered panel (Assets, Preview, Inspector, Timeline, and later History, Mixer, Export queue, plugin panels) with a checkmark on the open ones. Choosing a panel toggles it: a closed panel reopens where it was last docked in this workspace, or at its default dock. Each item is a command, so it can be bound to a key (WP-2H.05) and run from the palette (WP-2H.04).
    - **Reset:** View > Reset layout resets the current workspace (as the top bar's button does), and "Reset all workspaces" resets them all.
    - **Every panel closed:** the dock shows an empty state with "Reopen panels" and "Reset layout".
    - **State survives closing:** a panel keeps its state when closed and reopened (the bin's search and sort, the timeline's zoom and scroll), because that state lives in stores, not in the panel's component.
    - **Per workspace:** closed panels are part of the workspace's saved layout, so each workspace keeps its own set, and a relaunch keeps them closed.
  - **Context menus:** this WP merged on 2026-11-25, before the context-menu framework (WP-2H.12), with dockview's own tab menu (Close panel, Maximize, Float). Moving the dock tab menu onto the framework, with the full item list, and the empty dock's menu, is part of WP-2H.13 (plan in [09-context-menus.md](../research/09-context-menus.md)).
- **Interfaces:** `registerPanel({id,title,component,defaultDock})`, `workspace.*` view commands (UI-only); View > Panels and Reset layout.
- **Acceptance criteria:**
  - Playwright: switch Long→Short, drag the inspector to float, restart the app, the layout restores; corrupt `workspaces.json` → app starts with the default and a toast.
  - **A user flow (ADR-0018, F33):** close the Inspector with its tab's close button: the Preview widens and View > Panels shows Inspector unchecked; choose it: it comes back where it was. Close Assets in Long-form, switch to Short-form (Assets open there) and back (still closed). Close every panel: the empty state shows, and Reset layout brings all four back. Relaunch: a panel closed before quitting stays closed.
  - Layout change with the playback running causes no dropped preview frames (> 2 per 10 s) — measured by the playback probe.
  - `pnpm i18n:extract` is idempotent in CI; lint blocks a hard-coded string in a sample PR (test fixture).
- **Tests:** Playwright, lint test.
- **Out of scope:** translations other than `en`, RTL (P7), plugin panels (P6).
- **Moved to a follow-up (WP-2H.01b, integrator decision):** i18n tooling (`pnpm i18n:extract`, ICU plurals and their dependency, `lint:i18n` in `just check`) and "Save layout as…" (named custom workspaces). The layout-change dropped-frames check needs the playback probe and is not asserted in F33.

### WP-2H.02 — Schema-driven inspector renderer
- **Area:** packages/ui/src/schema-form/
- **Depends on:** E1N, E1M
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Form renderer: JSON Schema (+ `x-ui` hints) → controls. Widgets: number (drag-scrub label), slider, angle dial, color (with alpha, eyedropper later), toggle, enum/segmented, vec2 (linkable x/y), font (placeholder until P3), asset-ref (picker from bin), curve (uses `CurveEditor`), text, group/collapsible sections, conditional visibility (`x-ui.visibleWhen`), units and ranges, reset-to-default, multi-value ("mixed") display for multi-selection.
  - Keyframeable fields render a ◇ keyframe button and animated-state indicator (filled diamond when a keyframe is at the playhead); the form delegates to `item.set_props{auto_key}` / `keyframe.*` through a provided `onChange(path, value, {gesture:'start'|'update'|'end'})` callback; gestures use `gesture` tx.
  - Controlled by schema only: no per-command hand-written forms in this WP. Hints format specified in `packages/ui/SCHEMA_HINTS.md` and added to the command macro schema output (`x-ui`) via a small change in `cutweave-commands-derive` attribute (`#[ui(widget="slider", min=0, max=1, step=0.01, unit="%")]`).
  - Accessibility: label/for, keyboard operation, `aria-valuetext` with units.
- **Interfaces:** `<SchemaForm schema value hints onChange keyframes>`; `#[ui(...)]` derive attribute.
- **Acceptance criteria:**
  - Vitest/Storybook-style fixtures: ≥ 25 schema fixtures render the expected widget types (snapshot), including nested objects, arrays, enums, multi-select mixed values.
  - Dragging a slider issues ≤ 60 `update` callbacks/s and exactly one `end`.
  - A new `#[ui]` attribute changes the generated schema snapshot (insta) and TS types remain generated.
  - Render of 40 fields < 8 ms (React profiler bench).
  - axe 0 violations for all fixtures.
- **Tests:** Vitest, insta, axe.
- **Out of scope:** effect/composition params (P3/P4 reuse it), AI model forms (P5).

### WP-2H.03a — Inspector wiring: video, clip info, track, keyframes
- **Area:** packages/editor/src/inspector/ (and the editor wiring it needs), `#[ui]` hints on the `item.set_props` params in crates/cutweave-ops (with the regenerated schema and TS types), apps/desktop/e2e/flows, docs/user-flows.md
- **Depends on:** WP-2H.02, WP-2C.03
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Context inspector sections by selection: Video (transform, crop, opacity, flip, blend), Clip info (name, source, proxy status), Track (name, flags, gain). Multi-selection edits apply to all (`items[]`) in one undo step; fields where the clips differ show "Mixed".
  - The Video section is data-driven from the `item.set_props` input schema (`x-ui` hints, WP-2H.02): adding a property to the schema adds a control with no UI code change (tested).
  - Keyframe navigation (previous and next key), "add or remove keyframe at playhead" and "reset" per property, and a section reset; animated properties show their value at the playhead and edits key at the playhead.
  - Numeric entry supports expressions (`+10`, `*2`, `/2`, `50%`).
- **Interfaces:** consumes `item.set_props`, `item.reset_props`, `keyframe.*`, `track.set`; the `item.set_props` schema's `x-ui` hints.
- **Acceptance criteria:**
  - Playwright: select two clips, set opacity 50% for both in one undo step; keyframe a position animation and verify with `item.get`.
  - Adding a dummy optional prop in a test build schema adds a control without UI code changes (test).
  - Inspector update latency (typing a value → preview shows change) p95 < 100 ms (probe).
- **Tests:** Playwright, Vitest.
- **Out of scope:** Audio, Speed and Sequence sections (WP-2H.03b); effects, text, masks sections; color label (no model field).

### WP-2H.03b — Inspector wiring: audio, speed and sequence
- **Area:** packages/editor/src/inspector/, apps/desktop/e2e/flows, docs/user-flows.md
- **Depends on:** WP-2H.03a, WP-2E.01, WP-2D.01, WP-2B.01
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - Speed (rate, reverse, freeze button), Audio (volume, pan, fades, preserve-pitch, normalize) and Sequence (format, background, frame rate, shown when nothing is selected) sections, on the data-driven sections and keyframe wiring of WP-2H.03a. The Audio section replaces the hand-coded Gain row.
- **Interfaces:** consumes the schemas of `clip.set_speed`, `audio.*`, `sequence.set_format`.
- **Acceptance criteria:**
  - Playwright: set a clip's volume and pan for two clips at once; change the sequence format with nothing selected.
- **Tests:** Playwright, Vitest.
- **Out of scope:** effects, text, masks sections.

### WP-2H.04 — Command palette and registry-driven menus
- **Area:** packages/editor/src/palette/, packages/editor/src/menus/
- **Depends on:** E1N, WP-2H.01, WP-2H.12
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - `cmdk` palette (Ctrl/Cmd+K) listing every non-hidden registered command (from the generated catalog) with summary, bound shortcut, namespace chips, fuzzy search; commands with required args open an auto-generated argument form (via `SchemaForm`); `when` clauses (selection/track/playhead context) via a `ui.when` hint filter the list. Recent commands first. Palette also lists panels/workspaces, and the settings pages and rows (opened at the row with `openSettings`, WP-1M.11).
  - Application menus (File/Edit/Timeline/Clip/View) generated from `#[command(menu="Timeline/Split")]` metadata plus a small static layout file; Electron native menu mirrors it.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** reuses WP-2H.12's `when` evaluator and `UiCommandTable`; it does not build a second one. A command's `menu`/`when` hints are the defaults for its label and enabled state in surface tables. A palette row has its own menu (`palette.row`): Run, Copy command id, Assign shortcut….
- **Interfaces:** `menu`/`when` attributes in the command macro; `CommandCatalog` store.
- **Acceptance criteria:**
  - A newly registered command appears in the palette with no UI code change (test with a fixture command).
  - Palette opens in < 100 ms with 400 commands; search keystroke latency < 16 ms.
  - Playwright: run "Split at playhead" from the palette; keyboard-only operation works.
  - A fixture command with hints appears in a context-menu surface and in the palette with no UI code change.
- **Tests:** Vitest, Playwright.
- **Out of scope:** plugin-contributed menu items (P6).
- **As built (2026-11-25):**
  - **No `cmdk`:** the palette is the design system's `Dialog` and `SearchField` over a listbox (no new dependency).
  - **Catalog:** the editor reads `schema/commands.json` at build time into a `CommandCatalog` store; a command registered there, or a `CommandInfo` or UI command, shows with no UI code change.
  - **`when` and menus:** `CommandInfo.when` and `UiCommand.when` are the command's default clause. The palette leaves out a row whose clause fails; a menu row without its own `when` takes it as its enabled state.
  - **Argument form:** a small form generated from the input schema (text, number, switch, select, JSON). `SchemaForm` (WP-2H.02) was not merged, and replaces it when it lands (the field model stays).
  - **Not built here:** the `#[command(menu=...)]` attribute and the generated application menus and native menu mirror (the menu bar stays `shell/appMenu.ts`, and a `menu` hint needs a macro change outside this WP's Area).

### WP-2H.05 — Keymap engine, shortcut editor, NLE and CapCut presets
- **Area:** packages/editor/src/keymap/ (and the editor's existing key tables: `shell/appMenu.ts`, `transport/keymap.ts`, `panels/binKeys.ts`, `shortcuts/`), packages/timeline (`TIMELINE_KEYS` becomes a context of the keymap), packages/preferences (`shortcuts.*`), apps/desktop (global shortcut conflicts), docs/reference/settings.md, apps/desktop/e2e/flows, docs/user-flows.md
- **Depends on:** WP-2H.04, E1M, WP-1M.13
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Keymap engine: bindings `{key|chord, command, args?, when?}`, contexts (timeline focus, preview focus, text input suppresses), platform modifier mapping, chords (e.g. `Ctrl+K Ctrl+S`), conflict detection, key repeat rules (JKL shuttle), IME safety.
  - Shortcut editor UI as the Keyboard shortcuts page of Settings (it replaces WP-1M.13's read-only list in WP-1M.11's frame; its search also answers the dialog's search): searchable command list, record-a-shortcut, conflict warning with reassign/cancel, per-command reset, import/export JSON, preset switcher. Bindings and the preset are app preferences (`shortcuts.*`, catalog `docs/reference/settings.md`), included in Export settings….
  - Presets (L0 data files `keymaps/nle.json`, `keymaps/capcut.json`, plus `default`= NLE): NLE: J/K/L, I/O, B razor, A select, V, Q/W ripple trim previous/next, Home/End, Ctrl+Z/Shift+Ctrl+Z, M marker, comma/period nudge, Del / Shift+Del ripple delete. CapCut: Ctrl+B split, Del delete, Ctrl+Z, Ctrl+Shift+Z, Space, `[`/`]` trim to playhead, Ctrl+D... derived from research/01-capcut-features.md; any binding not verifiable is marked `unverified` in the file and in Notes N7.
  - Every command bindable (palette listing = bindable list).
  - (product owner request, 2026-11-24: "We need keyboard shortcut mapping in settings")
    - **One keymap for every handler.** The four tables WP-1M.13 lists today (the menu bar's `chordsFor`, the transport keymap, `TIMELINE_KEYS`, `BIN_KEYS`) become bindings of this engine with contexts, and `listBindings()` keeps listing them, so the page and the handlers still cannot drift.
    - **Undo and Redo:** Windows and Linux bind Redo to both Ctrl+Shift+Z and Ctrl+Y today, but the Edit menu shows only Ctrl+Y. A person can choose which binding the menu shows. The CapCut preset puts Ctrl+Shift+Z first, and macOS stays ⇧⌘Z.
    - **Any keyboard layout:** today letters match `KeyboardEvent.key`, so a shortcut can miss when a non-Latin layout is active (Hebrew, Russian). Letters fall back to the physical key (`KeyboardEvent.code`) then, as the native menu's accelerators do. Record-a-shortcut stores what the person pressed and shows it in their layout.
    - **Reserved keys** cannot be bound, and the editor says why: the menu bar's Alt access keys and F10, Alt+F4, the clipboard keys inside text fields, Esc and Enter in dialogs.
    - **Storage:** only the differences from the chosen preset are stored (`shortcuts.bindings` holds overrides and removals), so a preset fixed in an update reaches people who changed other keys. "Reset all shortcuts" is next to the preset switcher.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** Shift+F10 and the Menu key are reserved (they open the context menu and cannot be bound); a surface table's shortcut hints follow a rebinding live; the shortcut editor's row has a menu (`shortcuts.row`): Reset binding, Remove binding, Copy command id.
- **Interfaces:** keymap JSON schema; `keymap.*` view commands.
- **Acceptance criteria:**
  - Both presets load with 0 conflicts (CI check script) and cover ≥ 30 commands each.
  - Playwright: switch preset NLE→CapCut and `Ctrl+B` splits at the playhead; rebinding persists across restart; typing in the inspector's text field does not trigger shortcuts.
  - Vitest: with a Hebrew and a Russian layout simulated (`key` non-Latin, `code` `KeyZ`), Ctrl+Z still undoes; a reserved key is refused with its reason; the stored preferences hold only the overrides.
  - **A user flow (ADR-0018, F35):** Settings > Keyboard shortcuts: rebind Split to Ctrl+K and see it split on the timeline; bind it to Ctrl+Z and answer the conflict prompt with Cancel; make Ctrl+Shift+Z the Redo the menu shows and see it in Edit; export the shortcuts to a file (picker stubbed), reset all, import them back; relaunch: the bindings persist.
  - Keymap lookup < 0.2 ms.
  - Rebinding Split changes the hint shown in the timeline clip menu without restarting.
- **Tests:** Vitest, Playwright, conflict check script.
- **Out of scope:** per-workspace keymaps, gamepad/MIDI controllers.

### WP-2H.06 — Preview overlays: transform handles, crop, safe areas, grid
- **Area:** packages/editor/src/preview/overlays/
- **Depends on:** WP-2C.03, WP-2C.04, E1H, WP-2H.02
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - DOM/SVG overlay over the WebGPU canvas (same page, no z-order issues, §10): transform box with move/scale (corner = uniform, Shift = free)/rotate handles + anchor point; crop mode (edge/corner handles); multi-selection bounding box; snapping to canvas center/edges/other layers' edges with guides; arrow-key nudges (1 px / 10 px with Shift).
  - Overlay toggles: safe areas (action 90%/title 80% plus platform-specific UI-safe zones for 9:16 Shorts/Reels/TikTok as data), rule-of-thirds grid, center cross, custom grid, aspect-ratio frame guide, pixel-ratio display.
  - (product owner requests, 2026-11-24)
    - The safe-area pack uses one ID per platform, shared with WP-2G.01's platform presets (Instagram Stories added), so the preview can show the zone of the chosen export preset.
    - For two videos side by side, snapping also catches the canvas halves and quarters and the neighbouring clip's edges. Shift-dragging a crop edge crops the opposite edge too. WP-2C.08's layouts are the one-click way to get the same result.  - Gestures use `gesture` tx and `item.set_props{auto_key}` so the preview updates live; on animated props, dragging with auto-key creates a keyframe at the playhead.
  - Coordinates: conversions between overlay CSS px ↔ sequence px ↔ normalized, accounting for the preview scale and the object-fit; unit-tested.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** `preview.item` menu on a selected clip in the Preview: Reset transform, Fit to canvas, Fill canvas, Center, Flip horizontal, Flip vertical.
- **Interfaces:** `<PreviewOverlay>`; safe-area data pack `overlays/safe-areas.json`.
- **Acceptance criteria:**
  - Playwright: drag-move a clip 100 sequence px: `position` changes by 100 ± 0.5 px at any preview zoom (25%/50%/100%); rotate by 15° ± 0.5°; undo returns to the original.
  - 9:16 manual reframe task: scale to fill and pan a 16:9 clip so a subject is centered, in ≤ 10 UI actions (used in dogfood B).
  - Side-by-side task: two 16:9 clips on a 16:9 sequence arranged to fill the left and right halves with handles and crop, in ≤ 12 UI actions, with each clip filling its half within 1 px.
  - Overlay stays aligned with the video during live resize (pixel diff on a test pattern < 1 px).
  - Overlay interaction latency (pointer move → preview update) p95 < 50 ms.
- **Tests:** Vitest for transforms, Playwright.
- **Out of scope:** text editing caret, mask handles (P3/P4).

### WP-2H.07 — UX design review gate (human)
- **Area:** docs/reviews/p2-ux-review.md
- **Depends on:** WP-2A.10, WP-2H.01, WP-2H.03a, WP-2H.05, WP-2H.06
- **Size:** S
- **Executor:** human
- **Scope:** structured review of the editor against research/01-capcut-features.md behaviours worth matching (magnetic track, snapping feel, aspect switch, inspector structure, keyframe workflow). Also review the context menus: every surface in [09-context-menus.md](../research/09-context-menus.md) sections 2 and 6 has a menu or a stated reason for none, no menu shows more than about 12 rows, and the order follows frequency. Output a prioritized defect list (P0 blockers for dogfood, P1, P2), each filed as an issue with `phase:P2` label.
- **Interfaces:** n/a.
- **Acceptance criteria:**
  - Review doc committed with a checklist of ≥ 25 scenarios each marked pass/fail/N/A with notes; every fail has an issue link.
  - Zero open P0 UX issues before 2J.06 starts (or explicit maintainers' waiver in the doc).
- **Tests:** n/a.
- **Out of scope:** visual redesign.

### WP-2H.08 — Project settings: `project.set_settings` and File > Project settings…
- **Area:** crates/cutweave-ops (`project.set_settings`, `project.new` defaults), crates/cutweave-model (`ProjectSettings` schema hints), generated schema and TS types, packages/editor (Project settings dialog, inspector link, "Use as defaults for new projects"), docs/reference/settings.md, apps/desktop/e2e/flows, docs/user-flows.md
- **Depends on:** WP-1M.11, WP-2H.02, WP-2B.01, WP-2F.05
- **Size:** M
- **Executor:** sonnet
- **Scope:** (product owner request, 2026-11-24) project settings get their own surface, apart from the app's Settings (catalog: "Project settings").
  - `project.set_settings{patch}`: one undoable command for every `project.settings` key (a merge patch validated against the `ProjectSettings` JSON Schema, then semantic checks), MCP catalog; `project.info` returns the settings. Feature commands that set one key (`media.set_proxy_policy`, WP-2F.01; `render.cache_enable`, WP-2J.09) produce the same patch and stay as named shortcuts for agents. Keys whose change is a UI-only scope (the project's AI budget cap, WP-5A.07) are refused for agents with `forbidden`.
  - Defaults for new projects: `project.new` takes optional `settings`; the editor passes the app preferences that are defaults for new projects (still image duration, proxy policy, the last sequence format chosen).
  - Dialog: File > Project settings… (and "Project settings…" in the inspector when nothing is selected); a schema-driven form (WP-2H.02) over `ProjectSettings` in sections: New sequences (format), Media (still image duration, WP-2F.05; proxies, WP-2F.01), Preview (background rendering, WP-2J.09), Saving (autosave for engines without the app, WP-1C.03), AI (spending cap, WP-5A.07, when it lands). Each change is one undo step ("Change project settings"); changes made by agents show live; "Use as defaults for new projects" writes the matching preferences.
  - Keys added by later WPs appear in the dialog without UI work (schema-driven).
- **Interfaces:** `project.set_settings{patch}`; `project.new{…, settings?}`; the Project settings dialog.
- **Acceptance criteria:**
  - Registry property harness: undo identity and failure atomicity for `project.set_settings`; an unknown key fails with `Invalid` at its path.
  - **A user flow (ADR-0018, F24):** File > Project settings…, set still image duration to 3 s, drop an image on the timeline: its clip is 3 s long; Ctrl+Z puts 5 s back in the dialog; "Use as defaults for new projects", then create a new project: its still duration is 3 s; an agent (`cutweave exec` with `project.set_settings`) changes the proxy policy and the open dialog shows it.
  - Adding an optional key to `ProjectSettings` in a test build adds a control with no UI change (test).
- **Tests:** registry harness, Vitest, user flow.
- **Out of scope:** per-sequence settings (WP-2B.03); app preferences (WP-1M.11); the brand kit (WP-6I.05).

### WP-2H.09 — Settings: Appearance page (interface scale, density, reduce motion)
- **Area:** design/tokens.json and the `cargo xtask tokens` outputs (density sizes; design owner review), packages/ui (density through `data-density`), apps/desktop (zoom factor in main), packages/editor (Appearance rows), packages/timeline (hit-testing at any scale), docs/reference/settings.md, apps/desktop/e2e/flows, docs/user-flows.md
- **Depends on:** WP-1M.11, WP-2H.01, WP-2A.10
- **Size:** M
- **Executor:** sonnet
- **Scope:** (product owner request, 2026-11-24) the rest of the Appearance page of the settings catalog.
  - **Interface scale** 90, 100 (default), 110, 125, 150%: main sets the window's zoom factor at once and keeps it across relaunches; the preview canvas and the timeline stay crisp (backing size = CSS size × device pixel ratio, as today) and pointer hit-testing stays exact.
  - **Density** Standard (default) or Comfortable: a second set of size tokens (control heights, row and panel padding, track heights) under `data-density="comfortable"`, with usage notes and a passing contrast check; Standard is today's tokens, unchanged.
  - **Reduce motion:** follow the system (default), on, off; on makes every transition instant, as `prefers-reduced-motion` does today.
  - The high-contrast theme joins the theme row in WP-7C.02. Accent colors are not offered (marigold is the one "here" color, design/README.md) unless the product owner decides otherwise.
- **Interfaces:** preference keys `appearance.scale`, `appearance.density`, `appearance.reduce_motion`; density tokens in `design/tokens.json`.
- **Acceptance criteria:**
  - Visual baselines of the editor shell in both densities and both themes, at 100% and 125%.
  - Playwright: at 90% and 125%, dragging a clip by 100 CSS px moves it by the same time as at 100%, and the preview canvas backing size equals CSS size × DPR within 1 px.
  - **A user flow (ADR-0018, F26):** set the interface scale to 125% and density to Comfortable: the controls grow at once; relaunch: both persist; turn Reduce motion on: dialogs open with no transition.
  - axe: 0 violations in both densities.
- **Tests:** visual, Playwright, user flow.
- **Out of scope:** the high-contrast theme (WP-7C.02), custom accent colors, per-panel font sizes.

### WP-2H.10 — Settings: Playback page (preview quality, proxies, scrub audio, loop)
- **Area:** packages/editor (Playback page, transport bar), crates/cutweave-playback (scrub-audio switch, loop) and generated schema if a transport command changes, docs/reference/settings.md, apps/desktop/e2e/flows, docs/user-flows.md
- **Depends on:** WP-1M.11, WP-2F.02, E1H
- **Size:** S
- **Executor:** sonnet
- **Scope:** (product owner request, 2026-11-24) the Playback page of the settings catalog; app preferences.
  - Preview quality while playing: Auto (default), Full, Half, Quarter; when paused: Full (default) or Auto (the quality ladder, arch §8).
  - Use proxies in the preview when they are ready (on); play audio while scrubbing (on); loop playback (off); when playback stops, go back to where it started (off).
  - The transport bar's proxy, loop and quality controls and these rows are one value each, app-wide. The per-project `preview_quality` in `ui-state.json` (WP-1C.04) is no longer restored; the preference wins.
- **Interfaces:** preference keys `playback.*`; the existing `view.*` transport commands, plus a scrub-audio switch if the engine has none.
- **Acceptance criteria:**
  - Vitest: the transport's proxy and loop controls change the preference and the page shows it, and the reverse.
  - **A user flow (ADR-0018, F28):** turn "go back to where it started" on, play and stop: the playhead returns; turn Loop on and play past the end: playback wraps; set quality while playing to Half: the transport's quality indicator says Half while playing; relaunch: the values persist.
- **Tests:** Vitest, user flow.
- **Out of scope:** pre-roll and post-roll; shuttle speed presets.

### WP-2H.11 — History panel: every undo step, who made it, go back to any step
- **Area:** packages/editor (History panel in WP-2H.01's panel registry, Edit > History), crates/cutweave-commands (`steps` on `edit.undo` and `edit.redo`), generated schema and TS types, apps/desktop/e2e/flows, docs/user-flows.md
- **Depends on:** WP-2H.01, WP-1M.07, WP-1B.04
- **Size:** M
- **Executor:** sonnet
- **Scope:** (product owner request, 2026-11-24: "History panel") a dockable panel listing the project's undo history as ADR-0019 defines it: the editor's Undo and Redo cover every actor and name who made each step.
  - **Rows** come from `edit.history` (paged with `cursor`, virtualized) and follow `edit.history_changed`.
    - One row per change (cause `edit` or `restore`), oldest at the top, with a marker under the current state. Undone changes that can still be redone sit below the marker, dimmed.
    - Each row shows the step's name as the Undo button says it ("Split clip"), who made it (You, the agent's label, a plugin) with agent rows in agent blue (design/README.md), and when ("2 min ago", with the exact time in the tooltip).
    - Undo and redo records are not rows; they move the marker. Changes that can no longer be redone (after a new edit) disappear.
  - **Click a row to go back to it.**
    - Every change after it is undone, newest first, as `*` steps (ADR-0019). Clicking a dimmed row redoes up to it.
    - `edit.undo{actor:"*", steps}` and `edit.redo{actor:"*", steps}` (additive, at most 100) apply them in one turn of the doc actor, so no other actor's command lands in between.
    - Each step stays its own history record, so Ctrl+Z and Ctrl+Y keep working one step at a time.
    - A refused step (`Conflict{history_target_modified}`, arch §5.2) stops the jump there and returns `done`, the steps already applied. The panel shows the engine's message naming the later change and its actor.
  - **Filter:** All, You, Agents (dims the rest; jumps still follow the shared order). Keyboard: ↑/↓ move between rows, Enter goes to the row. Empty state: "No changes yet".
  - **Opening it:** View > Panels (WP-2H.01) and Edit > History. It is not in the default layouts; it opens as a tab next to the Inspector.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** a menu on a History row (`history.row`): Go to this step, Copy label; the Undo and Redo buttons' menus gain "Open History…".
- **Interfaces:** `edit.undo{actor?, steps?}`, `edit.redo{actor?, steps?}` (additive; output gains `done`); the panel registered as `history`.
- **Acceptance criteria:**
  - Unit: `edit.undo{actor:"*", steps:3}` gives the same document and the same history records as three single `*` undos; a conflict at the second step leaves the first applied and reports `done: 1`; the undo property harness covers `steps`.
  - Parity: the CLI, MCP and RPC give byte-identical documents for a script with `steps`.
  - Vitest: rows from a history fixture with user, agent and plugin steps, undone tails and a restore; the marker follows `edit.history_changed`; rows that can no longer be redone are gone.
  - **A user flow (ADR-0018, F32):** import a clip, make four edits (insert, move, opacity, trim) and let an agent (`cutweave exec`) trim a clip. The History panel lists the import, the four edits and the agent's trim, the agent's row named and in agent blue. Click the move's row: the timeline shows the state after the move and three rows are dimmed. Ctrl+Y redoes the opacity change. A new edit removes the remaining dimmed rows.
  - axe: 0 violations; visual baselines of the panel (a mixed history, the empty state) in dark and light.
- **Tests:** unit, property harness, parity, Vitest, user flow.
- **Out of scope:** a branching history tree; clearing the history; showing what a step changed (a diff view); history across sessions beyond what the engine keeps (arch §5.2).

### WP-2H.12 — Context-menu framework: host, surfaces, command-derived items, keyboard and touch, contributions
- **Area:** packages/ui (`Menu`: submenu entry, disabled reason, controlled open at a point), packages/editor/src/menus/ (host, surface registry, `UiCommandTable`, `when` evaluator, contribution registry), packages/timeline (a `contextmenu` and long-press event on the pointer stream), apps/desktop (allowlisted host-action IPC: reveal in the file manager, copy text), design/components/Menu/README.md, docs/adr/0026-context-menu-surfaces-and-contributions.md, apps/desktop/e2e/flows, docs/user-flows.md
- **Depends on:** E1M, E1N
- **Size:** M
- **Executor:** sonnet
- **Scope:** (product owner request, 2026-11-25: "almost everything on screen should react to context menu with appropriate actions"; research in [09-context-menus.md](../research/09-context-menus.md), decision in ADR-0026, Proposed) the machinery every menu uses. It builds menus for no surface beyond a test fixture, the dock tab stub and text fields; WP-2H.13 and each owning WP add the rest.
  - **Host:** one `ContextMenuHost` in `EditorShell`; `useContextMenu(surface, target)` and `<ContextSurface>` for DOM surfaces; `openContextMenu({surface, point, target})` for the canvas, the dock tab and the keyboard. The host renders `@cutweave/ui`'s `Menu` against a zero-size anchor at the point, closes on scroll, resize and Escape, and returns the focus to the surface element.
  - **Surface tables:** per surface ID (the list in ADR-0026), ordered groups of `{command, args(ctx), when, order, label?}` as data. Label, icon, destructive and undoable come from the generated catalog (`schema/commands.json`); the shortcut hint comes from the keymap (`keysFor`, today's `shortcuts/bindings.ts` tables until WP-2H.05).
  - **`when` evaluator and context keys** (`selection.count`, `selection.kind`, `item.linked`, `track.locked`, `playhead.inItem`, `clipboard.hasItems`, `media.used`, `engine.live`, `project.readOnly`, ...; the list is in ADR-0026): a small boolean expression language shared with the palette (WP-2H.04). Disabled items carry a one-line reason (`aria-describedby`; shown as a tooltip on hover and focus); items that make no sense for the target's kind are left out.
  - **`UiCommandTable`:** editor-side actions with no document effect (`workspace.*`, rename in place, open Settings at a row, copy text, reveal) in the same shape as catalog commands, so the palette and keymap list them too. Host actions go through allowlisted, zod-validated IPC (the pattern of `edit:run`; main resolves paths itself) and are listed in an exemption file with a reason, like `schema/parity-exempt.toml`.
  - **Selection and target rules:** the target is resolved once at open, by IDs. Right-click on an unselected item selects it first (`view.select`, not undoable); on a selected item the menu acts on the whole selection and its labels count it ("Delete 3 clips"); on an empty timeline lane the selection is kept, on an empty bin it is cleared. Menus on the canvas use the hit-test zone (`body`, `left-edge`, `right-edge`, `keyframe`, `lane`, `header`, `ruler`, `playhead`); the right button in `interaction.ts` no longer falls through silently.
  - **Keyboard and touch:** Shift+F10 and the Menu key open the menu for the focused surface (DOM: native `contextmenu`; canvas, dock tab and Preview: the host anchors it at the selection, the playhead or the focused card). A touch or pen long-press (500 ms, cancelled by a move over 8 px) opens it on the canvas and cancels the gesture; a pen's barrel button takes the right-click path.
  - **Contributions:** `MenuContributionRegistry.register({surface, command, group, when, order})` used by first-party WPs now and by plugins in WP-6B.03: a trailing group, at most 3 items per plugin before an overflow submenu, plain-text labels, only commands the plugin may run.
  - **Text fields:** one shared menu (Undo, Cut, Copy, Paste, Select all, through the existing `edit:run` IPC) installed by a capture handler for every input, textarea and contenteditable. A right-click outside any registered surface shows nothing (and not the browser's menu); in development builds it offers "Inspect element".
  - **`Menu` in `packages/ui`:** a `submenu` entry kind (one level, per the design spec), an optional `disabledReason`, a controlled `open` plus `anchor` for context mode, and the spec update in `design/components/Menu/README.md` (command names that match the registry). Stories and tests for each.
  - **Dock:** replaces the empty `getTabContextMenuItems` stub with the host (the tab's items arrive with WP-2H.01).
- **Interfaces:** `ContextMenuHost`, `useContextMenu`, `ContextSurface`, `openContextMenu`, `registerMenuSurface`, `MenuContributionRegistry`, `UiCommandTable`, `evaluateWhen`; the `Menu` additions above; host-action IPC channels; the surface ID and context key lists of ADR-0026.
- **Acceptance criteria:**
  - Vitest: a fixture command with menu and `when` hints appears in its surface and is disabled, with the reason, when the context says so, with no UI code change; the surface-table walk test fails for a row whose command is not in the catalog or the UI table, whose label key is missing from the `en` catalog, or whose shortcut text is a literal; a closure that would call two mutating commands is rejected (one composite command is required).
  - Vitest: the target rules (unselected, selected, several selected, empty lane, empty bin, locked track); the `when` evaluator (precedence, an unknown key is false and logged); the contribution registry limits (3 per plugin, clamped order, plain-text labels).
  - **A user flow (ADR-0018, F42):** right-click a text field in the Inspector and choose Paste; press Shift+F10 and then the Menu key on a focused control and see the menu open at it and Escape return the focus; right-click on dead space and see nothing. (WP-2H.13 extends F42 with the surfaces it adds.)
  - axe: 0 violations on a menu opened from a DOM surface, from the canvas and from the keyboard; the menu is named for its target (for example "Clip actions for interview_A.mov").
  - The canvas open path: `contextmenu` to menu visible p95 < 50 ms with 5,000 items in the timeline (the hit-test is an index lookup).
  - ADR-0026's pass criteria are checked (fixture command, walk test, F42, axe) and the integrator is told, so maintainers can accept it.
- **Tests:** Vitest, Storybook visual baselines (dark and light), user flow, axe.
- **Out of scope:** the menus of any other surface (WP-2H.13 and the owning WPs); the palette (WP-2H.04) and the keymap (WP-2H.05), which read the same registry; plugin manifest wiring (WP-6B.03); OS spellcheck suggestions in the text-field menu; native (OS) context menus.

### WP-2H.13 — Context menus for today's surfaces: bin, preview, timeline basics, inspector, chrome
- **Area:** packages/editor (media bin, preview, timeline panel, inspector, top bar, transport, toasts, settings rows, the surface tables under `menus/surfaces/`), apps/desktop/e2e/flows, docs/user-flows.md
- **Depends on:** WP-2H.12, E1M
- **Size:** M
- **Executor:** sonnet
- **Scope:** (product owner request, 2026-11-25) menus for everything on screen today, using only commands that exist now (section 6 of [09-context-menus.md](../research/09-context-menus.md) has the item lists, their order and their status). Items whose command arrives in a later WP are added by that WP, not here.
  - **Dock** (moved from WP-2H.01, which merged first; it ships dockview's tab menu with Close panel, Maximize and Float): a panel tab has a menu (`dock.tab`): Close panel, Maximize or Restore, Float, Move to… (Left, Right, Bottom), Reset layout, Save layout as…; the empty dock has one (`dock.empty`): Reopen panels, Reset layout. Each item is a `workspace.*` UI command, so the palette and keymap list it too. This replaces the empty `getTabContextMenuItems` stub.
  - **Bin card** (kind-aware; one or several selected): Insert at playhead, Append to timeline, Play in Source view, Reveal in Explorer / Finder, Copy path, Remove from project (the existing confirm dialog). **Bin background and empty bin:** Import…, Sort by and View as (the same state as the dropdowns), Show hover scrub. **Filter chip:** Remove filter, Clear all filters.
  - **Preview canvas:** Play/Pause, Go to start and end, Safe areas, quality and size choices (`view.set_preview_size`), Copy frame and Save frame as… (`render.frame` and a host save dialog). **Source view:** Back to timeline, Copy timecode. **Transport:** timecode (Copy timecode), volume (Mute, Reset to 100%), quality badge (the quality choices).
  - **Timeline** (the canvas, by hit zone): **clip:** Split at playhead, Delete, Enable/Disable, Rename…, Reveal in media bin, Copy path; **empty lane:** Move playhead here, Add track above and below, Select all clips on track; **track header:** Rename…, Mute, Solo, Hide, Lock (check items, `track.set`), Add track above and below, Delete track (confirm when it has clips); **ruler and playhead:** Move playhead here, Go to start and end, Zoom to fit, Copy timecode. Right-click selects as the target rules say. Every WP that adds a timeline command adds its item to these tables.
  - **Inspector:** section header (Collapse all, Expand all), media info path (Copy path, Reveal), media picture (Play in Source view, Copy frame), property row (Copy value). **Settings:** row (Reset to default, Copy setting key).
  - **Chrome:** save status (Save now, Autosave settings…), Export button and chip (Export…, Cancel export, Reveal file), workspace switcher and Reset layout button (Reset this workspace), Undo and Redo buttons (Undo, Redo), toast (Copy message, Copy error code, Dismiss, Dismiss all), engine status chip (Reconnect, Copy diagnostics, Open logs folder). A sash double-click resets its size (no menu).
  - **Shortcut hints** on every item come from the keymap; an item with no shortcut shows none.
  - Registers the `UiCommandTable` entries it needs (copy, reveal, collapse) and the host-action IPC channels, with their exemption entries.
- **Interfaces:** the surface tables for `bin.card`, `bin.background`, `bin.chip`, `preview.canvas`, `source.view`, `timeline.clip`, `timeline.lane`, `timeline.track`, `timeline.ruler`, `timeline.playhead`, `inspector.section`, `inspector.row`, `chrome.save`, `chrome.export`, `chrome.toast`; no new engine commands.
- **Acceptance criteria:**
  - **A user flow (ADR-0018, F42 extended):** right-click a video card in the bin: Insert at playhead places the clip; right-click an empty part of the bin: Import… opens the (stubbed) picker; right-click the timeline clip: it becomes the selection, Split at playhead splits it, and Ctrl+Z undoes it; right-click it again, Disable clip, and the preview stops showing it; right-click a track header: Mute, then Lock, and a drag on that track is refused; right-click an empty lane: Add track above; right-click the ruler: Move playhead here; press the Menu key on a focused bin card and choose Remove from project, then cancel its dialog; right-click the Export button during an export: Cancel export. Each choice is checked as an undo step or as state the person sees.
  - Parity: each menu choice that changes the document yields the same normalized document as the equivalent CLI or MCP command (the existing parity harness over a script of the menu's commands).
  - With three timeline clips selected the clip menu reads "Delete 3 clips" and Rename is disabled with its reason; on a locked track Split and Delete are disabled with "Track is locked".
  - axe: 0 violations on each opened menu; visual baselines of the bin card menu and the timeline clip menu in dark and light.
  - No literal colors, sizes or strings in the new code (the existing lint rules pass).
- **Tests:** Vitest for the surface tables and target rules, user flow, parity script, axe, visual.
- **Out of scope:** items that need commands from later WPs (clipboard, linking, speed, markers, keyframes, replace, folders and tags, relink), which their WPs add; plugin items (WP-6B.03); menus for panels that do not exist yet.

---

# Epic 2I — MCP core tool set

### WP-2H.14 — Detached panels I: popout windows, the shell and the layout
- **Area:** apps/desktop/src/main (`window-policy.ts`, `windows.ts`, `hardening.ts`, `index.ts` focus wiring, a `window:setPopoutOnTop` channel in the IPC contract and `handlers.ts`), apps/desktop/src/preload (the one new bridge method), packages/editor/public/popout.html, packages/editor/src/workspace/ (`dockHost.ts`, `commands.ts`), packages/editor/src/dock/, packages/editor/src/shell/ (a window registry), packages/editor/src/menus/surfaces (the `dock.tab` items), the `en` catalog, apps/desktop/e2e (security, flows), docs/user-flows.md, docs/adr/ (the ADR, Proposed), docs/01-architecture.md §2, §16, §18
- **Depends on:** WP-2H.01, WP-2H.12, WP-2H.13 (the dock tab menu), the ADR in section 9
  - **Decided in [ADR-0031](../adr/0031-detached-panels-popout-windows.md)** (Accepted 2026-11-26): popout windows run unsandboxed in the editor's process with no preload, an exception to arch §2/§18 and ADR-0001.
- **Size:** M
- **Executor:** opus (it changes the window security policy and the Electron process model)
- **Scope:**
  - **Main process:**
    - A `popout` window kind: the editor's process, `sandbox: false` (Electron 45 requires it to share the opener's process), no preload, the locked settings of every kind.
    - `setWindowOpenHandler` allows only an editor window's `popout.html` on the app origin (dev: the Vite server), with dockview's frame name (`<dock>-<group>`). Everything else is denied as today.
    - `did-create-window` registers the kind, asserts the policy, clamps the bounds to the connected displays (`fitToDisplays`), hides the menu bar and paints the theme's background.
    - A popout's focus counts as its editor's for `project.report_focus` (ADR-0021).
    - `window:setPopoutOnTop{frame, on}` (zod-validated) for Keep on top.
  - **Dock:**
    - `workspace.popout_panel{panel}` and `workspace.dock_panel{panel}` (UI commands, so the palette and keymap list them).
    - "Move to new window" and "Move back to main window" in the tab menu.
    - Dragging a tab outside the main window opens it in a new window at the pointer. A tab drag that ends with no drop outside the window counts as that.
    - Closing a detached window docks its panels back where they were.
    - A failed popout (`onDidOpenPopoutWindowFail`) leaves the panel docked, with a toast.
    - Reset layout and Reset all workspaces dock everything back. Switching workspaces closes the old workspace's windows and opens the new one's.
  - **Layout:**
    - `popoutGroups` are saved with each workspace and restored on launch. `layoutProblem` validates their panels like the grid's.
    - A layout saved by an older build still loads.
  - **Window registry** (`shell/windows.ts`):
    - Holds the main window and each popout, with `onDidAddWindow` and `onDidRemoveWindow`.
    - Mirrors `data-theme`, `data-density`, `lang` and `dir` into each popout and keeps them in sync.
    - Titles each popout "<Panel> - <Project> - Cutweave".
    - WP-2H.15 builds on it.
  - **Keep on top:** a toggle in the detached group's header.
- **Interfaces:** the `popout` window kind and its policy; `window:setPopoutOnTop`; `workspace.popout_panel`, `workspace.dock_panel`; `popoutPanel(id)`, `dockPanel(id)` in `dockHost.ts`; the window registry (`registeredWindows()`, `onDidAddWindow`, `onDidRemoveWindow`).
- **Acceptance criteria:**
  - Vitest:
    - The policy accepts a `popout` only as `sandbox: false` with no preload.
    - The handler allows only `popout.html` from an editor window with the frame-name pattern and denies everything else.
    - `dockHost` round-trips a layout with a popout and keeps a failed popout docked (jsdom fake window).
  - Security e2e (`security.spec.ts`): a popout has no `cutweave` global of its own and no `require`; its navigations and its own `window.open` are blocked; its document has the app CSP; the addon is loaded once.
  - **A new user flow, "Detached panels" (ADR-0018; its number is allocated at merge, since F44 is taken):**
    1. Right-click the Inspector's tab and choose Move to new window: a second window titled with the panel name shows the Inspector, and the Preview widens.
    2. Click a clip in the main timeline: the detached Inspector shows it. Change its opacity there: the engine has the new value.
    3. Switch to the light theme: the detached window follows.
    4. Close the detached window: the Inspector is docked where it was.
    5. Move it out again and relaunch: the window reopens with the Inspector.
    6. Reset layout: it docks back.
    7. `quitApp` leaves no window behind.
  - The engine's focus record does not change when the focus moves between the editor and its detached window.
- **Tests:** Vitest, security e2e, the user flow.
- **Out of scope:** keys, context menus, tooltips, the Preview and the Timeline inside a detached window (WP-2H.15); a full-screen program monitor on a second display; detaching plugin panels (P6).

### WP-2H.15 — Detached panels II: every panel works in its own window
- **Area:** packages/editor (keymap dispatch, transport keymap, app commands, bin keys, the menu host, the palette, `preview/`, the timeline panel, scrub and gesture schedulers), packages/ui (a portal-container provider for `Menu`, `Tooltip` and `Dialog`; `Tabs`), packages/timeline (`timeline-view.ts`), apps/desktop/src/preload (`present-loop.ts`, `api.ts`: present on the canvas's window, re-attach), a lint rule (biome or a script in `just check`), apps/desktop/e2e/flows, docs/user-flows.md
- **Depends on:** WP-2H.14, WP-2H.05
  - **Decided in [ADR-0031](../adr/0031-detached-panels-popout-windows.md)** (Accepted 2026-11-26): popout windows run unsandboxed in the editor's process with no preload, an exception to arch §2/§18 and ADR-0001.
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - **Per-window helpers:** `windowOf(node)`, `documentOf(node)`, `activeDocument()` and `forEveryWindow(install)`, built on WP-2H.14's registry.
  - **Keys:** every key listener (the keymap dispatcher, the transport, the app commands, the bin) is installed on every registered window. "Focused surface" checks read the focused window's active element. The menu bar's Alt handling stays in the main window.
  - **Menus and tooltips:**
    - `installContextMenuHandlers` runs on each window's document.
    - `packages/ui` gets a portal-container context. Each panel sets it from `panel.api.getWindow()` and updates it when the panel changes window, so `Menu`, `Tooltip` and the command palette open in the window the person is in.
    - App dialogs stay in the main window, which takes the focus.
  - **Preview:**
    - The preload's present loop schedules on the canvas's own window. The preload API keeps its shape.
    - The canvas is re-attached when its panel changes window only on a platform where a moved canvas loses its context. Windows does not.
    - Size and `devicePixelRatio` come from that window, and `view.set_preview_size` follows a DPI change.
  - **Timeline:**
    - `timeline-view.ts`, the gesture scheduler and the scrubber use their element's window for `ResizeObserver` and `requestAnimationFrame`.
    - So do the bin, History, `Tabs` and the media preview.
  - **Realm-safe checks:** the `instanceof` sites that can see popout events or popout-created nodes become duck-typed. A lint rule bans the bare globals `document`, `window`, `ResizeObserver`, `requestAnimationFrame` and `devicePixelRatio` in panel code, outside an allowlist.
- **Interfaces:** the helpers above; `PortalContainerProvider` in `packages/ui`; no engine or IPC change.
- **Acceptance criteria:**
  - Vitest: a key pressed in a second (jsdom) window runs its binding; a right-click there opens the menu in that window's document; the helpers return the element's own window; the lint rule flags a fixture.
  - **The "Detached panels" flow, extended (ADR-0018):**
    1. Detach the Timeline and the Preview.
    2. Press Space in the detached Timeline: the timecode runs, and the detached Preview's frames change.
    3. Right-click a clip in the detached Timeline: the menu opens in that window; choose Split at playhead. Press Ctrl+Z there: the split is undone.
    4. Press Ctrl+K there: the palette opens in that window.
  - Vitest: the present loop, the timeline and the gesture scheduler take their frames from their canvas's window.
  - A manual check on Windows, macOS and Linux: a detached Preview keeps playing while the main window is minimized. The flow cannot show it, because Playwright turns throttling off.
  - **Opt-in real-input steps** (`just e2e-real-input`): drag a bin card from the main window onto the detached Timeline (the clip lands at the drop point); drag the detached Timeline's tab back into the main dock.
  - With the Preview detached, playback drops no more frames than docked (the playback probe, under 2 per 10 s).
  - axe finds 0 violations in a detached window.
- **Tests:** Vitest, the user flow, the playback probe, axe.
- **Out of scope:** the second-display program monitor; per-window keymaps; detached plugin panels (P6).

### WP-2I.01 — Core tool set: mapping, annotations and completion (~30 tools)
- **Area:** crates/cutweave-api/src/mcp/, crates/cutweave-ops (`mcp = ` attributes), docs/reference/mcp-core-tools.md (generated)
- **Depends on:** E1J, WP-2A.03, WP-2A.04, WP-2B.01, WP-2C.03, WP-2E.01, WP-2G.03a, WP-2F.03
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Reconcile the architecture §14 example core list with the commands that actually exist after P2. Implement a naming rule `namespace.verb_object → namespace_verb_object` (`.` to `_`, plus explicit aliases for `item.delete → item_delete`, etc.) and a table `mcp_core.toml` of the final ≤ 32 core tools (decision A budget: standard ≤ 36 tools / ≤ 8k tokens including the 4 generic tools and `view_get`/`view_set`): `project_open, project_info, media_import, media_list, timeline_get, timeline_insert, clip_split, clip_trim, item_move, item_delete, item_set_props, track_add, sequence_create, sequence_set_format, timeline_add_marker, keyframe_set, audio_set, render_frame, render_contact_sheet, media_filmstrip, audio_waveform, export_start, job_status, job_wait, view_get, view_set, edit_undo, edit_redo, edit_checkpoint, edit_restore, clip_set_speed`, plus the 4 generic tools (`timeline_ripple_delete` and `media_proxy_status` stay reachable via `cutweave_execute`). `media_import` returns `media_ids` synchronously; analysis/proxy work is async and agents wait with `job_wait{job_id, timeout_ms}` (decision E).
  - `view_get`/`view_set` (N3 RESOLVED): expose playhead, selection, in/out and zoom through the MCP catalog (not core commands); non-undoable and not journaled; excluded from the parity test's command list but covered by their own schema test. Text/caption/AI tools are added in P3/P5 (placeholders for `text_add`, `caption_generate`, `effect_add`, `transition_add`, `ai_*`, `composition_create` are explicitly listed as "arrives in P3–P5").
  - Rich annotations (`readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`) and concise descriptions ≤ 300 chars with one example each; error text always includes a recovery hint (e.g., `Locked: track trk_… is locked; call track_set_flags{locked:false}`).
  - Every structured result carries `rev` and small summary (not full dumps).
  - `timeline_get` supports `depth`, `fields`, `range`, `limit`, `cursor` and by default returns a summarized overview (tracks, counts, item ids/names/ranges) under 4k tokens for a 400-item sequence.
- **Interfaces:** `mcp_core.toml`; generated reference doc.
- **Acceptance criteria:**
  - CI test: the `tools/list` for the `standard` profile is ≤ 36 tools and ≤ 8k tokens (budget file checked in; failing adds require a diff to the budget).
  - Parity test (E1O) green: schemas identical to RPC schemas; every `mcp="core"` command appears in `tools/list`; every non-hidden non-view command reachable via `cutweave_execute`.
  - MCP Inspector conformance passes on stdio and HTTP.
  - Snapshot test on the full core tool list with annotations (insta).
- **Tests:** insta, conformance script, parity.
- **Out of scope:** text/caption/AI tools; resources beyond what P1 provides (adds `cutweave://sequence/{id}/timeline` pagination in 2I.02 only if absent).
- **As built (2026-11-27):**
  - `mcp_core.toml` is `crates/cutweave-api/mcp_core.toml`: 28 listed core tools (each with its command, a description and one example), the budget, and the planned tools that are not listed yet. The naming rule has no exception, so there is no alias table: `item.delete` is `item_delete` by the rule, and a name that is not the rule's needs an ADR. `docs/reference/mcp-core-tools.md` is generated from it by `cargo xtask gen`.
  - Listed since this WP: `track_add`, `timeline_add_marker`, `clip_set_speed`, `edit_redo`, `edit_checkpoint`, `edit_restore`, `view_get`, `view_set`. `render_contact_sheet`, `media_filmstrip` and `audio_waveform` have no registered command yet and stay pending for WP-2I.02.
  - The full engine lists 32 tools in about 7,694 of 8,000 tokens. To get there, listed input schemas carry validation only: every parameter description is gone from `tools/list` (`cutweave_describe_command` has them) and each listed description ends with its example. The three visual tools need about 650 more tokens; ADR-0039 (accepted, option 1) keeps the 8,000-token budget and the validation-only schemas: WP-2I.02 lists `render_contact_sheet` if it fits and keeps the other two in the catalog.
  - `timeline_get` gained `range{from, to}`, `limit` (1 to 1000) and `cursor` / `next_cursor` at depth 2, and items list their own name. The default stays depth 1 (tracks and counts), far below 4k tokens for a 400-item sequence; a page of 50 items is under 4k tokens too.
  - Results carry the revision in `_meta` (`io.cutweave/rev`) and in the summary line, not inside `structuredContent`, which stays the command's output (ADR-0028). `view_get` is listed with `readOnlyHint: false`: the registry keeps that hint for queries, and `view.get` is an engine command.

### WP-2I.02 — Visual tools: contact sheet, filmstrip, waveform image
- **Area:** crates/cutweave-api/src/mcp/visual.rs, crates/cutweave-render (grid composer), crates/cutweave-cache
- **Depends on:** E1J, E1F, E1D, WP-2I.01
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - `render_contact_sheet{sequence?, from, to, count|every, columns, max_size, labels}`: renders N frames (evenly or at given times, or at marker/cut points `at: "cuts"`), composes a labelled grid (timecode burned-in under each cell, item/track hints) into one JPEG ≤ `max_size` (default 768 px long edge, hard cap 2048), returns image content + metadata JSON `{frames:[{t, items_visible[], captions?}], rev}`. Frames rendered through `render.frame` at preview quality, parallel in the render thread within a total budget: 12 frames at 1080p < 1.5 s on the reference machine.
  - `media_filmstrip{media, from, to, count, max_size}` (from the thumbnail/filmstrip cache; falls back to decode). Returns one strip image.
  - `audio_waveform{target: media|item|range, from, to, width, height, channels, scale_db}`: renders a PNG waveform image (peak pyramid) with a time ruler and, optionally, silence/loudness annotation (LUFS short-term sparkline) as metadata. `audio_levels` data in `structuredContent`.
  - Large results beyond a size threshold returned as resource links via the `/media/…` endpoints with hashed ETags.
  - Prompt-injection hygiene: item names/captions in metadata are labeled `untrusted`.
  - These are `mcp = core`, `readOnly`. (ADR-0039, accepted: the standard list stays at 8,000 tokens, so only `render_contact_sheet` is listed, and only if the list still fits with it; `media_filmstrip` and `audio_waveform` are catalog commands reached through `cutweave_query`. The 36-tool cap is decided again when the P3 tools arrive.)
- **Interfaces:** tools above; commands/queries `render.contact_sheet`, `media.filmstrip`, `audio.waveform`.
- **Acceptance criteria:**
  - Contact sheet golden (WARP/lavapipe): the 12-frame fixture grid matches the reference with SSIM ≥ 0.995 and PSNR ≥ 40 dB; the response content-type is `image/jpeg`, byte size ≤ 400 KB at default settings.
  - `max_size` honored (property test over values 128–2048).
  - Waveform of a 440 Hz sine at −6 dBFS produces peaks at the expected pixel rows (±1 px); a file with a known 2 s silence shows the silence annotation within ±50 ms.
  - Works over both stdio and HTTP transports; MCP Inspector displays the image.
- **Tests:** golden, property, Inspector smoke.
- **Out of scope:** video understanding, scene detection (P5).

### WP-2I.03 — Checkpoints: edit.checkpoint / edit.restore
- **Area:** crates/cutweave-commands/src/checkpoint.rs, crates/cutweave-ops/src/edit/
- **Depends on:** E1B, E1C
- **Size:** M
- **Executor:** opus
- **Scope:**
  - `edit.checkpoint{name?, note?} → {checkpoint, rev}` stores an `Arc` snapshot handle plus metadata (rev, actor, time) in an in-memory registry capped at 50 (LRU, named ones pinned up to 20); persisted in the journal as a marker event so restore works after crash recovery (the snapshot is re-derived by replaying to that rev if the in-memory handle is gone, bounded by journal retention; if unavailable, `NotFound` with a clear message).
  - `edit.restore{checkpoint}`: applies a **forward** transaction computing the inverse diff to bring the doc to the checkpoint state (so history stays linear and the restore itself is undoable; it doesn't truncate history), tagged with the caller's actor. Refuses when it would revert another actor's edits unless `force: true` (default policy: restore reverts all edits since the checkpoint; report lists actors affected).
  - `edit.list_checkpoints`, `edit.delete_checkpoint`. Checkpoints are not auto-created except `cutweave_execute{checkpoint:true}` (1 per call) used by MCP's destructive batch path.
  - MCP core: `edit_checkpoint`, `edit_restore`; `edit_list_checkpoints` catalog. *As built (2026-11-25):* while the P1 MCP gate is full (R33), all four checkpoint commands stay in the catalog, reached through `cutweave_execute`, `cutweave_query` and `cutweave_search_commands`; WP-2I.01 lists `edit_checkpoint` and `edit_restore` as core.
- **Interfaces:** commands above; `CheckpointId`; `EditReport`-like `RestoreReport{reverted_revs, actors}`. *As built:* the report is WP-1B.04's `Restored{tx, reverted[{actor, txs}], reverted_unknown}`; journal format 2 adds checkpoint records and carries checkpoints over saves (arch §5.2, §17).
- **Acceptance criteria:**
  - Property test: random command sequences, checkpoint at a random point, more commands, restore → doc equals the checkpointed doc (structural equality); then `edit.undo` returns to the pre-restore state.
  - Memory: 50 checkpoints on a 1-hour, 400-item project add < 20 MB (persistent-structure sharing assertion via allocation counter).
  - Restore on a 5,000-item doc completes < 50 ms.
  - After a forced engine crash + recovery, a named checkpoint made before the crash is restorable (e2e with the crash harness).
- **Tests:** proptest, memory test, e2e.
- **Out of scope:** per-actor selective restore (future), collaboration semantics.

### WP-2I.04 — Optimistic concurrency: expected_rev everywhere
- **Area:** crates/cutweave-commands, crates/cutweave-api (RPC + MCP + CLI)
- **Depends on:** E1B, E1I, E1J, E1K
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - Uniform `expected_rev` optional field on every mutating command envelope (not in each command struct: in the transport envelope) for RPC, MCP tools (injected into every mutating tool's schema through the generator), `cutweave_execute` (already per §14), and the CLI (`--expected-rev`).
  - On mismatch: `Conflict{current_rev, expected_rev, changed_since: summary}` where `changed_since` lists a compact list of item ids touched between revs (from retained patch metadata, bounded to the last 200 revs; otherwise `unknown`). MCP error text suggests re-reading `timeline_get`.
  - All successful mutating results include the new `rev`.
- **Interfaces:** envelope field; error type extension.
- **Acceptance criteria:**
  - Integration test: two clients, A reads rev N, B edits (rev N+1), A sends with `expected_rev: N` → `Conflict` with `current_rev: N+1`, and the doc is unchanged; A retries with the new rev and succeeds.
  - Parity test: same behaviour over CLI/MCP/RPC with identical error codes.
  - Schema snapshots updated; no per-command schema drift.
- **Tests:** integration, parity.
- **Out of scope:** automatic merge/rebase of agent edits.

### WP-2I.05 — Tool profiles, change notifications and tools/list budget
- **Area:** crates/cutweave-api/src/mcp/profiles.rs, crates/cutweave-cli
- **Depends on:** WP-2I.01
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - Profiles (single budget source): `minimal` ≤ 12 tools / ≤ 3k tokens (open/info/import/list/get/insert/split/trim/delete/render_frame/export/job_wait + generic where they fit), `standard` ≤ 36 tools / ≤ 8k tokens (the core set), `full` = standard + search-based long tail (no extra listed tools; the long tail is found via the search tool and run via `cutweave_execute`). Profiles are selected per MCP endpoint (`/mcp` = standard, `/mcp/minimal`, `/mcp/full`) or the `--profile` launch flag of `cutweave mcp` — never per connection.
  - Change notifications when the tool set changes (plugins load; stub emitter now, plugin loading arrives in P6) use the negotiated MCP revision's mechanism. The engine supports both spec revisions 2025-11-25 and 2026-07-28 (`resources/subscribe` vs `subscriptions/listen`).
  - Budget CI check for each profile: tool counts and estimated tokens (`minimal` ≤ 12 / ≤ 3k, `standard` ≤ 36 / ≤ 8k; `full` lists the same tools as `standard`).
  - `cutweave mcp --print-tools --profile X` prints the list for docs.
- **Interfaces:** CLI `--profile` flag, per-profile HTTP endpoints, `mcp_profiles.toml`.
- **Acceptance criteria:**
  - Inspector shows the tool count per profile matching the files; budgets enforced in CI with a failing sample.
  - A plugin load sends the change notification via the mechanism of each negotiated revision (test clients on 2025-11-25 and 2026-07-28 both receive it); a connection to `/mcp/minimal` never sees `standard` tools.
  - `cutweave_execute` still reaches every command under `minimal`.
- **Tests:** unit, conformance.
  - (2026-11-27, ADR-0040, Proposed) If the owner accepts its question 2, this WP also owns the budget of the `described` profile that WP-2I.07 adds (the same tools with parameter descriptions, for clients that defer tool definitions). Change notifications are for real catalog changes only (a plugin loads), never for switching toolsets in a session: that breaks prompt caching, and the 2026-07-28 revision forbids lists that differ per connection.
- **Out of scope:** per-client auto-profile detection; per-connection profile switching.

### WP-2I.06 — Scripted-agent e2e: YouTube edit and 9:16 short via MCP only
- **Area:** crates/cutweave-cli/tests/agent_e2e/, testdata manifests
- **Depends on:** WP-2I.01, WP-2I.02, WP-2I.03, WP-2I.04, WP-2G.03a, WP-2C.03, WP-2E.03
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Deterministic MCP client scripts (Rust or Python via the MCP SDK) that perform, without any screenshots and using only core tools plus `cutweave_execute`: (a) a rough-cut + B-roll insert + audio normalize + 1080p export from corpus media; (b) a 9:16 short: new sequence, manual reframe via `keyframe_set`/`item_set_props`, music with `audio_set` fades, export Shorts preset.
  - Both scripts assert state via `timeline_get`/`cutweave_query` and validate visually through `render_contact_sheet` byte counts and a rendered-frame golden compare; each script checkpoints before risky steps and tests restore.
  - Run in CI headless on 3 OSes (software rendering) and in a nightly with the attach transport against the GUI engine.
  - A written transcript of tool calls is kept as an example in `docs/examples/agent-session-p2.md` (generated, not hand-written).
  - (2026-11-27) Each script also records the bytes of every tool result and of `tools/list`, and writes them beside the transcript, so the cost of a scripted session is tracked from run to run. Whether a model uses the tools well is WP-2I.09's question, not this script's.
- **Interfaces:** none new; consumes all.
- **Acceptance criteria:**
  - Both scripts pass in CI on Windows/macOS/Linux; total tool calls for (a) ≤ 40 and for (b) ≤ 30 (efficiency guard).
  - The timeline produced by the MCP run equals (structural JSON diff, ignoring IDs) the one built by the equivalent CLI script (parity).
  - Exported files pass the ffprobe checks (duration ±1 frame, 1080×1920 for the short).
- **Tests:** the WP itself is the test suite.
- **Out of scope:** LLM-in-the-loop evals beyond WP-2I.09's on-demand run (P6).

**Epic 2I order after the MCP-at-scale research (2026-11-27).**

The research in `docs/research/11-mcp-at-scale.md` and [ADR-0040](../adr/0040-mcp-at-scale.md) (Proposed) change what the rest of this epic builds against. The remaining work runs in this order, ahead of other Phase 2 work that does not block the exit:

1. **ADR-0040**: the product owner answers its five questions.
2. **WP-2I.07** (hints for clients that defer tool definitions). Its first part needs no decision and starts at once.
3. **WP-2I.09** (the evaluation), so later choices are measured. It can run beside WP-2I.07.
4. **WP-2I.08** (discovery and concise results), then **WP-2I.05** (profiles, with the described profile) and **WP-2I.04**.
5. **WP-2I.02** (visual tools), then **WP-2I.06** (the scripted run), which now also records the size of what a run costs.
6. **WP-2I.10** (the command line with a skill in the Agent Kit), when WP-2I.09 can compare it.

### WP-2I.07 — Ready for tool search: always-loaded tools, the instructions as a map, words a user would say
- **Area:** crates/cutweave-api (`mcp_core.toml`, `src/mcp/core.rs`, `src/mcp/catalog.rs`, the server instructions), docs/reference/mcp-core-tools.md (generated), docs/01-architecture.md §14
- **Depends on:** WP-2I.01; ADR-0040 (questions 1 and 2) for the second part only
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - **Part 1, no decision needed (no schema and no list changes):**
    - A per-tool `always_load` flag in `mcp_core.toml`, emitted as the hint clients that defer definitions read (for Claude Code, `_meta` key `anthropic/alwaysLoad`; confirm the key and its meaning against the client's current documentation before building). Set on `project_info`, `timeline_get`, `cutweave_search_commands`, `cutweave_describe_command` and `cutweave_execute`. Clients that do not know the hint ignore it.
    - The server `instructions` rewritten as a map of the catalog in at most 4,096 characters: the command namespaces and what each is for, "describe a command before its first use", "batch through `cutweave_execute`", how to read a result's revision, and how to undo. A test fails above 4,096 characters.
    - Descriptions and the keywords `cutweave_search_commands` matches on use the words a user would say (cut, split, ripple, fade, crop, speed up, caption). Add a `keywords` list per command where the registry has none; no tool or command is renamed.
  - **Part 2, after ADR-0040:** the `described` profile of question 2: the same tools in the same order with full parameter descriptions, its own endpoint and its own budget in `mcp_core.toml`; a test compares the two standard lists (names, order, and that both validate the same values).
- **Interfaces:** `always_load` and `keywords` in `mcp_core.toml`; the `/mcp/described` endpoint (part 2). No command changes.
- **Acceptance criteria:**
  - The `tools/list` snapshot shows the hint on the five tools and nowhere else; the standard list stays within 8,000 tokens and 36 tools.
  - A search test: each of 30 plain-language phrases ("make the clip faster", "fade the music out", "cut here") returns the right command among the first three results.
  - Part 2: `/mcp/described` lists the same names in the same order as `/mcp`, every listed parameter has a description, and CI fails when it leaves its budget.
- **Tests:** unit, snapshot, the search phrase table.
- **Out of scope:** detecting the client; lists that differ per connection; renaming tools.

### WP-2I.08 — Discovery and results that stay small: search detail levels, describe several, concise reads
- **Area:** crates/cutweave-api (`src/mcp/catalog.rs`, the generic tools), crates/cutweave-ops (`timeline.get`, `media.list` output shaping), docs/reference
- **Depends on:** WP-2I.01; ADR-0040 (question 5)
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - `cutweave_search_commands{query, detail?: "names" | "summary" | "schema"}`: `summary` is today's result and stays the default; `names` returns names only; `schema` adds each hit's input schema, capped at 5 hits.
  - `cutweave_describe_command{name | names[]}`: up to 8 commands in one call.
  - `timeline_get` and `media_list` gain `detail?: "concise" | "full"` (`full` is today's result and stays the default): concise drops fields an agent can ask for again (per-item properties at their defaults, probe details) and keeps ids, names, times and the revision.
  - All additive and optional; today's calls return today's results byte for byte.
- **Interfaces:** the optional parameters above (ADR-0040, question 5).
- **Acceptance criteria:**
  - Snapshot tests for each detail level; a call without the new parameters matches the snapshot from before this work package.
  - On the P-400 fixture, a concise `timeline_get` at depth 2 is at most half the bytes of the full one.
  - The standard list stays within 8,000 tokens with the new parameters.
- **Tests:** unit, snapshot, parity (the same parameters through RPC and the CLI).
- **Out of scope:** changing what `full` returns; pagination beyond what WP-2I.01 added.

### WP-2I.09 — Agent evaluation: measure how a model uses the tools, on demand
- **Area:** crates/cutweave-cli/tests/agent_eval/ or tools/agent-eval/ (a runner outside the gate), docs/reference/agent-eval.md, docs/examples
- **Depends on:** WP-2I.01; WP-2J.01 (fixtures)
- **Size:** M
- **Executor:** opus
- **Scope:**
  - A runner that gives a model a task and an MCP connection to a headless engine on a generated fixture, and records every tool call. Ten tasks to start: split and trim, insert and reorder, set speed, add a marker, fade audio, keyframe a move, set a 9:16 format, undo and checkpoint, import and place, and one long-tail task that needs a catalog command.
  - Each task has a checker that reads the final document through `timeline_get` or `cutweave_query`; no judgement by a model.
  - Variants it can compare in one run: the standard list against the described one; a core tool against the same command through `cutweave_execute`; one batch against several calls; MCP against the command line with the skill (when WP-2I.10 exists).
  - Measures per task and variant: success; whether the first call to each tool was valid; describe and search calls; turns; tokens in and out; tokens of tool results.
  - It needs a model and a key, so it never runs in the gate or in CI. It runs on demand (`just agent-eval` or an xtask), reads the key from the environment, and writes a dated result file and a short report. With no key it prints how to run it and exits cleanly.
  - A smoke test in the gate replays one recorded session against the engine with no model, so the runner and the checkers cannot rot.
- **Interfaces:** none in the product.
- **Acceptance criteria:**
  - One full run on the standard and the described list, with the report committed under `docs/reference/agent-eval/`.
  - The report answers, with numbers: do parameter descriptions change first-call validity, and by how much; what does a core tool save over `cutweave_execute`; what does a batch save.
  - The replay smoke test passes in the gate with no network.
- **Tests:** the replay smoke test; the checkers have unit tests.
- **Out of scope:** judging the quality of an edit; a benchmark across many models; running in CI.

### WP-2I.10 — The command line with a skill, as a second way in for agents with a shell
- **Area:** the Agent Kit (the skill and its reference files), crates/cutweave-cli (only if a gap shows), docs
- **Depends on:** WP-2I.09
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - A skill that teaches an agent with a shell to drive Cutweave through `cutweave exec`, `cutweave query` and `cutweave describe`: the same catalog map as the server instructions, the batch form, how to read revisions and undo.
  - Measured against MCP with WP-2I.09 on the same tasks.
- **Interfaces:** none new.
- **Acceptance criteria:**
  - The ten tasks of WP-2I.09 run through the command line path, and the report compares success, turns and tokens with the MCP path.
- **Tests:** WP-2I.09's runner.
- **Out of scope:** replacing MCP; a scripting tool on the server.

---

# Epic 2J — Performance, reliability & dogfood

### WP-2J.01 — Benchmark fixtures and Rust criterion suite
- **Area:** crates/cutweave-bench/ (new bench crate), testdata/bench-projects/ generator
- **Depends on:** E1A, E1B, E1C, E1O, WP-2A.03
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Deterministic generator (seeded) `cargo xtask gen-bench-project --items N --tracks T --keyframes K` producing: **P-400** (1-hour, 400 items, 8 tracks), **P-5K** (5,000 items, 50,000 keyframes), **P-short** (9:16, 3 layers + animation). Media uses corpus-light synthesized clips (tiny generated test patterns to avoid large downloads) plus corpus references for decode tests.
  - criterion benches: open project (< 2 s to interactive, §19, uses P-400), command RTT in-process and over WS (p95 < 16 ms; 10,000 mixed commands, a histogram is saved), `doc.patched` fan-out, placement ripple on P-5K, `timeline.get` summary on P-400 (< 30 ms), idle memory (RSS after open and idle 60 s < 1.5 GB for the 1-hour 1080p fixture with real media), app start to editable empty project (< 2.5 s; Electron start included in 2J.02/03 harness).
  - Result JSON in a stable schema `bench-results/{machine}/{date}.json` consumed by 2J.04.
- **Interfaces:** `cutweave-bench` crate, `xtask gen-bench-project`.
- **Acceptance criteria:**
  - `cargo bench -p cutweave-bench -- --save-baseline p2` runs on the reference machines; each §19 metric it covers is reported with its budget and PASS/FAIL.
  - Fixture generation is byte-deterministic (hash check in CI).
  - Command RTT p95 < 16 ms on P-5K for 10,000 commands.
- **Tests:** the benches; unit for the generator.
- **Out of scope:** CI wiring (2J.04), GPU/UI benches (2J.02/03).

### WP-2J.02 — UI benchmarks: timeline pan/zoom and UI command RTT
- **Area:** packages/timeline/bench/, packages/editor/e2e/perf/
- **Depends on:** WP-2A.09, WP-2A.10, WP-2J.01
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - Playwright-driven perf tests on Electron with the P-5K project: scripted pan (wheel 20 s) and zoom sweeps, measuring frame times via `performance` + `requestAnimationFrame` probes (≥ 55 fps average and p5 frame time < 25 ms per §19); UI command RTT probe (UI → engine → patch → UI p95 < 16 ms over 500 drag updates); app-start-to-editable (< 2.5 s) on a cold and a warm start.
  - Output the same result JSON schema as 2J.01.
- **Interfaces:** perf JSON; `pnpm perf:ui`.
- **Acceptance criteria:**
  - `pnpm perf:ui` passes the thresholds on the reference machines and prints a table; a deliberately slowed build (test flag) fails it.
  - Results include GPU/CPU info of the machine for provenance.
- **Tests:** the perf suite.
- **Out of scope:** preview playback metrics (2J.03).

### WP-2J.03 — Playback, scrub and A/V sync benchmarks
- **Area:** crates/cutweave-bench/src/playback/, tools/bench-sync/
- **Depends on:** E1H, WP-2C.04, WP-2E.02, WP-2F.01, WP-2J.01
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Headless-capable playback harness (engine only; plus an Electron-attached variant reading the SHM ring) that runs: 1080p60 playback with 3 video layers + 2 effects (use the P1 transform/opacity layers + 2 blend modes as stand-ins for P4 effects until P4 lands; effects flag documented) + animated overlay; counts dropped frames over 60 s (budget 0); scrub tests: 200 random seeks measuring scrub-to-pixel p95 on proxy/intra (< 80 ms) and 4K long-GOP without proxy (< 150 ms); A/V sync flash/click test measuring offset (< 40 ms) and a 30-min soak mode (drift < 1 frame; nightly only); export speed 1080p simple timeline HW encode ≥ 2× realtime.
  - Output the results JSON schema.
- **Interfaces:** `cutweave-bench playback|scrub|sync|export`.
- **Acceptance criteria:**
  - All five §19 metrics reported with PASS/FAIL on the reference iGPU (M1 / Intel Xe) machine; failing ones create a tracking issue template (documented).
  - The sync harness detects a synthetic 60 ms offset (self-test) with ±5 ms accuracy.
- **Tests:** self-tests for each probe.
- **Out of scope:** CI gating logic (2J.04).

### WP-2J.04 — CI benchmark gate and trend dashboard
- **Area:** .github/workflows/bench.yml, tools/bench-compare/
- **Depends on:** WP-2J.01, WP-2J.02, WP-2J.03, E1O
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - Workflow: PR-level lightweight benches on GitHub runners (software-only: command RTT, open project, ripple, timeline fps in headless Chromium with a relaxed 'relative to main' gate: fail on > 15% regression vs the stored baseline); nightly job on self-hosted reference machines (Win/macOS/Linux iGPU) for the absolute §19 budgets; results stored in a `bench-data` branch or artifact store and a static trend page (HTML artifact) is generated.
  - `bench-compare` compares result JSONs, outputs a markdown summary comment on PRs and sets status.
  - Budget file `bench/budgets.toml` mirrors §19 (single source for thresholds).
- **Interfaces:** workflow, `bench/budgets.toml`.
- **Acceptance criteria:**
  - A PR with an injected 25% regression (test branch) fails the gate; a clean PR passes; nightly run publishes a trend page.
  - Absolute-budget failures page as a failed nightly check with the offending metric in the title.
- **Tests:** compare tool unit tests with sample JSON.
- **Out of scope:** purchasing/maintaining the self-hosted runners (process item for maintainers; see N9).

### WP-2J.05 — Crash-injection harness: 20 forced crashes, no data loss
- **Area:** crates/cutweave-bench/src/crash/, apps/desktop/e2e/crash/
- **Depends on:** E1C, E1L, WP-2A.10, WP-2G.03a, WP-2I.03
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Harness that drives a scripted editing workload (mixed commands incl. trims, ripple, keyframes, audio, speed, sequence ops, export in progress) via RPC and kills the engine at randomized points (SIGKILL / `TerminateProcess`; also GPU-device-lost injection and killing during atomic save, during autosave, during journal fsync boundary, mid-ephemeral-tx, mid-export, mid-proxy job, mid-recording) 20+ times per OS; after each crash the engine restarts through the supervisor, recovery runs, and the harness compares the recovered doc with the *model of acknowledged commands* (every command whose reply was received must be present; an in-flight unacknowledged command may be absent or present but never partial/corrupt).
  - Also the Electron e2e variant: kill the engine process while the UI is open, verify the UI reconnects and resyncs, with no lost acknowledged commands.
  - Corrupt-tail journal injection (truncate last record) test; lock-file stale takeover test.
  - Extra check: proxy/export/recording artifacts left in a consistent state (partial files cleaned or recoverable).
  - Reports a pass/fail matrix; nightly CI runs 20 crashes on each OS; a failing seed is printed for reproduction (`--seed`).
- **Interfaces:** `cutweave-bench crash --count 20 --seed N`.
- **Acceptance criteria:**
  - 20/20 crashes on each of Windows, macOS, Ubuntu with zero lost acknowledged commands and zero unreadable project files; 3 consecutive nightly green runs before the exit review.
  - Harness self-test: an intentionally injected bug (skip fsync, test flag) is caught within 20 runs.
- **Tests:** the harness; self-test.
- **Out of scope:** power-loss simulation (fsync fidelity of disk/hardware).

### WP-2J.06 — Dogfood edit A: 10-min YouTube video with B-roll (human)
- **Area:** docs/dogfood/p2-edit-a.md, testdata (non-committed footage list)
- **Depends on:** WP-2H.07, WP-2G.04, WP-2F.01, WP-2F.03, WP-2E.05, WP-2J.04
- **Size:** S
- **Executor:** human
- **Scope:**
  - A **non-developer** edits a real ≈ 10-min single-camera video (talking head + screen/B-roll + music) in the app: import, auto proxies, rough cut with ripple/roll/trim, B-roll on overlay tracks, ducked-by-hand music with keyframed volume and fades, markers, loudness normalization to −14 LUFS, export YouTube 1080p preset, optionally a 4K export. The session is screen-recorded.
  - The observer logs every friction point, crash, wrong behaviour, and time spent per phase.
- **Interfaces:** n/a.
- **Acceptance criteria:**
  - Final export plays correctly (checked: duration, no A/V drift at the end via sync clap check, loudness within ±1 LU of target, no black frames).
  - Report committed: total editing time, issue list with severities; 0 data-loss incidents; all P0/P1 issues filed with `phase:P2`.
- **Tests:** manual protocol checklist in the doc.
- **Out of scope:** captions (P3).

### WP-2J.07 — Dogfood edit B: 9:16 short with manual reframe, music and fades (human)
- **Area:** docs/dogfood/p2-edit-b.md
- **Depends on:** WP-2H.07, WP-2H.06, WP-2B.03, WP-2G.04, WP-2E.05
- **Size:** S
- **Executor:** human
- **Scope:**
  - The non-developer creates a 9:16 sequence from 16:9 footage, performs manual reframe (scale/position keyframes following the subject), adds a blurred-background fill, music with fade in/out, a few cuts with speed ramp substitute (constant speed + freeze frame), and exports the Shorts/TikTok preset. Screen-recorded and logged as in 2J.06.
- **Interfaces:** n/a.
- **Acceptance criteria:**
  - Output is 1080×1920, ≤ 60 s, music fades audible (loudness analysis shows tail < −40 LUFS-S in the last 0.5 s), reframe visibly tracks the subject (reviewer sign-off).
  - Friction report and issues filed; no data loss.
- **Tests:** manual checklist.
- **Out of scope:** captions, auto reframe (P5).

### WP-2J.08 — M2 exit review (human)
- **Area:** docs/reviews/m2-exit.md
- **Depends on:** WP-2J.04, WP-2J.05, WP-2J.06, WP-2J.07, WP-2I.06
- **Size:** S
- **Executor:** human
- **Scope:** check every roadmap exit criterion and every row of the traceability table below against evidence (links to CI runs, bench JSONs, dogfood reports); decide Go/No-Go for M2; record waivers; update the roadmap with measured numbers and re-estimate P3–P5.
- **Interfaces:** n/a.
- **Acceptance criteria:**
  - A signed-off checklist with every exit criterion linked to evidence: (1) two dogfood reports with non-developer, (2) a nightly bench run on the reference machines with all timeline/scrub/playback budgets PASS, (3) crash harness 20/20 on all 3 OSes, (4) zero open P0 issues.
  - Any waived item has a named owner and due date.
- **Tests:** n/a.
- **Out of scope:** planning of P3.

### WP-2J.09 — Background render cache (smart pre-render while idle)
- **Area:** crates/cutweave-cache/src/render_cache/, crates/cutweave-jobs, crates/cutweave-playback, packages/timeline (ruler indicator)
- **Depends on:** E1F, E1H, WP-2C.04, WP-2F.01, WP-2J.03, WP-1D.08
- **Size:** M
- **Executor:** opus
- **Scope:**
  - **Segment detection:** analyze the active sequence into time segments and estimate render cost from the scene graph (layer count, compound nesting, effects, blends, speed/reverse on long-GOP sources) using per-node cost estimates calibrated from measured GPU/CPU time (recorded by the playback path). A segment is "heavy" when its estimated frame time exceeds 70% of the frame budget at the current preview quality, or when playback measured dropped frames over it.
  - **Cache:** render heavy segments to an intra-frame cache file (reusing the proxy encoder profile) keyed by (subgraph hash, range, preview resolution) per §8 "cache by (subgraph hash, t)"; stored in the global cache dir under the LRU cap (default 20 GB, configurable) with entries in `cache.sqlite`. Granularity: fixed 2 s chunks aligned to the frame grid so small edits invalidate only affected chunks.
  - **Scheduling:** a job that runs only when idle (no playback, no gesture, no export; resume ≥ 1.5 s after the last edit), single concurrent, cancelled instantly on edit/playback start, lower priority than export and proxies, nearest-to-playhead segments first; battery/thermal aware pause option.
  - **Playback use:** the preview resolver substitutes cached chunks when the hash matches and quality ≤ cached resolution; falls back to live render otherwise. Pure optimization: results must equal live rendering (cached chunk vs live render at the same quality on the same adapter: bit-identical). Export does not use the cache in P2 (always live, for determinism).
  - Commands: `render.cache_enable{bool}`, `render.cache_clear`, `render.cache_status` query `{segments:[{from,to,state: none|queued|rendering|ready|stale}], bytes}`; project setting `render_cache.enabled` default on.
  - UI: ruler bar under the timeline ruler (grey none, orange queued/rendering, green ready).
  - Settings (catalog `docs/reference/settings.md`): background renders limit (20 GiB) with usage and Clear on Settings > Cache and storage; "Start background rendering after" (1.5 s idle) and "Pause background work on battery" (on) on Settings > Performance and hardware, all machine values applied through UI-only engine commands (WP-1D.08's pattern); `render_cache.enabled` in File > Project settings… (WP-2H.08).
  - MCP: catalog.
- **Interfaces:** commands above; event `render.cache_progress`; `MediaResolver` extension for cached segments.
- **Acceptance criteria:**
  - On a fixture sequence with a 10 s heavy segment (6 layers + 3-level compound, which drops frames live at 1080p60 on the iGPU reference), after the idle cache completes the segment plays with 0 dropped frames over 60 s of looped playback (2J.03 harness).
  - Editing a clip inside one chunk invalidates only chunks whose subgraph hash changed (assertion on chunk states); unaffected chunks stay ready.
  - Starting playback or an edit cancels the cache job in < 100 ms (probe) and editing command RTT stays p95 < 16 ms while the job runs.
  - Cached vs live bit-identical on 50 sample frames (same adapter); LRU cap enforced (cap 500 MB test).
  - Export output frame hashes are identical with the cache on and off.
- **Tests:** integration, invalidation unit tests, bench via 2J.03 harness.
- **Out of scope:** using the cache for export, distributed rendering, GPU-texture caches (already in-engine).

---

# Traceability

## Roadmap scope bullets → WPs

| Roadmap Phase 2 scope | WPs |
|---|---|
| Timeline: unlimited video/audio/overlay tracks | 2A.02, 2A.03, 2A.10 |
| Timeline: magnetic main track | 2A.01, 2A.02, 2A.03 |
| Timeline: linked clips | 2A.01, 2A.05 |
| Timeline: ripple, roll, slip, slide | 2A.01, 2A.03, 2A.04, 2A.08 |
| Timeline: snapping (playhead, edges, markers, beats) | 2A.01, 2A.07, 2A.08 (beat markers: see N5) |
| Timeline: markers, in/out | 2A.02, 2A.10 |
| Timeline: copy/paste/duplicate | 2A.06 |
| Timeline: multi-select, linked-group selection | 2A.01, 2A.08, 2A.10 |
| Timeline: track lock, mute, solo and hide | 2A.02, 2A.09, 2A.10, 2E.02 |
| Timeline: zoom/scroll performance | 2A.09, 2J.02 |
| Timeline: replace clip | 2A.11 |
| (product owner request, 2026-11-24) Source marks, insert and overwrite from the Source view, match frame | 2A.14 (after P1's 1H.04, 1M.14, 1M.15) |
| Timeline: compound clips (nested sequences) | 2B.04, 2B.05 |
| Timeline: auto-create tracks on overlap | 2A.01, 2A.03 |
| Timeline: gesture soft-locks (§5.2) | 2A.12, 2A.08 |
| Sequences: multiple per project | 2B.01, 2B.03 |
| Sequences: settings with aspect presets | 2B.01, 2B.03 |
| Sequences: background fill (color, blur, image) | 2B.02, 2B.03 |
| Clip properties: transform, crop, opacity, flip, core blend modes (§8) | 2C.03 (model fields), 2C.04, 2H.03a, 2H.06 |
| (product owner request, 2026-11-24) Two or more videos side by side, top and bottom, picture in picture, grids | 2C.08 (with 2C.03, 2C.04, 2H.03a, 2H.06) |
| Keyframes for all animatable props with easing | 2C.01, 2C.02, 2C.03 |
| Keyframe lanes in the timeline | 2C.05 |
| Bezier graph editor | 2C.07 |
| Keyframe math shared with UI via wasm (§16) | 2C.06, 2A.07 |
| Speed: constant with pitch preservation | 2D.01, 2D.02 |
| Speed: reverse, freeze frame | 2D.01, 2D.02 |
| Audio: fades, keyframed volume, pan | 2E.01, 2E.05 |
| Audio: track buses, master limiter | 2E.02 |
| Audio: loudness normalization presets | 2E.03 |
| Audio: voiceover recording | 2E.04 |
| Audio: extract audio | 2E.03 |
| Audio: auto-ducking (sidechain from voice tracks) | 2E.06 |
| Audio: basic voice effects (pitch, robot, echo, telephone, chipmunk/deep) | 2E.07 |
| Media: proxies (auto policy + manual) | 2F.01, 2F.02, 2F.03 |
| Media: relink | 2F.03 |
| Media: collect & archive | 2F.04 |
| Media: image and image-sequence support, still duration defaults | 2F.05 |
| Media: bin search, tags, smart collections | 1M.12 (search, filters, sort), 2F.06a–c |
| Media: folders and importing folder structure (product owner request, ADR-0020) | 2F.07, 2F.08, 2F.09 |
| Settings: Editing, Timeline, Playback, Audio and Appearance pages; Project settings (product owner request; catalog `docs/reference/settings.md`) | 2A.13, 2H.10, 2E.08, 2H.09, 2H.08 |
| Export: presets (YouTube 1080p/4K, Shorts/Reels/TikTok, GIF, audio-only, ProRes; product owner request: Instagram Stories and feed, X, dated limits, safe areas) | 2G.01, 2G.02, 2G.04, 2H.06 |
| Export: HEVC/AV1 | 2G.02 |
| Export: advanced controls (CBR/VBR/CRF, GOP, color tags, audio codec/bitrate) | 2G.05, 2G.04 |
| Export: background queue | 2G.03a, 2G.04 |
| Export: range or marker set | 2G.03b, 2G.04 |
| UI: dockable workspace, Short-form/Long-form | 2H.01 |
| (product owner requests, 2026-11-24) UI: close and reopen panels (View > Panels); History panel | 2H.01, 2H.11 |
| UI: schema-driven inspector | 2H.02, 2H.03a, 2H.03b |
| (product owner request, 2026-11-25) UI: context menus on every surface | 2H.12, 2H.13, plus a menu bullet in 39 WPs (research [09-context-menus.md](../research/09-context-menus.md)) |
| UI: command palette | 2H.04 |
| UI: shortcut editor (NLE + CapCut presets) | 2H.05 |
| UI: preview overlays (transform handles, safe areas, grid) | 2H.06 |
| UI: timeline thumbnails and waveforms | 2A.09, 2E.05 |
| UI: i18n scaffold | 2H.01 |
| MCP: core tool set (~30 tools) | 2I.01 |
| MCP: `render_contact_sheet`, `media_filmstrip`, `audio_waveform` | 2I.02 |
| MCP: checkpoints | 2I.03 |
| MCP: `expected_rev` | 2I.04 |
| MCP: tool profiles | 2I.05 |
| MCP: hints for clients that defer tool definitions, the described profile (ADR-0040) | 2I.07 |
| MCP: discovery detail levels and concise results (ADR-0040) | 2I.08 |
| MCP: on-demand agent evaluation | 2I.09 |
| Agents: the command line with a skill | 2I.10 |
| Performance: benchmark suite in CI (timeline, scrub, playback, command RTT) | 2J.01, 2J.02, 2J.03, 2J.04 |
| Background render cache (smart pre-render while idle) | 2J.09 |

## Exit criteria → WPs

| Exit criterion | WPs |
|---|---|
| Dogfood 1: 10-min YouTube video with B-roll by a non-developer | 2J.06 (needs 2A.*, 2E.*, 2F.*, 2G.*, 2H.*), 2J.08 |
| Dogfood 2: 9:16 short with manual reframe, music and fades | 2J.07 (needs 2B.*, 2C.*, 2H.06, 2E.*), 2J.08 |
| Performance: timeline budget (≥ 55 fps at 5,000 items; open < 2 s; command RTT p95 < 16 ms) | 2A.09, 2J.01, 2J.02, 2J.04, 2J.08 |
| Performance: scrub budget (p95 < 80 ms proxy/intra, < 150 ms 4K long-GOP) | 2F.01, 2F.02, 2J.03, 2J.04, 2J.08 |
| Performance: playback budget (1080p60 0 dropped frames / 60 s; A/V < 40 ms) | 2C.04, 2D.02, 2E.02, 2J.03, 2J.04, 2J.09, 2J.08 |
| No data loss across 20 forced crashes | 2J.05, 2I.03 (checkpoint persistence), 2J.08 |

---

# Notes for integrator

Contradictions, ambiguities and missing decisions found while writing this phase, with the suggested resolution. No other docs were edited.

- **N1. RESOLVED.** No overlaps within any track; overlays are additional video tracks; a drop/move onto an overlap auto-creates a track above (§4.3). Property-tested in 2A.03 and 2A.12/2B.04 extensions.
- **N2. RESOLVED.** `item` namespace added (generic delete/move/set_props/duplicate/link); `clip` is for media-clip specifics (§5.1). Applied: `item.move`, `item.delete`, `item.duplicate`, MCP `item_move`.
- **N3. RESOLVED — `view.*` is exposed via the MCP catalog (not core) as `view_get`/`view_set`** (playhead, selection, in/out, zoom); non-undoable, not journaled. Applied in 2A.10 and 2I.01.
- **N4. RESOLVED.** Keyframe/easing/time/snapping math ships to the UI as a wasm module inside `packages/protocol` (§16), built by WP-2C.06. No per-frame RPC; `keyframe.eval` remains as a query for agents/tests only.
- **N5. Beat snapping vs. beat detection.** The roadmap lists "beats" as a snapping target, but beat detection arrives in P5 (analysis job). *Resolution:* P2 supports `Marker.kind='beat'` from manual tap markers and imported markers; automatic beat detection is P5. Confirm this is acceptable for M2.
- **N6. RESOLVED.** Core blend modes in P2: normal, multiply, screen, overlay, darken, lighten, add, soft light; extended set in P4 (§8).
- **N7. CapCut shortcut accuracy.** `research/01-capcut-features.md` may not list exact CapCut default bindings (they differ by platform and version). *Resolution:* ship the CapCut preset with only verified bindings, mark the rest `unverified`, and have a human check against the current CapCut Desktop before M2. *As shipped (WP-2H.05):* `keymaps/capcut.json` marks `unverified: true` on Redo's Ctrl+Shift+Z first order, Space for play and Delete for delete clip; only Ctrl/Cmd+B for split comes from the research (`01-capcut-features.md` [8]). `pnpm --filter @cutweave/editor check:keymaps` lists them. The NLE keys for razor, select tool, ripple trim, markers, nudge and ripple delete arrive with their commands, which do not exist yet.
- **N8. Proxy codec availability.** ProRes Proxy / DNxHR encoders in the pinned FFmpeg depend on build flags and the GPL build; Linux software `prores_ks` is slow. Add to P0 FFmpeg build requirements: `prores_ks`, `dnxhd`, `mjpeg`, `libsvtav1`, `libx265`, `gif`/palette filters. Proxy frame mapping for VFR sources (CFR conform + `pts_map`) is a design decision worth an ADR.
- **N9. Reference machines and self-hosted runners.** §19 budgets and the P0 "reference machines" need to exist physically (Windows/macOS arm64/Linux iGPU) with runner maintenance. This phase assumes they exist by 2J.04; owner and budget are a maintainer decision (suggest decide in P0 WP-0I).
- **N10. `VisualProps.background_fill` (§4.2) vs. sequence background (roadmap).** The model has a per-clip `background_fill?`; the roadmap only requires a sequence-level background. *Resolution:* implement sequence-level only in P2 and defer the per-clip blur-fill (it is the CapCut "canvas blur" per clip) to P4 or drop the field.
- **N11. RESOLVED.** Epic 2J renamed "Performance, reliability & dogfood"; crash harness and dogfood WPs stay there (the 05-work-breakdown.md epic list should show the new name).
- **N12. Roadmap typo.** "multi-cam-free YouTube video" is read as "single-camera (no multicam)". Confirm.
- **N13. Schedule realism.** About 162 agent-days across 66 WPs for a 9-week phase needs ≥ 6 parallel agents and a review bottleneck of ≤ 1 day per PR. Review capacity (1–2 humans) is the real constraint; the 2A.01 spec, 2A.03/04, 2C.02/04, 2E.02, 2F.01, 2I.03, 2A.12, 2B.04, 2E.06 and 2J.09 (opus) deserve human review, and the rest can be reviewed by opus plus CI. Consider moving 2G.02 AV1/HEVC HW paths, 2F.04 archive zip, 2E.07 voice effects and 2J.09 render cache to "stretch" if the dogfood date slips.
- **N14. RESOLVED.** Gesture soft-lock rule per §5.2: a `gesture` tx soft-locks touched items, other actors get `Conflict{reason:"item_in_gesture", retry_after_ms}`, other items proceed, 30 s idle auto-cancel; `proposal` txs have no idle timeout. A minimal soft-lock ships in P1; WP-2A.12 extends it (retry semantics, UI feedback) with property tests; UI side in 2A.08.
- **N15. Voiceover latency and permissions** differ per OS (macOS mic entitlement in the signed build, Windows privacy settings). The entitlements file belongs to P1/P3 packaging; 2E.04 only verifies in dev builds. Flag to 3I/7A.
- **N16. RESOLVED.** Compound clips are in P2 scope: nested sequence as media, cycle detection, recursive render (WP-2B.04, UI 2B.05).
- **N17. New open decisions from the added scope.** (a) Voice effects (2E.07) are rendered by a job and cached, so they are not live-adjustable; confirm "offline DSP" means this. (b) The render cache (2J.09) is preview-only in P2; export reuse is deferred for determinism. (c) `Track.role` (voice/music/sfx) is added for auto-ducking; confirm it belongs in the §4.2 Track model. (d) Compound clips need an ADR on `sequence.duplicate` semantics (shared vs deep-copied inner sequences); default chosen is shared.
- **N18. Settings, folders and bin search (planned 2026-11-24, product owner requests).** Every setting, with its default, scope, storage, agent access and owning WP, is in the catalog [docs/reference/settings.md](../reference/settings.md); P2 adds the Editing and Timeline (2A.13), Audio (2E.08), Appearance (2H.09) and Playback (2H.10) pages and the Project settings dialog (2H.08), and existing WPs add rows (2E.04, 2E.05, 2F.03, 2F.04, 2F.05, 2G.04, 2H.01, 2H.05, 2J.09). Folders are 2F.07 to 2F.09 under ADR-0020 (Proposed). 2F.06 is re-scoped: its search box and the name/kind/duration filters moved to 1M.12 (P1), `media.search` became filters on `media.list`, `collection.*` became `media.*` commands (no new namespace), and FTS5 is dropped unless the 10,000-asset target fails. Schema versions: 2F.07 takes v2; any other P2 migration (2F.06, 2A.02) chains after it in merge order.
- **N19. Product owner requests after using the app (planned 2026-11-24, second batch).** Verdicts and recommended phases are in P1's Notes for integrator, item 23. In this phase:
  - extended: WP-2H.05 (shortcut editor), WP-2G.01 and WP-2G.04 (platform presets: Instagram Stories and feed, X, dated limits, safe areas), WP-2H.01 (close and reopen panels), WP-2H.06 (safe areas shared with the presets, half and quarter guides), WP-2C.03 (adds `crop`, `flip` and `blend_mode` to `VisualProps`, which no WP owned);
  - new: WP-2A.14 (source marks and three-point editing, after P1's source preview, ADR-0022 Proposed), WP-2C.08 (layouts for several clips: `item.apply_layout`) and WP-2H.11 (History panel, `edit.undo`/`edit.redo` `{steps}`);
  - recommended pull: WP-2G.01 into P1 (S, dependencies merged, H.264 + AAC presets only).
  - Schema versions: 2C.03's `VisualProps` migration joins the chain after 2F.07's v2 (N18).
- **N20. Context menus (planned 2026-11-25, product owner request).** Nothing in the editor reacts to a right-click today; the design system's `Menu` exists (context mode) but is unused. Research, inventory and per-surface menus: [docs/research/09-context-menus.md](../research/09-context-menus.md). New: WP-2H.12 (framework) and WP-2H.13 (today's surfaces); a menu bullet in the owning WPs of P2 to P7; ADR-0026 (Proposed) fixes the surface IDs and `when` keys that plugins will depend on. Dependencies added: WP-2H.01 and WP-2H.04 depend on WP-2H.12; WP-2A.10 depends on WP-2H.13 (it is on the critical path, so merge 2H.12 and 2H.13 early, both need only P1). New small commands: `sequence.create_from_media` (2B.01), `item.set_label_color` and `timeline.update_marker` (2A.02), `item.reset_props` (2C.03), `media.import_frame` (2F.05). Host actions (reveal, copy text and image, save frame) are IPC with an exemption list, not commands. Arch §23 needs the ADR-0026 row when maintainers accept it. Owner decisions 2026-11-25 (defaults accepted): [09-context-menus.md](../research/09-context-menus.md) §7.6.
