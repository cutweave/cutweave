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
// - An engine without the playback subsystem answers `not_found`: reported once, controls go
//   quiet, seeks still move the view's playhead (which `viewSync` mirrors to the engine).
import { type Project, RpcError } from "@cutweave/protocol";
import { frameDurationFlicks } from "@cutweave/timeline";
import type { StoreApi } from "zustand";
import type { EngineStoreState } from "../engine/engineStore.ts";
import type { DocStore } from "../state/docStore.ts";
import type { PlaybackStore } from "../state/playbackStore.ts";
import { selectActiveSequenceId } from "../state/selectors.ts";
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

  constructor(deps: TransportDeps) {
    this.#deps = deps;
  }

  play(): Promise<void> {
    return this.#command("view.play", {});
  }

  pause(): Promise<void> {
    return this.#command("view.pause", {});
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

  /** Moves the playhead to `t` (flicks); latest wins. */
  seek(t: number): Promise<void> {
    const at = Math.max(0, Math.round(t));
    const { playback, view } = this.#deps;
    playback.getState().setPlayhead(at);
    view.getState().setPlayhead(at);
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

  /** The last frame of the content. */
  toEnd(): Promise<void> {
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

  /** Moves the view's playhead with the engine's (`playback.state`); returns the unsubscribe. */
  attach(): () => void {
    const { playback, view } = this.#deps;
    return playback.subscribe((next, prev) => {
      if (next.playhead !== prev.playhead && next.playhead !== view.getState().playhead) {
        view.getState().setPlayhead(next.playhead);
      }
    });
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
