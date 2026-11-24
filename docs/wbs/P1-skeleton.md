# Phase 1 — Walking Skeleton: Work Breakdown

> Source: [04-roadmap.md](../04-roadmap.md) Phase 1, contract: [01-architecture.md](../01-architecture.md) (§ numbers referenced as "arch §n"). WP template: roadmap bottom. IDs `WP-1<epic>.<nn>`.
> Sizing: S = 1 agent-day, M = 3 (2–4). Executors: `sonnet` default; `opus` for cross-cutting design, concurrency, GPU/media subtleties; `human` for decisions/review gates.
> **Production crates are built fresh** (`crates/*`); Phase 0 spike code (`spikes/`) is never imported. Learnings come from `docs/spikes/S*-report.md` and ADRs 001–015. Phase 0 outcome dependency: **M0 = GO** (WP-0J.03) for every WP below; ADRs are **accepted in WP-0J.02**, so WPs that consume an ADR depend on 0J.02. Duration estimate: **about 10 weeks** (was 7) after adding the transaction-ownership, engine-wiring and security work.

## Phase goal

The thinnest end-to-end product through every architectural layer, with production-quality foundations: import → cut → export, from the UI **and** from an MCP agent, on one command bus.

## Exit criteria (M1, copied from roadmap)

1. Through the UI: import 3 clips, arrange, trim, split, export a 1080p MP4 that plays correctly.
2. Through MCP headless: a scripted client does the same and exports an identical-timeline file.
3. Kill the engine mid-edit: the UI recovers with no lost commands.
4. CI is green on 3 OSes; golden frames pass; the parity test (CLI = MCP = RPC) passes.

## Tolerance policy (single source for this phase)

| Comparison | Threshold |
|---|---|
| Preview (full quality) vs pre-encode export frames, same adapter | **bit-identical** |
| Regression vs stored per-adapter golden reference | **SSIM >= 0.995 AND PSNR >= 40 dB** |
| Spike/implementation quality vs an independent (ffmpeg/CPU float) reference | **PSNR > 45 dB** (and dE2000 < 2 for color) |
| Decoded lossy MP4 vs rendered frame | PSNR >= 38 dB (encoder loss allowance) |

## Epics (one-line goals)

| Epic | Goal |
|---|---|
| **1A** Domain model | `cutweave-model` v1: IDs, flicks time, entities (project/media/sequence/track/clip), schema v1 + migrations |
| **1B** Command bus, tx & undo | `cutweave-commands`/`-derive`/`cutweave-ops`: registry, doc actor, invertible patches, undo/redo, actors, revisions, tx ownership + soft-locks, deterministic IDs, v1 command set, property tests |
| **1C** Persistence & recovery | Project folder, atomic save, journal, autosave, backups, lock file, crash recovery |
| **1D** Media layer | `MediaBackend`/FFmpeg: probe, index, decode (CPU + HW), caches, thumbnails, waveforms, minimal job manager, cache and hardware settings |
| **1E** Export | H.264/AAC MP4 export (HW + software) through the render pipeline as a job |
| **1F** Render pipeline v0 | Timeline eval → single-layer wgpu compositor, linear light, quality ladder, frame readback |
| **1G** Audio playback v0 | Mixer, cpal output, master clock, volume, offline mixdown |
| **1H** Playback & SHM preview | Transport, scrub scheduler, JKL, SHM ring publisher, napi bridge reader |
| **1I** Local API server | axum server, credential classes (UI/agent), Host/Origin, run-file, WS JSON-RPC + events, media HTTP, engine root + subsystem wiring |
| **1J** MCP v0 | rmcp server (HTTP + stdio attach/headless), 15 core + 4 generic tools (19 listed), `render_frame`, `job_wait`, `cutweave_execute`, per-endpoint profiles, agent targeting and the Agents and MCP settings |
| **1K** CLI | `cutweave engine | mcp | exec | query | render` |
| **1L** Electron shell & supervision | Hardened Electron app, engine spawn/supervise/restart/resync |
| **1M** Editor UI v0 | Layout, stores, media bin (with search), preview, Canvas2D timeline, inspector, undo/redo, save/open, export dialog, settings framework |
| **1N** Codegen & protocol | JSON Schema + TS types + typed client generated, snapshot-gated in CI |
| **1O** CI quality gates | Golden frames, CLI=MCP=RPC parity, MCP conformance, M1 exit e2e |

## Critical path

1. **Command spine:** `1A.01 → 1A.02 → 1B.01 → 1B.02 → 1B.03 → 1B.04 → 1B.09 → 1B.05 → 1I.04 (engine root) → 1I.02 → 1N.02 → 1M.02 → 1M.05 → 1M.06 → 1M.07 → 1M.08 → 1O.04` (the UI cannot do real work until RPC + generated client exist; start `1M.01` and `1L.*` early against a mock).
2. **Media/render/wiring spine (parallel):** `1D.01 → 1D.02 → 1D.03 → 1F.02 → 1F.03 → 1I.05 (subsystem wiring) → 1J.03 / 1K.01 → 1O.03 → 1O.04`; export branch `1F.03 → 1E.03 → 1M.08 / 1J.01`.
3. **Playback spine (parallel):** `1G.01 → 1G.02 → 1H.01 → 1H.02 → 1H.03 → 1M.04` (joins 1I.05).
4. **Recovery spine:** `1B.03 → 1C.02 → 1I.04 → 1L.03 → 1O.04` (exit criterion 3).
5. **Security spine:** `1L.01 → 1L.02 → 1L.03` with `1I.01` credential classes.
Suggested wave plan (5 waves, ~10 weeks): W1 `1A, 1B.01/02/08, 1D.01, 1L.01, 1M.01, 1N.01 stubs`; W2 `1B.03–05, 1B.07, 1C.01, 1D.02–03, 1F.01, 1G.01, 1I.01`; W3 `1B.06, 1B.09, 1C.02–04, 1I.02–04, 1D.04–07, 1F.02–03, 1G.02, 1N`; W4 `1I.05, 1H, 1E, 1J, 1K, 1L.02–03, 1M.02–06`; W5 `1M.07–08, 1O`.
Human gates: PR review of `1B.01`, `1B.03`/`1B.09` (concurrency + ownership core), `1I.01` and `1L.02` (security), `1O.04` (M1 acceptance demo).

---

## 1A — Domain model (`cutweave-model`)

### WP-1A.01 — IDs, flicks time, frame-rate math
- **Area:** `crates/cutweave-model`
- **Depends on:** WP-0A.01, WP-0J.01 (ADR-004)
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - `Time(i64)` newtype in flicks (705,600,000/s) with checked arithmetic (`Add/Sub`, saturating variants explicit), `Duration`, `FrameRate{num,den}` (rational), `SampleRate`.
  - Allowed conversions only: `frame_to_time`, `time_to_frame(Rounding::{Floor,Nearest})`, `snap_to_frame`, `samples_to_time`/`time_to_samples`.
  - Parser/formatter for `"00:01:02:03"` (timecode at a given fps, drop-frame for 29.97/59.94), `"62.5s"`, and raw flicks integer; `Display` round-trips.
  - Typed IDs: `MediaId/SequenceId/TrackId/ItemId` as UUIDv7 strings with prefix `med_/seq_/trk_/itm_`; `new()` (time-ordered), `FromStr` rejects wrong prefix; `IdGen` trait with `Uuid7Gen` (default) and `SeededGen(seed)` (deterministic, used by `--deterministic-ids`, see WP-1B.08).
  - `serde` + `schemars` + `specta` derives; **wire format per arch §4.1 / ADR-015**: `Time` serializes as a JSON integer of flicks, exports to TS as `number` via specta type override, deserialization rejects non-safe integers (|v| > 2^53-1) and accepts timecode strings (`"00:01:02:03"`, `"62.5s"`) at API inputs; crate is `no_std`-friendly-ish: must pass `cargo xtask check-wasm`.
- **Interfaces:** `cutweave_model::{Time, Duration, FrameRate, SampleRate, MediaId, SequenceId, TrackId, ItemId, IdGen}`.
- **Acceptance criteria:**
  - Exhaustive test over frame rates {23.976, 24, 25, 29.97, 30, 50, 59.94, 60, 120} and sample rates {44.1k, 48k, 96k}: `frame_to_time` / `time_to_frame` round-trip exactly for frames 0..1,000,000 and 1 flick = exact integer ratios (no float).
  - Overflow cases return `Err`, never wrap (test with `i64::MAX`).
  - `cargo nextest run -p cutweave-model` and `cargo xtask check-wasm` pass.
- **Tests:** unit + `proptest` (parse∘format = identity for random timecodes; ordering of UUIDv7 IDs by creation).
- **Out of scope:** entities (1A.02), speed ramps.

### WP-1A.02 — Entities v1 (project, media, sequence, tracks, clips)
- **Area:** `crates/cutweave-model`
- **Depends on:** WP-1A.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Types per arch §4.2 restricted to v1: `Project{meta,settings,media,sequences}`, `MediaAsset{id,kind,path_abs,path_rel,content_hash,probe}` (+`ProbeInfo` with streams, duration, fps, vfr, rotation, color tags, audio rate/channels), exactly one `Sequence` in v1 UI but map-typed (`Map<SequenceId,Sequence>`), `Track{kind: Video|Audio, items, name, muted, hidden, locked, gain}` (other kinds are enum variants returning `Unsupported` in ops), `Item::Clip{media, source_in, duration/start, enabled, audio{gain}, visual{transform{position,scale,rotation,anchor}, opacity}}`.
  - Use `imbl::{OrdMap,Vector}` for O(1) structural-sharing snapshots; tracks hold `items` sorted by start with `ItemId → Item` map for O(log n) lookup. `Animatable<T>` type exists with only `Static(T)` constructible in v1 (Keyframes variant present, `#[non_exhaustive]`).
  - `Project::validate()` invariants: IDs referenced exist, items on a track sorted and non-overlapping, `source_in + duration ≤ media duration` (if known), sequence format sane.
  - Plugin data only via namespaced `custom: Map<String, Value>` (arch §4.3) — present but ops don't touch it.
  - JSON (de)serialization stable ordering (`BTreeMap`-like output) so saved files diff cleanly.
- **Interfaces:** `cutweave_model::{Project, MediaAsset, ProbeInfo, Sequence, Track, TrackKind, Item, Clip, VisualProps, Animatable, Invariant}`.
- **Acceptance criteria:**
  - `Project::default()` serializes → deserializes → equals; two `clone()`s share structure (benchmark: clone of a 10k-item project < 100 µs, asserted with a coarse test threshold of 5 ms).
  - `validate()` rejects each of 6 hand-crafted corrupt fixtures with the expected `Invariant` variant.
  - `cargo xtask check-wasm` passes (no non-wasm deps).
- **Tests:** unit; proptest generator `arb_project()` (reused by 1B.07, exported under `cutweave-model` feature `testing`).
- **Out of scope:** Text/Caption/Composition items, transitions, keyframes, effects, masks.

### WP-1A.03 — Schema versioning + migration framework
- **Area:** `crates/cutweave-model`
- **Depends on:** WP-1A.02
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - `schema_version: u32` in the project document (`1`), `Migration` trait + ordered registry `vN → vN+1` operating on `serde_json::Value`, runner `migrate_to_current(Value) -> Result<(Project, MigrationReport)>`.
  - Newer-than-supported version ⇒ `OpenOutcome::ReadOnly{reason}`; unknown fields preserved under `custom`/`extra` round-trip (no silent drop).
  - Golden fixtures dir `crates/cutweave-model/tests/fixtures/v1/*.cutweave` (3 files: empty, 3-clip, edge-case) + a dummy `v0→v1` migration with its own fixture proving the framework; CI check that every `schema_version` has a fixture.
  - `cargo xtask new-migration` scaffold prints the 3 files to add.
- **Interfaces:** `migrate_to_current`, `OpenOutcome`, `CURRENT_SCHEMA_VERSION`.
- **Acceptance criteria:**
  - Test loads each fixture and asserts `Project` equals the expected value; a fixture with `schema_version: 999` returns `ReadOnly`.
  - A `build.rs`/test fails if `CURRENT_SCHEMA_VERSION` has no fixture dir.
- **Tests:** unit + insta snapshot of migrated JSON.
- **Out of scope:** real historical migrations.

---

## 1B — Command bus, transactions & undo (`cutweave-commands`, `-derive`, `cutweave-ops`)

### WP-1B.01 — Core traits, registry, `Tx` and patch ops
- **Area:** `crates/cutweave-commands`
- **Depends on:** WP-1A.02, WP-0J.02 (ADR-009 accepted)
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Traits per arch §5.1: `Command { type Output; fn apply(&self, &mut Tx) -> Result<Output, CmdError> }`, `Query { type Output; fn run(&self, &QueryCtx) -> Result<Output, CmdError> }`, `CommandMeta{name, summary, undoable, scope: Scope(Any|UiOnly), mcp: McpExposure(Core|Catalog|Hidden), annotations{destructive,idempotent,read_only}, input_schema, output_schema}`.
  - `Registry` (static registration via `inventory`/`linkme` as chosen in S5) with `register`, `get(name)`, `list()`, `invoke_json(name, Value, &mut Tx)`.
  - `CmdError` enum `{NotFound{kind,id}, Locked{item}, Conflict{current_rev}, Invalid{path,msg}, Unsupported, Internal}` each with machine `code()` and human `message()`, serializable.
  - `Tx`: mutation API on the doc (`insert_item`, `remove_item`, `set_field(path, value)`, `insert_media`, …) that records **forward and inverse `PatchOp`s**, JSON-Pointer-addressed on the ID-based document (`/media/{id}`, `/sequences/{sid}/tracks/{tid}/items/{iid}/…`), ops: `Add{path,value}`, `Remove{path,old}`, `Replace{path,old,new}`, `Move`. `apply_patch(doc, ops)` pure function; `invert(ops)`.
  - Semantic validation hook before apply (IDs exist, locks, ranges) returning `CmdError::Invalid`.
- **Interfaces:** `cutweave_commands::{Command, Query, Registry, CommandMeta, CmdError, Tx, PatchOp, Patch}`; wasm-safe.
- **Acceptance criteria:**
  - For a hand-written test command, `apply_patch(apply_patch(doc, forward), invert(forward)) == doc`.
  - Registry rejects duplicate names at startup (test) and names not matching `^[a-z]+\.[a-z_]+$` (arch §5.1 naming).
  - `cargo xtask check-wasm` and `cargo nextest run -p cutweave-commands` pass.
- **Tests:** unit; proptest for patch invert on random JSON edits.
- **Out of scope:** macro (1B.02), actor/undo stack (1B.03/04), real ops.

### WP-1B.02 — `#[command]` / `#[query]` derive macros
- **Area:** `crates/cutweave-commands-derive`
- **Depends on:** WP-1B.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Productionize the S5 macro design: attribute args `name, summary, undoable, scope (any|ui_only), mcp, annotations(...)`; generates `CommandMeta`, registry registration, `schemars::JsonSchema`-based input/output schema, type-erased `invoke_json`, doc comments → schema `description`s.
  - Compile-time checks: name format, missing summary, `undoable` on a query, `mcp="hidden"` combos; helpful error spans.
  - `view.*` namespace commands marked `#[command(..., view)]`: **not undoable, not journaled** (arch §5.1), persisted separately in `.cutweave/ui-state.json` (WP-1C.04); not part of `cutweave_execute` transactions.
- **Interfaces:** `#[command(...)]`, `#[query(...)]` as in arch §5.1 example (`clip.split` compiles verbatim).
- **Acceptance criteria:**
  - The `SplitItem` example from arch §5.1 compiles and registers; `registry.get("clip.split").meta.input_schema` snapshot matches (insta).
  - `trybuild` compile-fail tests: ≥ 6 malformed attribute cases produce the expected diagnostics.
  - `cargo nextest run -p cutweave-commands-derive -p cutweave-commands`.
- **Tests:** trybuild + insta.
- **Out of scope:** TS/MCP/CLI generation (1N, 1J).

### WP-1B.03 — Single-writer document actor, snapshots, revisions, events
- **Area:** `crates/cutweave-commands`
- **Depends on:** WP-1B.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - `DocActor` running on a dedicated thread (not tokio) with an mpsc command inbox; handle `DocHandle{ submit(cmd, actor, expected_rev) -> Future<Result<Applied>>, snapshot() -> Arc<DocSnapshot>, committed_snapshot() -> Arc<DocSnapshot>, subscribe() -> EventStream }`; `submit` also carries the originating `ConnId` (used by 1B.09 transaction ownership).
  - `Arc` swap (e.g. `arc-swap`) of immutable `DocSnapshot{rev: u64, committed_rev: u64, doc: Project}` per revision; readers never lock. **`committed_snapshot()` is the last COMMITTED rev only** (excludes the live effects of an open gesture/proposal tx); saves, autosave and journaling use it exclusively.
  - `expected_rev` check → `CmdError::Conflict{current_rev}`. `rev` increments per committed tx (not per command inside a tx).
  - Events: `DocPatched{rev, actor, tx_id, ops}` broadcast to subscribers (bounded channel; slow subscriber gets `Lagged` and must resync via snapshot). `Actor` enum `{User, Agent(label), Plugin(id), System}`; **the actor is derived by the engine from the authenticated credential** (UI credential → `User`, agent token → `Agent(<token label>)`, plugin → `Plugin(<id>)`, internal jobs → `System`); `submit` takes a `Credential`/`ActorCtx`, never a client-asserted actor.
  - **Idempotency (arch §5.1):** every submit carries a client-generated `request_id` (UUIDv7; the engine generates one for clients that omit it, e.g. CLI/MCP). The engine keeps a dedup table `request_id -> Applied` for a **10-minute window** (**wall-clock timestamps** via an injectable `Clock`, persisted per transaction in the journal so the window survives engine restarts); a repeat within the window returns the original result without re-applying and without bumping `rev`. The table is persisted via the journal (1C.02) so it survives restart.
  - Panic in a command → tx rolled back, error returned, actor keeps running (catch_unwind), event `CmdPanicked` logged.
- **Interfaces:** `DocHandle`, `DocSnapshot`, `Applied{rev, output, ops}`, `DocEvent`, `Actor{User, Agent, Plugin, System}`, `ConnId`, `Clock`.
- **Acceptance criteria:**
  - Stress test: 8 threads submit 10,000 commands total; final doc equals sequential replay of the recorded order; no deadlock (test timeout 30 s); `loom` or `shuttle` model test on the submit/snapshot-swap path passes.
  - A reader holding an old snapshot sees no mutation after later commits.
  - While a gesture tx is open with uncommitted changes, `committed_snapshot()` still equals the pre-tx doc and `rev`/`committed_rev` diverge as specified (test).
  - Stale `expected_rev` returns `Conflict` with the correct current rev.
  - Submitting the same `request_id` twice (second after 9 min of fake time) applies once and returns identical output; after 11 min it is treated as new (test).
  - Round-trip latency for a no-op command on the actor p95 < 1 ms (criterion bench recorded; feeds arch §19 command RTT budget).
- **Tests:** unit, concurrency (loom/shuttle), criterion bench.
- **Out of scope:** undo stacks (1B.04), journal (1C.02).

### WP-1B.04 — Transactions, undo/redo, per-actor history
- **Area:** `crates/cutweave-commands`
- **Depends on:** WP-1B.03
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Transaction API: `edit.begin_tx{label}`, `edit.commit_tx`, `edit.cancel_tx` (rollback by applying inverse ops), implicit single-command tx otherwise; **tx kinds** (`edit.begin_tx{kind:"gesture"|"proposal", label}`): a `gesture` tx live-updates the doc (events emitted per update), collapses into one undo entry on commit, rolls back on cancel, and **auto-cancels after 30 s without activity**; a `proposal` tx (used by the copilot in P6) has no idle timeout, can be cancelled, and likewise commits as one undo entry. Ownership and soft-locks: WP-1B.09.
  - Per-actor history stacks (`undo`/`redo`), commands `edit.undo{actor?}`, `edit.redo{actor?}`, query `edit.history{limit,cursor}`; undo of actor A's last tx applied only if it still applies cleanly: if later edits by another actor conflict, return `CmdError::Conflict` (documented rule), never corrupt the doc.
  - New redo branch cleared on new edit by that actor.
  - Undo/redo themselves produce `DocPatched` events with `actor` and `cause: Undo|Redo`.
  - Checkpoints are **not** in P1 (see integrator note).
- **Interfaces:** `edit.*` commands (registered via 1B.02); `HistoryEntry{tx_id, actor, label, rev}`.
- **Acceptance criteria:**
  - Scenario tests: apply 50 random commands → undo all → doc equals initial; redo all → doc equals final (`Project` equality).
  - Gesture tx with 200 updates yields exactly 1 history entry; cancel restores pre-tx doc; `DocPatched` events were emitted for live updates; a gesture tx idle for 30 s (fake clock) is auto-cancelled and the doc restored.
  - Two actors interleave edits; each `edit.undo{actor}` reverts only its own last tx or returns `Conflict` (3 explicit scenarios).
- **Tests:** unit + scenario; property tests live in 1B.07.
- **Out of scope:** checkpoints, history persistence across restarts (journal replay reconstructs it in 1C.02).

### WP-1B.09 — Transaction ownership, connection-drop rollback, minimal soft-lock
- **Area:** `crates/cutweave-commands`
- **Depends on:** WP-1B.04
- **Size:** M
- **Executor:** opus
- **Scope:**
  - **A tx is owned by the connection (`ConnId`) that opened it.** Only the owner may add commands to, commit or cancel it (others get `CmdError::Locked{by}`); when the owning connection drops (WS close, MCP session end, stdio proxy exit) the engine **rolls the tx back** immediately (inverse ops, `DocPatched` event with `cause: Rollback`). Per-connection limit: one open tx.
  - **Minimal soft-lock:** while a tx is open, items it has touched are soft-locked for other actors/connections: their commands on those items fail `Locked{item, by_actor, tx_id}` (non-blocking, no waiting); locks release on commit/cancel/rollback. Commands on untouched items proceed. Needed because UI gestures and an attached MCP agent coexist in P1.
  - Tx kind behaviours from 1B.04 (gesture idle timeout 30 s using the injectable `Clock`; proposal: no timeout, explicit cancel) enforced here; `edit.cancel_tx` by owner only.
  - Saves/autosave use `committed_snapshot()` (1B.03): an open tx never leaks into a save, and a crash with an open tx loses only the uncommitted tx (it is never journaled until commit).
  - Events: `tx.opened{tx_id, kind, actor, items}`, `tx.closed{tx_id, outcome}` for UI "agent is editing" indicators.
- **Interfaces:** `ConnId`, `TxKind`, `CmdError::Locked{by, tx}`, events above.
- **Acceptance criteria:**
  - Test: connection A opens a gesture tx moving item X; connection B's `clip.trim` on X returns `Locked`, on Y succeeds; after A drops, X is restored to its pre-tx state and B's retry succeeds.
  - Save during an open gesture tx writes a document equal to the pre-tx committed doc (byte-compare) and journals nothing for the tx.
  - 100 random open/commit/cancel/drop interleavings (proptest with 3 connections) never leave a dangling lock or a doc differing from a replay of committed txs.
- **Tests:** unit + proptest + loom/shuttle for the ownership map.
- **Out of scope:** CRDT merging, lock UI beyond the events, hard/blocking locks.

---

### WP-1B.05 — Ops v1a: project, media, track, insert
- **Area:** `crates/cutweave-ops`
- **Depends on:** WP-1B.02, WP-1B.04, WP-1A.02
- **Size:** M
- **Executor:** sonnet
- **Scope:** implement with `#[command]`/`#[query]` (namespaces per arch §5.1):
  - `project.new{name, format?}` (creates one sequence with default 1920×1080@30, 48 kHz, 1 V track + 1 A track), `project.info` (q), `project.rename`.
  - `media.register{media_id?, path, probe, content_hash}` (internal to import; **import orchestration** lives in 1D.07 job and calls this), `media.remove{id}` (fails with `Invalid` if referenced), `media.list{limit,cursor}` (q).
  - `track.add{sequence, kind}`, `track.set{track, muted?, hidden?, locked?, gain?}`.
  - `timeline.insert{sequence, track, media, at, source_in?, duration?}` (creates linked V+A clips when media has both streams and both tracks exist — behaviour documented: `linked_group` field set; **overlap policy:** fails `Invalid{path:"at"}` if overlapping; no ripple in P1), `timeline.get{sequence, depth, fields}` (q).
  - Honour `locked` → `CmdError::Locked`.
- **Interfaces:** command names above; `Output` types derive `JsonSchema`+`specta::Type`.
- **Acceptance criteria:**
  - Each command has ≥ 5 unit tests (happy path, NotFound, Invalid, Locked, undo identity).
  - `timeline.get` with `depth=1` returns summaries only (< 2 KB for a 100-clip timeline, asserted).
  - `cargo nextest run -p cutweave-ops` passes; registry lists every command with non-empty summaries (test).
- **Tests:** unit + hooked into 1B.07 property harness automatically (registry-driven).
- **Out of scope:** import job (1D.07), ripple/magnetic behaviours (P2).

### WP-1B.06 — Ops v1b: clip edits (split, trim, move, delete, props) + item queries
- **Area:** `crates/cutweave-ops`
- **Depends on:** WP-1B.05
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - `clip.split{item, at}` → `{left,right}` (arch §5.1 example; new IDs from `IdGen`; linked partner split too; `at` must be strictly inside item), `clip.trim{item, edge: Start|End, to}` (adjusts `source_in`/duration; cannot exceed media bounds or overlap neighbour), `clip.move{item, to_track?, at}` (same-kind tracks only; overlap check; linked group moves together unless `unlink:true`), `item.delete{items[]}` (linked partners deleted too; no ripple), `item.set_props{item, props:{opacity?, transform?, audio_gain?, enabled?}}`.
  - Queries: `item.get{id, fields?}`; `timeline.get` extended with items.
  - Edit semantics specified in `docs/reference/edit-semantics.md` (short, with ASCII diagrams) — it is the contract for the UI timeline (1M.06) and tests.
- **Interfaces:** `clip.split`, `clip.trim`, `clip.move`, `item.delete`, `item.set_props`, `item.get`.
- **Acceptance criteria:**
  - Split at a frame boundary produces two clips whose durations sum to the original and whose `source_in` sequences are contiguous (test); split at start/end → `Invalid`.
  - Trim/move rejects overlap with `Invalid{path}`; moving a locked track's clip → `Locked`.
  - `cargo nextest run -p cutweave-ops` ≥ 40 tests in this WP; all commands undo to identity.
- **Tests:** unit + registry-driven property tests (1B.07).
- **Out of scope:** ripple/roll/slip/slide, speed, keyframes.

### WP-1B.07 — Property-test harness: undo/redo identity, round-trip, invariants
- **Area:** `crates/cutweave-ops/tests`, `crates/cutweave-commands`
- **Depends on:** WP-1B.06, WP-1A.03, WP-1B.09
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Registry-driven generator: `arb_command_for(&Project) -> impl Strategy<(name, Value)>` producing valid-ish args (IDs drawn from the doc, times on frame grid) for **every registered non-view command** plus deliberately invalid args; adding a command without a strategy fails a test (`missing property strategy for <name>`).
  - Properties (arch §20): (1) `undo(apply(doc)) == doc` for each successful command; (2) `redo(undo(apply)) == apply`; (3) failed commands leave doc and rev unchanged; (4) `serialize → deserialize` round-trip equality after random sequences of ≤ 100 commands; (5) invariants after every step: `Project::validate()` passes, no overlaps on tracks, linked groups intact; (6) patch events replayed on an empty-mirror reproduce the doc (the UI docStore contract).
  - `PROPTEST_CASES` default 256 in CI, 4096 nightly; failing seeds persisted to `proptest-regressions/` (committed).
- **Interfaces:** `cutweave_ops::testing::{arb_command_for, replay_events}` behind feature `testing`.
- **Acceptance criteria:**
  - `cargo nextest run -p cutweave-ops --features testing` passes with 256 cases; property (1) has been shown to catch a seeded bug (a mutation test in the PR: break the inverse of `clip.trim`, test fails, revert).
  - Strategy coverage report lists ≥ 95% of registered command names as exercised.
- **Tests:** this WP is the tests.
- **Out of scope:** fuzzing (P7).

### WP-1B.08 — Deterministic-ID test mode (`--deterministic-ids <seed>`)
- **Area:** `crates/cutweave-model`, `crates/cutweave-commands`, `crates/cutweave-engine`, `crates/cutweave-cli`
- **Depends on:** WP-1A.01, WP-1B.03
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - `SeededGen(seed: u64)` `IdGen`: valid prefixed UUIDv7-shaped IDs from a counter-based PRNG (ChaCha8 from seed, fixed timestamp bits), one stream per engine. All ID creation in `cutweave-ops`/`Tx` goes through the injected `IdGen` (a grep-based test forbids direct `Uuid::now_v7()` outside `Uuid7Gen`).
  - Engine/CLI flag `--deterministic-ids <seed>` (+ env `CUTWEAVE_DETERMINISTIC_IDS`) plumbed into `EngineConfig`; it also freezes `meta.created` and other timestamps written into project documents and the auto-generated `request_id`s, so two identical command scripts yield **byte-identical saved project documents** (arch §5.1).
  - In `Gui` mode the flag is refused unless `--i-know-this-is-a-test` is passed.
  - The journal records resulting IDs (1C.02), so replay is exact with or without the flag.
- **Interfaces:** `SeededGen`, `EngineConfig::deterministic_ids: Option<u64>`, CLI flag.
- **Acceptance criteria:**
  - The same script run in two fresh engines with seed 42 gives identical `.cutweave` bytes (sha256 equal); seed 43 gives different IDs but identical structure after ID normalization.
  - The grep test passes; `cargo nextest run -p cutweave-ops -p cutweave-commands`.
- **Tests:** unit + double-run determinism test (reused by 1O.02/1O.04).
- **Out of scope:** cross-OS floating-point determinism.

---

## 1C — Persistence & recovery

### WP-1C.01 — Project folder format, atomic save/open, lock file
- **Area:** `crates/cutweave-engine` (module `project_io`), or `cutweave-model` for pure parts
- **Depends on:** WP-1A.03
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Folder layout per arch §17: `<Name>/<Name>.cutweave`, `generated/`, `compositions/`, `.cutweave/{journal,backups,lock}`.
  - `save(path, &Project)` (called with `committed_snapshot()` only; an open tx's uncommitted effects are never persisted): write temp file in same dir → `fsync` file → atomic rename → `fsync` dir (Windows: `ReplaceFileW`/`MoveFileEx` with write-through); never leaves a partial `.cutweave` (test by failpoint injection at each step).
  - `open(path)`: lock acquisition, `migrate_to_current`, `ReadOnly` handling, relative-path resolution of media (`path_rel` fallback when `path_abs` missing; report missing media in `OpenReport{missing_media[]}`; relink UI is P2).
  - **Lock file** `.cutweave/lock` containing `{pid, host, started}`; stale-lock detection (pid dead on same host ⇒ steal with log; live pid ⇒ `CmdError::Locked`), released on clean close and via `Drop`; second open of the same project in another process fails with a clear error.
  - Commands in `cutweave-ops` (thin): `project.open{path}`, `project.save{}`, `project.save_as{path}`, `project.close{}`; registered as non-undoable.
- **Interfaces:** `ProjectStore::{create, open, save, close}`, `OpenReport`, `LockInfo`.
- **Acceptance criteria:**
  - Failpoint test: killing the process (simulated by aborting after each of 4 steps) leaves the previous `.cutweave` intact and loadable (4 cases).
  - Two processes: second `project.open` gets `Locked`; after `kill -9` of the first, the stale lock is reclaimed.
  - Round-trip: save → open equals original `Project`, on Windows paths with spaces and unicode.
- **Tests:** unit + multi-process integration test (spawn helper bin).
- **Out of scope:** autosave, journal (1C.02/03), collect & archive, relink.

### WP-1C.02 — Command journal + replay + crash-recovery test
- **Area:** `crates/cutweave-engine` (module `journal`), hooks in `cutweave-commands`
- **Depends on:** WP-1B.04, WP-1C.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Append-only journal `.cutweave/journal/<base-rev>.jnl`: one record per **committed tx**: `{rev, tx_id, wall_clock_ts, actor, request_id (UUIDv7), label, commands:[{name,args}], result_ids, applied_output, crc32}` length-prefixed (`u32 len + payload + crc`); written by the doc actor **before** the tx is acked; write policy: `write()` to OS before ack, `fdatasync` group-commit every ≤ 50 ms and always on `project.save` (documented durability: survives process kill; ≤ 50 ms window on power loss).
  - Gesture tx: only the final collapsed command list is journaled on commit; cancelled ones not.
  - `view.*` commands are not journaled.
  - On save: journal rotates (new base rev), old journal kept until the next successful save, then deleted.
  - Recovery: `ProjectStore::open` detects non-empty journal newer than the `.cutweave` ⇒ returns `RecoveryAvailable{records, last_rev, last_ts}`; `recover()` replays commands (deterministic `IdGen` seeded from recorded IDs — the journal stores the **resulting IDs** in each record so replay is exact, see integrator note) through the real registry; torn/CRC-bad tail record is truncated with a warning; history stacks rebuilt so undo works after recovery.
  - **Dedup after restart (arch §5.1):** recovery rebuilds the 10-minute `request_id -> Applied` table from the journal using each record's **persisted wall-clock timestamp** (a record is within the window if `now - ts <= 10 min`, so the window survives restart), so a client resending an unacknowledged request after an engine restart gets the original result and never a duplicate.
- **Interfaces:** `Journal::{append, rotate, replay}`, `RecoveryAvailable`, commands `project.recover{}` / `project.discard_recovery{}` (non-undoable).
- **Acceptance criteria:**
  - **Crash-recovery test** (`cargo nextest run -p cutweave-engine --test crash_recovery`): helper process applies N=500 random ops via the registry, is `SIGKILL`ed/`TerminateProcess`ed at 20 random points; recovery reproduces a doc equal to "last acked rev" with **0 acked commands lost** across 20 trials, and ≤ 1 un-acked tx lost.
  - Truncating the journal mid-record at every byte offset of its final record (test loop) never causes a panic and recovers up to the previous record.
  - Undo after recovery reverts the last recovered tx.
  - **Resend test (exit 3):** in the 20 kill trials the helper also holds 5 in-flight (unacked) requests; after recovery they are re-submitted with the same `request_id`s and each is applied **exactly once** overall (acked-before-kill ones return the original result, unapplied ones apply once); final doc equals an uninterrupted run.
- **Tests:** multi-process crash test, truncation fuzz loop, proptest(replay(journal) == live doc).
- **Out of scope:** autosave timers/backups (1C.03), UI.

### WP-1C.03 — Autosave, rotating backups, recovery API
- **Area:** `crates/cutweave-engine`
- **Depends on:** WP-1C.02
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - Autosave timer: when doc dirty, save on idle (no commands for 2 s) and at least every 30 s (arch §17) via the atomic path; configurable in `settings.autosave{enabled, interval_s}`; runs on a background task, never blocks the actor (saves the **committed** snapshot only).
  - `.cutweave/backups/`: keep last N=10 timestamped copies (rotate on autosave only when content hash changed).
  - Events: `project.dirty{bool}`, `project.saved{rev,path}`, `project.recovery_available{…}` for UI.
  - Dirty flag semantics defined: `last_saved_rev != committed_rev`.
- **Interfaces:** `AutosaveService`, events above (registered in the event catalog for 1N).
- **Acceptance criteria:**
  - With a fake clock (tokio `pause`), autosave fires at 30 s of continuous edits and at 2 s idle; after 12 changed autosaves exactly 10 backups remain.
  - Autosave during a 500-command burst does not delay command acks by > 5 ms p95 (bench).
- **Tests:** unit with virtual time.
- **Out of scope:** recovery UI (1M.07).

### WP-1C.04 — `view` namespace commands and `ui-state.json` persistence
- **Area:** `crates/cutweave-ops`, `crates/cutweave-engine`
- **Depends on:** WP-1B.02, WP-1C.01
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - `view.set{patch}` (merge into `UiState{playhead, zoom, scroll, selection[], panel_layout (opaque JSON), preview_quality, master_gain}`) and query `view.get`; the transport commands of 1H.01 and `view.set_master_gain` (1G.02) belong to the same namespace. Not undoable, **not journaled**, no `rev` bump, no `doc.patched`; emits `view.changed` to other clients.
  - Persistence in `<project>/.cutweave/ui-state.json` (atomic write, debounced 500 ms, also on close and with each save); loaded on `project.open` and returned by `view.get`; unknown keys preserved; a corrupt file is ignored with a warning and never blocks opening.
- **Interfaces:** `view.set`, `view.get`, `UiState`, event `view.changed`.
- **Acceptance criteria:**
  - After `view.set{playhead, selection}`, closing and reopening the project restores both (test); `edit.history` and the journal file size are unchanged by `view.*` calls (test).
  - A corrupt `ui-state.json` yields defaults plus a warning event; the project opens.
- **Tests:** unit + integration.
- **Out of scope:** per-user (non-project) preferences.

---

## 1D — Media layer (`cutweave-media`, `cutweave-cache`, `cutweave-jobs`)

### WP-1D.01 — `MediaBackend` trait + FFmpeg build/link strategy
- **Area:** `crates/cutweave-media`, `xtask`, `ci/ffmpeg/`, `docs/dev/ffmpeg.md`
- **Depends on:** WP-0J.02 (ADR-003), WP-0A.06
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Trait `MediaBackend` (async-free, `Send+Sync`): `probe(path) -> ProbeInfo`, `open_decoder(path, DecodeOpts{hw: Auto|Off, target_pixfmt}) -> Box<dyn VideoDecoder>`, `open_audio_decoder(path, stream) -> Box<dyn AudioDecoder>`, `open_encoder(EncodeSpec) -> Box<dyn Encoder>`, `capabilities() -> Capabilities{hw_decode[], hw_encode[], codecs}`. Frame types: `VideoFrame{pts: Time, w, h, fmt: Nv12|P010|Rgba8|Yuv420p.., planes, color: ColorTags}`, `AudioChunk{pts, rate, channels, f32 planar}`. Error enum `MediaError{Unsupported, Corrupt, Io, HwUnavailable,…}`.
  - FFmpeg impl crate-internal module `ffmpeg/` using ffmpeg-next (or fork per S3 verdict), **pinned FFmpeg 8.1.x shared libs** obtained by **`just ffmpeg-fetch`** (the CI-built bundle from WP-0A.06; brew/distro libs permitted for quick local hacking per 06-dev-process §2), `FFMPEG_DIR` wiring in `.cargo/config.toml`; runtime lib search on 3 OSes; license note (GPL build allowed).
  - `FakeBackend` (in-memory synthetic frames/tones) in `cutweave-media::testing` so upstream crates are testable without FFmpeg.
  - CI caches the FFmpeg bundle; `docs/dev/ffmpeg.md` per-OS setup (updates CONTRIBUTING).
- **Interfaces:** `cutweave_media::{MediaBackend, VideoDecoder, AudioDecoder, Encoder, VideoFrame, AudioChunk, MediaError, Capabilities, FakeBackend}`.
- **Acceptance criteria:**
  - `just ffmpeg-fetch && cargo nextest run -p cutweave-media` passes on the 3 OSes in CI; `ffmpeg -version` banner of the bundled libs is logged and equals the pinned version.
  - `cutweave-render`, `cutweave-audio`, `cutweave-model` do **not** depend on `cutweave-media` internals beyond the trait crate (dep rule checker, arch §3).
  - `FakeBackend` decodes a 10 s synthetic clip at 30 fps with exact pts.
- **Tests:** unit; backend contract tests (`backend_contract!` macro run against both `FfmpegBackend` and `FakeBackend`).
- **Out of scope:** actual probe/decode logic (1D.02/03), HW.

### WP-1D.02 — Probe, frame index, content hash
- **Area:** `crates/cutweave-media`, `crates/cutweave-cache`
- **Depends on:** WP-1D.01, WP-0I.01
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - `probe`: streams, duration, fps/VFR detection (pts delta analysis), rotation (display matrix), color tags (primaries/transfer/matrix/range), audio rate/channels, codec profile; maps to `ProbeInfo` (1A.02). Errors on corrupt files are typed, never panic.
  - **Frame index**: packet scan building `Vec<IndexEntry{pts, dts, is_key, pos}>` + GOP map; serialized into `cache.sqlite` (`rusqlite` bundled; schema versioned; table `frame_index(content_hash, stream, blob)`); VFR sources mapped through the index (arch §4.1).
  - Content hash: BLAKE3 over first 1 MiB + last 1 MiB + size (documented fast-hash, used for relink in P2) with full-hash option; computed in the background.
  - `CacheDb` struct in `cutweave-cache` (open, migrations, LRU size accounting stub).
- **Interfaces:** `cutweave_cache::{CacheDb, FrameIndex}`, `MediaBackend::probe`, `index_media(path) -> FrameIndex`.
- **Acceptance criteria:**
  - On the corpus: probe output matches `ffprobe -show_streams -of json` for duration (±1 frame), fps, rotation, color tags for every file (script-compared test); VFR clip flagged `vfr=true`; broken MP4 returns `MediaError::Corrupt` within 2 s.
  - Index for 4K30 long-GOP 60 s clip builds in < 3 s (recorded, not gating) and equals `ffprobe -show_packets` keyframe pts list (test).
- **Tests:** corpus-driven integration tests (`--tier small`), unit tests with `FakeBackend`.
- **Out of scope:** scene detection, loudness.

### WP-1D.03 — Decoder: frame-exact seek, CPU path, decoder pool
- **Area:** `crates/cutweave-media`
- **Depends on:** WP-1D.02
- **Size:** M
- **Executor:** opus
- **Scope:**
  - `VideoDecoder::seek_exact(t: Time) -> VideoFrame` via keyframe seek + decode-forward using the frame index; `next_frame()`; frame pts always returned in flicks; proper handling of B-frame reorder, rotation metadata, odd dimensions, no-audio files.
  - CPU decode to `Nv12/P010/Yuv420p` planes (no conversion on CPU; GPU converts in 1F.02) with `ColorTags`; software scaler only for the fallback `Rgba8` path used by thumbnails.
  - `DecoderPool` per media (bounded, LRU eviction, max 4 live decoders by default) with request cancellation tokens.
  - `AudioDecoder::decode_range(start, dur) -> AudioChunk` resampled lazily by 1G.
- **Interfaces:** `VideoDecoder::{seek_exact, next_frame}`, `DecoderPool::{checkout, release}`, `CancelToken`.
- **Acceptance criteria:**
  - **500 random seeks per corpus video file (H.264 4K30, HEVC 10-bit, ProRes, VFR phone clip): 100% frame-exact** — returned pts equals the index pts for the requested time and Y-plane CRC equals ffmpeg reference — run as `cargo nextest run -p cutweave-media --test frame_exact -- --ignored` in nightly and `--tier small` subset (100 seeks) in PR CI.
  - 4K long-GOP random seek p95 < 150 ms on a reference machine (bench recorded, non-gating on CI runners).
  - Cancelling a pending seek returns within 10 ms.
- **Tests:** corpus integration, bench.
- **Out of scope:** HW decode (1D.04), cache (1D.05).

### WP-1D.04 — Hardware decode (CPU-upload path) with fallback
- **Area:** `crates/cutweave-media`
- **Depends on:** WP-1D.03
- **Size:** M
- **Executor:** opus
- **Scope:**
  - `DecodeOpts{hw: Auto}`: D3D11VA (+NVDEC where available) on Windows, VideoToolbox on macOS, VA-API on Linux via FFmpeg hwaccel; `av_hwframe_transfer_data` to NV12/P010 CPU frames (path (a) of S3). **Zero-copy is out of scope** (feature flag stub only, per ADR-003).
  - Auto-detect with a cached capability probe (`Capabilities.hw_decode`), per-codec; any init/decode failure on a stream transparently reinitializes software decode and sets `hw_failed` flag for the session (logged with `tracing::warn`).
  - Frame-exactness preserved: seek logic shared with 1D.03.
- **Interfaces:** `DecodeOpts::hw`, `Capabilities`.
- **Acceptance criteria:**
  - On a machine with HW decode: corpus H.264 4K30 and HEVC 10-bit decode via HW (`decoder.is_hw() == true`) and the 100-seek frame-exact test passes with pts identical to SW and Y-plane PSNR vs SW > 50 dB.
  - With `CUTWEAVE_HW=off` or a forced init error the same test passes via SW (CI default on runners without GPU).
- **Tests:** corpus integration (HW tests `#[ignore]` unless `CUTWEAVE_HW_TESTS=1`, run on reference machines).
- **Out of scope:** HW encode (1E.02), zero-copy.

### WP-1D.05 — Frame cache, thumbnails, filmstrips
- **Area:** `crates/cutweave-cache`
- **Depends on:** WP-1D.03, WP-1D.07
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - In-RAM decoded **frame cache** (LRU by bytes; key `(content_hash, stream, pts)`; default budget 1 GiB or 25% RAM, configurable) used by scrub/preview.
  - Thumbnail generator: `thumb(media, t, w) -> Jpeg/Webp bytes`; filmstrip: N tiles at even intervals → one sprite image + manifest `{tile_w, tile_h, count, interval}`; disk cache in the OS cache dir (`dirs`), content-hash addressed, size-capped LRU (default 2 GiB) tracked in `cache.sqlite`; ETag = content hash + params hash.
  - Generated lazily and as a background job (1D.07) after import; cancellation supported.
- **Interfaces:** `cutweave_cache::{FrameCache, ThumbCache::{thumb, filmstrip}}`.
- **Acceptance criteria:**
  - Second request for the same thumb performs 0 decodes (counter assertion); cache evicts oldest entries when over cap (unit test with 1 MB cap).
  - Filmstrip of a 60 s 1080p clip with 100 tiles generates in < 5 s on a dev machine (recorded).
  - Cache dir is outside the project folder; deleting it never breaks a project (test: delete, regenerate).
- **Tests:** unit with `FakeBackend`; corpus integration.
- **Out of scope:** proxies (P2).

### WP-1D.06 — Waveform pyramids
- **Area:** `crates/cutweave-cache`
- **Depends on:** WP-1D.03, WP-1D.07
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - Decode audio once, compute min/max peak pyramid (levels of 256, 1024, 4096… samples per bin, i8/i16 pairs per channel mixed to mono + per-channel optional); store as a compact file in the disk cache; `waveform(media, lod, range) -> PeakData`; render-to-PNG helper (used by MCP `audio_waveform` later, P2).
  - Runs as a job; progress events.
- **Interfaces:** `WaveformCache::{get, ensure}`; binary format documented in `docs/reference/waveform-format.md`.
- **Acceptance criteria:**
  - Peaks for a synthetic −6 dBFS sine at 1 kHz are within ±1 LSB of the expected amplitude at every LOD (test); level N+1 bins are exactly the min/max of level N pairs (property test).
  - 1 h of 48 kHz stereo audio computes in < 60 s (recorded).
- **Tests:** unit + proptest.
- **Out of scope:** LUFS.

### WP-1D.07 — Job manager v0 and import job
- **Area:** `crates/cutweave-jobs`, `crates/cutweave-ops`
- **Depends on:** WP-1D.02, WP-1B.05
- **Size:** M
- **Executor:** opus
- **Scope:**
  - `cutweave-jobs` v0: in-memory job registry persisted to `jobs` table in `cache.sqlite` for history only (resume across restarts is P5): `JobId`, `kind`, `state{Queued,Running,Succeeded,Failed,Cancelled}`, `progress{fraction,message}`, `CancelToken`; worker pool on tokio `spawn_blocking` with a **GPU/IO budget** semaphore (max concurrent decode jobs = cores/2; export = 1); event stream `job.progress`.
  - Commands/queries: `job.status{id}` (q), `job.list` (q), `job.cancel{id}`.
  - `media.import{paths[], dest?}` command: validates paths (exist, within allowed roots in headless/MCP mode), then **registers media synchronously**: probe each file (per-file timeout, default 5 s, up to 4 in parallel) + fast content hash, then `media.register` through the bus; returns `{media_ids[], failed[{path, error}]}` as soon as registration is done. Probe failure/timeout/corrupt file → entry in `failed[]` (no asset created).
  - **Analysis is asynchronous:** after registration it spawns per-asset jobs (index → thumbnails (1D.05) → waveform (1D.06)) and returns their `job_ids[]`; emits `media.indexed{media_id}` when the index is ready. A failing stage sets `analysis_error` on the (already registered) asset so the UI shows a badge.
  - Query `job.wait{job_id, timeout_ms}` (timeout capped at 60 s): returns the job's current/final state; returns early on completion, otherwise `{state:"running", progress}` at timeout. Exposed through RPC/CLI and, via 1J.01, as the MCP tool `job_wait`.
- **Interfaces:** `cutweave_jobs::{JobManager, JobHandle, JobKind}`, commands `media.import` (returns `media_ids`, `failed`, `job_ids`), `job.cancel`; queries `job.status`, `job.list`, `job.wait`; events `job.progress`, `media.indexed`.
- **Acceptance criteria:**
  - `media.import` of 3 corpus clips returns 3 `media_ids` in < 2 s (synchronous registration; `media.list` immediately shows them with `probe` populated); thumbnails/waveforms are retrievable after `job.wait` on the returned job ids (< 10 s). Import of the broken MP4 returns it in `failed[]` (no asset in `media.list`); a probe that exceeds the timeout (fake slow backend) is reported in `failed[]` within timeout + 100 ms.
  - `job.wait{timeout_ms:100}` on a running job returns `state:"running"` after ~100 ms; on a finished job returns immediately (test).
  - Cancelling a running import stops decoding within 500 ms.
  - `media.import` of a path outside allowed roots (MCP/headless mode) returns `Invalid{path}` (path-allowlist test).
- **Tests:** unit with `FakeBackend`; integration with corpus.
- **Out of scope:** persistent resumption, AI jobs.

### WP-1D.08 — Settings pages for this computer: Cache and storage, Performance and hardware
- **Area:** `crates/cutweave-cache` (limits, usage, clear), `crates/cutweave-media` (decode policy), `crates/cutweave-engine` (`--cache-dir`, engine commands, UI-only scopes), `crates/cutweave-cli` (headless flag), generated schema and TS types, `apps/desktop` (launch flag from the preference), `packages/editor` (the two pages), `docs/reference/launch-protocol.md`, `docs/reference/settings.md`, `apps/desktop/e2e/flows`, `docs/user-flows.md`
- **Depends on:** WP-1M.11, WP-1D.04, WP-1D.05, WP-1D.06, WP-1I.05
- **Size:** M
- **Executor:** sonnet
- **Scope:** (product owner request, 2026-11-24: settings cover everything users can customize) the "This computer" pages of the settings catalog (`docs/reference/settings.md`) for the caches and hardware P1 already has. Values are machine preferences (WP-1M.11's `machine` group: never exported).
  - **Cache and storage page:** the cache folder (folder picker; default the OS cache folder; main passes it as `--cache-dir` at engine start, so the row says "Applies the next time Cutweave starts" and the old folder is left for the person to delete); for thumbnails and filmstrips (2 GiB default, WP-1D.05) and waveforms (512 MiB default, now enforced with WP-1D.06's LRU) the usage and a limit; "Clear" per kind and "Clear all" (confirm). Clearing never touches projects or `generated/` media; caches rebuild on demand. Later WPs add their rows to this page: proxies (WP-2F.03), background renders (WP-2J.09).
  - **Performance and hardware page:** hardware decoding Auto or Off (WP-1D.04's policy; applies to decoders opened after the change, and open pools reopen on their next seek); memory for decoded frames, Auto (the smaller of 1 GiB and a quarter of RAM) or 512 MiB / 1 / 2 / 4 GiB; read-only information: the GPU adapter in use and the hardware encoders found (`media.capabilities`). The hardware encoding default is on the Export page (WP-1M.13).
  - **Engine surface:** query `media.cache_status` → `{folder, kinds:[{kind, bytes, limit_bytes, entries}], decode_hw, frame_cache_bytes}` (readable by agents); commands `media.set_cache_config{limits?, frame_cache_bytes?}`, `media.clear_cache{kinds}` and `media.set_decode_policy{hw: auto|off}`: engine commands, not undoable, not journaled, never dirty the project, and **UI-only scope** (ADR-0016; an agent token gets `forbidden`). A lower limit shrinks the cache at once (LRU). The editor applies the preferences after every handshake, like WP-1M.10.
  - **Headless engines** take `--cache-dir` (and `CUTWEAVE_CACHE_DIR`) and the defaults; documented in the launch protocol and the CLI reference.
- **Interfaces:** `media.cache_status` (query), `media.set_cache_config`, `media.clear_cache`, `media.set_decode_policy` (UI-only engine commands); `--cache-dir`; preference keys `cache.*`, `performance.hw_decode`, `performance.frame_memory`.
- **Acceptance criteria:**
  - Integration: setting a 50 MB thumbnail limit shrinks the disk cache below it within 1 s; `media.clear_cache{kinds:["thumbnails"]}` empties it and the next thumbnail request regenerates the image.
  - Integration: an agent token calling `media.set_cache_config`, `media.clear_cache` or `media.set_decode_policy` over RPC, MCP (`cutweave_execute`) and the CLI gets `forbidden`; `media.cache_status` answers agents.
  - With hardware decoding Off, a newly opened decoder is the CPU one (decoder-kind probe in the integration test).
  - **A user flow (ADR-0018, F23):** Settings > Cache and storage lists each cache kind with its usage and limit; after the bin has drawn thumbnails of the demo media (real decode), "Clear" on thumbnails drops their usage to zero; choose another cache folder (OS picker stubbed), relaunch: the page shows the new folder and the engine writes there. Performance and hardware shows the GPU name and the hardware decoding choice survives the relaunch.
- **Tests:** integration (engine, cache), unit (launch args), Vitest (pages), user flow.
- **Out of scope:** choosing the GPU adapter, per-project cache folders (Resolve-style working folders), moving an old cache, proxy and render-cache rows (their WPs).

---

## 1E — Export (H.264/AAC MP4)

### WP-1E.01 — Software encode + MP4 mux (H.264 + AAC)
- **Area:** `crates/cutweave-media`
- **Depends on:** WP-1D.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - `Encoder` impl via FFmpeg: libx264 (CRF / bitrate, preset, `yuv420p`, BT.709 tags, `-movflags +faststart`), AAC (`aac` or libfdk-free: native `aac` encoder, 48 kHz stereo, 192 kbps default), MP4 muxer with correct timebase and A/V interleave; input frames as `Rgba8`/`Nv12` (RGB→YUV via swscale with BT.709 limited range, tagged accordingly).
  - Constant frame rate from the sequence's `FrameRate` rational; pts in flicks converted at the encoder boundary only.
  - Progress callback, cancel token; partial file removed on cancel/failure.
- **Interfaces:** `EncodeSpec{container: Mp4, video: H264{crf|bitrate,preset}, audio: Aac{bitrate}, size, fps, color}`, `Encoder::{push_video, push_audio, finish}`.
- **Acceptance criteria:**
  - Encoding synthetic 5 s 1080p30 frames + a 440 Hz tone yields an MP4 where `ffprobe` reports h264/aac, 1920×1080, 30 fps (r_frame_rate exactly 30/1), duration 5.00 s ±1 frame, A/V start offset < 40 ms, `colr` tags BT.709.
  - Cancel mid-encode leaves no file; error paths tested (disk full simulated via failing writer).
  - Plays in `ffplay`/VLC and Chromium `<video>` (manual checklist item once per OS in PR; automated: `ffmpeg -v error -i out.mp4 -f null -` exits 0 with empty stderr).
- **Tests:** unit + ffprobe-based integration tests.
- **Out of scope:** HW encoders (1E.02), HEVC/AV1, presets (P2).

### WP-1E.02 — Hardware encoders with software fallback
- **Area:** `crates/cutweave-media`
- **Depends on:** WP-1E.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - H.264 HW encoders: NVENC, AMF, QSV (Windows/Linux), VideoToolbox (macOS), VA-API (Linux) via FFmpeg; capability probe by attempting to open a 256×256 test encode (cached per session); selection order per OS documented; `EncodeSpec.hw: Auto|Off|Force(name)`.
  - Bitrate-control mapping: our single "quality" setting → CRF for x264, `-cq`/`-global_quality`/`-q:v` per HW encoder tuned to roughly equal VMAF/PSNR (table in `docs/reference/export-quality.md` with measured PSNR vs x264 CRF 20 on a corpus clip).
  - Automatic fallback to x264 on any HW init or mid-stream failure (re-encode from start of the export job, logged).
- **Interfaces:** `Capabilities.hw_encode`, `EncodeSpec.hw`.
- **Acceptance criteria:**
  - On a machine with a HW encoder: 1080p30 10 s encode via HW produces a valid MP4 (same ffprobe checks as 1E.01) at ≥ 2× realtime (arch §19 export budget, recorded on reference machine); with `hw: Off` and with a forced failure the output is produced by x264.
  - Probe returns an empty HW list gracefully on CI runners without GPU (test).
- **Tests:** HW tests `#[ignore]` unless `CUTWEAVE_HW_TESTS=1`; fallback test runs everywhere.
- **Out of scope:** HEVC/AV1 HW.

### WP-1E.03 — Export pipeline, `export.start` command, offline audio mixdown
- **Area:** `crates/cutweave-engine` (module `export`), `crates/cutweave-ops`
- **Depends on:** WP-1E.01, WP-1F.03, WP-1G.02, WP-1D.07
- **Size:** M
- **Executor:** opus
- **Scope:**
  - `export.start{sequence, path, range?: {in,out}, preset: "h264-mp4-1080p" | custom{width,height,fps,quality}, hw?}` command → returns `{job_id}`; export job: snapshot the doc at start (immutable; later edits don't affect it), for each frame `t = i / fps` render via `cutweave-render` (full quality, originals not proxies, headless wgpu — device shared with preview via a GPU-budget lock) → read back → `Encoder`; audio: offline mixdown (1G.02 `mixdown`) of the timeline range fed to AAC.
  - Frame loop uses decode-ahead prefetch (decoder pool reuse, sequential decode — no per-frame seek) so 1080p simple timeline export ≥ 2× realtime with HW encode on reference machine.
  - Progress events (`job.progress` with fps + ETA), cancel (stops within 1 s, deletes partial), error mapping (`Unsupported`, `Io`, `HwUnavailable` fallback).
  - Output path validation (writable, not the project's source media, extension `.mp4`), overwrite requires `overwrite: true`.
  - `export.status` is just `job.status`.
- **Interfaces:** `export.start`, job kind `Export`, events `job.progress`.
- **Acceptance criteria:**
  - Export of a 3-clip 20 s 1080p30 timeline: output duration = timeline duration ±1 frame, frame count exact, `ffprobe` clean, audio present; frame at t=5 s decoded from the MP4 matches `render.frame` at 5 s within PSNR ≥ 38 dB (H.264 loss allowance; test).
  - Cancel at 50% → job state `Cancelled`, no output file.
  - **Preview matches export:** for 5 timestamps, the frame read back from the export pipeline before encoding is **bit-identical** to `render.frame` at full quality on the same adapter (tolerance policy).
  - Exporting twice from the same snapshot yields **frame-hash-identical** decoded output (decode both MP4s to raw and compare hashes) with threads fixed (x264 `threads=1` flag used in the determinism test only).
- **Tests:** integration with corpus + `FakeBackend`; golden export test hooks into 1O.
- **Out of scope:** presets beyond one, queues, HEVC/AV1.

---

## 1F — Render pipeline v0 (`cutweave-render`)

### WP-1F.01 — Timeline evaluation → scene description
- **Area:** `crates/cutweave-render` (wasm-safe core)
- **Depends on:** WP-1A.02
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - Pure function `evaluate(snapshot: &DocSnapshot, seq: SequenceId, t: Time, quality: Quality) -> Scene` where `Scene{size, background, layers: Vec<Layer{item, media, source_time: Time, transform, opacity, color_tags}>}` for visible video clips (top track last), mapping timeline time → source time (`source_in + (t - start)`), skipping disabled/hidden/muted-not-relevant items; for transitions/effects variants: not present.
  - Also `evaluate_audio(snapshot, seq, range) -> Vec<AudioSegment{media, src_range, dst_time, gain}>` shared by 1G.
  - Deterministic: no clocks/RNG; wasm32-compatible (`cargo xtask check-wasm`).
- **Interfaces:** `cutweave_render::{evaluate, evaluate_audio, Scene, Layer, Quality}`.
- **Acceptance criteria:** 12 table-driven unit cases (gaps, boundaries at start/end inclusive/exclusive, 2 stacked tracks, hidden track, disabled clip, source_in offset) pass; `cargo xtask check-wasm` passes; evaluation of a 5,000-item project at one time < 1 ms (bench, recorded).
- **Tests:** unit + proptest (every layer's `source_time` lies within media bounds).
- **Out of scope:** keyframes, speed, effects.

### WP-1F.02 — wgpu compositor: linear light, color conversion, transform/opacity
- **Area:** `crates/cutweave-render`
- **Depends on:** WP-1F.01, WP-0J.02 (ADR-007), WP-0F.03 (color notes), WP-1D.03
- **Size:** M
- **Executor:** opus
- **Scope:**
  - `GpuContext` (wgpu 30.x instance/adapter/device selection incl. software adapters via `CUTWEAVE_GPU=software`; device-lost handling = recreate device + rebuild caches + retry once; render thread watchdog logs frames > 500 ms).
  - Working space **`Rgba16Float` linear light, Rec.709 primaries** (arch §8): input transform per `ColorTags` (matrix, range, transfer: BT.709/sRGB/BT.601/PQ→SDR tone-map per S4 notes; HLG treated as per S4 decision) in a WGSL shader from NV12/P010/Yuv420p plane textures; output transform to sRGB/BT.709 8-bit for preview/export.
  - Single-layer-capable compositor handling N layers in the **Scene** order with `transform{position,scale,rotation,anchor}` + `opacity` + straight-alpha "over" blend; aspect-fit placement into the sequence canvas with background color.
  - Texture pool and upload of CPU frames (path (a)), plane-texture reuse; `RenderGraph` skeleton with a single "composite" pass node + cache key (subgraph hash) hooks (unused).
  - Determinism: same inputs → bit-identical output on the same adapter (test); no `Instant::now` in render path.
- **Interfaces:** `Renderer::{new(GpuContext), render(&Scene, &FrameSource, target: Target) -> RenderedFrame}`, `Target::{Rgba8Cpu, Texture}`, `FrameSource` trait (resolves `(media, source_time) -> VideoFrame` via decoder pool / frame cache).
- **Acceptance criteria:**
  - On the software adapter (WARP/lavapipe): rendering a known test card (corpus synthetic color bars, BT.709 limited range) matches the CPU float reference with ΔE2000 < 2 and PSNR > 45 dB (thresholds from S4); BT.601 and full-range variants correct (test with 3 tagged sources).
  - Identity transform + opacity 1 reproduces the source (after YUV→RGB) within PSNR > 50 dB; 50% opacity over a black background yields 50% luminance in linear light (± 1 LSB after encoding).
  - Device-lost simulated (drop device) → next `render` succeeds.
- **Tests:** headless GPU tests (software adapter in CI), unit for matrix math; golden hooks in 1O.01.
- **Out of scope:** effects, masks, text, multi-layer blend modes beyond normal, HDR output.

### WP-1F.03 — Quality ladder, frame readback, `render.frame` query
- **Area:** `crates/cutweave-render`, `crates/cutweave-ops`
- **Depends on:** WP-1F.02
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Preview quality ladder `Quality::{Full, Half, Quarter}` (scale applied at output size; decode at reduced size is future), `AutoQuality` controller (inputs: recent frame time, target fps; drop one level after 5 consecutive late frames, raise when paused → **refine to Full when paused**, arch §8).
  - Readback: async buffer mapping (double-buffered staging buffers, no GPU stall on the render thread) producing `Rgba8`/`Nv12` for the SHM ring and `Jpeg/Png` for agents (JPEG via `jpeg-encoder`/`image`, default max 768 px long side, `max_size` param, quality 80).
  - Query `render.frame{sequence, t, max_size?, fmt: "jpeg"|"png", quality?}` → `{image: bytes|resource, width, height, time, items_visible: [ItemId], rev}`; at `t` outside content returns the background frame, never an error.
  - Render determinism guard test: same `(snapshot rev, t, quality)` twice → byte-identical.
- **Interfaces:** `Quality`, `AutoQuality`, `render.frame` query, `Renderer::render_to_image`.
- **Acceptance criteria:**
  - `render.frame` of a 3-clip project at 3 distinct times returns correct frames (compared with golden refs via 1O.01 harness once available; before that against analytic expected colors of synthetic clips within ΔE < 2).
  - Auto-quality unit tests: simulated frame times drive ladder transitions exactly as specified (5 late → down, paused → Full).
  - 1080p `render.frame` to 768 px JPEG end-to-end < 150 ms on dev machine with warm cache (recorded).
- **Tests:** unit + headless GPU.
- **Out of scope:** SHM publishing (1H.02), contact sheets (P2).

---

## 1G — Audio playback v0 (`cutweave-audio`)

### WP-1G.01 — Mixer graph and clip audio sources
- **Area:** `crates/cutweave-audio`
- **Depends on:** WP-1F.01 (`evaluate_audio`), WP-1D.03, WP-0D.02
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Mixer graph v0: per-clip sources → per-track bus (gain, mute) → master (gain, hard-clip protection `tanh`-free soft limiter placeholder at 0 dBFS) → output; fixed 2-ch f32 output at the device rate.
  - **Real-time thread discipline** (arch §9): lock-free SPSC ring buffers fed by decode workers (decode → `rubato` resample to device rate → ring); the callback does no allocation, no locks, no syscalls (verified by `assert_no_alloc` in tests).
  - Prefetch policy: workers keep ≥ 500 ms ahead of the clock; seek flushes rings and refills; graph updates (doc changes) delivered via a lock-free command queue / double-buffered `Arc` swap of the segment list.
  - `cutweave-audio` depends only on `MediaBackend`'s `AudioDecoder` trait — usable with `FakeBackend`.
  - Offline renderer `mixdown(segments, range, rate) -> impl Iterator<AudioChunk>` using the **same graph code** (shared test: offline == realtime-render-to-buffer within 1e-6).
- **Interfaces:** `cutweave_audio::{Mixer, MixerHandle::{set_segments, seek, set_master_gain}, mixdown}`.
- **Acceptance criteria:**
  - Test with 4 overlapping tone clips mixed offline equals the analytical sum within 1e-5; clip boundaries are sample-accurate (impulse test: first sample of clip at exactly `time_to_samples(start)`).
  - 10 min simulated realtime run using a mock sink at 256-sample buffers with 8 sources: 0 underruns, 0 allocations in the callback (assert_no_alloc).
  - Resampling 44.1k→48k of a 1 kHz sine has THD+N < −90 dB (test).
- **Tests:** unit, property (offline==realtime), soak test (short in CI, long nightly).
- **Out of scope:** time-stretch, pan, EQ, ducking, fades (P2), cpal (1G.02).

### WP-1G.02 — cpal output, master clock, volume
- **Area:** `crates/cutweave-audio`
- **Depends on:** WP-1G.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - `AudioOutput` over cpal 0.18: device enumeration, default device, buffer size request (256; fall back per S2 recommendation), sample-format negotiation, reconnect/default-device-change handling (restart stream; clock stays monotonic).
  - **Master clock:** `AudioClock` atomics: `samples_played` (callback-updated), device output latency from cpal timestamps (`OutputCallbackInfo`), `position() -> Time` = timeline start + (samples_played − latency_samples)/rate; `is_running`; readable lock-free from any thread; used by 1H for A/V sync.
  - Null/dummy device mode (`CUTWEAVE_AUDIO=null`) with a wall-clock-driven fake clock for CI and headless MCP (explicitly deterministic variant: `ManualClock` advanced by tests).
  - Per-track volume (`audio_gain`) and mute applied; master volume as `view.set_master_gain` (`view` namespace: not undoable, not journaled; persisted via 1C.04).
  - Meters: peak per bus published at 30 Hz via a lock-free ring (consumed by UI later).
- **Interfaces:** `AudioOutput`, `AudioClock`, `ManualClock`, `view.set_master_gain`.
- **Acceptance criteria:**
  - On a dev machine with an audio device: 10 min playback of the 8-source mix at 256-sample buffer reports 0 underruns, clock tracking error vs the device's sample progress < 2 ms over 10 min (the clock follows the device crystal by design, §9; drift vs `Instant` is reported, not gated: on the reference headset it is +11 ppm ≈ 6.6 ms/10 min, as Spike S2 measured).
  - Unplugging/changing the default device during playback (manual test on Windows + macOS, documented) restarts the stream within 2 s without crash.
  - Headless CI passes using null device; `AudioClock::position()` is monotonic non-decreasing (property test).
- **Tests:** unit with null device; manual hardware checklist recorded in PR; soak nightly.
- **Out of scope:** latency calibration UI, recording.

---

## 1H — Playback & SHM preview (`cutweave-playback`, `cutweave-bridge`)

### WP-1H.01 — Transport, A/V sync, scrub scheduler, JKL
- **Area:** `crates/cutweave-playback`
- **Depends on:** WP-1G.02, WP-1F.03, WP-1D.03
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Transport state machine `Stopped | Playing{rate} | Scrubbing` with commands in `view` namespace: `view.play`, `view.pause`, `view.seek{t}`, `view.set_rate{rate}` (JKL semantics: J = reverse step levels −1×/−2×/−4×… (**reverse playback = frame-step video, muted audio in P1**), K = pause, L = 1×/2×/4×; K+J/K+L = slow ±0.5× not required), `view.step{frames}`, `view.set_loop` (optional in/out not in P1), plus `playback.state{playing,rate,t,rev}` events at ≤ 30 Hz.
  - Render loop thread: when Playing, follows the **audio clock** (`AudioClock::position`), picks target frame = floor to sequence frame grid, renders via `Renderer` with `AutoQuality`, skip-frames policy (drop late frames, never delay audio); prefetch of next N frames per active clip via decoder pool.
  - **Latest-wins scrub scheduler:** seeks cancel in-flight render/decode (`CancelToken`), try frame cache first, emit low-res (Quarter) frame immediately then refine to Full when idle 100 ms (arch §10 step 5).
  - Transport independent of UI: works headless (null audio clock) for MCP/CLI/tests.
  - Playback state is not part of the document and not journaled (`view` namespace, arch §5.1); the playhead is persisted via 1C.04.
- **Interfaces:** `Transport`, `PlaybackEngine::{attach(doc, renderer, audio, FrameSink)}`, `FrameSink` trait (SHM publisher or test sink), `view.*` commands.
- **Acceptance criteria:**
  - With `FakeBackend` + `ManualClock`: 10 s playback at 30 fps delivers exactly 300 frames in order, A/V offset (frame pts vs clock) ≤ 1 frame for 100% of frames; dropping a render artificially 3× slower than real time produces frame drops but never audio stalls (assert clock advanced steadily).
  - 100 random seeks in 1 s intervals: no stale frame is ever delivered after a newer seek request (assert by seq numbers).
  - On a dev machine, 1080p30 single-layer playback of a corpus clip for 60 s with real audio: 0 dropped frames, A/V offset < 40 ms measured via the flash/click corpus clip and a timestamp log.
- **Tests:** unit with fake clock; integration; nightly soak 10 min (full 30-min soak is P2/7D).
- **Out of scope:** HW-measured budgets (arch §19 iGPU 3-layer), audio scrubbing, loop ranges.

### WP-1H.02 — SHM ring publisher
- **Area:** `crates/cutweave-playback` (module `shm`)
- **Depends on:** WP-1H.01, WP-0J.02 (ADR-001 transport choice), WP-0C.04 (RING.md)
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Ring per `RING.md` from S1: named shared memory (Windows named file mapping; macOS/Linux POSIX `shm_open` or file in `$XDG_RUNTIME_DIR`/`/tmp` with 0600 perms via `memmap2`), name includes pid + random suffix, created by the engine, announced in the run-file (1I.01) and handshake; 4–6 slots; header `{magic, version, slot_count, slot_stride, write_idx: AtomicU64, audio_clock_flicks: AtomicI64, playing: AtomicBool}` and per-slot `{pts, w, h, format(NV12|RGBA8), rev, seq, size}`; seqlock-style slot publication (write `seq` odd → data → even) so readers detect torn reads.
  - Publisher is a `FrameSink`: converts the rendered frame to RGBA8 (or NV12 if S1 showed benefit) at the **preview canvas size** reported by the UI (`view.set_preview_size{w,h}`), resizes the ring on size change with a generation counter.
  - Audio clock word updated at ≥ 200 Hz from `AudioClock`.
  - Cleanup on exit and stale-ring reaping at startup (rings from dead pids).
  - Permission: object accessible only by the current user.
- **Interfaces:** `ShmRing::{create, publish, set_clock}`, header layout in `crates/cutweave-playback/RING.md` (normative copy updated from S1).
- **Acceptance criteria:**
  - A Rust reader test (separate process) reading while the writer publishes 1080p60 for 60 s sees 0 torn frames (checksum in pixel data) and monotonic `seq`.
  - Writer overhead: publish of a 1080p RGBA frame (8 MB) p95 < 3 ms excluding render (bench).
  - After `kill -9` of the engine, the next engine start removes the stale ring (test).
- **Tests:** multi-process integration; unit for seqlock.
- **Out of scope:** Electron reader (1H.03), zero-copy GPU sharing.

### WP-1H.03 — `cutweave-bridge` napi-rs addon (SHM reader)
- **Area:** `crates/cutweave-bridge`, `apps/desktop/native`
- **Depends on:** WP-1H.02, WP-1L.01, WP-0J.02
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - napi-rs v3 addon exposing `openRing(name) -> Ring`, `Ring.header()`, `Ring.latestSlot(maxPtsFlicks?) -> {pts,w,h,format,rev,seq,buffer: ArrayBuffer}` (zero-copy external ArrayBuffer over the mapping, valid until the next call guarded by seqlock re-check), `Ring.audioClock()`, `Ring.close()`; plus `perfCounters()` stub.
  - Prebuilt per-platform binaries built in CI (`@napi-rs/cli`, targets win32-x64, darwin-arm64, linux-x64-gnu), implementing **the transport chosen in ADR-001** (S1 report). Transport (a), the default: the addon is loaded by the **preload of the main editor window only, which runs with `sandbox:false`** (Electron sandboxed preloads cannot load native modules), `contextIsolation:true`, `nodeIntegration:false`, and is exposed to the page only through a narrow `contextBridge` API (`preview.getLatestFrame()`); the window loads local `app://` content only under the strict CSP of 1L.02; plugin iframes and all other windows get no preload and no addon. Transport (b), if ADR-001 chose it: the addon runs in a `utilityProcess` and frames are delivered over a transferred `MessagePort` to a fully sandboxed renderer.
  - Failure modes: ring missing → typed JS error; version mismatch → error.
- **Interfaces:** `window.cutweave.preview.{open, getLatestFrame, getClock, close}` (typed in `apps/desktop/src/preload.d.ts`).
- **Acceptance criteria:**
  - Node test (Vitest, node environment) with a Rust-writer fixture binary: reader obtains ≥ 590 distinct frames in 10 s at 60 fps with 0 torn reads and buffer views are zero-copy (assert `buffer.byteLength === w*h*4` and no copy by checking `process.memoryUsage().arrayBuffers` growth < 20 MB).
  - Addon builds in CI on 3 OSes and loads in the **non-sandboxed main-window preload** (transport a) or the utilityProcess (transport b) per ADR-001 (smoke test in 1L.01 e2e); the renderer main world has no `require`/`process`, and a second window has no access to the addon (e2e).
- **Tests:** Vitest + Rust fixture, CI build matrix.
- **Out of scope:** perf counters, WebGPU drawing (1M.04).

---

## 1I — Local API server (`cutweave-api`, `cutweave-engine`)

### WP-1I.01 — axum server skeleton: auth, Host/Origin, run-file
- **Area:** `crates/cutweave-api`
- **Depends on:** WP-0J.02, WP-1B.03
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - axum server bound to `127.0.0.1:<port>` (`--port N`; `0` = random), tower middleware with **credential classes**: (1) the **UI credential**: a per-launch 256-bit token generated by Electron main and delivered to the engine **only over the spawn handshake pipe** (stdin), held in memory, **never written to disk or the run-file**, passed to the renderer through the preload bridge; (2) **agent tokens** (256-bit, labelled, e.g. `cli`, `mcp`) generated by the engine at start and stored in the run-file for CLI/MCP attach; (3) **internal** (in-process jobs). Presented as `Authorization: Bearer`; WS upgrade may use a subprotocol header. Media URLs for `<img>`/`<video>` use **short-lived (<= 5 min), path-scoped HMAC-signed URLs** derived from the UI credential, valid for read-only media routes only. Headless mode has **no UI credential class** at all. **Host** header must be `127.0.0.1:<port>` or `localhost:<port>`, **Origin** must be absent (non-browser), `file://`/`app://cutweave`-style origin configured by the launcher, or the engine origin (DNS-rebinding defense, arch §6/§18); constant-time token compare; 401/403 JSON errors.
  - Run-file `~/.cutweave/run/<pid>.json` `{pid, port, agent_tokens:[{label, token}], project, started, version, shm?}` (no UI token) created with user-only permissions (0600 / Windows ACL via `windows-acl`/`SetNamedSecurityInfo` restricting to current user), removed on exit (including SIGINT/SIGTERM/ctrl-c handler), stale files cleaned at startup.
  - **Actor derivation:** the engine maps the credential to the actor (UI → `user`, agent token → `agent:<label>`, plugin → `plugin:<id>`, internal → `system`); no request field can assert an actor.
  - **UI-only scopes** (`Scope::UiOnly` commands): `ai.set_key`, approval minting, roots allowlist, budget config, plugin permission grants (commands arrive in later phases; P1 ships the enforcement mechanism plus a test-only `test.ui_only` command). Agent tokens calling a UI-only command get `Forbidden`.
  - Graceful shutdown.
- **Interfaces:** `ApiServer::{start(config) -> RunningServer{port}}`, `Credential{Ui, Agent(label), Internal}`, `RunFile::{write, remove, discover()}`, `Scope::UiOnly` enforcement.
- **Acceptance criteria:**
  - Integration tests: no token → 401; wrong token → 401; right token + `Host: evil.com` → 403; right token + `Origin: https://evil.com` → 403; valid → 200 (≥ 8 cases).
  - Credential tests: the UI token never appears in the run-file, in logs, or on disk (grep of the engine's data dir after a session); an agent token invoking `test.ui_only` → `Forbidden`, the UI credential succeeds; the derived actor of a command is `user` / `agent:mcp` / `system` per credential (asserted via `doc.patched.actor`); a forged `_meta.actor` is ignored; a headless engine rejects any UI-credential handshake.
  - Run-file perms: on Unix `stat` mode is `0600`; on Windows the DACL grants only the current user (test via `icacls` parse).
  - `kill -9` leaves a stale run-file that `discover()` ignores (pid check) and startup cleans.
- **Tests:** integration (axum test server), unit.
- **Out of scope:** RPC/media routes (1I.02/03).

### WP-1I.02 — JSON-RPC over WebSocket + event subscriptions
- **Area:** `crates/cutweave-api`
- **Depends on:** WP-1I.01, WP-1B.09, WP-1I.04
- **Size:** M
- **Executor:** opus
- **Scope:**
  - `ws://127.0.0.1:P/rpc`: JSON-RPC 2.0; methods = every registered command/query by name (`clip.split`, `timeline.get`, …) with `params` = command input; `request_id` (UUIDv7) is **required** on every mutating call from RPC clients (missing -> `Invalid`) and engine-generated for CLI/MCP; the engine dedupes per 1B.03 and a request resent after reconnect returns the original result; extra reserved methods `rpc.hello{client, version}` (returns `{engine_version, schema_hash, rev, shm?}`), `rpc.subscribe{events[]}`/`rpc.unsubscribe`, `project.snapshot` (full doc + rev; used for resync).
  - Standard envelope extension: `params._meta{expected_rev, request_id, tx}` (**no client-asserted actor**: the actor is derived from the connection's credential, 1I.01); errors map `CmdError` → JSON-RPC error `{code, message, data{code,path,current_rev}}`.
  - Events as notifications: `doc.patched{rev, actor, tx_id, ops}`, `playback.state`, `job.progress`, `media.indexed`, `project.dirty/saved/recovery_available`, with per-connection backpressure (bounded queue; on overflow drop the connection with close code 4001 `resync_required`).
  - Rev-gap protocol: each `doc.patched` carries `prev_rev`; client detects gaps and calls `project.snapshot`.
  - Concurrency: many connections; each request handled async but commands go through the single doc actor. Each WS connection has a `ConnId`; its closure triggers tx rollback (1B.09).
- **Interfaces:** the RPC method namespace = registry; `RpcClientHello`, error code table in `docs/reference/rpc.md` (generated section in 1N).
- **Acceptance criteria:**
  - Test client issues `project.new`, `timeline.insert`, `clip.split`, `edit.undo` and receives the corresponding `doc.patched` events in order with consecutive revs; replaying events on an empty mirror equals `project.snapshot` (property test via `replay_events` from 1B.07).
  - The same `request_id` sent over two different connections returns the same result and `rev` increases once (test).
  - Stale `expected_rev` yields JSON-RPC error with `data.code="Conflict"` and the right `current_rev`.
  - Command round-trip over WS localhost p95 < 16 ms for `clip.trim` on a 400-item project (bench; arch §19 budget, recorded; gating threshold 25 ms on CI to allow noise).
  - 50 concurrent connections issuing 100 commands each: no deadlock, final rev = total committed txs.
- **Tests:** integration with `tokio-tungstenite` client; bench.
- **Out of scope:** MessagePack, auth beyond 1I.01.

### WP-1I.03 — Media HTTP endpoints
- **Area:** `crates/cutweave-api`
- **Depends on:** WP-1I.01, WP-1I.05, WP-1D.05, WP-1D.06, WP-1F.03
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - `GET /media/{id}/thumb?t=&w=`, `/media/{id}/filmstrip?w=&h=&count=`, `/media/{id}/waveform?lod=&start=&end=` (binary peaks or PNG via `fmt=png`), `/media/frame?seq=&t=&w=&h=&fmt=jpeg|png` (uses `render.frame`; the preview's JPEG fallback from WP-1M.04 calls it with `t` floored to the frame grid in integer flicks, even `w`/`h`, the token in `Authorization: Bearer`, and a CORS preflight from `app://cutweave` that must allow `authorization`); all authenticated (header or short-lived URL token param); validate params (bounds, `w` ≤ 4096); content-hash **ETag** + `Cache-Control: private, max-age=31536000, immutable` when keyed by content hash and `If-None-Match → 304`; range requests not needed.
  - Concurrency limits (semaphore) and request cancellation when the client disconnects.
  - 404 for unknown media; 422 with JSON error for bad params.
- **Interfaces:** routes above; `docs/reference/http-media.md`.
- **Acceptance criteria:**
  - Integration test with corpus: thumb returns decodable JPEG of requested width; repeated request with `If-None-Match` → 304 with 0 decodes; `/media/frame` for t=2 s returns the same bytes as `render.frame` query.
  - Disconnecting mid-request cancels the decode within 100 ms (test).
  - Requests without token → 401; `w=100000` → 422.
- **Tests:** integration (reqwest).
- **Out of scope:** proxies, streaming video.

### WP-1I.04 — `cutweave-engine` root: doc actor, project, jobs, API, launch protocol
- **Area:** `crates/cutweave-engine`
- **Depends on:** WP-1B.09, WP-1C.03, WP-1D.07, WP-1I.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - `Engine::start(EngineConfig{mode: Gui|Headless|Oneshot, project?, port?, deterministic_ids?: u64, audio, gpu, allowed_roots, log})` wires the **core**: `DocActor` + registry (`cutweave-ops`), `ProjectStore` + journal + autosave, `JobManager`, `ApiServer` (1I.01, RPC 1I.02 mounts here). Media/render/audio/playback are attached by WP-1I.05 through a `Subsystems` extension point; without them the engine serves all document commands (enough for the UI and parity work to proceed in parallel).
  - One engine = one open project (arch §14); session lifecycle `NoProject → Open → Closing`; `project.open/new/close` swap project state (cancel jobs, new journal). `Engine::shutdown()` flushes the journal, saves nothing implicitly, releases the lock, removes the run-file.
  - **Launch protocol** (`docs/reference/launch-protocol.md`): CLI `--port N` (Electron main chooses the port on every (re)start); in `Gui` mode the UI credential arrives as the first line on stdin (never argv/env/disk); readiness line `{"event":"ready","port":P,"pid":N,"version":…}` on stdout (no tokens); orphan prevention (exit when stdin closes in `Gui` mode). Exit codes: 0 clean, 10 project-locked, 11 gpu-init-failed, 12 **port bind error** (so the supervisor can pick a new port), 13 bad handshake.
  - `tracing` JSON logs to `~/.cutweave/logs/engine-<pid>.log` (rotation 5×10 MB), panic hook writes a crash marker, **no telemetry** (test asserts no outbound sockets during startup/shutdown).
- **Interfaces:** `Engine`, `EngineConfig`, `Subsystems` trait, launch-protocol doc; `cutweave engine` (1K) wraps this.
- **Acceptance criteria:**
  - Integration test: start a `Headless` engine (core only), open a fixture project over RPC, run a command, `shutdown()`: run-file gone, lock released, thread count returns to baseline ±2.
  - Cold start to ready with an empty project < 1.0 s (core only; recorded).
  - Startup with a locked project exits 10 with a JSON error line; binding an occupied port exits 12.
  - In `Gui` mode with no handshake line within 5 s the engine exits 13.
- **Tests:** integration; no-network test; handshake tests.
- **Out of scope:** media/render/audio/playback wiring (1I.05); multi-project headless servers.

---

### WP-1I.05 — Engine subsystem wiring: media, render, audio, playback
- **Area:** `crates/cutweave-engine`
- **Depends on:** WP-1I.04, WP-1D.05, WP-1D.06, WP-1F.03, WP-1G.02, WP-1H.01, WP-1H.02
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Implement `Subsystems` for the full engine: `MediaBackend` + `DecoderPool` + caches (`FrameCache`, `ThumbCache`, `WaveformCache`), `Renderer`/`GpuContext` (software or real per `EngineConfig.gpu`), `AudioOutput`/null clock per `EngineConfig.audio`, `PlaybackEngine` + `ShmRing` publisher (Gui mode), registration of the `render.frame`, `export.start`, `view.*` transport commands and the media HTTP state for 1I.03.
  - Lifecycle: ordered startup/shutdown (stop playback → join render/audio threads → close caches), device-lost propagation, per-project teardown on `project.close`; SHM ring name published in the run-file/handshake; GPU/audio failure degrade rules (audio fails → null clock + warning event; GPU fails → exit 11 in Gui mode, software fallback in headless).
  - `EngineConfig.deterministic_ids` honoured end-to-end (1B.08).
- **Interfaces:** `FullSubsystems`, events `engine.degraded{subsystem, reason}`.
- **Acceptance criteria:**
  - Full headless engine (null audio, software GPU) integration test: import corpus clip → insert → `render.frame` → play 2 s on a manual clock → export 1 s → shutdown with no leaked threads or SHM objects.
  - Cold start to ready with an empty project < 1.5 s on a dev machine (recorded; arch §19 start budget is 2.5 s for the whole app).
  - Forcing audio init failure yields a working engine with `engine.degraded` (test).
- **Tests:** integration (corpus).
- **Out of scope:** new features; this WP only composes existing crates.

---

## 1J — MCP v0 (`cutweave-api`)

### WP-1J.01 — rmcp server: registry-derived tools, `cutweave_execute`, annotations
- **Area:** `crates/cutweave-api` (module `mcp`)
- **Depends on:** WP-1I.01, WP-1B.09, WP-1B.06, WP-1D.07 (job status/wait provider), WP-1E.03 (`export.start`)
- **Size:** M
- **Executor:** opus
- **Scope:**
  - rmcp (version pinned per S5/ADR-009) server mounted on `/mcp` (Streamable HTTP) with the same token/Host/Origin middleware.
  - **15 core tools (mcp="core")**: `project_open`, `project_info`, `media_import`, `media_list`, `timeline_get`, `timeline_insert`, `clip_split`, `clip_trim`, `clip_move`, `item_delete`, `edit_undo`, `export_start`, `job_status`, **`job_wait{job_id, timeout_ms}`**, and `render_frame` (implemented in 1J.02). Names derive from command names with `.`→`_`; descriptions, schemas and **annotations** (`readOnlyHint`, `destructiveHint`, `idempotentHint`) are generated from `CommandMeta`. `edit_redo` and the rest of the registry are reachable through `cutweave_execute`/`cutweave_query`.
  - `media_import` returns `media_ids` synchronously (1D.07); agents then call `job_wait` on the returned analysis job ids if they need thumbnails/index; `export_start` returns `job_id` and `job_wait` blocks up to `timeout_ms` (<= 60 s) so agents need no polling loops.
  - **4 generic tools:** `cutweave_search_commands{query}` (fuzzy over name/summary), `cutweave_describe_command{name}` (full JSON Schema), `cutweave_execute{steps[{cmd,args}], dry_run?, expected_rev?}` — **transactional batch** (one tx → one undo step; `$ref` between steps like `{"$ref":"steps[0].output.left"}`; on any step error the whole tx rolls back and the error names the failing step index), `cutweave_query{name,args}`.
  - Tool results: structured content (JSON) + short text summary; errors as MCP tool errors with `code`, `message`, `path`, `hint`.
  - **Tool profiles are per endpoint, never per connection:** `/mcp` (standard = the 19 above in P1), `/mcp/minimal` (<= 12 tools: `project_info`, `media_list`, `timeline_get`, `render_frame`, `job_wait`, `edit_undo` + the 4 generic tools), `/mcp/full` (standard + search-based long tail; identical to standard in P1); stdio servers take `--profile minimal|standard|full`.
  - **Tool-list budgets (single source):** minimal <= 12 tools / <= 3k tokens; standard <= 36 tools / <= 8k tokens; full = standard + search-based long tail; **P1 gate: `/mcp` lists <= 19 tools and <= 5k tokens** (tokens estimated as ceil(bytes of compact `tools/list` JSON / 4), method documented). CI fails when exceeded.
  - Transcripts/metadata strings in results wrapped `{"untrusted": true, ...}` per arch §14.
  - Actor for MCP calls = `agent:<token label>` (credential-derived, 1I.01); MCP session end/disconnect rolls back that connection's open tx (1B.09).
- **Interfaces:** MCP tool names above; `cutweave_execute` schema.
- **Acceptance criteria:**
  - An rmcp client test on `/mcp` lists exactly the 19 tools (15 core + 4 generic; `render_frame` arrives with 1J.02, so this WP's gate lists 18 and 1J.02 raises it to 19), within the 5k-token/19-tool gate; `/mcp/minimal` lists <= 12 tools and <= 3k tokens; each core tool call returns the same output as the RPC call for the same args (spot-checked by 1O.02 parity).
  - `cutweave_execute` with 3 steps where step 3 fails leaves the doc and rev unchanged and returns `failed_step: 2`; with `dry_run: true` the doc is unchanged and the response lists predicted ops.
  - Tools have correct annotations: `clip_split` not read-only; `timeline_get` read-only (test per tool).
- **Tests:** rmcp client integration (HTTP), snapshot of `tools/list` (insta) — becomes the committed MCP catalog snapshot.
- **Out of scope:** resources (1J.02), prompts, tasks extension, elicitation, per-connection profiles.

### WP-1J.02 — `render_frame` image content + resources
- **Area:** `crates/cutweave-api` (module `mcp`)
- **Depends on:** WP-1J.01, WP-1F.03, WP-1I.05
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - Tool `render_frame{sequence?, t, max_size?=768, fmt?="jpeg"}` returning an MCP **image content block** (base64 JPEG ≤ 768 px long side default) **plus** structured metadata `{time, rev, width, height, items_visible[], notes}`; never exceeds a 1 MB payload (re-encode at lower quality / reject with hint to lower `max_size`).
  - Resources: `cutweave://project` (summary JSON), `cutweave://sequence/{id}/timeline` (paginated summary, first page ≤ 4 KB), `cutweave://catalog/commands` (command list); change notifications on `cutweave://project` use **whatever the negotiated MCP revision supports** (`resources/subscribe` + `notifications/resources/updated` on 2025-11-25; `subscriptions/listen` on 2026-07-28) behind one `Notifier` abstraction fed by `doc.patched`; nothing depends on `ping` for the newer revision.
  - Large frames returned as resource links (`cutweave://frame/{rev}/{t}`) when requested `max_size` > 1568 (document threshold).
- **Interfaces:** tool `render_frame`; resources above.
- **Acceptance criteria:**
  - MCP client test calls `render_frame` on the 3-clip fixture at t=2 s: result has exactly one image content block, valid JPEG ≤ 768 px, metadata `items_visible` contains the expected item ID.
  - Under **each** of the two MCP revisions (rmcp client configured per revision), subscribing to `cutweave://project` and then running `clip_split` produces an update notification within 200 ms; `tools/list` now has 19 tools.
- **Tests:** rmcp client integration.
- **Out of scope:** `render_contact_sheet`, `media_filmstrip`, `audio_waveform` (P2).

### WP-1J.03 — stdio transports: `attach` proxy and `headless`
- **Area:** `crates/cutweave-api`, `crates/cutweave-cli`
- **Depends on:** WP-1J.02, WP-1I.05, WP-1K.01
- **Size:** M
- **Executor:** opus
- **Scope:**
  - `cutweave mcp --headless [--project X] [--allow-root DIR…]`: in-process `Engine` (Headless, audio Null) serving MCP over stdio (rmcp stdio transport); stdout reserved strictly for protocol frames, logs to stderr/file.
  - `cutweave mcp --attach [--run-file PATH|--pid N] [--profile minimal|standard|full]`: stdio proxy discovering the running GUI engine via the run-file (1I.01), connecting to `/mcp` (or `/mcp/<profile>`) over HTTP with an **agent token from the run-file** (never the UI credential), forwarding JSON-RPC both ways incl. notifications; multiple engines → error listing pids and require `--pid`; engine vanishes → proxy exits non-zero with a clear JSON-RPC error.
  - Filesystem allowlist (arch §14/§18): headless mode allows project folder + `--allow-root`; `media.import`/`export.start` paths outside → error `Invalid` with hint.
  - Documentation snippet `docs/reference/mcp-clients.md` with config JSON for Claude Desktop/Code stdio (`cutweave mcp --headless`).
- **Interfaces:** CLI flags above.
- **Acceptance criteria:**
  - Scripted client over stdio headless: initialize → list tools → `project_open` → `media_import` (returns `media_ids`) → `job_wait` → `timeline_insert` → `clip_split` → `render_frame` all succeed on the corpus (this is the M1 exit-2 flow, reused by 1O.04).
  - `attach` test: start engine (GUI mode stub), connect via proxy, issue `timeline_get`, kill engine, proxy returns error and exits ≠ 0 within 2 s.
  - Writing anything non-protocol to stdout in headless mode is impossible (test greps stdout of a 100-call session: every line parses as JSON-RPC).
- **Tests:** process-level integration.
- **Out of scope:** multi-project headless, auto-config installers (`agent-kit install`, P6).

### WP-1J.04 — Agent targeting with several engines: list, choose by project, last-focused default
- **Area:** `crates/cutweave-cli`, `crates/cutweave-api` (attach proxy), `crates/cutweave-engine` (run-file fields), `apps/desktop` (window focus report), `docs/reference/cli.md`, `docs/reference/mcp-clients.md`, `docs/reference/launch-protocol.md`, `docs/user-flows.md` and `apps/desktop/e2e/flows`
- **Depends on:** WP-1J.03, WP-1K.01, WP-1I.05
- **Size:** M
- **Executor:** sonnet
- **Scope:** (product owner request, 2026-11-24) when several engines run (the app plus headless engines from scripts, today; one per window later), an agent must be able to see them and pick one without a process id in its config.
  - **The run-file says what a chooser needs:** the open project (name and path, or none), mode (GUI or headless), start time, and for GUI engines when its window last had focus. Main reports window focus to its engine; run-file writes stay atomic and stale files keep being ignored by the pid check.
  - **`cutweave engines`** lists the running engines (pid, mode, project, port, started, last focused), newest focus first; `--json` for scripts.
  - **Choosing:** `mcp --attach`, `exec` and `query` take `--project <name or path>` (a name matches case-insensitively; a path matches the project folder or document). Without `--project` or `--pid` and with several engines, they pick the GUI engine focused most recently (the window the person is looking at). If no GUI engine runs and several headless ones do, they fail with exit 3, listing the engines and how to choose.
  - **MCP on the attach proxy:** proxy-level tools `engines_list` and `engine_attach{project|pid}` switch the upstream engine without restarting the client; the proxy sends `notifications/tools/list_changed` when the catalog differs.
  - Docs: the CLI reference, the MCP client guide (with a config that needs no pid), and the run-file fields in the launch protocol.
- **Interfaces:** run-file fields `project`, `mode`, `started`, `focused_at`; `cutweave engines [--json]`; `--project` on `mcp --attach`, `exec`, `query`; MCP tools `engines_list`, `engine_attach`.
- **Acceptance criteria:**
  - Integration: two headless engines on projects A and B plus a GUI-mode engine on C. `cutweave engines --json` lists all three with their projects. `exec --project B` edits only B. With no flags the GUI engine is chosen. With only the two headless engines, no flags exits 3 and lists both.
  - MCP: a scripted client on `mcp --attach` calls `engines_list`, then `engine_attach{project:"B"}`, then `timeline_get`, and gets B's timeline.
  - **A user flow (ADR-0018):** with the app open and a headless engine running, an agent attaching without flags edits the app's project, and the edit shows in the window.
- **Tests:** process-level integration, user flow.
- **Out of scope:** several windows in one app (one engine per window), engines on other machines.

### WP-1J.05 — Settings > Agents and MCP: connect a client, folders agents may use, who is connected
- **Area:** `crates/cutweave-engine` (allowed roots at runtime, connections query and events, UI-only scopes), `crates/cutweave-ops` only if `project.info` must change there, generated schema and TS types, `apps/desktop` (folders passed as `--allow-root` on every engine start), `packages/editor` (Agents and MCP page, Notifications row, connect notices), `docs/reference/launch-protocol.md`, `docs/reference/mcp-clients.md`, `docs/reference/settings.md`, `apps/desktop/e2e/flows`, `docs/user-flows.md`
- **Depends on:** WP-1M.11, WP-1J.04, WP-1I.05
- **Size:** M
- **Executor:** opus (security review by human)
- **Scope:** (product owner request, 2026-11-24) agents attached to the app can only touch the open project's folder today (the GUI engine gets `--allow-root` only from a development variable), and nothing in the app says how to connect an agent or which agents are connected.
  - **Folders agents can use** (preference `agents.allowed_folders`): the open project's folder is always listed and cannot be removed; "Add folder…" (native folder picker) and remove. Main passes the list as `--allow-root` on every engine start and supervised restart, in production builds (the development variable stays for tests). Changes apply at once through a new engine command `project.set_allowed_roots{roots}`: not undoable, not journaled, never dirties the project, and a **UI-only scope** (ADR-0016 lists "changing filesystem allowlists"), so an agent token gets `forbidden`. Paths are canonicalized (symlinks resolved, `..` refused, existing folders only). Removing a folder stops access at once, also for connections already open.
  - `project.info` returns the roots in effect (`allowed_roots`), so an agent can tell where it may import, open and save; the `Invalid` error for a path outside them says to add the folder in Settings > Agents and MCP.
  - **Also UI-only:** WP-1M.10's autosave command, so an agent cannot override the person's preference until the next handshake. (Every later "apply a preference to the engine" command follows the same rule; see WP-1M.11's arch paragraph.)
  - **Connect a client:** a status line ("Agents can connect to this window" or why not) and copyable setups for Claude Code (`claude mcp add …`), Claude Desktop and Cursor (JSON), Codex and a generic stdio client. Each runs `cutweave mcp --attach` with the bundled CLI's absolute path and **no pid** (WP-1J.04 picks this window's engine), with the tool set from `agents.client_profile` (Standard by default; Minimal, Full). No token or port is ever shown; the attach proxy reads the run-file.
  - **Connected agents:** the agent connections open on this engine (actor label, connected since, last command and when), from a new read-only engine query (for example `project.connections`) and events when a connection opens or closes. Read-only: no disconnect button.
  - **Notifications page** (first row): "Show a notice when an agent connects or disconnects" (`notify.agent_connected`, on), a toast naming the agent.
- **Interfaces:** preferences `agents.allowed_folders`, `agents.client_profile`, `notify.agent_connected`; engine command `project.set_allowed_roots{roots}` (UI-only); `project.info.allowed_roots`; query `project.connections` and its events; the Agents and MCP page.
- **Acceptance criteria:**
  - Integration: an agent token calling `project.set_allowed_roots` or WP-1M.10's autosave command, over RPC, MCP (`cutweave_execute`) and the CLI, gets `forbidden`.
  - Integration: with folder D allowed, an attached agent imports a file from D; after D is removed, the same import on the same open connection fails with `Invalid` naming Settings > Agents and MCP.
  - Unit: main passes every stored folder as `--allow-root` on start and after a supervised restart; a folder that no longer exists is skipped with a warning in the main log.
  - **A user flow (ADR-0018, F20):** in Settings > Agents and MCP add a folder (OS picker stubbed); `cutweave exec` attached to the app imports a file from it and its card appears in the bin; a notice names the agent and it is listed under Connected agents while connected; remove the folder and the agent's next import is refused with the message; the list survives a relaunch. "Copy" for Claude Code puts a command on the clipboard that names the CLI's path and no pid, port or token.
- **Tests:** engine integration, launch-args unit, Vitest, user flow.
- **Out of scope:** per-agent permissions and spending budgets (P5: WP-5A.08, WP-5F.07 put the budget rows on this page); disconnecting or blocking an agent; remote agents; `cutweave agent-kit install` (P6).

---

## 1K — CLI

### WP-1K.01 — `cutweave` binary: engine | mcp | exec | query | render
- **Area:** `crates/cutweave-cli`
- **Depends on:** WP-1I.05, WP-1I.02
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - `clap` derive CLI, single binary (ADR-002): 
    - `cutweave engine [--project P] [--port N] [--token-stdin]` — daemon for the GUI (wraps `Engine::start(Gui)`), prints readiness line (1I.04).
    - `cutweave exec '<json>'` / `--file f.json` / `-` (stdin) (every engine-spawning subcommand accepts `--deterministic-ids <seed>`, WP-1B.08): `{"cmd":"clip.split","args":{…}}` or array/steps (transactional); `cutweave query <name> '<json>'`; both **attach to a running engine** (run-file discovery; `--pid`) **or spawn a headless in-process engine** when none/`--headless` given (with `--project`), JSON in → JSON out on stdout, errors as JSON on stderr, exit codes 0 / 1 (command error) / 2 (usage) / 3 (no engine) .
    - `cutweave render --project P --sequence S --out out.mp4 [--range in:out] [--preset …] [--sw]` headless export, progress on stderr, final JSON `{job_id, output, frames, duration_s}`.
    - `cutweave mcp …` flags delegated to 1J.03 (wired here), `cutweave commands [--json]` lists registry with summaries, `cutweave describe <name>` prints schema; `cutweave --version` includes engine/schema hash.
  - **Same dispatcher as RPC/MCP** — the CLI calls `Registry::invoke_json` through `DocHandle`, never reimplements logic.
  - `cutweave worker` subcommand reserved: prints "not implemented in this build" (exit 64).
- **Interfaces:** CLI surface above; `docs/reference/cli.md` auto-generated from clap (`cargo xtask gen`).
- **Acceptance criteria:**
  - `cutweave exec '{"cmd":"project.new","args":{"name":"t"}}' --headless` prints a JSON object with `rev` and output; invalid command name exits 1 with `{"code":"NotFound"}`.
  - `cutweave render` of the 3-clip fixture project yields an MP4 passing 1E.03's ffprobe checks.
  - `cutweave commands --json | jq length` equals the registry size (test).
- **Tests:** `assert_cmd` integration tests, snapshot of `--help` (insta).
- **Out of scope:** `plugin`, `agent-kit`, `worker`.

---

## 1L — Electron shell & engine supervision (`apps/desktop`)

### WP-1L.01 — Electron app scaffold (main, preload, Vite renderer, dev/packaging)
- **Area:** `apps/desktop`
- **Depends on:** WP-0A.02, WP-0J.02 (ADR-001), WP-0J.03 (ADR-014 min OS)
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Electron (latest stable pinned at scaffold) + `electron-vite`/Vite 8 + TypeScript strict: `src/main/index.ts`, `src/preload/index.ts`, renderer entry hosting `@cutweave/editor`; single main editor `BrowserWindow` loading local content from an **`app://` protocol handler registered by main** (so main controls response headers incl. CSP; no `file://`), with native menu skeleton (File/Edit/View/Help, accelerators for Undo/Redo/Save/Open/Export routed to renderer via validated IPC), window state persistence (`electron-window-state`-like own code, tiny).
  - Dev: `pnpm --filter @cutweave/desktop dev` launches Vite + Electron + a dev engine binary from `target/debug/cutweave`; prod build `pnpm --filter @cutweave/desktop package --dir` via electron-builder to an **unpacked app dir** (installers are P7); engine + FFmpeg libs copied into `resources/engine/` via electron-builder `extraResources` (staging script `cargo xtask stage-desktop`).
  - Native Open/Save dialogs (project folder), `shell.showItemInFolder` for export results — via IPC only.
  - Loads the `cutweave-bridge` addon in preload (1H.03 hook point).
  - Playwright-Electron e2e harness (`apps/desktop/e2e/`) with fixtures that launch the packaged-dir app with a temp HOME and a test engine in Null-audio/Software-GPU mode; one smoke test "app starts, window title visible, engine connected indicator".
- **Interfaces:** IPC channel registry `apps/desktop/src/ipc.ts` (typed, allowlisted — used by 1L.02); `pnpm e2e` script.
- **Acceptance criteria:**
  - `pnpm --filter @cutweave/desktop build && pnpm --filter @cutweave/desktop e2e` passes on 3 OSes in CI (Linux under xvfb X11 job and Wayland job both run the smoke test).
  - App start to window shown < 2.5 s (recorded, not gating).
  - Packaged dir contains `resources/engine/cutweave[.exe]` and runs without dev tooling.
- **Tests:** Playwright-Electron smoke.
- **Out of scope:** installers/signing/auto-update (P7), menus beyond skeleton.

### WP-1L.02 — Electron security hardening (arch §18)
- **Area:** `apps/desktop`
- **Depends on:** WP-1L.01
- **Size:** M
- **Executor:** opus (security review by human)
- **Scope:**
  - `webPreferences` (via one `createWindow()` factory): **main editor window**: `contextIsolation: true`, `nodeIntegration: false`, **`sandbox: false` for its preload only** (required to load the napi addon under transport (a) of ADR-001; if ADR-001 chose transport (b) this window is `sandbox: true` as well), preload minimal and audited, narrow `contextBridge`; **every other window** (dialogs, future plugin panels) and all iframes: `sandbox: true`, no preload, no addon; the factory rejects any other combination (unit test), `webSecurity: true`, `allowRunningInsecureContent: false`, `enableBlinkFeatures` none, `webviewTag: false`, `nodeIntegrationInSubFrames: false`; no `remote`.
  - **Strict CSP** set via `session.webRequest.onHeadersReceived` and `<meta>`: `default-src 'none'; script-src 'self'; style-src 'self' (no 'unsafe-inline'; any exception must be justified in the checklist); img-src 'self' data: blob: http://127.0.0.1:<enginePort>; connect-src ws://127.0.0.1:<port> http://127.0.0.1:<port>; font-src 'self'; media-src blob: http://127.0.0.1:<port>; object-src 'none'; frame-src 'none'; base-uri 'none'; form-action 'none'` (**the port is chosen by Electron main at app launch** and passed to the engine with `--port` on every (re)start; the policy is built by one function `buildCsp(port)` with unit tests and **served by main's `app://` protocol handler as response headers**, so it can change without touching disk. If the engine's bind fails (exit 12) main picks a new port, restarts the engine and **reloads the renderer** with the updated CSP).
  - **Validated IPC:** `ipcMain.handle` only for allowlisted channels, each with a **zod schema** for args, `event.senderFrame.url` origin check, no generic `invoke(channel, ...)` exposed — preload exposes only named functions through `contextBridge`.
  - `setWindowOpenHandler` → deny all (external links via `shell.openExternal` only for `https:` and only through an allowlisted IPC); `will-navigate` blocked except app origin; `setPermissionRequestHandler`/`CheckHandler` deny all except none needed; `session.setCertificateVerifyProc` default; block `file://` navigation outside app bundle.
  - Electron fuses flipped at package time (`@electron/fuses`): `RunAsNode=false`, `EnableNodeOptionsEnvironmentVariable=false`, `EnableNodeCliInspectArguments=false`, `EnableEmbeddedAsarIntegrityValidation=true`, `OnlyLoadAppFromAsar=true`.
  - Credential handoff: main generates the **UI token**, sends it to the engine over the spawn handshake pipe (stdin; never argv/env/disk/run-file) and to the renderer only through the preload `contextBridge` getter (never URL, DOM, localStorage or logs). Agent tokens live in the engine's run-file and are never exposed to the renderer.
  - Audit checklist `docs/security/electron-checklist.md` with each item and the test that proves it.
- **Interfaces:** `createWindow(kind)`, `buildCsp(enginePort)`, `app://` protocol handler, `registerIpc(handlers)`, fuses config.
- **Acceptance criteria:**
  - Playwright security tests (all must pass): `window.require`/`process` are `undefined` in the renderer main world (main window has `sandbox:false` for the preload only); a second `BrowserWindow` created through the factory is `sandbox:true` and has no `window.cutweave.preview`; an iframe has no bridge; the window factory throws on `sandbox:false` for a non-main window; `fetch('http://example.com')` blocked by CSP; `fetch(engine /rpc)` without token returns 401; navigation to `https://example.com` is blocked; `window.open` returns null; unregistered IPC channel invocation rejected; an invalid-args IPC call rejected with a zod error.
  - `npx @electron/fuses read --app <packaged>` output matches the expected fuse state (test parses it).
  - Port-change test: force exit 12 on the first start (occupied port); main picks a new port, restarts, reloads the renderer, and the response CSP header (read via Playwright `response.headers()`) contains the new port and not the old.
  - Third-party scan with `electronegativity` (or equivalent) reports no High findings (CI step, allowlist file reviewed by human).
- **Tests:** Playwright security suite; unit tests for CSP builder and IPC validator.
- **Out of scope:** plugin iframe sandbox (P6), code signing.

### WP-1L.03 — Engine supervision, restart and UI resync
- **Area:** `apps/desktop` (main), `packages/protocol` hooks
- **Depends on:** WP-1L.01, WP-1L.02, WP-1C.02, WP-1I.02, WP-1I.04, WP-1N.02
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Main process `EngineSupervisor`: spawn `cutweave engine` with args/env per launch protocol (1I.04), parse readiness line (timeout 10 s), choose the port at launch (`--port`, pick a new free port on exit 12 and on every restart if the previous one is not free; reload the renderer with the new CSP per 1L.02), send the freshly generated **UI token over the spawn handshake pipe** (never written to disk), watch `exit`/`error`; states `starting → ready → crashed → restarting → failed`; exponential backoff restart (0.5 s, 1 s, 2 s; give up after 3 crashes in 60 s ⇒ `failed` with user-visible error dialog offering "Restart"/"Quit"), kill-on-quit (graceful `SIGTERM`/`rpc shutdown`, force kill after 3 s), orphan prevention (engine exits when parent pipe closes).
  - On restart the engine re-opens the same project folder → journal recovery runs **automatically in "auto-recover" mode for supervised restarts** (replay journal; no prompt) — user-initiated cold open of a dirty project shows the restore/discard dialog (1M.07). Documented difference.
  - Renderer connection layer (in `packages/protocol` client + `docStore`): on WS close → state `reconnecting`, queued **in-flight commands retained with `request_id`**, after new `ready` the client re-handshakes (`rpc.hello`), compares `schema_hash`, fetches `project.snapshot`, **re-sends un-acked commands** (idempotent via `request_id`: UUIDv7, 10-min dedup window per arch §5.1, engine dedupes through the journal; no duplicates, no losses), replays `view` state (playhead, selection kept client-side), SHM ring reopened.
  - UI banner "Engine restarted — recovered" for 3 s; failure state blocks editing with a clear message.
  - Test hooks: `CUTWEAVE_TEST_CRASH_AFTER=N` (engine aborts after N commands) and a Playwright helper `killEngine()`.
- **Interfaces:** `EngineSupervisor`, connection state machine in `packages/protocol/src/connection.ts`, IPC `engine:status`.
- **Acceptance criteria:**
  - Security: after a restart the engine accepts the fresh UI token main generates for the new engine process (a token is never reused across processes) and rejects the old agent token's UI-only calls; the UI token is absent from disk (grep of the temp HOME) and from the run-file.
  - Playwright test: edit (3 commands), `killEngine()` (SIGKILL), UI shows `reconnecting`, then recovers in < 5 s; `timeline_get` shows all 3 commands applied exactly once; a command fired **during** the outage is applied exactly once after recovery (**0 lost, 0 duplicated**, repeated 10×).
  - 4 kills in 60 s → app enters `failed` with the dialog (test asserts state).
  - Closing the app leaves no `cutweave` process (test checks process list on 3 OSes).
- **Tests:** Playwright e2e, unit tests for the state machine (fake timers).
- **Out of scope:** crash reporting UI, GPU-process crash handling of Chromium itself.

---

## 1M — Editor UI v0 (`packages/editor`, `packages/ui`, `packages/timeline`)

### WP-1M.01 — Design-system base, app layout, dock skeleton
- **Area:** `packages/ui`, `packages/editor`
- **Depends on:** WP-0A.02, WP-0J.01 (ADR-013)
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Tailwind v4 + Radix + shadcn components copied into `packages/ui` (Button, IconButton, Tooltip, Dialog, Menu, Slider, Input, Progress, Toast, Tabs, ScrollArea); design tokens (dark-first, light stub), CSS variables, `reduced-motion` respect, focus-visible styles; i18next scaffold with `en` namespace and a lint rule banning raw JSX strings in `packages/editor` (Biome rule or script `pnpm lint:i18n`).
  - `packages/editor` shell: CapCut-familiar layout (arch §16): left media panel, center preview, right inspector, bottom timeline, top bar with Open/Save/Export/Undo/Redo; panels in `dockview` with default layout only (no workspace presets); mock-data Storybook-free "playground" page `pnpm --filter @cutweave/editor dev` running in plain Vite (no engine) for UI work in parallel with the backend.
  - Accessibility baseline: all interactive controls have accessible names; `axe-core` check in a Vitest test for the shell.
- **Interfaces:** `@cutweave/ui` exports; `<EditorShell>`.
- **Acceptance criteria:**
  - `pnpm --filter @cutweave/editor test` includes an axe test with 0 violations on the shell; `pnpm lint:i18n` fails on a raw string in a test fixture.
  - Layout renders correctly at 1280×720 and 1920×1080 (Playwright screenshot visual-regression baselines committed, 2 images).
- **Tests:** Vitest + testing-library, Playwright screenshot (Chromium).
- **Out of scope:** real data, dock presets, command palette (P2).

### WP-1M.02 — Protocol client wiring and Zustand stores
- **Area:** `packages/protocol`, `packages/editor`
- **Depends on:** WP-1N.02, WP-1M.01, WP-1I.02
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - `RpcConnection` (in `packages/protocol`): WS connect with token from `window.cutweave.engine.getConnection()`, JSON-RPC req/resp correlation, typed `call<K extends CommandName>(name, args)` using generated types (1N.02), event subscription, reconnect state machine with command queue hooks (completed in 1L.03).
  - `docStore` (Zustand): normalized read-only projection `{rev, project}`; **applies `doc.patched` ops** with the same JSON-Pointer patch semantics (shared `applyPatch` in `packages/protocol`, tested against the Rust-generated fixtures: golden pairs `(doc, ops, doc')` exported by a Rust test into `schema/patch-fixtures.json`, checked by Vitest); gap detection → `project.snapshot` resync.
  - `viewStore` (selection, zoom, scroll, tool mode, panel state; the persisted subset (playhead, zoom, scroll, selection, panel layout, preview quality) is mirrored to the engine via `view.set` debounced 500 ms and restored from `view.get` on open, WP-1C.04), `playbackStore` (playing, rate, playhead, quality, from `playback.state`), `jobStore` (progress). UI never mutates `docStore` directly (store exposes only `applyEvent/resync`; action creators call commands).
  - Hooks: `useCommand`, `useQuery`, selectors for timeline items/media.
- **Interfaces:** `@cutweave/protocol` `RpcConnection`, `applyPatch`; editor stores.
- **Acceptance criteria:**
  - `pnpm --filter @cutweave/protocol test`: patch-application fixtures (≥ 30 pairs from Rust) all pass; doc mirror after 200 random events equals engine snapshot in an integration test against a real headless engine (spawned via `cutweave exec`-compatible harness).
  - A forced gap in revs triggers exactly one `project.snapshot` call (test).
  - Type-level test: calling `call("clip.split", {item: "x"})` (missing `at`) fails `tsc`.
- **Tests:** Vitest unit + integration with real engine in CI (Linux/Windows/macOS).
- **Out of scope:** optimistic updates (not in v1; UI waits for engine ack, latency budget makes it unnecessary).

### WP-1M.03 — Media bin: import, drag-drop, thumbnails
- **Area:** `packages/editor`, `apps/desktop`
- **Depends on:** WP-1M.02, WP-1I.03, WP-1D.07
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Media panel listing `media.list` (virtualized grid/list toggle), asset card: thumbnail from `/media/{id}/thumb` (with token URL), duration, resolution/fps, codec badge, import-progress/ failed badge from `job.progress`/`analysis_error`; broken/missing media state.
  - Import: "Import" button (native file dialog via validated IPC → paths), **OS drag-drop** of files onto the panel (Electron `webUtils.getPathForFile` in preload; validated list sent to `media.import`), multi-select import of ≥ 3 files; progress toasts; removal (`media.remove`, with confirm if referenced → error message from engine surfaced).
  - Drag from media card onto the timeline starts a DnD session consumed by 1M.06 (`dataTransfer` with media id + custom payload through a shared drag store because Chromium restricts DnD data during `dragover`).
  - Keyboard: arrow navigation, Enter = insert at playhead on the selected track.
- **Interfaces:** `<MediaPanel>`, `useImport()`, drag-session store.
- **Acceptance criteria:**
  - Playwright e2e: drop 3 corpus files → 3 cards appear within 2 s (synchronous registration from `media.import`), thumbnails fill in within 10 s as `media.indexed` events arrive; a corrupt file is reported in `failed[]` and shown as an error toast (no card); removing a referenced asset shows the engine's message.
  - axe: 0 violations; list view navigable by keyboard only (e2e test).
- **Tests:** Vitest component tests; Playwright-Electron e2e.
- **Out of scope:** folders/bins, search, tags, proxies.

### WP-1M.04 — Preview panel: SHM → WebGPU canvas, transport controls, overlay shell
- **Area:** `packages/editor`, `apps/desktop`
- **Depends on:** WP-1H.03, WP-1M.02, WP-0J.02
- **Size:** M
- **Executor:** opus
- **Scope:**
  - `<PreviewCanvas>`: WebGPU canvas drawn in `requestAnimationFrame` presenting the newest ring slot with `pts ≤ audio clock` using the upload path chosen by S1 (documented in report); size observer → `view.set_preview_size{w,h}` (device-pixel dims, debounced 50 ms, rounded to even) for live resize.
  - **Fallback path** when WebGPU unavailable/fails or ring missing: poll `/media/frame?...&fmt=jpeg` into a 2D canvas at ≤ 15 fps (arch §10 fallback); banner explains reduced mode; `CUTWEAVE_PREVIEW=jpeg` env forces it (used in CI where no GPU).
  - DOM/Canvas **overlay layer** above the canvas (same page, arch §10 step 4): safe-area guides toggle, a static selection outline for the selected clip bounding box (display-only in v1), timecode readout; no transform handles in v1.
  - Transport bar: play/pause, step ±1 frame, JKL key handling (J/K/L, Space, ←/→), loop off, timecode display (non-drop/drop by fps), quality indicator, volume slider (`view.set_master_gain`), mute.
  - Playhead follows `playback.state`; scrubbing the timeline ruler sends `view.seek` (latest wins).
- **Interfaces:** `<PreviewCanvas>`, `<TransportBar>`, key handlers registered through a `keymap` module (simple map in v1; rebinding is P2).
- **Acceptance criteria:**
  - Playwright e2e (software GPU where needed, `--use-angle=swiftshader` for WebGPU if S1 allows else JPEG fallback path asserted): load 3-clip project, press Space → canvas pixel content changes over time (hash of sampled pixels differs ≥ 10 times in 2 s) and the timecode advances; pause stops it.
  - Window resize: canvas backing size equals CSS size × DPR within 1 px after each resize event (e2e test, 5 sizes); no overlay misalignment (DOM rect assertion).
  - On a reference machine, 60 s playback of a 1080p30 clip: ≥ 59.5 fps average presented, 0 dropped frames reported by the presenter instrumentation (recorded; non-gating in CI).
- **Tests:** Vitest for presenter logic (frame selection by pts), Playwright e2e.
- **Out of scope:** transform handles/crop box, aspect switcher, zero-copy GPU path.

### WP-1M.05 — Timeline v0: Canvas2D virtualized renderer (`packages/timeline`)
- **Area:** `packages/timeline`
- **Depends on:** WP-0J.02, WP-0C.04 (S1 timeline stress learnings), WP-1A.02 (types via 1N.02)
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Framework-agnostic `TimelineView` class: Canvas2D renderer that draws **only visible items** for 1 video + 1 audio track (tracks data-driven; N tracks supported structurally): ruler with timecode ticks adaptive to zoom, track headers (name, mute, lock, hide toggles), clip rectangles with name, thumbnails/filmstrip tiles from `/media/{id}/filmstrip` (bitmap cache keyed by id/zoom bucket, async load with placeholders), audio waveform from `/waveform` as peaks path, playhead line, selection highlight, snapping guide lines (drawn when instructed).
  - Zoom (wheel + ctrl, `+/-`) and horizontal scroll (wheel/shift/drag), time↔pixel mapping in flicks with integer-safe math; DPR-aware crisp rendering; `ResizeObserver`; `requestAnimationFrame` dirty-rect or full redraw only when state changes.
  - Spatial index (interval tree) for hit-testing (`hitTest(x,y) -> {item, zone: Body|LeftEdge|RightEdge|Playhead|Ruler|Header}`).
  - Pure view-model input `TimelineModel{tracks, items, markers?}` + `ViewState{zoom, scrollX, playhead, selection}` → no engine dependency (testable with fixtures).
- **Interfaces:** `@cutweave/timeline`: `TimelineView`, `TimelineModel`, `hitTest`, events `onPointer(zone,…)` consumed by 1M.06.
- **Acceptance criteria:**
  - Vitest: time↔pixel round-trips exactly at 9 zoom levels; hit-test returns correct zone for 20 fixture points; visible-item culling draws ≤ (visible count + 2) rects for a 5,000-item fixture (spy on ctx calls).
  - Bench in Playwright/Chromium: pan/zoom sweep over 5,000 items ≥ 55 fps on the dev machine (p5 of frame rate; recorded in `bench/` using the 0I.03 result schema; gating threshold 45 fps on CI).
  - Visual regression: 3 screenshot baselines (empty, 3 clips with filmstrips, zoomed-in with waveform).
- **Tests:** Vitest (+ `vitest-canvas-mock`), Playwright visual + perf.
- **Out of scope:** interactions (1M.06), multi-track UI polish, keyframe lanes, markers.

### WP-1M.06 — Timeline interactions: select, move, trim, split, delete, snapping
- **Area:** `packages/timeline`, `packages/editor`
- **Depends on:** WP-1M.05, WP-1M.02, WP-1B.06, WP-1M.03, WP-1B.09
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Explicit **interaction state machine** (arch §16; XState-lite own implementation, pure & unit-testable): states `Idle → Hovering → Selecting | Moving | TrimmingStart | TrimmingEnd | ScrubbingRuler | DroppingMedia | Panning`; transitions emit **intents** `{kind: Move, item, to_track, at}` etc., which the editor layer converts into commands.
  - Gestures use **gesture transactions** (arch §5.2): pointer-down on a clip begins `edit.begin_tx{kind:"gesture"}`; drag sends `clip.move`/`clip.trim` updates throttled to animation frames (engine applies live → preview updates); pointer-up `commit_tx`; Esc cancels (`cancel_tx`) restoring state; each gesture = exactly **one** undo step.
  - Operations: click select (+shift multi-select, click empty deselect, marquee optional), move within and across same-kind tracks, trim edges, **split at playhead** (`S` key / toolbar button, applies to selected or clip under playhead), **delete** (`Delete/Backspace`), drop media from bin to timeline (`timeline.insert` at pointer time/track, snaps), playhead scrub on ruler.
  - **Snapping** to playhead, clip edges and frame grid within 8 px (toggle `N`), computed client-side in flicks; commands always carry frame-aligned times.
  - Engine errors (`Invalid` overlap/`Locked`) revert the gesture-tx state and show a toast; no client-side duplicate validation beyond snapping.
- **Interfaces:** `@cutweave/timeline` state machine API `reduce(state, event) → {state, intents}`; editor `useTimelineIntents()`.
- **Acceptance criteria:**
  - State-machine Vitest suite ≥ 40 cases (every transition incl. Esc cancel, drag outside bounds, trim beyond media length).
  - Playwright e2e against a real engine: drag clip 100 px → doc shows moved start (frame-aligned), one undo entry (`edit.history` length +1), undo restores; trim, split-at-playhead, delete behave per `edit-semantics.md` (6 e2e tests).
  - 60 pointer-move events during a drag produce ≤ 60 engine updates and exactly 1 history entry; UI stays responsive (no long task > 50 ms, measured via PerformanceObserver in the test).
- **Tests:** Vitest state machine, Playwright e2e.
- **Out of scope:** ripple/roll/slip/slide, multi-clip group moves with ripple, copy/paste, markers.

### WP-1M.07 — Inspector, undo/redo UI, save/open, recovery dialog
- **Area:** `packages/editor`
- **Depends on:** WP-1M.02, WP-1M.06, WP-1C.03, WP-1B.04
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - **Minimal inspector** (right panel) for the selected clip: name, start/duration (**numeric/timecode inputs that dispatch `clip.move` / `clip.trim`**, so every timeline edit has a non-pointer path used by the M1 acceptance test; timecode inputs with parse via a TS port of the `Time` timecode parser (arch §4.1) tested against shared fixtures), opacity slider, transform (x, y, scale, rotation), audio gain dB; edits send `item.set_props` using **gesture tx while dragging sliders** and commit on release; shows "Multiple selection — mixed" placeholders; media info panel when a bin asset is selected (probe info). (Schema-driven renderer is P2; this one is hand-built but takes field definitions from one typed table so migration is mechanical.)
  - **Undo/redo:** toolbar buttons + `Ctrl/Cmd+Z`, `Shift+Ctrl/Cmd+Z`/`Ctrl+Y`; disabled state from `edit.history` events; labels (“Undo Split clip”).
  - **Project menu actions:** New (dialog for name/location → `project.new`+`save_as`), Open (native dialog → `project.open`), Save (`Ctrl/Cmd+S`), Save As, dirty indicator in title (`project.dirty`), prompt on close with unsaved changes (Save/Discard/Cancel), read-only banner for newer-schema projects.
  - **Recovery dialog:** on `project.recovery_available` (cold open): "Restore unsaved changes (N edits, last at HH:MM)" / "Discard" → `project.recover` / `project.discard_recovery`.
  - Recent projects list (stored by main process, validated IPC).
- **Interfaces:** `<Inspector>`, `<UndoRedoButtons>`, menu IPC handlers.
- **Acceptance criteria:**
  - Playwright e2e: change opacity via slider drag → one undo entry; `Ctrl+Z` restores; `Ctrl+S` clears dirty dot; closing with dirty state shows the prompt; reopening a project saved by the app restores the identical timeline (`timeline_get` equal).
  - Recovery e2e: create edits, SIGKILL app + engine without saving, relaunch → dialog appears; Restore yields all edits (**0 lost**); Discard yields the last saved state.
  - TS `parseTime` passes the shared fixture file generated by Rust tests (≥ 40 cases).
- **Tests:** Vitest + Playwright.
- **Out of scope:** schema-driven inspector, keyframing UI, color/effects panels.

### WP-1M.08 — Export dialog and progress
- **Area:** `packages/editor`, `apps/desktop`
- **Depends on:** WP-1M.07, WP-1E.03
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - Export dialog: output path (native save dialog via IPC, default `<project>/<name>.mp4`), preset (single: "MP4 H.264 1080p"), resolution/fps readonly from sequence with override fields (width, height, fps), quality (Low/Medium/High mapped to `quality`), range (entire sequence | in/out not in P1), "Use hardware encoder" toggle (auto default; shows detected encoder name from capabilities query), overwrite confirm.
  - Starts `export.start`; modal progress (percent, fps, ETA, cancel); on success toast "Reveal in folder" (IPC `showItemInFolder`), failure shows engine error message; export continues if the dialog is dismissed (status via `jobStore`, small status chip in top bar).
  - Disable Export when timeline empty.
- **Interfaces:** `<ExportDialog>`; capability query `media.capabilities` (q, added in `cutweave-ops` here as a read-only wrapper over `MediaBackend::capabilities` — one-line addition acknowledged in PR).
- **Acceptance criteria:**
  - Playwright e2e (**M1 exit 1**, also run in 1O.04): import 3 corpus clips, place, trim, split, export 1080p MP4 via dialog → file exists, `ffprobe` valid, duration within 1 frame of the timeline, plays without error in Chromium `<video>` (test loads it and asserts `readyState>=3`, `videoWidth==1920`).
  - Cancel button stops export and no file remains.
- **Tests:** Vitest + Playwright.
- **Out of scope:** presets, queue, HEVC/AV1, range export.

---

## 1N — Codegen & protocol package

### WP-1N.01 — Schema emission + snapshot gate
- **Area:** `crates/cutweave-commands`, `xtask`, `schema/`
- **Depends on:** WP-1B.06, WP-0H.01
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - `cargo xtask gen` builds a small `cutweave-gen` bin (in `xtask`, depending on `cutweave-ops`) that dumps: `schema/commands.json` and `schema/queries.json` (full catalog: name, summary, flags, mcp exposure, annotations, input/output JSON Schema, error codes), `schema/events.json` (event catalog from 1I.02/1C.03), `schema/patch-fixtures.json` (for 1M.02), `schema/mcp-tools.json` (core tool names/annotations as produced by 1J.01), and `docs/reference/commands.md` (human table).
  - **Snapshot gate (arch §5.3):** `cargo xtask gen --check` regenerates into a temp dir and fails with a unified diff if committed files differ; plus `insta` snapshot tests for `schema/commands.json` in `cutweave-ops` so a diff requires `cargo insta review` approval. `schema_hash` (sha256 of commands.json) embedded in the engine (`rpc.hello`) and in `packages/protocol` generated constants.
  - Deterministic output (sorted keys, stable formatting); CI job `schema-snapshot` (0A.03) calls `--check`.
- **Interfaces:** `schema/*.json`, `docs/reference/commands.md`, `SCHEMA_HASH`.
- **Acceptance criteria:**
  - `cargo xtask gen --check` exits 0 on main; after adding a field to a command without regenerating, it exits non-zero printing the diff (verified in a negative test in `xtask` tests using a temp fixture crate or a feature flag).
  - Running `gen` twice yields byte-identical files; all registry commands appear (count equals `cutweave commands --json | jq length`).
- **Tests:** xtask unit tests + insta snapshot.
- **Out of scope:** TS output (1N.02), per-surface (CLI/MCP) docs beyond the above.

### WP-1N.02 — TS protocol package: generated types and typed RPC client
- **Area:** `packages/protocol`, `xtask`
- **Depends on:** WP-1N.01, WP-1A.02, WP-0H.01
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - `cargo xtask gen` additionally emits `packages/protocol/src/generated/{types.ts, commands.ts, events.ts, schema-hash.ts}` via **specta** (`specta-typescript`): all model types (`Project`, `Item`, `Time` as TS `number` per arch §4.1 / ADR-015, IDs as branded string types `MediaId = string & {__brand}`), command input/output maps `CommandMap = { "clip.split": {input: SplitItem, output: SplitOutput}, … }`, `EventMap`, and `CmdErrorCode` union. Header comment `// @generated — do not edit; run cargo xtask gen`.
  - Hand-written (small) `packages/protocol/src/{rpc.ts, patch.ts, index.ts}`: typed `RpcClient.call<K extends keyof CommandMap>(name, input) => Promise<CommandMap[K]["output"]>` and `on<E extends keyof EventMap>`; `applyPatch` (1M.02 completes use).
  - Biome ignores `generated/`; `tsc --strict` compiles; package builds ESM + d.ts via `tsup`.
  - CI gate: the `schema-snapshot` job runs `cargo xtask gen --check` including TS files, so a Rust change without regenerated TS fails.
- **Interfaces:** `@cutweave/protocol` public API as above; `docs/reference/protocol.md` (usage).
- **Acceptance criteria:**
  - `pnpm --filter @cutweave/protocol build && pnpm --filter @cutweave/protocol test` pass; a type test (`tsd`/`expectTypeOf`) proves `call("clip.split", {item, at})` typed and `call("clip.split", {item})` is a compile error; `call("nope")` is a compile error.
  - Changing `SplitItem` in Rust without regenerating makes `cargo xtask gen --check` fail (as in 1N.01).
  - Generated files contain no `any` (grep test) and no `bigint` anywhere (grep) and `Time` is `number` per ADR-015.
- **Tests:** Vitest + type tests.
- **Out of scope:** `@cutweave/sdk` plugin client (P6).

---

## 1O — CI quality gates

### WP-1M.09 — Custom title bar: native window controls, in-app menu bar
- **Area:** `apps/desktop` (window options, menu, IPC), `packages/editor` (top bar), `packages/ui` (menu bar component, tokens), `apps/desktop/e2e/flows` (window and menu flow), `docs/user-flows.md`, `docs/01-architecture.md` (desktop shell note)
- **Depends on:** WP-1M.01, WP-1M.07
- **Size:** M
- **Executor:** opus
- **Scope:**
  - **Remove the native title bar and menu bar on Windows and Linux** (`titleBarStyle: "hidden"` with `titleBarOverlay`).
    - The OS still draws its own minimize, maximize/restore and close buttons over the app: Windows 11 Snap Layouts on maximize hover, Aero snap by dragging, keyboard and screen-reader access all keep working.
    - The overlay's color and symbol color follow the theme (dark/light) through design tokens.
    - Its height matches the top bar token.
  - **The top bar becomes the title bar.**
    - Its empty areas are the drag region (`-webkit-app-region: drag`); every control in it is `no-drag`.
    - Double-click on the drag region maximizes or restores.
    - The layout reserves the overlay's area through `env(titlebar-area-*)` / `navigator.windowControlsOverlay`.
    - The window title (project name and unsaved state) stays in `document.title` for the taskbar and Alt+Tab.
  - **An in-app menu bar** (File, Edit, View, Help) built from the `packages/ui` menu components, with exactly what the native menu has today:
    - File: New Project, Open Project, Open Recent, Save, Save As, Export, Exit;
    - Edit: Undo, Redo, Cut, Copy, Paste, Select All;
    - View: Toggle Full Screen, plus Reload and Toggle Developer Tools in dev builds only;
    - Help: About Cutweave.
    - Shortcuts work with or without the menu open: Ctrl+N/O/S/Shift+S/M, Ctrl+Z/Y, F11.
    - Alt (or F10) focuses the menu bar and the arrow keys move through it, as in a native Windows menu.
    - Main-process actions (quit, full screen, dev tools, about) go through new allowlisted, zod-validated IPC channels. `SECURITY-MODEL.md` gets a row for them.
  - **macOS keeps the native app menu** at the top of the screen and the traffic lights (`hiddenInset`); the in-app menu bar is hidden there. Record the per-OS behaviour in arch §1/§2 (desktop shell).
- **Interfaces:** `<TitleBar>`/top-bar drag region; `<MenuBar>` in `packages/ui`; IPC `app:quit`, `window:toggleFullScreen`, `app:about` (and dev-only `window:toggleDevTools`, `window:reload`).
- **Acceptance criteria:**
  - On Windows the native title bar and menu bar are gone. The overlay controls are visible, follow the theme and match the top bar's height. Snap Layouts appear on maximize hover (manual check, recorded with a screenshot in the PR).
  - **A user flow (`e2e/flows`, ADR-0018)** drives every menu item through the in-app menu bar with real clicks and with its shortcut, and checks the outcome: project created, opened, saved, exported where available, undone, full screen toggled, About shown, the app quits through File > Exit answering the unsaved-changes dialog. It also checks that dragging and double-clicking the title area moves and maximizes the window, by reading the window's bounds and state back.
  - Visual baselines for the menu bar (closed, open, keyboard focus) in dark and light; axe clean.
  - No regression in the existing e2e and user-flow suites.
- **Tests:** unit (menu model, IPC validation), Storybook visual, e2e user flow.
- **Out of scope:** a command palette (P2); custom-drawn window buttons (the OS draws them).

### WP-1M.10 — Autosave settings and the title bar's save status
- **Area:** `crates/cutweave-engine` (autosave, session engine command), `crates/cutweave-ops` only if the command must live there, generated schema and TS types, `packages/editor` (Settings > Saving, title-bar save status, preference persistence), `apps/desktop/e2e/flows`, `docs/user-flows.md`, `docs/01-architecture.md` §17
- **Depends on:** WP-1C.03, WP-1M.07, WP-1M.09, WP-1I.05
- **Size:** S
- **Executor:** sonnet
- **Scope:** (product owner request, 2026-11-24) saving is automatic and configurable; the title bar keeps no Save button (File > Save, Ctrl+S remain).
  - **App-wide preference** in File > Settings… > Saving: "Save automatically" on/off; while editing, save at least every 30 s / 1 min / 2 min / 5 min; backups to keep (5 / 10 / 20). Kept with the editor's other preferences (like the theme), so it applies to every project.
  - **The editor applies it to its engine** after every handshake and on change, through a new engine command (not undoable, not journaled, does not make the project dirty) that sets the session's autosave config: enabled, interval, kept backups. The idle save (2 s after the last edit) stays. Engines without an app (headless, agents) keep using the project's `settings.autosave`, then the defaults.
  - **Title-bar save status** (replacing the Save button): an icon with a tooltip, "All changes saved", "Unsaved changes" (with "saving automatically" or "autosave is off"), "Saving…", and "Autosave failed" from `project.autosave_failed`. The unsaved state keeps `data-testid="dirty-dot"`.
  - Arch §17: record the precedence (the app's preference for its own engine, the project setting otherwise).
- **Interfaces:** engine command for the autosave config (name it in the session namespace, e.g. `project.set_autosave{enabled, interval_s, keep_backups}`); Settings > Saving; title-bar save status.
- **Acceptance criteria:**
  - Unit: the engine applies a new config at once (an interval change takes effect for the current dirty period); disabling stops autosaves; the command never marks the project dirty; kept backups follow the setting.
  - **A user flow (ADR-0018):** in Settings turn autosave off, edit, wait past the interval: the status says unsaved and the file on disk is unchanged. Turn it on with 30 s: the edit is saved, the status says saved. The choice survives a relaunch.
  - The title bar has no Save button; File > Save and Ctrl+S still save.
- **Tests:** unit, Vitest, user flow.
- **Out of scope:** cloud sync, version history UI (backups stay files in `.cutweave/backups/`).

### WP-1M.11 — Settings framework: tabbed pages, search, preferences store, reset, import and export
- **Area:** `apps/desktop` (preferences store and IPC in main, values main needs before the window exists), `packages/editor` (settings dialog, page registry, theme migration), `packages/ui` (`SettingsLayout`, `SettingRow`; specs in `design/components/`), one preferences schema module that main and the editor both import (a small `packages/preferences` workspace package, or another single home; justify it in the PR), `docs/reference/settings.md`, `apps/desktop/e2e/flows`, `docs/user-flows.md`
- **Depends on:** WP-1M.10, WP-1M.09, WP-1M.12
- **Size:** M
- **Executor:** sonnet
- **Scope:** (product owner request, 2026-11-24: "tabbed with categories … everything that can be customizable, will be") turn the quick File > Settings… (one Theme row) into the frame that every settings page plugs into. The full inventory (every setting with its default, scope, storage, agent access and owning WP) is the catalog `docs/reference/settings.md`; this WP builds the frame and the pages whose features exist with no new engine work. The other P1 pages are WP-1M.13 (Export, Keyboard shortcuts), WP-1J.05 (Agents and MCP, Notifications) and WP-1D.08 (Cache and storage, Performance and hardware); later pages come with their features.
  - **Three homes** (arch paragraph below): *app preferences* (the person's, every project), *this computer* (machine values: the same file, a `machine` group that export and import skip), and *project settings* (in the document, undoable, agents read and change them; edited in File > Project settings…, WP-2H.08, not in this dialog). Secrets never go in preferences: keys go to the keychain through write-only `ai.set_key` (arch §13).
  - **Preferences store in main:** `<userData>/preferences.json` next to `recent-projects.json` and `window-state.json`. One versioned schema (keys, types, defaults, ranges); a damaged or partly invalid file never blocks startup (bad keys fall back to their defaults, the file is rewritten clean, a warning goes to the main log); atomic writes (temp file, fsync, rename); schema migrations like the project's. The renderer gets the snapshot before first paint and subscribes to changes; it writes one key at a time through new allowlisted, zod-validated IPC (`prefs:get`, `prefs:set`, `prefs:reset`, event `prefs:changed`), with a row in `SECURITY-MODEL.md`.
  - **Apply plumbing:** each key says who applies it: the renderer, main, the engine through a UI-only command after each handshake and on change (WP-1M.10's pattern), or main at engine start (a flag). Rows that need the engine are disabled with "Needs the engine" while it is disconnected; rows that apply at the next start say so.
  - **Moves what exists onto the store:** the theme (read once from the renderer's old `localStorage` value, then removed there) and WP-1M.10's Saving rows. Main reads the theme before it creates the window, so the window background and the title-bar overlay open in the right colors with no flash.
  - **The frame:** a large modal dialog in the main window (other windows are sandboxed with no preload and cannot reach the engine, arch §2): a vertical side tab list of pages grouped as in the catalog (Preferences; Agents and AI; Extensions; Privacy and updates; This computer; Advanced), one page per category, and a search field above the tabs (focused on open). Each page is a list of `SettingRow`s: label, control, one-line description, and a reset button when the value differs from its default. Changes apply at once; "Done" closes. A page appears once it has a shipped row.
  - **Page registry:** pages and rows are data (`{id, title, icon, group, order, rows: [{key, label, description, keywords, control, applies}]}`, controls: switch, segmented, select, number with unit, folder, list, action, custom). Later WPs register a page or a row with no change to the frame. A test checks that every registered key has a row in `docs/reference/settings.md`.
  - **Search** across every page: matches labels, descriptions and keywords in the UI language, shows the matching rows grouped by page with the match marked, and "No settings match "x"" otherwise; Esc clears it and returns to the page that was open.
  - **Reset:** per row; "Reset page"; "Reset all settings…" (confirmed) in Advanced. Resets never touch recent projects, window positions or keys.
  - **Import and export** in Advanced: "Export settings…" writes App rows only (no machine values, keys, recent projects or window state) as JSON with the schema version; "Import settings…" validates every key, applies the valid ones and reports what it skipped. Also "Open settings folder" and "Open logs folder".
  - **Deep links:** `openSettings(page, key?)` opens the dialog at a row (for example from the title bar's save-status tooltip to Saving and backups). The command palette (WP-2H.04) lists pages and rows through it.
  - **Pages in this WP:**
    - General: on startup (open the last project or start with an empty project; the welcome screen option arrives with WP-3I.02); default folder for new projects (used by File > New project…); recent projects to list, and "Clear recent projects"; "Reset 'Don't ask again' choices" (the mechanism dialogs use to remember a choice).
    - Appearance: theme Dark / Light / Match system (`nativeTheme`, follows OS changes live).
    - Saving and backups: WP-1M.10's rows on the store.
    - Advanced: as above.
  - **Placement:** File > Settings… (Ctrl+,) on Windows and Linux; Cutweave > Settings… (⌘,) in the macOS app menu (already wired); one dialog at a time.
  - **Proposed arch paragraph** (§16 UI architecture, with a pointer from §17; for the integrator to place): "**Settings (WP-1M.11).** Settings have three homes. *App preferences* are the person's choices for every project (theme, startup, saving, editing defaults, shortcuts, agents' folders, AI routing); *this computer's settings* belong to the installation (cache folder and limits, hardware, audio devices). Both live in `preferences.json` in the app's user-data folder, owned by Electron main, which validates every write against one versioned schema shared with the editor and broadcasts changes; export and import carry app preferences only. *Project settings* live in the document (`project.settings`) and change only through undoable commands, so agents read and change them like any edit. Defaults for new projects are preferences that `project.new` copies into the document; after that the project's value rules. A preference that changes engine behaviour is applied by the app to its own engine, at spawn as a flag or after each handshake through an engine command in a UI-only scope (ADR-0016); headless engines take flags, environment variables and project settings instead. Agents read the effective values (`project.info`, `media.cache_status`) but never change preferences. No secret is stored in preferences: keys go to the OS keychain through write-only `ai.set_key` (§13). The catalog of every setting is `docs/reference/settings.md`."
- **Interfaces:** `preferences.json` schema v1; IPC `prefs:get`, `prefs:set`, `prefs:reset`, `prefs:changed`, `prefs:export`, `prefs:import`; `registerSettingsPage`, `registerSettingRow`, `openSettings(page, key?)`; `<SettingsLayout>`, `<SettingRow>` in `packages/ui`.
- **Acceptance criteria:**
  - Unit (main): a damaged file, an unknown key, a wrong type and an out-of-range value each start with defaults for that key and a warning; writes are atomic (a kill between temp write and rename leaves the old file); a v0 fixture migrates.
  - Unit (IPC): unknown keys, wrong types and calls from another frame are rejected; export never contains `machine` values, recent projects or window state; import skips invalid keys and reports them.
  - The registry test: every registered key has a row in `docs/reference/settings.md`.
  - **A user flow (ADR-0018, F19):** open Settings with Ctrl+, and from File; type "auto" and the Saving and backups rows show; change the theme to Light and "On startup" to open the last project; reset the startup row and see its default; export to a file (OS save picker stubbed); Reset all; import the file: the theme comes back; relaunch: the values persist and the window opens in the chosen theme.
  - F16 keeps passing on the store; a profile holding only the old `localStorage` theme keeps that theme after the update.
  - axe: 0 violations; visual baselines of the dialog (General page, a search result, the empty search) in dark and light.
- **Tests:** unit (store, migration, IPC), Vitest (registry, search, reset, dialog), visual, user flow.
- **Out of scope:** project settings (WP-2H.08); the pages listed in the catalog for other WPs; settings sync between machines; per-workspace preferences.

### WP-1M.12 — Find media in the bin: search, filters, sort
- **Area:** `crates/cutweave-ops` (`media.list` filter and sort), generated schema and TS types, `packages/editor` (media panel), `packages/ui` (`SearchField`; spec in `design/components/`), `apps/desktop/e2e/flows`, `docs/user-flows.md`
- **Depends on:** WP-1M.03, WP-1D.07
- **Size:** M
- **Executor:** sonnet
- **Scope:** (product owner request, 2026-11-24) a fast way to find media by name, kind, length and use, the same way for people and agents. Tags, notes, ratings and saved searches stay in WP-2F.06; the folder scope arrives with WP-2F.08.
  - **One filter, two implementations, one fixture.** `MediaFilter{text?, kinds?, min_duration?, max_duration?, used?, problems?}` and `MediaSort{by: imported|name|duration|kind, order: asc|desc}`.
    - `text`: every whitespace-separated term must occur in the asset's name (the file name until assets can be renamed, WP-2F.07), ignoring case and accents.
    - `used`: placed on a timeline of any sequence (or not).
    - `problems`: assets nothing can be read from (no streams; offline media joins with WP-2F.03). Analysis failures are events, not document state (arch §7), so they stay a badge, not a filter.
    - Durations are flicks; stills have none and match only when no duration bound is set. "Imported" order is ID order (UUIDv7 IDs are time-ordered).
    - The engine's `media.list` (a core MCP tool, so no new tool and no tool-list growth) gains optional `filter` and `sort`; paging keeps `cursor` = the last asset's ID, read in the requested order.
    - The editor filters its document mirror with the TS predicate, with no round trip per keystroke. A fixture file of documents, filters and expected IDs, generated by a Rust test, is checked by both implementations (as the timecode parser's fixtures are).
  - **Search field** at the top of the Media tab ("Search media", a clear button): Esc clears, a second Esc leaves the field; ↓ moves to the first result; Ctrl/Cmd+F focuses it while the Assets panel has focus (rebindable with WP-2H.05).
  - **Filters** button with a count badge, opening a popover: Kind (Video, Audio, Image), Duration (any, under 10 s, 10 s–1 min, 1–5 min, over 5 min), Use (any, on the timeline, not used), and "Only media with problems". Active filters show as removable chips under the field, with "Clear all".
  - **Sort** menu: date imported (newest first, the default, or oldest first), name, duration, kind. Sort and filters persist with the panel's view state; the search text is kept for the session and cleared when another project opens.
  - **Results** use the existing grid and list with "12 of 340" in the toolbar; arrow keys, Enter-to-insert and dragging to the timeline work on the results; a selected asset the filter hides stays selected.
  - **Empty states:** no media yet (unchanged); "No media matches "x"" with "Clear search"; with filters on, "Clear filters" too.
- **Interfaces:** `media.list{limit?, cursor?, filter?, sort?}` (additive, backward compatible); `MediaFilter`, `MediaSort`; `<SearchField>` in `packages/ui` (WP-1M.11 reuses it).
- **Acceptance criteria:**
  - The Rust and TS predicates agree on the shared fixture (at least 40 cases: accents, several terms, stills with duration bounds, used and unused across two sequences, every sort, paging under each sort).
  - Filtering 10,000 assets in the editor takes < 8 ms p95 per keystroke (Vitest bench); `media.list` with a filter over 10,000 assets < 20 ms p95 (Rust bench).
  - MCP: `media_list` with `filter.text` returns the IDs the bin shows for the same text (scripted client on the fixture).
  - **A user flow (ADR-0018, F21):** import the demo media; type part of a name: only matching cards remain and the count says so; add Kind = Audio, then Not used after placing one clip; sort by name; a search with no match shows the empty state and "Clear search" brings everything back; ↓ and Enter insert a result at the playhead.
  - axe: 0 violations; visual baselines of the toolbar with chips in dark and light.
- **Tests:** Rust unit and fixture generation, Vitest (predicate, fixture, panel), benches, user flow.
- **Out of scope:** tags, notes, ratings, saved searches and smart collections (WP-2F.06); the folder scope (WP-2F.08); transcript text (P3) and content search (P5); offline status (WP-2F.03).

### WP-1M.13 — Settings pages for existing features: Export defaults, Keyboard shortcuts (read-only)
- **Area:** `packages/editor` (Export and Keyboard shortcuts pages, export dialog defaults, the keymap and menu tables as one listable source), `docs/reference/settings.md`, `apps/desktop/e2e/flows`, `docs/user-flows.md`
- **Depends on:** WP-1M.11, WP-1M.08
- **Size:** S
- **Executor:** sonnet
- **Scope:** (product owner request, 2026-11-24) the catalog's P1 pages for features that already exist and need no engine change.
  - **Export page:** save exports in the project folder (default), the last folder used, or a chosen folder; default quality Low / Medium (default) / High; use hardware encoders Auto (default) / Off. The export dialog (WP-1M.08) starts from these; choosing a different folder in the dialog updates "last folder used" only.
  - **Keyboard shortcuts page (read-only until WP-2H.05):** every shortcut the app has (menu bar, transport, timeline, bin, dialogs), grouped by where it works, searchable from the page and from the dialog's search, with the OS's modifier names. The list is generated from the same tables the handlers use (the menu model and the keymap module), so it cannot drift; a note says rebinding arrives with the shortcut editor.
- **Interfaces:** preference keys `export.folder`, `export.quality`, `export.hw`; one exported table of bindings (`listBindings()`).
- **Acceptance criteria:**
  - Vitest: every key handler registered by the editor appears in `listBindings()` (a test walks the handlers' table), and the page renders them all.
  - **A user flow (ADR-0018, F19 extended):** set the export quality to High, hardware encoders Off and a chosen folder (picker stubbed); File > Export… opens with High, the hardware toggle off and a path in that folder. On the Keyboard shortcuts page, searching "split" finds S.
  - axe: 0 violations on both pages.
- **Tests:** Vitest, user flow.
- **Out of scope:** export presets and file-name patterns (WP-2G.04); editing shortcuts, presets and conflicts (WP-2H.05).

### WP-1O.01 — Golden-frame test infrastructure
- **Area:** `crates/cutweave-render/tests`, `testdata/golden`, `xtask`, `.github/workflows`
- **Depends on:** WP-1F.03, WP-0J.02, WP-0F.03 (adapter behaviour notes), WP-0I.01, WP-0I.04 (macOS reference runner)
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Golden **test projects** as `.cutweave` fixtures (generated by `cargo xtask golden-projects` from `cutweave-ops` commands so they stay schema-valid): (1) single synthetic color-bars clip at 100%, (2) clip with scale 50% + position offset, (3) opacity 50% over a second track, (4) rotated phone clip metadata rotation, (5) BT.601 and full-range tagged sources, (6) 2 clips with a cut at frame N — each rendered at 3 timestamps. All sources are synthetic corpus items (`lavfi` generated, 0I.01) so no licensing issues.
  - Test runner `cutweave render --frames "t1,t2,t3" --out-dir …` (headless, **software adapter** forced: WARP on Windows, lavapipe on Linux; macOS uses Metal: on the hosted `macos-14` runner only if WP-0A.03 verified a usable Metal adapter, otherwise **only on the self-hosted `ref-mac-m1` runner**) → PNGs; comparator crate `cutweave-golden` (dev-dependency module in `xtask` or `crates/cutweave-render/tests/common`) computing **PSNR, SSIM (and FLIP via `flip_evaluator` crate if S4 recommended)**, **tolerance policy (single source)**: regression vs the stored **per-adapter reference** passes only if SSIM ≥ 0.995 **and** PSNR ≥ 40 dB; no per-OS overrides; cross-adapter comparison is never done; on failure writes `actual/expected/diff` PNGs as CI artifacts.
  - Reference frames committed as lossless PNG (< 150 KB each, total < 10 MB, tracked with Git LFS per 06-dev-process §2); references live per adapter class (`expected/lavapipe/`, `expected/warp/`, `expected/metal/`); `cargo xtask golden --update` regenerates the references **for the adapter it runs on** and requires `--i-know` + PR label `golden-update`; CI check fails if references change without that label.
  - Determinism test: render each golden twice → byte-identical on the same adapter; **preview vs pre-encode export frames bit-identical** (compare the 1E.03 readback path with `render.frame`).
  - Workflow job `golden-frames` on 3 OSes.
- **Interfaces:** `just golden [--update]` (delegates to `cargo xtask golden`), `testdata/golden/{projects,expected/<adapter>}`.
- **Acceptance criteria:**
  - `cargo xtask golden` passes on ubuntu-24.04 (lavapipe), windows-latest (WARP) and macOS (hosted `macos-14` if Metal is usable, else `ref-mac-m1`) within the tolerance policy; a deliberate 1-pixel transform offset change makes it fail with a diff artifact (negative test in PR evidence).
  - ≥ 6 projects × 3 frames = 18 goldens; total runtime < 3 min per OS.
  - Reference-update guard: modifying a PNG without the label fails the `golden-frames` job.
- **Tests:** this WP is test infra; comparator has unit tests (identical, off-by-1, shifted images).
- **Out of scope:** effects/text goldens (P3/P4), HW-adapter goldens.

### WP-1O.02 — CLI = MCP = RPC parity test
- **Area:** `crates/cutweave-api/tests/parity`, `xtask`
- **Depends on:** WP-1J.03, WP-1K.01, WP-1I.02
- **Size:** M
- **Executor:** opus
- **Scope:**
  - A **command script** format (`testdata/scripts/*.json`: ordered `[{cmd,args,expect?}]` with `$ref` between steps) executed three ways against fresh headless engines with `--deterministic-ids 42` (WP-1B.08) and a fixed clock: (a) `cutweave exec` (CLI), (b) MCP over stdio headless via `cutweave_execute`/tool calls using an rmcp test client, (c) raw WS JSON-RPC client. After each run: `project.save` and compare the resulting `.cutweave` files **byte-for-byte** (arch §20 "byte-identical") and the sequence of `doc.patched` ops.
  - **Registry-enumeration parity check** (arch §14): for every registered non-`view` command assert it is reachable via MCP (core tool or via `cutweave_execute`/`cutweave_describe_command`) with an input schema **identical** to the RPC/registry schema (JSON equality after normalization), and via CLI `describe`.
  - ≥ 4 scripts: basic edit, split/trim/move/delete + undo/redo, error cases (expected error codes identical across surfaces), `cutweave_execute` rollback.
  - Runs in CI on 3 OSes as `parity` job.
- **Interfaces:** script format doc `testdata/scripts/README.md`; test crate `parity`.
- **Acceptance criteria:**
  - `cargo nextest run -p cutweave-api --test parity` passes: for all scripts, the three `.cutweave` outputs have equal sha256 and equal error codes/messages' `code` fields.
  - Adding a new command with `mcp="hidden"` and no exemption fails the enumeration test with a message naming it (exemption list file `schema/parity-exempt.toml`, reviewed by CODEOWNERS).
- **Tests:** this WP is the test.
- **Out of scope:** plugin commands, LLM-driven runs.

### WP-1O.03 — MCP conformance + scripted-agent e2e
- **Area:** `crates/cutweave-api/tests/mcp`, `.github/workflows`
- **Depends on:** WP-1J.03
- **Size:** S
- **Executor:** sonnet
- **Scope:**
  - CI job running **MCP Inspector CLI** (`npx @modelcontextprotocol/inspector --cli …`, version pinned) against both stdio (`cutweave mcp --headless`) and Streamable HTTP (engine with token) transports: `tools/list` (asserts <= 19 tools / <= 5k tokens), call of each core tool on the fixture project (including `media_import` -> `job_wait`), `resources/list`/`read`, ping; assertions on result shapes (JSON schema validation of tool outputs vs output schemas).
  - **Scripted-agent e2e**: a deterministic TS/Rust client script (not an LLM) "imports 3 clips (`media_import` returns `media_ids`; `job_wait` for analysis), arranges, trims, splits, renders a frame, exports (`export_start` + `job_wait`)" over stdio headless using only MCP tools (this is the M1 exit-2 flow): asserts no non-protocol stdout, final timeline structure equals expected JSON, `render_frame` image decodes.
  - Tool-list budget assertion (1J.01 limit) and `tools/list` insta snapshot gate.
- **Interfaces:** CI job `mcp-conformance`.
- **Acceptance criteria:** the rmcp-based suite passes under **both** MCP revisions (2025-11-25 and 2026-07-28) on stdio and HTTP; Inspector run exits 0 on both transports on Linux and Windows (macOS best-effort allowed to be required once green for a week — documented); scripted agent passes on 3 OSes; breaking the schema of one tool output makes the job fail.
- **Tests:** this WP is the test.
- **Out of scope:** LLM-in-the-loop evals (P6).

### WP-1O.04 — M1 exit acceptance suite (UI + MCP + crash)
- **Note (ADR-0018):** the UI exits are the milestone subset of the user-flow suite built in WP-1O.05; this WP adds the M1-specific flows and the CI aggregation.
- **Area:** `apps/desktop/e2e`, `crates/cutweave-cli/tests`, `docs/m1-acceptance.md`
- **Depends on:** WP-1M.08, WP-1L.03, WP-1M.07, WP-1O.01, WP-1O.02, WP-1O.03, WP-1B.08, WP-1B.09
- **Size:** M
- **Executor:** opus (human signs off the demo)
- **Scope:**
  - **Exit 1 (UI):** Playwright-Electron test importing the 3 corpus clips (H.264 1080p60, HEVC 10-bit, VFR phone), arranging, trimming, splitting, exporting 1080p MP4; validation script `scripts/validate-mp4.ts`/`xtask validate-mp4`: ffprobe codec/size/fps/duration (±1 frame), decode to null with no errors, frame hash comparison at 3 timestamps vs `cutweave render` golden reference (PSNR ≥ 38 dB), audio level non-silent.
  - **Exit 2 (MCP headless equivalent):** the UI path is driven through **numeric inputs** (inspector start/duration fields, split button/key, media bin Enter-to-insert), not pixel drags, using `--deterministic-ids 42` for both engines. The saved project documents are compared by **normalized structure** (ID-normalized, UI-only fields dropped via `xtask normalize-project`) and must be equal to the MCP-produced document; they are **not** required to be byte-identical (byte-identical is reserved for CLI = MCP = RPC in 1O.02). The exported MP4s must be **frame-equivalent** (decoded frame hashes equal at every 10th frame, PSNR >= 45 dB).
  - **Exit 3 (crash):** run 20 trials: mid-edit `killEngine()` (varying point in a 30-command script, also kill during a gesture-tx drag) → UI recovers, final doc equals an uninterrupted run; plus app-level SIGKILL of everything → relaunch → recovery dialog → Restore → equal to last acked state. 0 lost commands in all 40 trials.
  - **Exit 4 (CI):** a `m1-gate` workflow aggregating required checks (rust/ts ×3 OS, golden-frames, parity, mcp-conformance, schema-snapshot, deny, e2e) and `docs/m1-acceptance.md` checklist with links to green runs; record the demo video of the flow on 1 machine per OS (human).
- **Interfaces:** `m1-gate` workflow; acceptance checklist.
- **Acceptance criteria:** all four exit criteria items evidenced by green CI run URLs in `docs/m1-acceptance.md`; Human reviewer signs off by checking boxes in the PR.
- **Tests:** this WP is e2e.
- **Out of scope:** performance budgets beyond those recorded in earlier WPs.

### WP-1O.05 — User-flow e2e suite (real app, real input, real dialogs)
- **Area:** `apps/desktop/e2e/flows/`, `apps/desktop/test-support`, `docs/user-flows.md`, `justfile` (`e2e-flows`), `scripts/check-affected.py` (the flow step)
- **Depends on:** WP-1M.03, WP-1M.06, WP-1M.07, WP-1L.03
- **Size:** M
- **Executor:** opus
- **Scope:**
  - The living suite of ADR-0018: one spec per flow in `docs/user-flows.md`, against the built app and the real engine (`cutweave-engine-serve`), with real files and the demo and corpus media.
    - The engine runs with `--dev-stub-media` until WP-1I.05 wires real probing and decode.
    - Flows drive the UI like a user: pointer clicks and drags, typing, shortcuts, menus (through the app's menu commands), and the app's own dialogs answered in the UI.
    - Only OS file pickers are stubbed, in the main process, to return the path a user would pick.
    - Read-only test hooks may observe state; no hook may change app behaviour.
  - This WP covers flows F1–F12, F15 and F16. F13, F14 and F17 stay blocked, with their WPs named.
  - Shared flow helpers in `e2e/flows/`: launch with the real engine, the media bin, timeline drag targets, the inspector, menus, and `quitApp`.
  - Assertions cover what is visible, the saved `.cutweave` on disk, and engine state read back through `timeline.get`.
  - A `just e2e-flows` recipe. `check-affected` runs it when the desktop app, editor, UI kit or engine are affected.
  - A run-level check that no Electron or engine process survives the suite.
- **Interfaces:** `just e2e-flows`; `e2e/flows/*.spec.ts`; `docs/user-flows.md` statuses.
- **Acceptance criteria:**
  - All flows in scope pass twice in a row on Windows with no retries, and the whole suite takes under 10 minutes.
  - No flow uses a behaviour-changing hook or a fixed sleep (review checklist).
  - After the run, no Electron or engine process is left (asserted).
  - `docs/user-flows.md` marks every flow in scope as covered, and each blocked flow names its WP.
- **Tests:** this WP is e2e.
- **Out of scope:** playback with the real engine, export and real decode (F13, F14, F17); the CI workflow aggregation (WP-1O.04).

---

## Traceability

### Roadmap Phase 1 scope bullets → WPs

| Roadmap scope bullet | WPs |
|---|---|
| **Model & commands:** `cutweave-model` v1 (project, media, one sequence, tracks, clips), flicks time, IDs, schema v1, migrations framework | 1A.01, 1A.02, 1A.03 |
| `cutweave-commands` with transactions, invertible patches, undo/redo, revisions, actors | 1B.01, 1B.02, 1B.03, 1B.04, 1B.09 (tx ownership/soft-lock), 1B.05, 1B.06 |
| …and property tests; deterministic-ID test mode | 1B.07, 1B.08 |
| **Persistence:** folder format, atomic save, autosave, journal, crash recovery, lock file (+ `view` state persistence) | 1C.01, 1C.02, 1C.03, 1C.04 |
| **Media:** probe, index, decode (CPU + HW), thumbnails, waveforms, frame cache | 1D.01, 1D.02, 1D.03, 1D.04, 1D.05, 1D.06, 1D.07 |
| **Export:** H.264/AAC MP4 (HW and software) | 1E.01, 1E.02, 1E.03 |
| **Render:** single-layer compositor (transform/opacity), linear-light pipeline, preview quality ladder | 1F.01, 1F.02, 1F.03 |
| **Audio:** playback of clip audio, master clock, volume | 1G.01, 1G.02 |
| **Playback:** SHM preview ring, transport (play/pause/seek/scrub/JKL) | 1H.01, 1H.02, 1H.03, 1M.04 |
| **API:** WS JSON-RPC, media HTTP endpoints, token auth (credential classes), run-file discovery, engine root + subsystem wiring | 1I.01, 1I.02, 1I.03, 1I.04, 1I.05 |
| **MCP v0:** rmcp server (HTTP + stdio attach/headless), ~10-15 core tools (15 core + 4 generic), `render_frame`, `job_wait`, `cutweave_execute` | 1J.01, 1J.02, 1J.03 |
| **CLI:** `cutweave engine | mcp | exec | query | render` | 1K.01 (+1J.03 for `mcp`) |
| **UI:** Electron shell | 1L.01, 1L.02 |
| engine supervision and restart | 1L.03 |
| media bin (import/drag-drop) | 1M.03 |
| preview | 1M.04 |
| basic timeline (1V + 1A: select, move, trim, split, delete) | 1M.05, 1M.06 |
| minimal inspector, undo/redo, save/open | 1M.07 |
| export dialog | 1M.08 |
| **Codegen:** TS protocol package generated in CI with a snapshot diff gate | 1N.01, 1N.02 |
| (arch §18) Electron security hardening | 1L.02 |
| (arch §20) golden-frame infra, parity test, schema snapshot gate, journal crash test, undo/redo property tests | 1O.01, 1O.02, 1N.01, 1C.02, 1B.07 |

### Exit criteria → WPs

| Exit criterion (M1) | Implemented by | Verified by |
|---|---|---|
| 1. UI: import 3 clips, arrange, trim, split, export 1080p MP4 that plays | 1M.03, 1M.05, 1M.06, 1M.08, 1E.03 | 1O.04 (Exit 1), 1M.08 e2e |
| 2. MCP headless scripted client does the same and exports identical-timeline file | 1J.01–1J.03, 1K.01 | 1O.03 (scripted agent), 1O.04 (Exit 2), 1O.02 |
| 3. Kill engine mid-edit: UI recovers with no lost commands | 1C.02, 1C.03, 1B.03 (request_id dedup), 1C.02 (journaled request_id + resend test), 1I.02, 1L.03, 1M.07 (recovery dialog) | 1C.02 crash test, 1L.03 e2e, 1O.04 (Exit 3) |
| 4a. CI green on 3 OSes | 0A.03 + all WPs | 1O.04 `m1-gate` |
| 4b. Golden frames pass | 1F.02, 1F.03 | 1O.01 |
| 4c. Parity test (CLI = MCP = RPC) passes | 1I.02, 1J.01, 1K.01 | 1O.02 |

---

## Notes for integrator

Status per item after the two integrator rounds.

1. **RESOLVED: "identical-timeline file" (exit 2).** UI path driven by numeric inputs (not pixel drags), `--deterministic-ids <seed>` on both engines, comparison by normalized structure; MP4s frame-equivalent. Byte-identical documents only for CLI = MCP = RPC (1O.02).
2. **RESOLVED: epics for `cutweave-jobs` / `cutweave-engine`.** Jobs in 1D.07; engine root in 1I.04 and subsystem wiring in 1I.05 (split so the document-only engine unblocks the UI spine).
3. **RESOLVED: `view` namespace.** Not undoable, not journaled, persisted in `.cutweave/ui-state.json` (1C.04).
4. **RESOLVED: idempotency (exit 3).** `request_id` (UUIDv7) journaled with a persisted wall-clock timestamp (10-min window survives restart): 1B.03, 1C.02, 1I.02, 1L.03.
5. **RESOLVED: MCP P1 tool set.** 15 core tools (incl. `job_wait` and `render_frame`) + 4 generic = 19 listed; P1 gate <= 19 tools / <= 5k tokens; minimal <= 12 / 3k, standard <= 36 / 8k; profiles per endpoint (`/mcp`, `/mcp/minimal`, `/mcp/full`) or `--profile`, never per connection. 1J.01 now depends on 1D.07 and 1E.03.
6. **RESOLVED: `Time` wire format** (ADR-015): 1A.01, 1N.02, 1M.07.
7. **RESOLVED: FFmpeg distribution** (0A.06 + `just ffmpeg-fetch`); 1D.01 depends on 0A.06.
8. **RESOLVED: golden frames.** Tolerance policy single source (bit-identical preview vs export; SSIM >= 0.995 AND PSNR >= 40 dB vs stored per-adapter reference; PSNR > 45 dB for spike references); references per adapter; macOS goldens on `ref-mac-m1` if hosted Metal is unusable.
9. **RESOLVED: checkpoints** stay out of P1.
10. **RESOLVED: Electron sandbox vs native addon.** Main editor window preload is `sandbox:false` (transport a) or a utilityProcess + MessagePort (transport b) per ADR-001; all other windows sandboxed (1H.03, 1L.02).
11. **RESOLVED: credential classes (former note 12).** UI token over the spawn handshake pipe, never on disk; agent tokens in the run-file; actor derived from the credential (`Actor::System` added); UI-only scopes enforced (1I.01, 1I.04, 1L.03).
12. **RESOLVED: media import semantics.** `media.import` registers synchronously (probe timeout) and returns `media_ids`; analysis is async; `job.wait`/`job_wait` added (1D.07, 1J.01, 1O.03).
13. **RESOLVED: transactions.** Saves persist the last committed rev only; a tx is owned by its connection and rolled back on drop; gesture (30 s idle auto-cancel) and proposal kinds; minimal soft-lock (1B.03, 1B.04, 1B.09).
14. **RESOLVED: MCP spec revisions.** Tests under both 2025-11-25 and 2026-07-28; notifications per negotiated revision; no `ping` reliance (1J.02, 1O.03).
15. **RESOLVED: CSP/port.** Main chooses the port, passes `--port` on every start, serves the renderer from an `app://` protocol with controlled headers, reloads on port change (1L.02, 1L.03, 1I.04 exit code 12).
16. **RESOLVED: ADR dependencies.** WPs consuming ADRs depend on 0J.02 where ADRs become Accepted.
17. **OPEN (minor): journal durability.** Write before ack, `fdatasync` group-commit <= 50 ms; confirm against the 16 ms RTT budget.
18. **OPEN (minor): supervised restart vs cold open.** Supervised restarts auto-recover silently; cold open of a dirty project prompts restore/discard (arch §17).
19. **OPEN (minor): CI without GPU/audio.** `CUTWEAVE_AUDIO=null`, `CUTWEAVE_GPU=software`, `CUTWEAVE_PREVIEW=jpeg`; HW tests on the self-hosted reference runners (0I.04).
20. **OPEN (minor): spike coupling.** If S1 lands a GPUI fallback, 1L, 1M and 1H.03 change materially.
21. **OPEN (minor): phase size.** 60 WPs, ~10 weeks; first merge candidates: 1D.06 into 1D.05, 1K.01 into 1J.03.
22. **PLANNED (product owner requests, 2026-11-24): settings, bin search, folders.** Settings: WP-1M.11 (framework: tabbed pages, search, preferences store in main, reset, import and export; General, Appearance, Saving and backups, Advanced), WP-1M.13 (Export, Keyboard shortcuts read-only), WP-1J.05 (Agents and MCP, Notifications), WP-1D.08 (Cache and storage, Performance and hardware). Every setting, its default, scope, storage, agent access and owning WP is in the catalog [docs/reference/settings.md](../reference/settings.md); later pages are owned by their feature WPs (noted in each). Bin search: WP-1M.12 (`media.list` filter and sort, shared with the editor's predicate). Folders are P2 (WP-2F.07 to WP-2F.09, ADR-0020 Proposed). For the integrator: (a) place WP-1M.11's proposed arch paragraph in §16 with a pointer from §17; (b) the "apply a preference to the engine" commands are UI-only scopes (ADR-0016's mechanism): `project.set_autosave` (WP-1M.10; made UI-only in WP-1J.05), `project.set_allowed_roots`, `media.set_cache_config`, `media.clear_cache`, `media.set_decode_policy`; (c) these commands use the existing `project` and `media` namespaces; no new namespace.
