// SPDX-License-Identifier: GPL-3.0-or-later
// The fake engine's preview half (WP-1M.04 e2e): a transport, a frame ring and still frames,
// standing in for the playback subsystem the real engine gets in WP-1I.05.
//   - Transport: `view.play`, `view.pause`, `view.set_rate`, `view.seek`, `view.step` and
//     `view.set_preview_size` answer the engine's `PlaybackState` shape; `playback.state` is
//     broadcast at 30 Hz while playing; playback wraps at the end of the content.
//   - Ring (FAKE_ENGINE_RING=<ring_fixture exe>): the crates/cutweave-bridge `ring_fixture`
//     publishes real frames through the engine's `FramePublisher`; it is idle while paused and
//     publishes while playing (a short burst after a seek or step), `view.set_preview_size`
//     resizes it, and `rpc.hello` announces it as `shm: {name, epoch}`.
//     FAKE_ENGINE_RING_SIZE=WxH and FAKE_ENGINE_RING_FPS=N pick its start size and rate;
//     FAKE_ENGINE_RING_PIN=1 keeps that size (benchmarks of a fixed frame size).
//   - Still frames: `GET /media/frame?t=&w=&h=&fmt=jpeg` with the bearer token answers a PNG
//     whose colour follows the frame index at `t` (the JPEG fallback path; a real engine serves
//     JPEG, WP-1I.03).
//   - FAKE_ENGINE_PROJECT=three-clips: `project.snapshot` returns one 1080p30 sequence with three
//     2 s clips.

import { type ChildProcess, spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { deflateSync } from "node:zlib";

export const FLICKS = 705_600_000;
const FPS = 30;
const FRAME = FLICKS / FPS;
/** Content end of the three-clip project. */
const CLIP_SECS = 2;

export interface PlaybackState {
  state: "stopped" | "playing" | "scrubbing";
  playing: boolean;
  rate: number;
  t: number;
  rev: number;
  quality: "full";
  dropped: number;
}

// ----- the three-clip project ----------------------------------------------------------------

function clip(n: number) {
  return {
    kind: "clip",
    id: `itm_${n}`,
    name: null,
    start: (n - 1) * CLIP_SECS * FLICKS,
    duration: CLIP_SECS * FLICKS,
    enabled: true,
    linked_group: null,
    attached_to: null,
    markers: {},
    media: `med_${n}`,
    source_in: 0,
    audio: { gain: 0 },
    visual: {
      transform: {
        position: { x: 0, y: 0 },
        scale: { x: 1, y: 1 },
        rotation: 0,
        anchor: { x: 0.5, y: 0.5 },
      },
      opacity: 1,
    },
    custom: {},
  };
}

function media(n: number) {
  return {
    id: `med_${n}`,
    kind: "video",
    path_abs: `C:\\media\\clip${n}.mp4`,
    path_rel: null,
    content_hash: null,
    sequence: null,
    probe: {
      streams: [{ index: 0, kind: "video", codec: "stub", width: 1920, height: 1080 }],
      duration: 10 * FLICKS,
      fps: { num: FPS, den: 1 },
      vfr: false,
      rotation: 0,
      color: {},
      audio: null,
      stub: true,
    },
    custom: {},
  };
}

/** One 1920 × 1080 sequence at 30 fps: three 2 s clips back to back on a video track. */
export function threeClipProject() {
  return {
    meta: { name: "Three clips", schema_version: 2, created: null, app_version: null },
    settings: { default_format: {} },
    media: Object.fromEntries([1, 2, 3].map((n) => [`med_${n}`, media(n)])),
    sequences: {
      seq_1: {
        id: "seq_1",
        name: "Main",
        format: {
          width: 1920,
          height: 1080,
          frame_rate: { num: FPS, den: 1 },
          sample_rate: 48000,
          color_space: "rec709",
          background: { r: 0, g: 0, b: 0, a: 1 },
        },
        track_order: ["trk_v"],
        markers: {},
        in_out: null,
        tracks: {
          trk_v: {
            id: "trk_v",
            kind: "video",
            name: "V1",
            items: Object.fromEntries([1, 2, 3].map((n) => [`itm_${n}`, clip(n)])),
            muted: false,
            hidden: false,
            locked: false,
            solo: false,
            magnetic: false,
            height: null,
            gain: 0,
            custom: {},
          },
        },
        custom: {},
      },
    },
    transcripts: {},
    compositions: {},
    custom: {},
  };
}

// ----- the ring --------------------------------------------------------------------------------

export interface RingInfo {
  name: string;
  epoch: string;
}

/** The `ring_fixture` writer, driven over stdin. */
class Ring {
  readonly child: ChildProcess;
  info: RingInfo | null = null;
  #burst: ReturnType<typeof setTimeout> | null = null;

  constructor(exe: string, width: number, height: number, fps: number) {
    this.child = spawn(
      exe,
      ["--width", String(width), "--height", String(height), "--fps", String(fps)],
      { stdio: ["pipe", "pipe", "inherit"] },
    );
    this.child.on("error", (e) => console.error(`[fake-engine] ring_fixture: ${e.message}`));
    const lines = createInterface({ input: this.child.stdout as NodeJS.ReadableStream });
    this.ready = new Promise((resolve) => {
      lines.on("line", (line) => {
        const event = JSON.parse(line) as { event: string; name: string; epoch: string };
        if (event.event === "ready") {
          // Idle until playback starts: a paused engine publishes nothing new.
          this.send("idle");
          this.info = { name: event.name, epoch: event.epoch };
          resolve();
        } else if (event.event === "moved") {
          // A resize moved frames to a larger ring: announce that one from now on.
          this.info = { name: event.name, epoch: event.epoch };
        }
      });
      this.child.once("exit", () => resolve());
    });
  }

  readonly ready: Promise<void>;

  send(command: string): void {
    if (this.child.exitCode === null && this.child.stdin?.writable) {
      this.child.stdin.write(`${command}\n`);
    }
  }

  publishing(on: boolean): void {
    if (this.#burst !== null) clearTimeout(this.#burst);
    this.#burst = null;
    this.send(on ? "resume" : "idle");
  }

  /** A few frames for a seek or step while paused. */
  burst(): void {
    this.publishing(true);
    this.#burst = setTimeout(() => this.publishing(false), 80);
  }

  /** Ends the fixture's stdin: it closes the ring and exits (also when this process dies). */
  stop(): void {
    this.send("quit");
    this.child.stdin?.end();
  }
}

// ----- the transport ----------------------------------------------------------------------------

export class FakePreview {
  readonly project = process.env.FAKE_ENGINE_PROJECT === "three-clips" ? threeClipProject() : null;
  readonly #broadcast: (event: string, params: unknown) => void;
  readonly #end: number;
  #ring: Ring | null = null;
  #pin = process.env.FAKE_ENGINE_RING_PIN === "1";
  #playing = false;
  #rate = 0;
  #anchorT = 0;
  #anchorAt = 0;
  #timer: ReturnType<typeof setInterval> | null = null;

  constructor(broadcast: (event: string, params: unknown) => void) {
    this.#broadcast = broadcast;
    this.#end = this.project === null ? 10 * FLICKS : 3 * CLIP_SECS * FLICKS;
    const exe = process.env.FAKE_ENGINE_RING;
    if (exe) {
      const [w, h] = (process.env.FAKE_ENGINE_RING_SIZE ?? "640x360").split("x").map(Number);
      const fps = Number(process.env.FAKE_ENGINE_RING_FPS ?? FPS);
      this.#ring = new Ring(exe, w || 640, h || 360, fps || FPS);
    }
  }

  /** Resolves once the ring (if any) is announced. */
  ready(): Promise<void> {
    return this.#ring?.ready ?? Promise.resolve();
  }

  /** `shm` for `rpc.hello`, when there is a ring. */
  shm(): RingInfo | undefined {
    return this.#ring?.info ?? undefined;
  }

  stop(): void {
    if (this.#timer !== null) clearInterval(this.#timer);
    this.#ring?.stop();
  }

  #now(): number {
    if (!this.#playing) return this.#anchorT;
    const t = this.#anchorT + ((performance.now() - this.#anchorAt) / 1000) * this.#rate * FLICKS;
    // Playback wraps at the end of the content (and at the start in reverse).
    const wrapped = ((t % this.#end) + this.#end) % this.#end;
    return Math.round(wrapped);
  }

  state(): PlaybackState {
    return {
      state: this.#playing ? "playing" : "stopped",
      playing: this.#playing,
      rate: this.#playing ? this.#rate : 0,
      t: this.#now(),
      rev: 1,
      quality: "full",
      dropped: 0,
    };
  }

  #set(playing: boolean, rate: number, t = this.#now()): PlaybackState {
    this.#anchorT = Math.min(Math.max(0, t), this.#end);
    this.#anchorAt = performance.now();
    this.#playing = playing;
    this.#rate = rate;
    this.#ring?.publishing(playing);
    if (playing && this.#timer === null) {
      this.#timer = setInterval(() => this.#broadcast("playback.state", this.state()), 33);
    } else if (!playing && this.#timer !== null) {
      clearInterval(this.#timer);
      this.#timer = null;
    }
    const state = this.state();
    this.#broadcast("playback.state", state);
    return state;
  }

  /** Answers a `view.*` transport command, or `undefined` when `method` is not one. */
  handle(method: string, params: Record<string, unknown> | undefined): unknown {
    switch (method) {
      case "view.play":
        return this.#set(true, 1);
      case "view.pause":
        return this.#set(false, 0);
      case "view.set_rate": {
        const rate = Number(params?.rate ?? 0);
        return rate === 0 ? this.#set(false, 0) : this.#set(true, rate);
      }
      case "view.seek": {
        const state = this.#set(this.#playing, this.#rate, Number(params?.t ?? 0));
        if (!this.#playing) this.#ring?.burst();
        return state;
      }
      case "view.step": {
        const frames = Math.trunc(Number(params?.frames ?? 0));
        const at = Math.floor(this.#now() / FRAME + frames) * FRAME;
        const state = this.#set(false, 0, at);
        this.#ring?.burst();
        return state;
      }
      case "view.set_preview_size": {
        const width = Math.max(2, Math.floor(Number(params?.width ?? 0) / 2) * 2);
        const height = Math.max(2, Math.floor(Number(params?.height ?? 0) / 2) * 2);
        if (!this.#pin) this.#ring?.send(`resize ${width} ${height}`);
        return { width, height };
      }
      default:
        return undefined;
    }
  }
}

// ----- still frames (PNG, for the fallback path) ---------------------------------------------

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  return table;
})();

function crc32(buf: Buffer): number {
  let c = 0xffffffff;
  for (const byte of buf) c = (CRC_TABLE[(c ^ byte) & 0xff] ?? 0) ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type: string, data: Buffer): Buffer {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

/** An RGB PNG of one colour per frame index (cheap: at most 320 px wide; the client scales). */
export function framePng(t: number, w: number, h: number): Buffer {
  const width = Math.max(2, Math.min(320, Math.round(w)));
  const height = Math.max(2, Math.round((h / Math.max(w, 1)) * width));
  const frame = Math.floor(t / FRAME);
  const rgb = [(frame * 53) & 0xff, (frame * 101 + 64) & 0xff, (frame * 17 + 128) & 0xff];
  const row = Buffer.alloc(1 + width * 3);
  for (let x = 0; x < width; x++) row.set(rgb, 1 + x * 3);
  const raw = Buffer.concat(Array.from({ length: height }, () => row));
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, 2, 0, 0, 0], 8);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(raw, { level: 1 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}
