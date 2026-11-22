// SPDX-License-Identifier: GPL-3.0-or-later
//
// Spike S4b (WP-0G.01): host-internal module ("cutweave:host"). Evaluated once per runtime BEFORE
// any composition code. It (1) hardens the realm (no code generation from strings, no clock, no
// unseeded RNG, no blocking primitives, capped console) and (2) exposes `bind`/`describe`, which
// the Rust host calls. Compositions cannot import it (the transpiler rejects every import except
// `@cutweave/compose` and its jsx-runtime).

import { CompositionError, __internal, hashSeed } from "@cutweave/compose";

const { BRAND, flattenInto } = __internal;
const g = globalThis;

// ---- Frame-scoped deterministic state ------------------------------------------------------

const clock = { ms: 0 };
let rngState = 0;
const logs = [];
let logLimit = 50;
let droppedLogs = 0;

function lock(obj, name, value) {
  Object.defineProperty(obj, name, {
    value,
    writable: false,
    enumerable: false,
    configurable: false,
  });
}

// ---- 1. No code generation from strings ------------------------------------------------------
// `eval` is removed; every route to the Function constructors (global, and `.constructor` of
// plain/generator/async/async-generator function prototypes) throws.

function blocked(what) {
  return function blockedCodeGeneration() {
    throw new EvalError(
      `${what} is disabled in compositions (code generation from strings is not allowed)`,
    );
  };
}

delete g.eval;
const FunctionStub = blocked("the Function constructor");
FunctionStub.prototype = Function.prototype;
for (const proto of [
  Function.prototype,
  Object.getPrototypeOf(function* () {}),
  Object.getPrototypeOf(async () => {}),
  Object.getPrototypeOf(async function* () {}),
]) {
  lock(proto, "constructor", FunctionStub);
}
lock(g, "Function", FunctionStub);

// ---- 2. No wall clock: time is the composition time of the frame being rendered -------------

function DateStub() {
  throw new TypeError(
    "Date is not available in compositions: derive time from `frame` and `fps` " +
      "(Date.now() returns the composition time in ms)",
  );
}
DateStub.now = () => clock.ms;
lock(g, "Date", Object.freeze(DateStub));
lock(g, "performance", Object.freeze({ now: () => clock.ms }));

// ---- 3. Seeded RNG: Math.random is mulberry32, reseeded per (item seed, frame) --------------
// Reseeding per frame makes every frame independent of evaluation order (scrubbing, parallel
// export, cache misses all see the same numbers).

lock(Math, "random", function random() {
  rngState = (rngState + 0x6d2b79f5) | 0;
  let t = Math.imul(rngState ^ (rngState >>> 15), rngState | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
});

// ---- 4. No blocking or GC-observable primitives ----------------------------------------------

for (const name of [
  "Atomics",
  "SharedArrayBuffer",
  "WeakRef",
  "FinalizationRegistry",
  "queueMicrotask",
]) {
  delete g[name];
}

// ---- 5. Console: captured per frame (capped), returned to the host with the scene -----------

function fmt(v) {
  if (typeof v === "string") return v;
  try {
    const s = JSON.stringify(v);
    return s === undefined ? String(v) : s;
  } catch {
    return String(v);
  }
}
function makeLog(level) {
  return (...args) => {
    if (logs.length >= logLimit) {
      droppedLogs++;
      return;
    }
    let message = args.map(fmt).join(" ");
    if (message.length > 1000) message = `${message.slice(0, 1000)}…`;
    logs.push({ level, message });
  };
}
lock(
  g,
  "console",
  Object.freeze({
    log: makeLog("log"),
    info: makeLog("info"),
    warn: makeLog("warn"),
    error: makeLog("error"),
    debug: makeLog("debug"),
  }),
);

// ---- Host API ----------------------------------------------------------------------------------

function deepFreeze(v) {
  if (v !== null && typeof v === "object" && !Object.isFrozen(v)) {
    Object.freeze(v);
    for (const k of Object.keys(v)) deepFreeze(v[k]);
  }
  return v;
}

function checkDefinition(def) {
  if (def === null || typeof def !== "object" || def[BRAND] !== true)
    throw new CompositionError(
      "CW_EXPORT",
      "the default export must be the result of defineComposition({...})",
    );
}

/** Metadata + props JSON Schema of a composition, as JSON. */
export function describe(def) {
  checkDefinition(def);
  return JSON.stringify({
    id: def.id,
    title: def.title ?? null,
    durationInFrames: def.durationInFrames,
    fps: def.fps ?? null,
    propsSchema: {
      $schema: "https://json-schema.org/draft/2020-12/schema",
      ...def.props.toJSONSchema(),
    },
  });
}

/**
 * Validates props (applying defaults) and returns `renderFrame(frame) → JSON string` for one
 * timeline item. Called once per item; `renderFrame` runs per frame.
 */
export function bind(def, propsJson, optionsJson) {
  checkDefinition(def);
  const { width, height, fps, seed, maxNodes, maxLogs } = JSON.parse(optionsJson);
  const props = deepFreeze(def.props.parse(JSON.parse(propsJson), ""));
  const size = Object.freeze({ w: width, h: height });
  const render = def.render;
  const durationInFrames = def.durationInFrames;
  logLimit = maxLogs;
  // `asJson`: return the scene as a JSON string (canonical bytes for hashing/debugging) or as the
  // plain object, which the host reads property by property (no serialisation round trip).
  return function renderFrame(frame, asJson) {
    clock.ms = (frame * 1000) / fps;
    rngState = hashSeed(`${seed}:${frame}`) | 0;
    logs.length = 0;
    droppedLogs = 0;
    const out = render(Object.freeze({ frame, fps, props, size, durationInFrames }));
    if (out !== null && typeof out === "object" && typeof out.then === "function")
      throw new CompositionError(
        "CW_ASYNC",
        "render() must return scene nodes synchronously, not a Promise",
      );
    const nodes = [];
    flattenInto(nodes, out);
    let count = 0;
    const stack = nodes.slice();
    while (stack.length > 0) {
      const n = stack.pop();
      if (++count > maxNodes)
        throw new CompositionError(
          "CW_SCENE_TOO_LARGE",
          `render() returned more than ${maxNodes} nodes`,
        );
      if (n !== null && typeof n === "object" && Array.isArray(n.children))
        for (const c of n.children) stack.push(c);
    }
    const result = { nodes, logs, droppedLogs };
    return asJson ? JSON.stringify(result) : result;
  };
}
