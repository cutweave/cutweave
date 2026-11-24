# ADR-0022: The preview plays a media item outside any sequence (source preview)

- **Status:** Proposed
- **Date:** 2026-11-24
- **Deciders:** product owner and Cutweave maintainers (drafted by the planner for the integrator)
- **Supersedes:** none (extends arch §10 "one transport per document", which today plays only the active sequence)
- **Architecture refs:** `docs/01-architecture.md` §5.1, §9, §10, §16

## Context

The preview shows only the timeline. The transport (WP-1H.01) and its engine wiring (WP-1I.05) play the document's first sequence: `PreviewSource` indexes that sequence, and the mixer is fed from its clips. To see or hear a clip in the bin, a person has to put it on the timeline first and take it off again. The product owner hit this while editing real media (2026-11-24): "I don't have a way to preview without adding to timeline. We need both the asset item to be playable and a preview inside the Inspector."

Every NLE has a way to do this: Premiere Pro and Resolve have a source monitor, Final Cut Pro plays a browser clip in its viewer, and CapCut desktop plays a selected media item in its player. Seeing a frame can be done today without the engine's transport, with `/media/{id}/thumb?t=` (WP-1I.03), and WP-1M.14 builds hover-scrub and the Inspector's picture on it. Playing a clip with its sound, frame-accurately and at the clip's own rate, needs the transport. The transport and its events are contracts (`view.*` commands, `playback.state`), so the change needs an ADR.

## Decision drivers

- Playback of a source must be as good as playback of the timeline: the same decoders, proxies, quality ladder, A/V sync, JKL and frame-exact steps, with no second implementation.
- Nothing about previewing may change the project: no items, no sequences, no undo step, no journal record, no unsaved state.
- The preview's real-time path keeps its rules (arch §10 "Threads"): no locks or per-frame allocation of our own on the render thread.
- No new process, ring or bridge in P1; the preview budget and the GPU memory stay as they are.
- Agents keep their ways to see media (`/media/{id}/thumb`, and `media_filmstrip` in P2); previewing is a person's view, like the playhead.

## Considered options

1. **One transport per document with two possible targets** (leading): the active sequence, or one media asset played through a transient one-clip timeline that the engine builds from the probe and never stores. A `view` command switches the target; `playback.state` says what is playing.
2. **A second transport and ring per document (a dedicated source monitor).** Both viewers could play side by side, as in Premiere. Rejected for now: it doubles the decoder, GPU and ring budgets, needs a second ring in the bridge and a second presenter path, and nothing the owner asked for needs both playing at once. It stays open as a later extension of option 1 (the target type becomes per transport).
3. **A temporary sequence in the document.** Rejected: it would be an edit (undo history, journal, autosave, unsaved state), agents would see it, and a crash could leave it behind.
4. **An HTML `<video>` element over the file in the renderer.** Rejected: Chromium does not decode many formats people edit (HEVC 10-bit, ProRes, VFR phone footage reliably), it would show different pixels from the engine's decode and colour pipeline, it ignores proxies, and the renderer would need to read media files, which the security model forbids (arch §18).
5. **JPEG frames from `/media/{id}/thumb` played in sequence.** Kept for hover-scrub and still pictures (WP-1M.14); rejected for playback: no sound, at most about 15 frames a second.

## Decision

**Decided by:** the product owner and the maintainers, before WP-1H.04 starts. Pass criteria for building on it, in WP-1H.04: a source plays with A/V offset within one frame on the fake clock and with 0 dropped frames for a 1080p30 corpus clip on the dev machine; switching targets never delivers a frame of the old target after the switch returns; the project, its journal and its undo history are byte-identical before and after a source session; the engine's `preview_alloc` test passes with a source target; and the F31 user flow passes. Until then this is a proposal and nothing builds on it.

We propose that:

- **The document's `PlaybackEngine` plays one target at a time**: the active sequence (today's behaviour) or one media asset. `view.set_source{media?}` makes an asset the target; with no `media` it returns to the sequence. It is a `view` command: not undoable, not journaled, MCP catalog only.
- **A source is a transient one-clip timeline.** The engine builds it from the asset's probe (its video and its audio, at the asset's own frame rate, fitted into the preview size with the asset's aspect) and renders and mixes it through the same `PreviewSource` and mixer paths as a sequence. It exists only in the engine and only while it is the target.
- **Each target has its own playhead.** `view.seek`, `view.step`, `view.set_rate`, `view.play` and `view.pause` act on the current target. The sequence's playhead does not move while a source plays, and returning shows its frame exactly. Each asset's source playhead is kept for the session.
- **`playback.state` gains `source: {media, duration, fps} | null`** (additive). After `view.set_source` returns, every frame in the ring belongs to the new target (the transport's existing latest-wins rule), so the ring format does not change.
- **The source ends when its asset goes away:** removing the asset, opening another project or `doc.reset` returns to the sequence.
- In/out marks on a source and three-point editing from it are view state and ordinary `timeline.insert` / overwrite commands (WP-2A.14). They do not change this decision.

**Fallback** if the pass criteria fail (for example, switching makes the sequence's playback stutter): keep hover-scrub and the Inspector's scrubbable picture (WP-1M.14, which needs none of this), and reconsider option 2 in P2.

## Consequences

**Positive**

- People can watch and hear any clip before using it, in the Preview and in the Inspector, with the same playback quality as the timeline.
- One implementation of playback: proxies, the quality ladder, hardware decode and later effects of the preview path apply to sources for free.
- Previewing never touches the project, so it cannot make it dirty, fill the undo history or confuse an agent working on the same project.

**Negative / costs**

- The timeline and a source cannot play at the same time (no side-by-side source and program monitors) until option 2 is built.
- The Preview needs a clear "you are watching a source" state and a way back, or people will try to edit the timeline while a source is shown (WP-1M.15).
- `view.set_source` and `playback.state.source` join the generated schema; parity tests treat them as `view` commands.

**Follow-ups**

- WP-1H.04 (engine: the source target), WP-1M.14 (hover-scrub and the Inspector picture, independent of this ADR), WP-1M.15 (play from the bin and the Inspector, the Source view), WP-2A.14 (marks and three-point editing).
- Arch §10 (the decided paragraph) and §23 (decision log) once accepted.

## Links

- Architecture: [§5.1](../01-architecture.md), [§9](../01-architecture.md), [§10](../01-architecture.md), [§16](../01-architecture.md), [§18](../01-architecture.md)
- Work packages: [WP-1H.04, WP-1M.14, WP-1M.15](../wbs/P1-skeleton.md), [WP-2A.14](../wbs/P2-core-editor.md)
- Related ADRs: ADR-0001 (preview delivery), ADR-0002, ADR-0005, ADR-0019
- Media endpoints: [http-media.md](../reference/http-media.md)
