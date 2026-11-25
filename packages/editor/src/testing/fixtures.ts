// SPDX-License-Identifier: GPL-3.0-or-later
// A small engine document for the component tests: one sequence at 30 fps with a video clip and
// its linked audio clip, plus a stub-probed media asset. Hand-written (no engine), only the
// fields the editor reads.

import type { Project } from "@cutweave/protocol";

export const FLICKS = 705_600_000;
export const sec = (s: number): number => Math.round(s * FLICKS);

const clip = (id: string, extra: object = {}) => ({
  kind: "clip",
  id,
  name: null,
  start: sec(1),
  duration: sec(4),
  enabled: true,
  linked_group: "grp_1",
  attached_to: null,
  markers: {},
  media: "med_a",
  source_in: 0,
  audio: { gain: 0 },
  visual: {
    transform: {
      position: { x: 0, y: 0 },
      scale: { x: 1, y: 1 },
      rotation: 0,
      anchor: { x: 0, y: 0 },
    },
    opacity: 1,
  },
  custom: {},
  ...extra,
});

const track = (id: string, kind: string, items: object[], extra: object = {}) => ({
  id,
  kind,
  name: id.toUpperCase(),
  items: Object.fromEntries(items.map((i) => [(i as { id: string }).id, i])),
  muted: false,
  hidden: false,
  locked: false,
  solo: false,
  magnetic: false,
  height: null,
  gain: 0,
  custom: {},
  ...extra,
});

/** `itm_v` (video) and `itm_a` (its linked audio); `edits` patches the clips by ID. */
export function sampleProject(edits: Record<string, object> = {}): Project {
  return {
    meta: { name: "Sample", schema_version: 2, created: null, app_version: null },
    settings: { default_format: {} },
    media: {
      med_a: {
        id: "med_a",
        kind: "video",
        path_abs: "C:\\media\\beach.mp4",
        path_rel: null,
        content_hash: null,
        sequence: null,
        probe: {
          streams: [
            {
              index: 0,
              kind: "video",
              codec: "stub",
              width: 1920,
              height: 1080,
            },
          ],
          duration: sec(10),
          fps: { num: 30, den: 1 },
          vfr: false,
          rotation: 0,
          color: {},
          audio: { rate: 48000, channels: 2 },
          stub: true,
        },
        custom: {},
      },
    },
    sequences: {
      seq_1: {
        id: "seq_1",
        name: "Main",
        format: {
          width: 1920,
          height: 1080,
          frame_rate: { num: 30, den: 1 },
          sample_rate: 48000,
          color_space: "rec709",
          background: { r: 0, g: 0, b: 0, a: 1 },
        },
        track_order: ["trk_v", "trk_a"],
        markers: {},
        in_out: null,
        tracks: {
          trk_v: track("trk_v", "video", [clip("itm_v", edits.itm_v)]),
          trk_a: track("trk_a", "audio", [clip("itm_a", edits.itm_a)]),
        },
        custom: {},
      },
    },
    transcripts: {},
    compositions: {},
    custom: {},
  } as unknown as Project;
}
