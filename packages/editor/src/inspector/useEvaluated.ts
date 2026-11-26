// SPDX-License-Identifier: GPL-3.0-or-later
// The value an animated property has at the playhead, from the engine's `keyframe.eval` (the
// interpolation lives in the engine only). Static properties need no call.
import type { PropId } from "@cutweave/protocol";
import { useEffect, useState } from "react";
import { useStore } from "zustand";
import { engineStore } from "../engine/engineStore.ts";
import type { SelectedClip } from "./fields.ts";
import { animatedPaths } from "./video.ts";

/** Per clip, the evaluated value of each animated property, by dot path. */
export type Evaluated = readonly Readonly<Record<string, unknown>>[];

/** Evaluations at most this often while the playhead moves (playback, scrubbing). */
const EVAL_INTERVAL_MS = 80;

const NONE: Evaluated = [];

export function useEvaluated(clips: readonly SelectedClip[], playhead: number): Evaluated {
  const rpc = useStore(engineStore, (s) => (s.blocked === null ? s.rpc : null));
  const [state, setState] = useState<{ clips: readonly SelectedClip[]; values: Evaluated }>({
    clips: [],
    values: NONE,
  });
  useEffect(() => {
    if (rpc === null) return;
    if (clips.every((c) => animatedPaths(c.item).length === 0)) {
      setState((s) => (s.values === NONE ? s : { clips, values: NONE }));
      return;
    }
    let live = true;
    const timer = setTimeout(() => {
      void Promise.all(
        clips.map(async ({ item }) => {
          const out: Record<string, unknown> = {};
          for (const path of animatedPaths(item)) {
            try {
              const result = await rpc.call("keyframe.eval", {
                item: item.id,
                prop: path as PropId,
                at: playhead,
              });
              const sample = result.samples[0];
              if (sample !== undefined) out[path] = sample.value;
            } catch {
              // The first key's value stays on screen.
            }
          }
          return out;
        }),
      ).then((values) => {
        if (live) setState({ clips, values });
      });
    }, EVAL_INTERVAL_MS);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [rpc, clips, playhead]);
  // A value from before the selection changed does not belong to these clips.
  return state.clips.length === clips.length &&
    state.clips.every((c, i) => c.item.id === clips[i]?.item.id)
    ? state.values
    : NONE;
}
