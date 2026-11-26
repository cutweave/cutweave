// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2C.06 acceptance for the shared math module (`@cutweave/protocol/wasm`):
// - parity: every row of the native fixture (crates/cutweave-wasm/tests/parity.rs runs the same
//   exports natively) gives the same bits through wasm, 0 mismatches; float evaluation is also
//   within 1e-9 of the keyframe vectors, and time parsing matches the time-input fixture exactly;
// - the typings snapshot, the version tie to this package, the loader's refusals, the size budget;
// - speed: init under 30 ms, `evalRange` of 1,000 samples under 0.3 ms.

import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";
import { beforeAll, describe, expect, it } from "vitest";
import type { FrameRate, Interp, PropId, Rounding } from "../generated/types.ts";
import { mathModuleBytes, repoRoot } from "../test-support/math-wasm.ts";
import { readAbi, renderExportsModule } from "./abi.ts";
import { MODULE_VERSION } from "./generated/exports.ts";
import {
  type AnimTarget,
  anim,
  type EvalValue,
  easing,
  initMath,
  loadMath,
  MathError,
  type MathModule,
  MathVersionError,
  math,
  SNAP_KINDS,
  type SnapKindName,
  time,
  type ValueKind,
} from "./index.ts";

type Json = unknown;
type Failed = { readonly error: string };
/** `[input, result]` rows of one call. */
type Pairs = [string, string | Failed][];

const readJson = (rel: string): Json => JSON.parse(readFileSync(path.join(repoRoot, rel), "utf8"));

interface Fixture {
  version: { len: number; text: string };
  conversions: ({ rate: FrameRate; frame_to_time: Pairs } & {
    [K in `${"time_to_frame" | "snap_to_frame"}_${Rounding}`]: Pairs;
  })[];
  conversion_errors: [string, string, number, number, Failed][];
  timecode: {
    rate: FrameRate;
    drop_frame: boolean;
    rounding: Rounding;
    labels: [string, string | Failed][];
  }[];
  format: [string, number, string | Failed][];
  parse_errors: [string, FrameRate, Failed][];
  sample_times: [string, string, number, number[] | Failed][];
  presets: { name: string; interp: Interp }[];
  easing: [string, number][];
  easing_sample: [Json, string, number, number[] | Failed][];
  eval: [string, number[]][];
  eval_range: [string, string, string, number, number[] | Failed][];
  invalid: [string, string][];
  snap: {
    name: string;
    rate: [number, number];
    candidates: [number, string][];
    queries: { r: string; offsets: string[]; px: number; threshold: number; enabled: number }[];
    results: [string, number, string, number][];
  }[];
  snap_errors: { case: string; t?: string | Failed; handle?: number; message?: string }[];
}

interface VectorRow {
  id: string;
  type: ValueKind;
  prop: PropId | null;
  animatable: Json;
  t: number;
  expected: Json;
  exact?: boolean;
  interp: Interp;
  dt: number;
  span: number;
  error: string;
}

const fixture = readJson("crates/cutweave-wasm/tests/fixtures/parity.json") as Fixture;
const vectors = readJson("docs/spec/keyframes-vectors.json") as Record<string, VectorRow[]>;
const timeInput = readJson("crates/cutweave-model/tests/fixtures/time-input.json") as {
  cases: { input: string; rate: FrameRate | null; flicks: number | null }[];
};
const byId = (section: string) =>
  new Map((vectors[section] as VectorRow[]).map((row) => [row.id, row]));
const TOLERANCE = 1e-9;

const isFailed = (v: unknown): v is Failed =>
  typeof v === "object" && v !== null && "error" in (v as object);

/** The components of an evaluated value, as the number buffer lays them out. */
function components(v: unknown): number[] {
  if (typeof v === "number") return [v];
  if (typeof v === "boolean") return [v ? 1 : 0];
  const o = v as Record<string, number>;
  return "x" in o ? [o.x as number, o.y as number] : [o.r, o.g, o.b, o.a].map(Number);
}

const sameBits = (a: readonly number[], b: readonly number[]) =>
  a.length === b.length && a.every((x, i) => Object.is(x, b[i]));

function targetOf(row: { type: ValueKind; prop: PropId | null }): AnimTarget {
  return row.prop === null ? { type: row.type } : { type: row.type, prop: row.prop };
}

let bytes: Uint8Array;
let m: MathModule;

beforeAll(async () => {
  bytes = mathModuleBytes();
  m = await loadMath(bytes);
}, 900_000);

/**
 * Runs `cases`, comparing each result with `expected` (or the error message), and returns the
 * mismatches as text.
 */
function compare<C>(
  cases: readonly C[],
  run: (c: C) => { got: () => unknown; want: unknown; same?: (a: unknown, b: unknown) => boolean },
): string[] {
  const mismatches: string[] = [];
  for (const c of cases) {
    const { got, want, same = Object.is } = run(c);
    let value: unknown;
    try {
      value = got();
    } catch (e) {
      if (isFailed(want) && e instanceof MathError && e.message === want.error) continue;
      mismatches.push(`${JSON.stringify(c)}: threw ${String(e)}`);
      continue;
    }
    if (isFailed(want) || !same(value, want)) {
      mismatches.push(`${JSON.stringify(c)}: got ${JSON.stringify(value)}`);
    }
  }
  return mismatches;
}

describe("the shared module before initMath()", () => {
  it("refuses synchronous calls", () => {
    expect(() => time.frameToTime(1, { num: 30, den: 1 })).toThrow(/await initMath\(\) first/);
    expect(() => math()).toThrow(MathError);
  });
});

describe("parity with the native exports (0 mismatches)", () => {
  it("frame conversions at every rate, bit for bit", () => {
    let n = 0;
    const mismatches = fixture.conversions.flatMap((row) => {
      const rate = row.rate;
      const cases = [
        ...row.frame_to_time.map(([f, want]) => ({
          got: () => String(m.time.frameToTime(BigInt(f), rate)),
          want,
        })),
        ...(["floor", "nearest"] as const).flatMap((r) => [
          ...row[`time_to_frame_${r}`].map(([t, want]) => ({
            got: () => String(m.time.timeToFrame(BigInt(t), rate, r)),
            want,
          })),
          ...row[`snap_to_frame_${r}`].map(([t, want]) => ({
            got: () => String(m.time.snapToFrame(BigInt(t), rate, r)),
            want,
          })),
        ]),
      ];
      n += cases.length;
      return compare(cases, (c) => c);
    });
    expect(mismatches).toEqual([]);
    expect(n).toBeGreaterThan(1000);
  });

  it("refused conversions, with the engine's messages", () => {
    const mismatches = compare(fixture.conversion_errors, ([call, input, num, den, want]) => {
      const rate = { num, den };
      const t = BigInt(input);
      const got = {
        frame_to_time: () => m.time.frameToTime(t, rate),
        time_to_frame: () => m.time.timeToFrame(t, rate, "floor"),
        snap_to_frame: () => m.time.snapToFrame(t, rate, "nearest"),
        timecode: () => m.time.timecode(t, rate),
        timecode_drop: () => m.time.timecode(t, rate, { dropFrame: true }),
      }[call];
      if (got === undefined) throw new Error(`unknown call ${call}`);
      return { got, want };
    });
    expect(mismatches).toEqual([]);
    expect(fixture.conversion_errors.length).toBeGreaterThan(20);
  });

  it("timecode labels, drop-frame and not", () => {
    const mismatches = fixture.timecode.flatMap((row) =>
      compare(row.labels, ([t, want]) => ({
        got: () =>
          m.time.timecode(BigInt(t), row.rate, {
            dropFrame: row.drop_frame,
            rounding: row.rounding,
          }),
        want,
      })),
    );
    expect(mismatches).toEqual([]);
  });

  it("time text, canonical and approximate", () => {
    const mismatches = compare(fixture.format, ([t, precision, want]) => ({
      got: () => m.time.format(BigInt(t), precision < 0 ? undefined : precision),
      want,
    }));
    expect(mismatches).toEqual([]);
  });

  it("time parsing matches the time-input fixture exactly", () => {
    const mismatches: string[] = [];
    for (const c of timeInput.cases) {
      const out = m.time.parse(c.input, c.rate);
      const ok = c.flicks === null ? !out.ok : out.ok && out.flicks === c.flicks;
      if (!ok) mismatches.push(`${JSON.stringify(c)}: got ${JSON.stringify(out)}`);
    }
    for (const [input, rate, want] of fixture.parse_errors) {
      const out = m.time.parse(input, rate.num === 0 && rate.den === 0 ? null : rate);
      if (out.ok || out.error !== want.error) {
        mismatches.push(`${input} @ ${JSON.stringify(rate)}: got ${JSON.stringify(out)}`);
      }
    }
    expect(mismatches).toEqual([]);
    expect(timeInput.cases.length).toBeGreaterThan(300);
  });

  it("sample times", () => {
    const mismatches = compare(fixture.sample_times, ([t0, t1, n, want]) => ({
      got: () => [...m.time.sampleTimes(BigInt(t0), BigInt(t1), n)],
      want,
      same: (a, b) => sameBits(a as number[], b as number[]),
    }));
    expect(mismatches).toEqual([]);
  });

  it("easing progress: bit for bit with native, within 1e-9 of the vectors", () => {
    const rows = byId("easing");
    const mismatches: string[] = [];
    for (const [id, native] of fixture.easing) {
      const row = rows.get(id) as VectorRow;
      const got = m.easing.progress(row.interp, row.dt, row.span);
      const want = row.expected as number;
      const close = row.exact ? got === want : Math.abs(got - want) <= TOLERANCE;
      if (!Object.is(got, native) || !close) mismatches.push(`${id}: got ${got}, native ${native}`);
    }
    expect(mismatches).toEqual([]);
    expect(fixture.easing.length).toBe(rows.size);
  });

  it("easing curves of every name, and refused interpolations", () => {
    const presets = new Map(fixture.presets.map((p) => [p.name, p.interp]));
    const mismatches = compare(fixture.easing_sample, ([name, span, n, want]) => {
      const interp = (typeof name === "string" ? presets.get(name) : name) as Interp;
      return {
        got: () => [...m.easing.sample(interp, BigInt(span), n)],
        want,
        same: (a, b) => sameBits(a as number[], b as number[]),
      };
    });
    expect(mismatches).toEqual([]);
  });

  it("the easing list", () => {
    expect(m.easing.presets()).toEqual(fixture.presets);
    expect(m.easing.presets().map((p) => p.name)).toEqual([
      "hold",
      "linear",
      ...Object.keys((vectors as unknown as { presets: object }).presets),
    ]);
  });

  it("keyframe evaluation: bit for bit with native, within 1e-9 of the vectors", () => {
    const rows = byId("eval");
    const mismatches: string[] = [];
    for (const [id, native] of fixture.eval) {
      const row = rows.get(id) as VectorRow;
      const got = components(m.anim.eval(row.animatable, targetOf(row), row.t));
      const want = components(row.expected);
      const close = got.every((x, i) =>
        row.exact ? x === want[i] : Math.abs(x - (want[i] as number)) <= TOLERANCE,
      );
      if (!sameBits(got, native) || !close) {
        mismatches.push(`${id}: got ${got}, native ${native}, expected ${want}`);
      }
    }
    expect(mismatches).toEqual([]);
    expect(fixture.eval.length).toBe(rows.size);
  });

  it("keyframe ranges (evalRange), bit for bit", () => {
    const rows = byId("eval");
    const mismatches = compare(fixture.eval_range, ([id, t0, t1, n, want]) => {
      const row = rows.get(id) as VectorRow;
      return {
        got: () => [...m.anim.evalRange(row.animatable, targetOf(row), BigInt(t0), BigInt(t1), n)],
        want,
        same: (a, b) => sameBits(a as number[], b as number[]),
      };
    });
    expect(mismatches).toEqual([]);
  });

  it("invalid animatables fail with the engine's message and invariant code", () => {
    const rows = byId("invalid");
    const mismatches: string[] = [];
    for (const [id, message] of fixture.invalid) {
      const row = rows.get(id);
      let target: AnimTarget;
      let animatable: Json;
      if (row !== undefined) {
        target = targetOf(row);
        animatable = row.animatable;
      } else {
        const [type, prop, json] = id.split("|") as [string, string, string];
        target = { ...(type ? { type } : {}), ...(prop ? { prop } : {}) } as AnimTarget;
        animatable = JSON.parse(json);
      }
      try {
        m.anim.compile(animatable, target).free();
        mismatches.push(`${id}: compiled`);
      } catch (e) {
        // A vector's message may echo its value, which JSON.stringify writes as JavaScript does
        // (`1`, where the vectors file has `1.0`): compare the invariant and where it broke.
        const ok =
          e instanceof MathError &&
          (row === undefined
            ? e.message === message
            : e.code === row.error && e.message.split(": ")[0] === message.split(": ")[0]);
        if (!ok) mismatches.push(`${id}: ${String(e)}`);
      }
    }
    expect(mismatches).toEqual([]);
    expect(m.liveAnimatables()).toBe(0);
  });

  it("the module version is this package's", () => {
    const pkg = JSON.parse(
      readFileSync(fileURLToPath(new URL("../../package.json", import.meta.url)), "utf8"),
    ) as { version: string };
    expect(m.version).toBe(fixture.version.text);
    expect(MODULE_VERSION).toBe(m.version);
    expect(MODULE_VERSION).toBe(pkg.version);
  });
});

describe("typings snapshot", () => {
  it("generated/exports.ts is what `just wasm` renders from the module", () => {
    const committed = readFileSync(
      fileURLToPath(new URL("./generated/exports.ts", import.meta.url)),
      "utf8",
    ).replace(/\r\n/g, "\n");
    expect(
      renderExportsModule(readAbi(bytes), m.version),
      "the module's exports changed: run `just wasm` and commit the typings",
    ).toBe(committed);
  });
});

/** A module of one function `cw_version: () -> i32` and a memory. */
const TINY_MODULE = new Uint8Array([
  0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7f, 0x03,
  0x02, 0x01, 0x00, 0x05, 0x03, 0x01, 0x00, 0x01, 0x07, 0x17, 0x02, 0x0a, 0x63, 0x77, 0x5f, 0x76,
  0x65, 0x72, 0x73, 0x69, 0x6f, 0x6e, 0x00, 0x00, 0x06, 0x6d, 0x65, 0x6d, 0x6f, 0x72, 0x79, 0x02,
  0x00, 0x0a, 0x06, 0x01, 0x04, 0x00, 0x41, 0x00, 0x0b,
]);

/** The module with a custom section appended: the same code, but new bytes (no compile cache). */
function withNonce(source: Uint8Array, nonce: number): Uint8Array {
  const name = new TextEncoder().encode(`nonce${nonce}`);
  const section = new Uint8Array([0x00, name.length + 1, name.length, ...name]);
  const out = new Uint8Array(source.length + section.length);
  out.set(source);
  out.set(section, source.length);
  return out;
}

describe("loading", () => {
  it("refuses a module whose exports differ from the typings", async () => {
    expect(readAbi(TINY_MODULE).functions).toEqual({
      cw_version: { params: [], results: ["i32"] },
    });
    const err = await loadMath(TINY_MODULE).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(MathVersionError);
    expect(String(err)).toMatch(/cw_anim_eval is missing/);
  });

  it("refuses a module of another version", async () => {
    const patched = bytes.slice();
    const from = new TextEncoder().encode(MODULE_VERSION);
    const to = new TextEncoder().encode("9".repeat(from.length));
    let at = -1;
    for (let i = 0; i + from.length <= patched.length && at < 0; i++) {
      if (from.every((b, j) => patched[i + j] === b)) at = i;
    }
    expect(at).toBeGreaterThan(0);
    patched.set(to, at);
    await expect(loadMath(patched)).rejects.toThrow(MathVersionError);
  });

  it("initMath() loads the staged module once; the shared API is synchronous after it", async () => {
    const first = initMath();
    expect(initMath()).toBe(first);
    const loaded = await first;
    expect(math()).toBe(loaded);
    expect(time.frameToTime(30, { num: 30, den: 1 })).toBe(705_600_000);
    expect(time.timeToFrame(705_600_000, { num: 30_000, den: 1001 }, "floor")).toBe(29);
    expect(time.timecode(1800 * 23_543_520, { num: 30_000, den: 1001 }, { dropFrame: true })).toBe(
      "00:01:00;02",
    );
    expect(time.parse("62.5s")).toEqual({ ok: true, flicks: 44_100_000_000 });
    expect(easing.progress({ kind: "linear" }, 1, 4)).toBe(0.25);
    expect(anim.eval(0.5, { prop: "opacity" }, 0)).toBe(0.5);
  });
});

describe("the typed API", () => {
  const opacity = {
    keyframes: [
      { t: 0, value: 0, interp: { kind: "linear" } },
      { t: 705_600_000, value: 1, interp: { kind: "hold" } },
    ],
  };

  it("shapes values by type and lays out ranges interleaved", () => {
    const pos = m.anim.compile<"vec2">(
      {
        keyframes: [
          { t: 0, value: { x: 0, y: 10 }, interp: { kind: "linear" } },
          { t: 4, value: { x: 4, y: 20 }, interp: { kind: "linear" } },
        ],
      },
      { prop: "transform.position" },
    );
    expect(pos.type).toBe("vec2");
    expect(pos.components).toBe(2);
    expect(pos.eval(1)).toEqual({ x: 1, y: 12.5 });
    const out = new Float64Array(10);
    expect(pos.evalRange(0, 4, 3, out)).toBe(out);
    expect([...out.subarray(0, 6)]).toEqual([0, 10, 2, 15, 4, 20]);
    expect(() => pos.evalRange(0, 4, 6, out)).toThrow(RangeError);
    pos.free();
    expect(() => pos.eval(0)).toThrow(/freed/);
    pos.free();
    const flip = m.anim.compile(true, { prop: "flip.horizontal" });
    expect(flip.eval(0)).toBe(true);
    flip.free();
    const red: EvalValue = m.anim.eval("#ff0000", { type: "color" }, 0);
    expect(red).toEqual({ r: 1, g: 0, b: 0, a: 1 });
    expect(m.liveAnimatables()).toBe(0);
  });

  it("clamps for a property and reports keyframe invariants by code", () => {
    const overshoot = {
      keyframes: [
        { t: 0, value: 0, interp: { kind: "bezier", x1: 0.34, y1: 1.56, x2: 0.64, y2: 1 } },
        { t: 100, value: 1, interp: { kind: "linear" } },
      ],
    };
    const raw = m.anim.evalRange(overshoot, { type: "scalar" }, 0, 100, 101);
    const clamped = m.anim.evalRange(overshoot, { prop: "opacity" }, 0, 100, 101);
    expect(Math.max(...raw)).toBeGreaterThan(1);
    expect(Math.max(...clamped)).toBe(1);
    const err = (() => {
      try {
        m.anim.compile({ keyframes: [] }, { type: "scalar" });
      } catch (e) {
        return e;
      }
      return undefined;
    })();
    expect(err).toBeInstanceOf(MathError);
    expect((err as MathError).code).toBe("KF-1");
    expect(m.anim.eval(opacity, { prop: "opacity" }, 352_800_000)).toBe(0.5);
  });

  it("guards arguments the ABI cannot carry", () => {
    const r30 = { num: 30, den: 1 };
    expect(() => m.time.frameToTime(1.5, r30)).toThrow(RangeError);
    expect(() => m.time.frameToTime(2n ** 64n, r30)).toThrow(RangeError);
    expect(() => m.time.frameToTime(1, { num: 29.97, den: 1 })).toThrow(RangeError);
    expect(() => m.time.frameToTime(1, { num: -30, den: 1 })).toThrow(RangeError);
    expect(() => m.time.sampleTimes(0, 1, -1)).toThrow(RangeError);
    expect(() => m.time.format(0, 1.5)).toThrow(RangeError);
    expect(() => m.time.timeToFrame(0, r30, "up" as Rounding)).toThrow(RangeError);
    expect(m.time.frameToTime(2n ** 30n, { num: 1000, den: 1 })).toBe(757_632_231_014_400);
    // Strings round-trip through the input buffer as UTF-8.
    expect(m.time.parse("é")).toEqual({ ok: false, error: expect.stringContaining("é") });
  });
});

describe("budgets", () => {
  it("is at most 150 KB gzipped", () => {
    const gzipped = gzipSync(bytes, { level: 9 }).length;
    console.info(`math module: ${bytes.length} bytes, ${gzipped} gzipped`);
    expect(gzipped).toBeLessThanOrEqual(150_000);
  });

  it("initializes in under 30 ms", { retry: 2 }, async () => {
    const times: number[] = [];
    for (let i = 0; i < 3; i++) {
      const fresh = withNonce(bytes, Date.now() * 10 + i);
      const start = performance.now();
      await loadMath(fresh);
      times.push(performance.now() - start);
    }
    times.sort((a, b) => a - b);
    const median = times[1] as number;
    console.info(
      `math module init: median ${median.toFixed(2)} ms of ${times.map((t) => t.toFixed(2))}`,
    );
    expect(median).toBeLessThan(30);
  });

  it("evaluates 1,000 samples in under 0.3 ms", { retry: 2 }, () => {
    const S = 705_600_000;
    const curve = m.anim.compile(
      {
        keyframes: [
          { t: 0, value: 0, interp: { kind: "bezier", x1: 0.42, y1: 0, x2: 0.58, y2: 1 } },
          {
            t: S,
            value: 1,
            interp: { kind: "spring", stiffness: 170, damping: 26, mass: 1, bounce: false },
          },
          {
            t: 2 * S,
            value: 0.25,
            interp: { kind: "bezier", x1: 0.34, y1: 1.56, x2: 0.64, y2: 1 },
          },
          { t: 3 * S, value: 0.75, interp: { kind: "linear" } },
          { t: 4 * S, value: 0.5, interp: { kind: "hold" } },
        ],
      },
      { prop: "opacity" },
    );
    const out = new Float64Array(1000);
    for (let i = 0; i < 200; i++) curve.evalRange(0, 4 * S, 1000, out);
    const times: number[] = [];
    for (let i = 0; i < 200; i++) {
      const start = performance.now();
      curve.evalRange(0, 4 * S, 1000, out);
      times.push(performance.now() - start);
    }
    curve.free();
    times.sort((a, b) => a - b);
    const median = times[100] as number;
    console.info(
      `evalRange(1000): median ${(median * 1000).toFixed(1)} µs, p95 ${((times[190] as number) * 1000).toFixed(1)} µs`,
    );
    expect(median).toBeLessThan(0.3);
  });
});

const kindsOf = (bits: number): SnapKindName[] =>
  SNAP_KINDS.filter((_, i) => (bits & (1 << i)) !== 0);

describe("snapping (WP-2A.07)", () => {
  it("gives the native result for every scenario, bit for bit", () => {
    let queries = 0;
    const mismatches: string[] = [];
    for (const sc of fixture.snap) {
      const index = m.snap.createIndex(
        { num: sc.rate[0], den: sc.rate[1] },
        sc.candidates.map(([k, t]) => ({ kind: SNAP_KINDS[k] as SnapKindName, t: BigInt(t) })),
      );
      sc.queries.forEach((q, i) => {
        const [t, kind, targetT, probe] = sc.results[i] as [string, number, string, number];
        const got = index.snap(BigInt(q.r), q.offsets.map(BigInt), {
          pxPerSecond: q.px,
          thresholdPx: q.threshold,
          enabled: kindsOf(q.enabled),
        });
        const want = {
          t: Number(t),
          target: kind < 0 ? null : { kind: SNAP_KINDS[kind], t: Number(targetT) },
          probe,
        };
        queries++;
        if (JSON.stringify(got) !== JSON.stringify(want)) {
          mismatches.push(
            `${sc.name} #${i}: got ${JSON.stringify(got)}, want ${JSON.stringify(want)}`,
          );
        }
      });
      index.free();
    }
    expect(queries).toBeGreaterThan(200);
    expect(mismatches).toEqual([]);
    expect(m.snap.liveIndexes()).toBe(0);
  });

  it("refuses what the engine refuses, with the engine's text", () => {
    const index = m.snap.createIndex({ num: 25, den: 1 }, [{ kind: "edge", t: 0 }]);
    for (const row of fixture.snap_errors.filter((e) => e.t !== undefined)) {
      const [px, threshold, r] =
        {
          "zoom 0": [0, 8, 0],
          "zoom NaN": [Number.NaN, 8, 0],
          "threshold -1": [100, -1, 0],
          "unsafe time": [100, 8, 2 ** 53],
        }[row.case] ?? [];
      const message = (row.t as Failed).error;
      expect(
        () =>
          index.snap(r as number, [0], {
            pxPerSecond: px as number,
            thresholdPx: threshold as number,
          }),
        row.case,
      ).toThrow(message);
    }
    index.free();
    const unknown = fixture.snap_errors.find((e) => e.case === "bad rate");
    expect(() => m.snap.createIndex({ num: 0, den: 1 }, [{ kind: "edge", t: 0 }])).toThrow(
      unknown?.message as string,
    );
    expect(() =>
      m.snap.createIndex({ num: 25, den: 1 }, [{ kind: "nope" as "edge", t: 0 }]),
    ).toThrow(RangeError);
  });

  it("frees an index once and refuses later calls", () => {
    const index = m.snap.createIndex({ num: 25, den: 1 }, [{ kind: "edge", t: 0 }]);
    expect(index.size).toBe(1);
    expect(m.snap.liveIndexes()).toBe(1);
    index.free();
    index.free();
    expect(m.snap.liveIndexes()).toBe(0);
    expect(() => index.snap(0, [0], { pxPerSecond: 100 })).toThrow(/freed/);
  });

  it("updates one kind in place and snaps to the new candidates", () => {
    const F = 28_224_000; // one frame at 25 fps
    const index = m.snap.createIndex({ num: 25, den: 1 }, [
      { kind: "edge", t: 10 * F },
      { kind: "playhead", t: 50 * F },
    ]);
    expect(index.snap(10 * F + 1000, [0], { pxPerSecond: 100 }).target).toEqual({
      kind: "edge",
      t: 10 * F,
    });
    expect(index.setKind("edge", [20 * F, 30 * F])).toBe(3);
    expect(index.snap(10 * F + 1000, [0], { pxPerSecond: 100 }).target).toBeNull();
    expect(index.snap(30 * F - 1000, [0], { pxPerSecond: 100 })).toEqual({
      t: 30 * F,
      target: { kind: "edge", t: 30 * F },
      probe: 0,
    });
    index.free();
  });

  it("snaps off with no enabled kinds: the frame snap, on the grid", () => {
    const F = 28_224_000;
    const index = m.snap.createIndex({ num: 25, den: 1 }, [{ kind: "edge", t: 10 * F }]);
    const got = index.snap(10 * F + 5, [0], { pxPerSecond: 100, enabled: [] });
    expect(got).toEqual({ t: 10 * F, target: null, probe: 0 });
    index.free();
  });
});

describe("snap lookup budget", () => {
  it("looks up in 50,000 candidates in under 0.1 ms (p95), through wasm", { retry: 2 }, () => {
    const S = 705_600_000;
    const kinds = ["edge", "marker", "beat"] as const;
    const candidates = Array.from({ length: 50_000 }, (_, i) => ({
      kind: kinds[i % 3] as (typeof kinds)[number],
      t: Math.floor((i * S) / 5),
    }));
    const built = performance.now();
    const index = m.snap.createIndex({ num: 25, den: 1 }, candidates);
    console.info(`snap index of 50,000: built in ${(performance.now() - built).toFixed(1)} ms`);
    expect(index.size).toBe(50_000);
    const options = { pxPerSecond: 100 };
    const offsets = [0, 4 * S];
    let r = 0;
    const next = () => {
      r = (r + 7_919_001) % (9_000 * S);
      return r;
    };
    for (let i = 0; i < 2000; i++) index.snap(next(), offsets, options);
    const times: number[] = [];
    for (let i = 0; i < 2000; i++) {
      const at = next();
      const start = performance.now();
      index.snap(at, offsets, options);
      times.push(performance.now() - start);
    }
    index.free();
    times.sort((a, b) => a - b);
    const p95 = times[Math.floor(times.length * 0.95)] as number;
    console.info(`snap(50,000): p95 ${(p95 * 1000).toFixed(1)} µs`);
    expect(p95).toBeLessThan(0.1);
  });
});
