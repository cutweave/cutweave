// SPDX-License-Identifier: GPL-3.0-or-later
// The transport (WP-1M.04): every action is an engine `view.*` command (arch §10 "Wire": `view.play`,
// `view.pause`, `view.seek`, `view.set_rate`, `view.step`), so MCP and the CLI drive playback the
// same way. Each command answers with the engine's playback state, which goes to `playbackStore`;
// `playback.state` events (<= 30 Hz) keep it current while playing, and `attach()` moves the
// view's playhead (the timeline's) with it.
//
// - Seeks are latest-wins: one `view.seek` in flight; changes meanwhile collapse into the newest,
//   sent when it returns. The playhead shows the user's choice at once and holds it until then.
// - J/K/L follow the engine's shuttle rule (cutweave-playback `shuttle`): L plays forward and
//   speeds up 1x → 2x → 4x, J the same in reverse, K pauses.
// - Volume and mute are the engine's linear `master_gain` (`view.set`; there is no separate
//   `view.set_master_gain` or mute flag yet): mute sends 0 and keeps the volume to return to.
// - "Go back to where it started" (`playback.return_to_start`, WP-2H.10) is the editor's: when
//   the person pauses, or playback runs into the end, the transport seeks back to the position
//   playback began at. A step, and a pause someone else made (an agent), stay where they are.
// - An engine without the playback subsystem answers `not_found`: reported once, controls go
//   quiet, seeks still move the view's playhead (which `viewSync` mirrors to the engine).
import { type MediaId, type Project, RpcError } from "@cutweave/protocol";
import { frameDurationFlicks } from "@cutweave/timeline";
import type { StoreApi } from "zustand";
import type { EngineStoreState } from "../engine/engineStore.ts";
import type { DocStore } from "../state/docStore.ts";
import type { PlaybackStore } from "../state/playbackStore.ts";
import { selectActiveSequenceId } from "../state/selectors.ts";
import {
  type SourceHost,
  type SourceViewStore,
  sourceViewStore,
} from "../state/sourceViewStore.ts";
import type { ViewStore } from "../state/viewStore.ts";

export type ShuttleKey = "J" | "K" | "L";

/** The rate after pressing `key` while playing at `current` (`null` = stopped); `null` = pause. */
export function nextShuttleRate(current: number | null, key: ShuttleKey): number | null {
  if (key === "K") return null;
  const dir = key === "L" ? 1 : -1;
  if (current === null || current === 0 || Math.sign(current) !== dir) return dir;
  return dir * (Math.abs(current) >= 2 ? 4 : 2);
}

/** End of the active sequence's content (flicks): the end of its last item; 0 when empty. */
export function sequenceEnd(project: Project | null): number {
  const id = selectActiveSequenceId(project);
  const sequence = id === null ? undefined : project?.sequences[id];
  let end = 0;
  for (const track of Object.values(sequence?.tracks ?? {})) {
    for (const item of Object.values(track.items)) {
      end = Math.max(end, item.start + item.duration);
    }
  }
  return end;
}

export interface TransportDeps {
  readonly engine: StoreApi<EngineStoreState>;
  readonly playback: StoreApi<PlaybackStore>;
  readonly view: StoreApi<ViewStore>;
  readonly doc: StoreApi<DocStore>;
  /** Where the source is shown (the app's own when left out). */
  readonly sourceView?: StoreApi<SourceViewStore>;
  /**
   * The `playback.return_to_start` preference, read at each stop (so a change applies at once);
   * off when left out.
   */
  readonly returnToStart?: () => boolean;
  /** The engine has no transport commands (the first `not_found`). */
  readonly onUnsupported?: () => void;
  readonly onError?: (error: unknown) => void;
}

/** `view.*` transport commands are not in the generated schema until the engine registers them. */
type UntypedCall = (method: string, params: Record<string, unknown>) => Promise<unknown>;

export class TransportController {
  readonly #deps: TransportDeps;
  #seeking = false;
  #nextSeek: number | null = null;
  #gainInFlight = false;
  #gainAgain = false;
  #reported = false;
  /** Where the playback under way began: what played (a source, or `null`) and its position. */
  #began: { readonly media: MediaId | null; at: number } | null = null;
  /** A pause the person asked for is on its way to the engine. */
  #pausing = false;

  constructor(deps: TransportDeps) {
    this.#deps = deps;
  }

  play(): Promise<void> {
    return this.#command("view.play", {});
  }

  async pause(): Promise<void> {
    this.#pausing = true;
    try {
      await this.#command("view.pause", {});
    } finally {
      this.#pausing = false;
    }
  }

  togglePlay(): Promise<void> {
    return this.#deps.playback.getState().playing ? this.pause() : this.play();
  }

  shuttle(key: ShuttleKey): Promise<void> {
    const pb = this.#deps.playback.getState();
    const rate = nextShuttleRate(pb.playing ? pb.rate : null, key);
    return rate === null ? this.pause() : this.#command("view.set_rate", { rate });
  }

  /** Pauses and moves by whole frames of the sequence grid (negative: back). */
  step(frames: number): Promise<void> {
    return this.#command("view.step", { frames: Math.trunc(frames) });
  }

  /**
   * Plays `media` on its own, outside any sequence (ADR-0022): `view.set_source`, then
   * `view.play`. `host` is the picture that shows it; `from` the position to start at (flicks,
   * the source's own time). A source already set for `media` is not restarted.
   */
  async playSource(media: MediaId, host: SourceHost = "preview", from = 0): Promise<void> {
    if (!(await this.#setSource(media, host))) return;
    if (from > 0) await this.seek(from);
    await this.play();
  }

  /**
   * Shows `media` in the Preview's Source view, paused at `at` (flicks, the source's own time):
   * match frame (WP-2A.14). Resolves to whether the source is shown.
   */
  async showSource(media: MediaId, at = 0): Promise<boolean> {
    const { playback } = this.#deps;
    if (playback.getState().source?.media === media && playback.getState().playing) {
      await this.pause();
    }
    if (!(await this.#setSource(media, "preview"))) return false;
    await this.seek(at);
    return true;
  }

  /** Makes `media` the engine's source, shown by `host`; `false` when the engine refuses. */
  async #setSource(media: MediaId, host: SourceHost): Promise<boolean> {
    const { playback } = this.#deps;
    this.#sourceView().getState().setHost(host);
    if (playback.getState().source?.media !== media) {
      const result = await this.#call("view.set_source", { media });
      if (result === undefined) {
        // Refused (a still, a removed asset): nothing is shown anywhere else.
        this.#sourceView().getState().setHost("preview");
        return false;
      }
      playback.getState().applyState(result);
    }
    return true;
  }

  /**
   * The play key of a card or the Inspector: pauses the source when `media` plays in `host`,
   * else plays it there (moving it from the other picture without a restart).
   */
  toggleSource(media: MediaId, host: SourceHost = "preview", from = 0): Promise<void> {
    const pb = this.#deps.playback.getState();
    const here = pb.source?.media === media && this.#sourceView().getState().host === host;
    return here && pb.playing ? this.pause() : this.playSource(media, host, from);
  }

  /** Back to the timeline: the engine plays the sequence again, at the playhead it kept. */
  async closeSource(): Promise<void> {
    const { playback } = this.#deps;
    if (playback.getState().source === null) return;
    const result = await this.#call("view.set_source", {});
    this.#sourceView().getState().setHost("preview");
    if (result !== undefined) playback.getState().applyState(result);
  }

  #sourceView(): StoreApi<SourceViewStore> {
    return this.#deps.sourceView ?? sourceViewStore;
  }

  /**
   * Moves the playhead to `t` (flicks); latest wins. While a source plays it is the source's
   * playhead (its own time) that moves, never the timeline's.
   */
  seek(t: number): Promise<void> {
    const at = Math.max(0, Math.round(t));
    const { playback, view } = this.#deps;
    // A seek while playing is the person's new place: a stop comes back here.
    if (playback.getState().playing && this.#began !== null) this.#began.at = at;
    if (playback.getState().source !== null) playback.getState().setSourcePlayhead(at);
    else {
      playback.getState().setPlayhead(at);
      view.getState().setPlayhead(at);
    }
    if (this.#deps.engine.getState().rpc === null) return Promise.resolve();
    playback.getState().setHoldPlayhead(true);
    if (this.#seeking) {
      this.#nextSeek = at;
      return Promise.resolve();
    }
    return this.#sendSeek(at);
  }

  toStart(): Promise<void> {
    return this.seek(0);
  }

  /** The last frame of the content (of the source while one plays). */
  toEnd(): Promise<void> {
    const source = this.#deps.playback.getState().source;
    if (source !== null) {
      return this.seek(Math.max(0, source.duration - frameDurationFlicks(source.fps)));
    }
    const project = this.#deps.doc.getState().project;
    const end = sequenceEnd(project);
    const id = selectActiveSequenceId(project);
    const fps = id === null ? undefined : project?.sequences[id]?.format.frame_rate;
    const frame = fps === undefined ? 1 : frameDurationFlicks(fps);
    return this.seek(Math.max(0, end - frame));
  }

  /** Output volume (linear gain, 1 = unity); moving it while muted unmutes. */
  setVolume(gain: number): void {
    const view = this.#deps.view.getState();
    view.setMasterGain(gain);
    if (view.muted && gain > 0) view.setMuted(false);
    this.#sendGain();
  }

  toggleMute(): void {
    const view = this.#deps.view.getState();
    view.setMuted(!view.muted);
    this.#sendGain();
  }

  /**
   * Moves the view's playhead with the engine's (`playback.state`), and returns to where
   * playback began when it stops and the preference asks for it; returns the unsubscribe.
   */
  attach(): () => void {
    const { playback, view } = this.#deps;
    return playback.subscribe((next, prev) => {
      if (next.playhead !== prev.playhead && next.playhead !== view.getState().playhead) {
        view.getState().setPlayhead(next.playhead);
      }
      if (next.playing && !prev.playing) this.#noteStart(next, prev);
      else if (!next.playing && prev.playing) this.#returnToStart(next, prev);
    });
  }

  /** The end of what plays and one frame of it, in flicks. */
  #content(state: PlaybackStore): { end: number; frame: number } {
    if (state.source !== null) {
      return { end: state.source.duration, frame: frameDurationFlicks(state.source.fps) };
    }
    const project = this.#deps.doc.getState().project;
    const id = selectActiveSequenceId(project);
    const fps = id === null ? undefined : project?.sequences[id]?.format.frame_rate;
    return { end: sequenceEnd(project), frame: fps === undefined ? 1 : frameDurationFlicks(fps) };
  }

  /** Playback began: remembers its position (the start, when `view.play` restarts at the end). */
  #noteStart(next: PlaybackStore, prev: PlaybackStore): void {
    const at = next.source !== null ? prev.sourcePlayhead : prev.playhead;
    const { end } = this.#content(next);
    const restarts = next.rate > 0 && end > 0 && at >= end;
    this.#began = { media: next.source?.media ?? null, at: restarts ? 0 : at };
  }

  /**
   * Playback stopped. With `playback.return_to_start` on, a pause the person made and a
   * playback that ran into the end (or, in reverse, the start) go back to where it began.
   */
  #returnToStart(next: PlaybackStore, prev: PlaybackStore): void {
    const began = this.#began;
    this.#began = null;
    if (began === null || this.#deps.returnToStart?.() !== true) return;
    if (began.media !== (next.source?.media ?? null)) return;
    const at = next.source !== null ? next.sourcePlayhead : next.playhead;
    const { end, frame } = this.#content(next);
    const ranOut = prev.rate < 0 ? at <= 0 : end > 0 && at >= end - frame;
    if (this.#pausing || ranOut) void this.seek(began.at);
  }

  async #sendSeek(t: number): Promise<void> {
    this.#seeking = true;
    try {
      const result = await this.#call("view.seek", { t });
      if (result !== undefined && this.#nextSeek === null) {
        this.#deps.playback.getState().setHoldPlayhead(false);
        this.#deps.playback.getState().applyState(result);
      }
    } finally {
      this.#seeking = false;
      const next = this.#nextSeek;
      this.#nextSeek = null;
      if (next !== null) await this.#sendSeek(next);
      else this.#deps.playback.getState().setHoldPlayhead(false);
    }
  }

  #sendGain(): void {
    const rpc = this.#deps.engine.getState().rpc;
    if (rpc === null) return;
    if (this.#gainInFlight) {
      this.#gainAgain = true;
      return;
    }
    const { masterGain, muted } = this.#deps.view.getState();
    this.#gainInFlight = true;
    rpc
      .call("view.set", { patch: { master_gain: muted ? 0 : masterGain } })
      .catch((error: unknown) => this.#deps.onError?.(error))
      .finally(() => {
        this.#gainInFlight = false;
        if (this.#gainAgain) {
          this.#gainAgain = false;
          this.#sendGain();
        }
      });
  }

  async #command(method: string, params: Record<string, unknown>): Promise<void> {
    const result = await this.#call(method, params);
    if (result !== undefined) this.#deps.playback.getState().applyState(result);
  }

  /** Runs a transport command; `undefined` when there is no engine or it failed. */
  async #call(method: string, params: Record<string, unknown>): Promise<unknown> {
    const rpc = this.#deps.engine.getState().rpc;
    if (rpc === null) return undefined;
    const call = rpc.call as unknown as UntypedCall;
    try {
      const result = await call.call(rpc, method, params);
      const playback = this.#deps.playback.getState();
      if (playback.transport !== "available") playback.setTransport("available");
      return result ?? null;
    } catch (error) {
      if (error instanceof RpcError && error.code === "not_found") {
        this.#deps.playback.getState().setTransport("unsupported");
        if (!this.#reported) {
          this.#reported = true;
          this.#deps.onUnsupported?.();
        }
      } else {
        this.#deps.onError?.(error);
      }
      return undefined;
    }
  }
}
