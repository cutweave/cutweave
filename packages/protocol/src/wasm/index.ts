// SPDX-License-Identifier: GPL-3.0-or-later
// `@cutweave/protocol/wasm`: the engine's own time, keyframe and easing math in the UI (WP-2C.06,
// arch §16). The module is `crates/cutweave-wasm` (a thin C ABI over `cutweave-model`) built for
// wasm32 by `just wasm`, so the timeline and the inspector convert times, draw curves and sample
// easings with exactly the engine's bits and no RPC per point. Snapping (WP-2A.07) adds `snap`.
//
//   const math = await initMath();          // once, at startup; rejects on a version mismatch
//   time.frameToTime(30, { num: 30, den: 1 });           // 705600000
//   const curve = anim.compile(item.visual.opacity, { prop: "opacity" });
//   const ys = curve.evalRange(0, duration, 1000);        // Float64Array, one value per sample
//   curve.free();
//
// After `initMath()` resolves, everything is synchronous. Times are flicks: pass a safe integer
// `number` (the protocol's `Time`) or a `bigint`; results are `number`s (always safe integers).
// i64 crosses the wasm boundary as a BigInt only inside this module.

import type {
  Animatable,
  Easing,
  FrameRate,
  Interp,
  PropId,
  Rounding,
  Time,
  Vec2,
} from "../generated/types.ts";
import { abiMismatches, readAbi } from "./abi.ts";
import { EXPORT_SIGNATURES, MODULE_VERSION, type RawExports } from "./generated/exports.ts";

/** A time or frame argument: a safe integer `number` or a `bigint`. */
export type Flicks = number | bigint;

/** The value types an animatable can have (keyframes §5), as the vectors name them. */
export type ValueKind = "scalar" | "vec2" | "angle" | "angle_shortest" | "color" | "bool";

/** An evaluated colour: linear light, premultiplied alpha (keyframes §5.4). */
export interface LinearRgba {
  readonly r: number;
  readonly g: number;
  readonly b: number;
  readonly a: number;
}

/** What evaluating an animatable gives, by value type. */
export interface EvalValues {
  scalar: number;
  angle: number;
  angle_shortest: number;
  vec2: Vec2;
  color: LinearRgba;
  bool: boolean;
}

/** One evaluated value of any type. */
export type EvalValue = EvalValues[ValueKind];

/** Numbers per sample in `evalRange` output, by value type. */
export const COMPONENTS: Readonly<Record<ValueKind, number>> = {
  scalar: 1,
  angle: 1,
  angle_shortest: 1,
  bool: 1,
  vec2: 2,
  color: 4,
};

/** What an animatable is: a value type, a property (which adds its range check and clamp, §7), or both. */
export type AnimTarget =
  | { readonly type: ValueKind; readonly prop?: PropId }
  | { readonly type?: ValueKind; readonly prop: PropId };

/** The result of parsing a time the user typed. */
export type TimeParseResult =
  | { readonly ok: true; readonly flicks: Time }
  | { readonly ok: false; readonly error: string };

/** An easing name with its interpolation (§4.6). */
export interface EasingPreset {
  readonly name: Easing;
  readonly interp: Interp;
}

/** A math call the engine's code refused; `message` is the engine's own text. */
export class MathError extends Error {
  /** The keyframe invariant broken (`"KF-1"` ... `"KF-7"`), when there is one. */
  readonly code: string | undefined;

  constructor(message: string) {
    super(message);
    this.name = "MathError";
    this.code = /^(KF-\d)\b/.exec(message)?.[1];
  }
}

/** The module does not match this `@cutweave/protocol` (rebuild it with `just wasm`). */
export class MathVersionError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "MathVersionError";
  }
}

/** Time conversions (arch §4.1): the only allowed ones, exact integer math. */
export interface TimeMath {
  /** The start of `frame` at `rate`. */
  frameToTime(frame: Flicks, rate: FrameRate): Time;
  /** The frame containing `t` (`"floor"`) or the nearest frame boundary (`"nearest"`). */
  timeToFrame(t: Flicks, rate: FrameRate, rounding: Rounding): number;
  /** `t` moved onto the frame grid. */
  snapToFrame(t: Flicks, rate: FrameRate, rounding: Rounding): Time;
  /**
   * A time as typed: flicks, seconds (`"62.5s"`), a frame (`"f:1500"`) or a timecode
   * (`"00:01:02:03"`, drop-frame `"00:01:02;03"`). Frame-based forms need `rate`.
   */
  parse(text: string, rate?: FrameRate | null): TimeParseResult;
  /** The lossless canonical text (`"62.5s"` or raw flicks), or seconds with `precision` decimals. */
  format(t: Flicks, precision?: number): string;
  /** The timecode label of `t` at `rate` (`"01:02:03:04"`, drop-frame `"01:02:03;04"`). */
  timecode(
    t: Flicks,
    rate: FrameRate,
    options?: { readonly dropFrame?: boolean; readonly rounding?: Rounding },
  ): string;
  /** The `n` sample times from `t0` to `t1` that `evalRange` uses (keyframes §6). */
  sampleTimes(t0: Flicks, t1: Flicks, n: number): Float64Array;
}

/** An animatable parsed once, evaluated many times. Call `free()` when done. */
export interface CompiledAnim<K extends ValueKind = ValueKind> {
  readonly type: K;
  /** Numbers per sample in `evalRange` output. */
  readonly components: number;
  /** The value at item time `t` (clamped when compiled for a property). */
  eval(t: Flicks): EvalValues[K];
  /**
   * `n` samples from `t0` to `t1` at `time.sampleTimes(t0, t1, n)`, interleaved (`x, y` for a
   * vec2; `r, g, b, a` for a colour; 0 or 1 for a bool), into `out` when given.
   */
  evalRange(t0: Flicks, t1: Flicks, n: number, out?: Float64Array): Float64Array;
  /** Releases the module's copy. Later calls throw. */
  free(): void;
}

/** Keyframe evaluation (keyframes §6, §7). */
export interface AnimMath {
  /** Parses (and validates, KF-1 to KF-7) an animatable's wire form (§3.1). */
  compile<K extends ValueKind = ValueKind>(
    animatable: Animatable<unknown>,
    target: AnimTarget,
  ): CompiledAnim<K>;
  /** One value: compile, evaluate, free. */
  eval(animatable: Animatable<unknown>, target: AnimTarget, t: Flicks): EvalValue;
  /** One range: compile, `evalRange`, free. */
  evalRange(
    animatable: Animatable<unknown>,
    target: AnimTarget,
    t0: Flicks,
    t1: Flicks,
    n: number,
  ): Float64Array;
}

/** Easing curves (keyframes §4). */
export interface EasingMath {
  /** The progress `dt` flicks into a segment of `span` flicks. */
  progress(interp: Interp, dt: Flicks, span: Flicks): number;
  /** The progress at the `n` sample times from 0 to `span` (curve editor previews). */
  sample(interp: Interp, span: Flicks, n: number, out?: Float64Array): Float64Array;
  /** Every easing name a command accepts with its interpolation, in picker order. */
  presets(): readonly EasingPreset[];
}

/** A loaded math module. */
export interface MathModule {
  /** The version the module reports (equals `MODULE_VERSION`). */
  readonly version: string;
  readonly time: TimeMath;
  readonly anim: AnimMath;
  readonly easing: EasingMath;
  /** Compiled animatables not yet freed (leak checks). */
  liveAnimatables(): number;
}

/** Where the module comes from: a URL (fetched, or read from disk in Node), bytes or a response. */
export type MathSource = URL | ArrayBuffer | ArrayBufferView | Response;

/** The staged module, next to this package (`just wasm` puts it there). */
export const DEFAULT_MODULE_URL = new URL("../../wasm/cutweave_math.wasm", import.meta.url);

const ERR_TIME = -(2n ** 63n);
const ROUNDING: Readonly<Record<Rounding, number>> = { floor: 0, nearest: 1 };

function big(v: Flicks, what: string): bigint {
  const b = typeof v === "bigint" ? v : BigInt(v);
  if (BigInt.asIntN(64, b) !== b) throw new RangeError(`${what} ${b} does not fit in 64 bits`);
  return b;
}

function u32(v: number, what: string): number {
  if (!Number.isInteger(v) || v < 0 || v > 0xffff_ffff) {
    throw new RangeError(`${what} must be an integer in [0, 2^32), got ${v}`);
  }
  return v;
}

function rateArgs(rate: FrameRate): [number, number] {
  return [u32(rate.num, "frame rate numerator"), u32(rate.den, "frame rate denominator")];
}

function rounding(r: Rounding): number {
  const code = Object.hasOwn(ROUNDING, r) ? ROUNDING[r] : undefined;
  if (code === undefined) throw new RangeError(`unknown rounding ${String(r)}`);
  return code;
}

/** Marshalling over one instance's exports and memory. */
class Raw {
  readonly x: RawExports;
  #encoder = new TextEncoder();
  #decoder = new TextDecoder();
  #floats: Float64Array;
  #bytes: Uint8Array;

  constructor(x: RawExports) {
    this.x = x;
    this.#floats = new Float64Array(x.memory.buffer);
    this.#bytes = new Uint8Array(x.memory.buffer);
  }

  /** Views over the current memory (a call can grow it, which detaches the old buffer). */
  #views(): void {
    const buffer = this.x.memory.buffer;
    if (this.#bytes.buffer !== buffer) {
      this.#bytes = new Uint8Array(buffer);
      this.#floats = new Float64Array(buffer);
    }
  }

  /** Writes the strings one after another into the input buffer; returns their byte lengths. */
  input(...parts: string[]): number[] {
    const encoded = parts.map((p) => this.#encoder.encode(p));
    const total = encoded.reduce((n, e) => n + e.length, 0);
    const ptr = this.x.cw_in(total) >>> 0;
    this.#views();
    let at = ptr;
    for (const e of encoded) {
      this.#bytes.set(e, at);
      at += e.length;
    }
    return encoded.map((e) => e.length);
  }

  /** The output buffer's text. */
  output(): string {
    const len = this.x.cw_out_len() >>> 0;
    const ptr = this.x.cw_out_ptr() >>> 0;
    this.#views();
    return this.#decoder.decode(this.#bytes.subarray(ptr, ptr + len));
  }

  fail(): never {
    throw new MathError(this.output());
  }

  /** A time or frame result. */
  time(result: bigint): number {
    if (result === ERR_TIME) this.fail();
    return Number(result);
  }

  /** The text of a call that returned a length. */
  text(len: number): string {
    if (len < 0) this.fail();
    return this.output();
  }

  /** Copies the `count` numbers a call left in the number buffer into `out` (or a new array). */
  numbers(count: number, out?: Float64Array): Float64Array {
    if (count < 0) this.fail();
    const ptr = this.x.cw_f64_ptr() >>> 0;
    this.#views();
    const target = out ?? new Float64Array(count);
    if (target.length < count) {
      throw new RangeError(`the output array holds ${target.length} numbers, ${count} needed`);
    }
    target.set(this.#floats.subarray(ptr / 8, ptr / 8 + count));
    return target;
  }

  /** Number `index` of those a call left in the number buffer. */
  number(index: number): number {
    const ptr = this.x.cw_f64_ptr() >>> 0;
    this.#views();
    return this.#floats[ptr / 8 + index] as number;
  }
}

function samples(n: number): number {
  return u32(n, "sample count");
}

function createTime(raw: () => Raw): TimeMath {
  return {
    frameToTime(frame, rate) {
      const r = raw();
      return r.time(r.x.cw_frame_to_time(big(frame, "frame"), ...rateArgs(rate)));
    },
    timeToFrame(t, rate, round) {
      const r = raw();
      return r.time(r.x.cw_time_to_frame(big(t, "time"), ...rateArgs(rate), rounding(round)));
    },
    snapToFrame(t, rate, round) {
      const r = raw();
      return r.time(r.x.cw_snap_to_frame(big(t, "time"), ...rateArgs(rate), rounding(round)));
    },
    parse(text, rate) {
      const r = raw();
      const [num, den] = rate ? rateArgs(rate) : [0, 0];
      const [len] = r.input(text);
      const flicks = r.x.cw_time_parse(len as number, num, den);
      if (flicks === ERR_TIME) return { ok: false, error: r.output() };
      return { ok: true, flicks: Number(flicks) };
    },
    format(t, precision) {
      const r = raw();
      if (precision !== undefined && (!Number.isInteger(precision) || precision < 0)) {
        throw new RangeError(`precision must be a non-negative integer, got ${precision}`);
      }
      return r.text(r.x.cw_time_format(big(t, "time"), Math.min(precision ?? -1, 0x7fff_ffff)));
    },
    timecode(t, rate, options = {}) {
      const r = raw();
      const drop = options.dropFrame === true ? 1 : 0;
      const round = rounding(options.rounding ?? "floor");
      return r.text(r.x.cw_timecode(big(t, "time"), ...rateArgs(rate), drop, round));
    },
    sampleTimes(t0, t1, n) {
      const r = raw();
      return r.numbers(r.x.cw_sample_times(big(t0, "time"), big(t1, "time"), samples(n)));
    },
  };
}

/** Frees compiled animatables that were dropped without `free()`. */
const finalizers =
  typeof FinalizationRegistry === "undefined"
    ? undefined
    : new FinalizationRegistry<{ raw: Raw; handle: number }>(({ raw, handle }) => {
        raw.x.cw_anim_free(handle);
      });

function shape(r: Raw, type: ValueKind): EvalValue {
  switch (type) {
    case "vec2":
      return { x: r.number(0), y: r.number(1) };
    case "color":
      return { r: r.number(0), g: r.number(1), b: r.number(2), a: r.number(3) };
    case "bool":
      return r.number(0) !== 0;
    default:
      return r.number(0);
  }
}

function compile<K extends ValueKind>(
  r: Raw,
  animatable: Animatable<unknown>,
  target: AnimTarget,
): CompiledAnim<K> {
  const json = JSON.stringify(animatable);
  if (json === undefined) throw new MathError("the animatable is not JSON");
  const [a, b, c] = r.input(target.type ?? "", target.prop ?? "", json);
  const handle = r.x.cw_anim_new(a as number, b as number, c as number) >>> 0;
  if (handle === 0) r.fail();
  const type = r.text(r.x.cw_anim_type(handle)) as K;
  let live = true;
  const token = {};
  finalizers?.register(token, { raw: r, handle }, token);
  const check = () => {
    if (!live) throw new MathError("this animatable was freed");
  };
  return {
    type,
    components: COMPONENTS[type],
    eval(t) {
      check();
      const count = r.x.cw_anim_eval(handle, big(t, "time"));
      if (count < 0) r.fail();
      return shape(r, type) as EvalValues[K];
    },
    evalRange(t0, t1, n, out) {
      check();
      const count = r.x.cw_anim_eval_range(handle, big(t0, "time"), big(t1, "time"), samples(n));
      return r.numbers(count, out);
    },
    free() {
      if (!live) return;
      live = false;
      finalizers?.unregister(token);
      r.x.cw_anim_free(handle);
    },
  };
}

function createAnim(raw: () => Raw): AnimMath {
  return {
    compile: <K extends ValueKind>(animatable: Animatable<unknown>, target: AnimTarget) =>
      compile<K>(raw(), animatable, target),
    eval(animatable, target, t) {
      const c = compile(raw(), animatable, target);
      try {
        return c.eval(t);
      } finally {
        c.free();
      }
    },
    evalRange(animatable, target, t0, t1, n) {
      const c = compile(raw(), animatable, target);
      try {
        return c.evalRange(t0, t1, n);
      } finally {
        c.free();
      }
    },
  };
}

function createEasing(raw: () => Raw): EasingMath {
  let presets: readonly EasingPreset[] | undefined;
  return {
    progress(interp, dt, span) {
      const r = raw();
      const [len] = r.input(JSON.stringify(interp));
      const p = r.x.cw_easing_progress(len as number, big(dt, "time"), big(span, "time"));
      if (Number.isNaN(p)) r.fail();
      return p;
    },
    sample(interp, span, n, out) {
      const r = raw();
      const [len] = r.input(JSON.stringify(interp));
      return r.numbers(r.x.cw_easing_sample(len as number, big(span, "time"), samples(n)), out);
    },
    presets() {
      if (presets === undefined) {
        const r = raw();
        const list = JSON.parse(r.text(r.x.cw_easing_presets())) as EasingPreset[];
        presets = Object.freeze(list.map((p) => Object.freeze(p)));
      }
      return presets;
    },
  };
}

/** The marshalling behind each loaded module (for the shared `time`, `anim` and `easing`). */
const rawOf = new WeakMap<MathModule, Raw>();

function createModule(raw: Raw, version: string): MathModule {
  const get = () => raw;
  const module: MathModule = {
    version,
    time: createTime(get),
    anim: createAnim(get),
    easing: createEasing(get),
    liveAnimatables: () => raw.x.cw_anim_live() >>> 0,
  };
  rawOf.set(module, raw);
  return module;
}

async function bytesOf(source: MathSource): Promise<Uint8Array> {
  if (source instanceof ArrayBuffer) return new Uint8Array(source);
  if (ArrayBuffer.isView(source)) {
    return new Uint8Array(source.buffer, source.byteOffset, source.byteLength);
  }
  if (source instanceof URL && source.protocol === "file:") {
    // Node (tests, scripts): fetch() has no file: URLs. The specifier is a variable so bundlers
    // leave it alone; browsers load over http(s) or the app's protocol.
    const fsModule = "node:fs/promises";
    const fs = (await import(/* @vite-ignore */ fsModule)) as {
      readFile(url: URL): Promise<Uint8Array>;
    };
    return new Uint8Array(await fs.readFile(source));
  }
  const response = source instanceof Response ? source : await fetch(source);
  if (!response.ok) {
    throw new MathError(`cannot load the math module from ${response.url}: ${response.status}`);
  }
  return new Uint8Array(await response.arrayBuffer());
}

/**
 * Loads a fresh instance of the module. Rejects with `MathVersionError` when its exports or its
 * version do not match this package (a stale build: run `just wasm`).
 */
export async function loadMath(source: MathSource = DEFAULT_MODULE_URL): Promise<MathModule> {
  const bytes = await bytesOf(source);
  const problems = abiMismatches(readAbi(bytes), EXPORT_SIGNATURES);
  if (problems.length > 0) {
    throw new MathVersionError(
      `the math module does not match @cutweave/protocol (rebuild it with \`just wasm\`): ${problems.join("; ")}`,
    );
  }
  const module = await WebAssembly.compile(bytes as Uint8Array<ArrayBuffer>);
  const instance = await WebAssembly.instantiate(module, {});
  const raw = new Raw(instance.exports as unknown as RawExports);
  const version = raw.text(raw.x.cw_version());
  if (version !== MODULE_VERSION) {
    throw new MathVersionError(
      `the math module is version ${version}, @cutweave/protocol expects ${MODULE_VERSION} (rebuild it with \`just wasm\`)`,
    );
  }
  return createModule(raw, version);
}

let pending: Promise<MathModule> | undefined;
let ready: MathModule | undefined;

/**
 * Loads the shared module once (later calls return the same promise); the synchronous `time`,
 * `anim` and `easing` below work after it resolves. A failed load can be retried.
 */
export function initMath(source?: MathSource): Promise<MathModule> {
  pending ??= loadMath(source).then(
    (m) => {
      ready = m;
      return m;
    },
    (e: unknown) => {
      pending = undefined;
      throw e;
    },
  );
  return pending;
}

/** The shared module; throws before `initMath()` has resolved. */
export function math(): MathModule {
  if (ready === undefined)
    throw new MathError("the math module is not loaded: await initMath() first");
  return ready;
}

function sharedRaw(): Raw {
  return rawOf.get(math()) as Raw;
}

/** Time conversions of the shared module (after `initMath()`). */
export const time: TimeMath = createTime(sharedRaw);
/** Keyframe evaluation of the shared module (after `initMath()`). */
export const anim: AnimMath = createAnim(sharedRaw);
/** Easing curves of the shared module (after `initMath()`). */
export const easing: EasingMath = createEasing(sharedRaw);
