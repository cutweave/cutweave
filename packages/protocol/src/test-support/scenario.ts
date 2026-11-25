// SPDX-License-Identifier: GPL-3.0-or-later
// Test support: seeds a fresh engine with a project and drives a seeded random edit session, so
// integration tests exercise many different `doc.patched` events against a real engine.

import type { ItemId, MediaId, Project, SequenceId, TrackId } from "../generated/types.ts";
import type { RpcClient } from "../rpc.ts";

/** Seconds as flicks: 705,600,000 per second (ADR-0004). */
const FLICKS_PER_SECOND = 705_600_000;

export interface Seed {
  readonly sequence: SequenceId;
  readonly videoTrack: TrackId;
  readonly audioTrack: TrackId;
  readonly media: MediaId;
}

/** Small fast seeded PRNG (mulberry32); tests must not depend on the wall clock or `Math.random`. */
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** Creates a project with `V1`/`A1` and registers one probed video file (no file is read). */
export async function seedProject(rpc: RpcClient, name = "Demo", seconds = 600): Promise<Seed> {
  const created = await rpc.call("project.new", { name });
  const registered = await rpc.call("media.register", {
    path: "/media/demo/clip.mp4",
    content_hash: "demo-clip-hash",
    probe: {
      streams: [
        {
          index: 0,
          kind: "video",
          codec: "h264",
          profile: null,
          width: 1920,
          height: 1080,
          pixel_format: null,
          sample_rate: null,
          channels: null,
          language: null,
        },
      ],
      duration: seconds * FLICKS_PER_SECOND,
      fps: { num: 30, den: 1 },
      vfr: false,
      rotation: 0,
      color: { primaries: null, transfer: null, matrix: null, range: null },
      audio: null,
    },
  });
  return {
    sequence: created.sequence,
    videoTrack: created.video_track,
    audioTrack: created.audio_track,
    media: registered.media_id,
  };
}

function itemsOf(
  project: Project,
  sequence: SequenceId,
): { id: ItemId; start: number; duration: number }[] {
  const seq = project.sequences[sequence];
  if (seq === undefined) {
    return [];
  }
  const out: { id: ItemId; start: number; duration: number }[] = [];
  for (const track of Object.values(seq.tracks)) {
    for (const item of Object.values(track.items)) {
      out.push({ id: item.id, start: item.start, duration: item.duration });
    }
  }
  return out;
}

/**
 * Runs up to `maxAttempts` random edits (insert, split, move, delete, set_props, track.add,
 * undo, redo) and stops once `done()` is true. Commands the engine refuses (overlaps, bad
 * times) are expected and ignored. Returns how many calls succeeded.
 */
export async function randomEdits(
  rpc: RpcClient,
  seed: Seed,
  rnd: () => number,
  done: () => boolean,
  maxAttempts = 2000,
): Promise<number> {
  let succeeded = 0;
  for (let attempt = 0; attempt < maxAttempts && !done(); attempt++) {
    const { project } = await rpc.snapshot();
    const items = itemsOf(project, seed.sequence);
    const pick = () => items[Math.floor(rnd() * items.length)];
    const roll = rnd();
    const second = (max: number) => `${Math.floor(rnd() * max)}s`;
    try {
      if (items.length < 3 || roll < 0.3) {
        await rpc.call("timeline.insert", {
          sequence: seed.sequence,
          track: seed.videoTrack,
          media: seed.media,
          at: second(300),
          source_in: second(60),
          duration: `${1 + Math.floor(rnd() * 8)}s`,
          overlap: rnd() < 0.5 ? "new_track" : "reject",
        });
      } else if (roll < 0.5) {
        const it = pick();
        if (it !== undefined) {
          await rpc.call("clip.split", {
            item: it.id,
            at: it.start + Math.floor(it.duration / 2),
          });
        }
      } else if (roll < 0.65) {
        const it = pick();
        if (it !== undefined) {
          await rpc.call("item.move", {
            item: it.id,
            start: second(300),
            overlap: "new_track",
          });
        }
      } else if (roll < 0.75) {
        const it = pick();
        if (it !== undefined) {
          await rpc.call("item.delete", { items: [it.id] });
        }
      } else if (roll < 0.85) {
        const it = pick();
        if (it !== undefined) {
          await rpc.call("item.set_props", {
            item: it.id,
            props: {
              name: `clip ${Math.floor(rnd() * 1000)}`,
              enabled: rnd() < 0.8,
              opacity: Math.round(rnd() * 100) / 100,
              audio_gain: null,
              audio_pan: null,
              transform: null,
              crop: null,
              flip: null,
              blend_mode: null,
            },
          });
        }
      } else if (roll < 0.9) {
        await rpc.call("track.add", { sequence: seed.sequence, kind: "video" });
      } else if (roll < 0.95) {
        await rpc.call("edit.undo", {});
      } else {
        await rpc.call("edit.redo", {});
      }
      succeeded += 1;
    } catch {
      // refused by the engine: part of the scenario
    }
  }
  return succeeded;
}
