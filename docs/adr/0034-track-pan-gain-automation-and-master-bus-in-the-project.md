# ADR-0034: Store track pan, keyframed track gain and the master bus settings in the project

- **Status:** Accepted (product owner, 2026-11-28)
- **Date:** 2026-11-26
- **Deciders:** Cutweave maintainers, on the review of WP-2E.02 (before the follow-up that adds the fields)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §4.2, §9, §17

## Context

WP-2E.02 ([P2 WBS](../wbs/P2-core-editor.md)) gives every audio track a bus (gain, pan, mute, exclusive solo-in-place) and the master a true-peak limiter and loudness meters (arch §9, "decided in WP-2E.02"). Its scope also asks for **keyframable track gain**, a **track pan**, and **master settings** the editor can change: `audio.set_master{gain, limiter{enabled, ceiling}}`.

The model has `Track.gain` (a plain dB number), `Track.muted` and `Track.solo`, and nothing for the master: the master volume is view state (`view.set{master_gain}`), a monitoring level that never reaches an export. Track pan, a keyframed track gain and master settings that export would all be new project-format fields, which ADR-0006 says need an ADR. The bus graph, the limiter, the meters, `audio.get_levels` and the export's `advanced.audio.limiter` switch do not depend on them, so WP-2E.02 shipped those with what the model has (bus pan fixed at the centre, static track gain, the limiter always on at −1 dBTP in playback) and left the fields to this decision.

## Decision drivers

- An editor's mix must travel with the project: the pan and fader moves made in the mixer panel (WP-2E.05), and whether the master is limited, must sound the same on every machine and in every export.
- Every change is an undoable command agents can make too (ADR-0002, ADR-0005), with UI, MCP and CLI parity.
- Older builds keep opening newer documents (unknown fields in `extra`), and default projects and golden fixtures do not change.
- Preview equals export bit for bit (arch §9): whatever the document says must be applied by the shared lane and bus code.
- The monitoring volume stays a view setting: turning playback down must never change an export.

## Considered options

1. **Optional fields with defaults, no schema bump (leading):**
   - `Track.pan: f64` (`-1..=1`, default 0, left out of the file when 0): the bus balance.
   - `Track.gain` becomes `Animatable<f64>`: a static gain keeps its wire form (a bare number, so every existing file reads the same); a keyframed gain is `{"keyframes": [...]}` with key times in **sequence** flicks (a track has no start), easing as ADR-0024.
   - `Sequence.mix: Option<MixSettings>` with `master_gain_db: f64` (0, before the limiter, exported) and `limiter: {enabled: bool, ceiling_dbtp: f64}` (on, −1 dBTP; look-ahead and release stay engine constants). Absent means the defaults; left out of the file when default.
2. **Keyframes in a separate optional field** (`Track.gain_keys`): an older build keeps it in `extra` and plays the static gain instead of failing to parse a keyframed `gain`. Safer for old readers, but two sources of truth for one fader and a second keyframe shape next to the clip's.
3. **Master settings per project** (`settings.audio_master`) instead of per sequence: one place, but a project's sequences (a long-form edit and its shorts) are mixed and exported separately and may need different ceilings (−1 dBTP streaming, −2 dBTP broadcast).
4. **Keep it all out of the document** (engine or app preferences): no format change, but the mix would not travel with the project and an agent could not see or change it.

## Decision

Accepted by the product owner on 2026-11-28: option 1, with the master settings per sequence (`Sequence.mix`), and the keyframed track gain joins the unreleased v2 (arch §17, "One v2"): no v3 and no schema version bump.

**Decided by:** the maintainers' review of WP-2E.02. Pass criteria for the follow-up (WP-2E.02b): a round-trip test (a document without the fields opens with the defaults and saves byte-identical; a document with them opens in an older build, which keeps `pan` and `mix` in `extra`); undo identity and CLI = MCP = RPC parity for the new commands; the mixer applies them in playback and export alike (null test between the real-time render and the mixdown under −120 dBFS, keyframed gain without zipper noise: the 64-frame control points of WP-2E.01); the v2 golden fixtures unchanged.

If accepted, option 1. WP-2E.02b then adds:

- **Model:** `Track.pan`, `Track.gain: Animatable<f64>` (sequence-time keys), `Sequence.mix`. A keyframed track gain is a parse error for a build older than WP-2E.02b, so either it joins the unreleased v2 migration (arch §17 "One v2") or it is the first v3 step; the maintainers pick when accepting.
- **Commands:** `track.set_gain{track, db, at?}` (keys at `at` when the gain is keyframed, like `audio.set_volume`; `track.set{gain}` keeps setting a static gain); `track.set_pan{track, pan}` (or `track.set{pan}`); `audio.set_master{sequence?, gain?, limiter?{enabled?, ceiling?}}` (undoable, MCP catalog); the `keyframe.*` commands accept a track gain target.
- **Engine:** `BusGraph::of_sequence` reads the pan; the lane renderer multiplies by the track gain envelope (control points every 64 frames, as clip automation); the mixer takes the master gain and limiter from the sequence (the limiter's latency does not depend on `enabled`, so switching it during playback never moves the timeline); exports read `Sequence.mix`, and `advanced.audio.limiter` overrides it per export.

**Fallback** (rejected or deferred): what WP-2E.02 shipped stays: static track gain, the bus pan centred, the limiter always on at −1 dBTP in playback and on in exports unless `advanced.audio.limiter` is false, and the master volume as a monitoring level only.

## Consequences

**Positive**

- The mixer panel's faders, pan knobs and master section save with the project; agents read and change them like any edit, and undo covers them.
- No migration for static gains and default mixes: existing projects and fixtures do not change.

**Negative / costs**

- Three more fields in the format contract that every future reader must keep, and a keyframe time base (sequence time) that differs from the clips' item-relative keys.
- A keyframed track gain needs a reader that knows the new shape (v2 or v3, above).
- Two master levels to explain: the document's master gain (exported) and the monitoring volume (view state, after the limiter, never exported).

**Follow-ups**

- WP-2E.02b (the fields, commands and engine reads above); WP-2E.05 (the mixer panel drives them); WP-2E.03 (`audio.normalize{mode: master_gain}` writes `Sequence.mix.master_gain_db`).

## Links

- Architecture: [§4.2](../01-architecture.md), [§9](../01-architecture.md), [§17](../01-architecture.md)
- Work breakdown: [WP-2E.02, WP-2E.03, WP-2E.05](../wbs/P2-core-editor.md)
- Related: [ADR-0006](0006-project-format.md) (format changes need an ADR), [ADR-0024](0024-keyframes.md) (keyframes), [ADR-0027](0027-proxy-policy-in-the-project.md) (optional fields without a schema bump)
