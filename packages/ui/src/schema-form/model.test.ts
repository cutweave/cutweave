// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { compileSchema } from "./compile.ts";
import { type Clock, GestureBus, MAX_UPDATES_PER_SECOND } from "./gesture.ts";
import type { ChangeMeta } from "./types.ts";
import { deepEqual, getPath, humanize, mergeSelection, setPath } from "./values.ts";
import { evalVisible, parseVisibleWhen } from "./visibleWhen.ts";

describe("visibleWhen", () => {
  const visible = (src: string, values: Record<string, unknown>) => {
    const expr = parseVisibleWhen(src);
    expect(expr, src).not.toBeNull();
    return evalVisible(expr as NonNullable<typeof expr>, (p) => getPath(values, p));
  };
  it("compares to literals", () => {
    expect(visible("mode == 'crf'", { mode: "crf" })).toBe(true);
    expect(visible('mode == "crf"', { mode: "cbr" })).toBe(false);
    expect(visible("mode != 'crf'", { mode: "cbr" })).toBe(true);
    expect(visible("n == 3", { n: 3 })).toBe(true);
    expect(visible("flag == true", { flag: true })).toBe(true);
    expect(visible("x == null", { x: null })).toBe(true);
  });
  it("combines with !, && and || (&& binds tighter) and parentheses", () => {
    expect(visible("a && !b", { a: true, b: false })).toBe(true);
    expect(visible("a || b && c", { a: true, b: false, c: false })).toBe(true);
    expect(visible("(a || b) && c", { a: true, b: false, c: false })).toBe(false);
  });
  it("reads dotted paths", () => {
    expect(visible("transform.mode == 'free'", { transform: { mode: "free" } })).toBe(true);
  });
  it("rejects malformed expressions", () => {
    for (const bad of ["", "a ==", "== 'x'", "a && ", "(a", "a b"]) {
      expect(parseVisibleWhen(bad), bad).toBeNull();
    }
  });
});

describe("values", () => {
  it("sets nested paths immutably, through arrays too", () => {
    const before = { a: { b: [1, { c: 2 }] } };
    const after = setPath(before, "a.b.1.c", 9) as typeof before;
    expect(after).toEqual({ a: { b: [1, { c: 9 }] } });
    expect(before.a.b[1]).toEqual({ c: 2 });
  });
  it("compares deeply", () => {
    expect(deepEqual({ a: [1, { b: 2 }] }, { a: [1, { b: 2 }] })).toBe(true);
    expect(deepEqual({ a: [1] }, { a: [2] })).toBe(false);
    expect(deepEqual([1], { 0: 1 })).toBe(false);
  });
  it("folds a multi-selection into one value plus the mixed paths", () => {
    const { value, mixed } = mergeSelection([
      { opacity: 1, position: { x: 0, y: 5 }, name: "a" },
      { opacity: 0.5, position: { x: 0, y: 7 }, name: "a" },
    ]);
    expect(mixed.sort()).toEqual(["opacity", "position.y"]);
    expect(value).toEqual({ opacity: 1, position: { x: 0, y: 5 }, name: "a" });
    expect(mergeSelection([{ a: 1 }]).mixed).toEqual([]);
  });
  it("humanizes keys", () => {
    expect(humanize("max_bitrate")).toBe("Max bitrate");
    expect(humanize("keyIntSeconds")).toBe("Key int seconds");
  });
});

describe("compileSchema", () => {
  it("orders by x-ui.order, then schema order, and drops hidden fields", () => {
    const nodes = compileSchema({
      type: "object",
      properties: {
        z: { type: "number", "x-ui": { order: 2 } },
        h: { type: "string", "x-ui": { widget: "hidden" } },
        a: { type: "number", "x-ui": { order: 1 } },
        m: { type: "number" },
      },
    });
    expect(nodes.map((n) => n.key)).toEqual(["m", "a", "z"]);
  });
  it("lets hints passed by path override the schema's x-ui", () => {
    const [node] = compileSchema(
      { type: "object", properties: { v: { type: "number", "x-ui": { unit: "px" } } } },
      { hints: { v: { unit: "%", widget: "angle" } } },
    );
    expect(node?.unit).toBe("%");
    expect(node?.widget).toBe("angle");
  });
  it("uses the title, then the humanized key, for the label", () => {
    const nodes = compileSchema({
      type: "object",
      properties: {
        fade_in: { type: "number" },
        t: { type: "number", title: "Time" },
        u: { type: "number", "x-ui": { label: "U!" } },
      },
    });
    expect(nodes.map((n) => n.label)).toEqual(["Fade in", "Time", "U!"]);
  });
  it("survives a recursive $ref without looping", () => {
    const nodes = compileSchema({
      type: "object",
      properties: { child: { $ref: "#/$defs/Tree" } },
      $defs: {
        Tree: {
          type: "object",
          properties: { child: { $ref: "#/$defs/Tree" }, n: { type: "number" } },
        },
      },
    });
    expect(nodes[0]?.widget).toBe("object");
  });
  it("renders a non-object root as one field", () => {
    const nodes = compileSchema({ type: "number" });
    expect(nodes).toHaveLength(1);
    expect(nodes[0]?.widget).toBe("number");
  });
});

/** A manual clock, so the throttle is tested without real time. */
function manualClock() {
  let now = 0;
  let id = 0;
  const timers = new Map<number, { at: number; fn: () => void }>();
  const clock: Clock = {
    now: () => now,
    setTimeout: (fn, ms) => {
      timers.set(++id, { at: now + ms, fn });
      return id;
    },
    clearTimeout: (h) => {
      timers.delete(h as number);
    },
  };
  const advance = (ms: number) => {
    const target = now + ms;
    for (;;) {
      const next = [...timers.entries()]
        .filter(([, t]) => t.at <= target)
        .sort((a, b) => a[1].at - b[1].at)[0];
      if (!next) break;
      timers.delete(next[0]);
      now = next[1].at;
      next[1].fn();
    }
    now = target;
  };
  return { clock, advance };
}

describe("GestureBus", () => {
  function setup() {
    const calls: { path: string; value: unknown; meta: ChangeMeta }[] = [];
    const t = manualClock();
    const bus = new GestureBus(
      () => (path, value, meta) => calls.push({ path, value, meta }),
      () => {},
      t.clock,
    );
    return { calls, bus, ...t };
  }

  it("a lone change plus commit is one one-shot call", () => {
    const { bus, calls } = setup();
    bus.change("a", 1);
    bus.commit("a", 1);
    expect(calls).toEqual([{ path: "a", value: 1, meta: {} }]);
  });

  it("a drag is start, throttled updates, and exactly one end", () => {
    const { bus, calls, advance } = setup();
    // 500 moves in one second (a high-rate mouse).
    for (let i = 1; i <= 500; i++) {
      bus.change("a", i / 500);
      advance(2);
    }
    bus.commit("a", 1);
    const starts = calls.filter((c) => c.meta.gesture === "start");
    const updates = calls.filter((c) => c.meta.gesture === "update");
    const ends = calls.filter((c) => c.meta.gesture === "end");
    expect(starts).toHaveLength(1);
    expect(ends).toHaveLength(1);
    expect(updates.length).toBeLessThanOrEqual(MAX_UPDATES_PER_SECOND);
    expect(updates.length).toBeGreaterThan(MAX_UPDATES_PER_SECOND / 2);
    expect(calls[0]?.meta.gesture).toBe("start");
    expect(calls.at(-1)).toEqual({ path: "a", value: 1, meta: { gesture: "end" } });
  });

  it("the trailing value is flushed while the pointer rests", () => {
    const { bus, calls, advance } = setup();
    bus.change("a", 1);
    bus.change("a", 2); // start
    bus.change("a", 3); // too soon: held
    expect(calls.map((c) => c.value)).toEqual([2]);
    advance(20);
    expect(calls.map((c) => c.value)).toEqual([2, 3]);
    expect(calls[1]?.meta.gesture).toBe("update");
  });

  it("cancel closes an open gesture at the original value", () => {
    const { bus, calls } = setup();
    bus.change("a", 5, 0);
    bus.change("a", 6, 0);
    bus.cancel("a");
    expect(calls.at(-1)).toEqual({ path: "a", value: 0, meta: { gesture: "end" } });
    // A cancel before anything was emitted says nothing at all.
    const quiet = setup();
    quiet.bus.change("a", 5, 0);
    quiet.bus.cancel("a");
    expect(quiet.calls).toEqual([]);
  });

  it("closes open gestures on dispose", () => {
    const { bus, calls } = setup();
    bus.change("a", 1);
    bus.change("a", 2);
    bus.dispose();
    expect(calls.at(-1)).toEqual({ path: "a", value: 2, meta: { gesture: "end" } });
  });

  it("keeps paths independent", () => {
    const { bus, calls } = setup();
    bus.change("a", 1);
    bus.change("b", 1);
    bus.commit("b", 1);
    bus.commit("a", 1);
    expect(calls.map((c) => c.path)).toEqual(["b", "a"]);
  });
});
