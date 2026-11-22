// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Spike S4b (WP-0G.01): scratch `@cutweave/compose` runtime, evaluated as an ES module inside the
// QuickJS sandbox (module name "@cutweave/compose"). Original implementation: the ideas (props
// schema builder, interpolation, damped springs, easing curves, seeded noise) are common to
// motion-graphics tooling; no code or API surface is copied from Remotion.
//
// Everything here is a pure function of its arguments. There is no clock and no I/O: `frame`,
// `fps`, `props` and `size` arrive from the host through `render(input)`.

const BRAND = Symbol.for("cutweave.composition");

/** Error with a stable `code` (and a JSON-Pointer `path` for props errors) for host diagnostics. */
export class CompositionError extends Error {
  constructor(code, message, path) {
    super(message);
    this.name = "CompositionError";
    this.code = code;
    if (path !== undefined) this.path = path;
  }
}

// ---------------------------------------------------------------------------------------------
// Props schema builder (`p`): a tiny zod-like builder that both validates props (applying
// defaults) and emits JSON Schema 2020-12 for the inspector and MCP tool parameters.
// ---------------------------------------------------------------------------------------------

const COLOR_RE = /^#(?:[0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/;
const COLOR_PATTERN = "^#(?:[0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$";

function describeValue(v) {
  if (v === null) return "null";
  if (Array.isArray(v)) return "array";
  return typeof v;
}

class PropType {
  constructor(kind, schema, extra) {
    this.kind = kind;
    this.schema = schema;
    this.extra = extra || {};
    this.isOptional = false;
    this.hasDefault = false;
    this.defaultValue = undefined;
  }

  _with(patch, flags) {
    const c = new PropType(this.kind, { ...this.schema, ...patch }, this.extra);
    c.isOptional = this.isOptional;
    c.hasDefault = this.hasDefault;
    c.defaultValue = this.defaultValue;
    if (flags) Object.assign(c, flags);
    return c;
  }

  /** Default value (validated eagerly); makes the prop optional for callers. */
  default(value) {
    const v = this.parse(value, "(default)");
    return this._with({ default: v }, { hasDefault: true, defaultValue: v });
  }
  optional() {
    return this._with({}, { isOptional: true });
  }
  /** Human label shown by the inspector (JSON Schema `title`). */
  label(text) {
    return this._with({ title: String(text) });
  }
  describe(text) {
    return this._with({ description: String(text) });
  }
  /** The inspector may keyframe this prop (host-side interpolation between frames). */
  animatable() {
    return this._with({ "x-cutweave-animatable": true });
  }
  min(n) {
    const key =
      this.kind === "string" ? "minLength" : this.kind === "array" ? "minItems" : "minimum";
    return this._with({ [key]: n });
  }
  max(n) {
    const key =
      this.kind === "string" ? "maxLength" : this.kind === "array" ? "maxItems" : "maximum";
    return this._with({ [key]: n });
  }
  /** Slider step for numbers (inspector hint). */
  step(n) {
    return this._with({ "x-cutweave-step": n, "x-cutweave-widget": "slider" });
  }
  multiline() {
    return this._with({ "x-cutweave-widget": "textarea" });
  }

  toJSONSchema() {
    const s = { ...this.schema };
    if (this.kind === "array") s.items = this.extra.item.toJSONSchema();
    if (this.kind === "object") {
      const properties = {};
      const required = [];
      for (const key of Object.keys(this.extra.shape)) {
        const t = this.extra.shape[key];
        properties[key] = t.toJSONSchema();
        if (!t.isOptional && !t.hasDefault) required.push(key);
      }
      s.properties = properties;
      if (required.length > 0) s.required = required;
      s.additionalProperties = false;
      s["x-cutweave-order"] = Object.keys(this.extra.shape);
    }
    return s;
  }

  /** Validates `value` (JSON data) and applies defaults. Throws CompositionError(CW_PROPS). */
  parse(value, path) {
    const fail = (msg) => {
      throw new CompositionError("CW_PROPS", msg, path === "" ? "/" : path);
    };
    const s = this.schema;
    switch (this.kind) {
      case "string":
      case "color":
        if (typeof value !== "string") fail(`expected ${this.kind}, got ${describeValue(value)}`);
        if (this.kind === "color" && !COLOR_RE.test(value))
          fail(`expected a hex colour like "#7c5cff", got "${value}"`);
        if (s.minLength !== undefined && value.length < s.minLength)
          fail(`must be at least ${s.minLength} characters`);
        if (s.maxLength !== undefined && value.length > s.maxLength)
          fail(`must be at most ${s.maxLength} characters`);
        return value;
      case "number":
      case "integer":
        if (typeof value !== "number" || !Number.isFinite(value))
          fail(`expected a finite number, got ${describeValue(value)}`);
        if (this.kind === "integer" && !Number.isInteger(value)) fail("expected an integer");
        if (s.minimum !== undefined && value < s.minimum) fail(`must be >= ${s.minimum}`);
        if (s.maximum !== undefined && value > s.maximum) fail(`must be <= ${s.maximum}`);
        return value;
      case "boolean":
        if (typeof value !== "boolean") fail(`expected boolean, got ${describeValue(value)}`);
        return value;
      case "enum":
        if (!s.enum.includes(value))
          fail(`expected one of ${s.enum.map((v) => JSON.stringify(v)).join(", ")}`);
        return value;
      case "array": {
        if (!Array.isArray(value)) fail(`expected array, got ${describeValue(value)}`);
        if (s.minItems !== undefined && value.length < s.minItems)
          fail(`must have at least ${s.minItems} items`);
        if (s.maxItems !== undefined && value.length > s.maxItems)
          fail(`must have at most ${s.maxItems} items`);
        return value.map((v, i) => this.extra.item.parse(v, `${path}/${i}`));
      }
      case "object": {
        if (value === null || typeof value !== "object" || Array.isArray(value))
          fail(`expected object, got ${describeValue(value)}`);
        const shape = this.extra.shape;
        for (const key of Object.keys(value)) {
          if (!Object.hasOwn(shape, key))
            throw new CompositionError("CW_PROPS", `unknown prop "${key}"`, `${path}/${key}`);
        }
        const out = {};
        for (const key of Object.keys(shape)) {
          const t = shape[key];
          const sub = `${path}/${key}`;
          if (value[key] === undefined) {
            if (t.hasDefault) out[key] = t.defaultValue;
            else if (!t.isOptional) throw new CompositionError("CW_PROPS", "required", sub);
          } else {
            out[key] = t.parse(value[key], sub);
          }
        }
        return out;
      }
      default:
        return fail(`unknown prop kind ${this.kind}`);
    }
  }
}

/** Props schema builder. Every method returns a new immutable PropType. */
export const p = Object.freeze({
  string: () => new PropType("string", { type: "string" }),
  number: () => new PropType("number", { type: "number" }),
  integer: () => new PropType("integer", { type: "integer" }),
  boolean: () => new PropType("boolean", { type: "boolean" }),
  color: () =>
    new PropType("color", {
      type: "string",
      format: "color",
      pattern: COLOR_PATTERN,
      "x-cutweave-widget": "color",
    }),
  enum: (values) => {
    if (!Array.isArray(values) || values.length === 0)
      throw new CompositionError("CW_DEFINE", "p.enum() needs a non-empty array of values");
    return new PropType("enum", { enum: values.slice() });
  },
  array: (item) => new PropType("array", { type: "array" }, { item }),
  object: (shape) => {
    for (const key of Object.keys(shape)) {
      if (!(shape[key] instanceof PropType))
        throw new CompositionError("CW_DEFINE", `p.object(): "${key}" is not a p.* type`);
    }
    return new PropType("object", { type: "object" }, { shape: { ...shape } });
  },
});

// ---------------------------------------------------------------------------------------------
// defineComposition
// ---------------------------------------------------------------------------------------------

const ID_RE = /^[a-z0-9]+(?:[.-][a-z0-9]+)*$/;

/**
 * Declares a composition. `render(input)` must be a pure function of
 * `{ frame, fps, props, size: { w, h }, durationInFrames }` returning nodes.
 */
export function defineComposition(def) {
  const bad = (msg) => {
    throw new CompositionError("CW_DEFINE", `defineComposition: ${msg}`);
  };
  if (def === null || typeof def !== "object") bad("expected an object");
  if (typeof def.id !== "string" || !ID_RE.test(def.id))
    bad('`id` must be a lowercase dotted id like "acme.lower-third"');
  const props = def.props === undefined ? p.object({}) : def.props;
  if (!(props instanceof PropType) || props.kind !== "object")
    bad("`props` must be p.object({...})");
  if (!Number.isInteger(def.durationInFrames) || def.durationInFrames <= 0)
    bad("`durationInFrames` must be a positive integer");
  if (def.fps !== undefined && !(typeof def.fps === "number" && def.fps > 0))
    bad("`fps` must be a positive number");
  if (typeof def.render !== "function") bad("`render` must be a function");
  return Object.freeze({
    [BRAND]: true,
    id: def.id,
    title: typeof def.title === "string" ? def.title : undefined,
    props,
    durationInFrames: def.durationInFrames,
    fps: def.fps,
    render: def.render,
  });
}

// ---------------------------------------------------------------------------------------------
// Animation helpers
// ---------------------------------------------------------------------------------------------

/**
 * Piecewise-linear map of `input` through `inputRange` → `outputRange` (same length ≥ 2,
 * strictly increasing input). `clamp`: true | "start" | "end" | false (extrapolate). `easing`
 * shapes each segment.
 */
export function interpolate(input, inputRange, outputRange, options) {
  const n = Array.isArray(inputRange) ? inputRange.length : 0;
  if (n < 2 || !Array.isArray(outputRange) || outputRange.length !== n)
    throw new CompositionError(
      "CW_INTERPOLATE",
      "interpolate(): inputRange and outputRange must be arrays of equal length >= 2",
    );
  for (let i = 1; i < n; i++) {
    if (!(inputRange[i] > inputRange[i - 1]))
      throw new CompositionError("CW_INTERPOLATE", "interpolate(): inputRange must increase");
  }
  const clamp = options === undefined ? false : options.clamp;
  const ease = options === undefined ? undefined : options.easing;
  if (input <= inputRange[0] && (clamp === true || clamp === "start")) return outputRange[0];
  if (input >= inputRange[n - 1] && (clamp === true || clamp === "end"))
    return outputRange[n - 1];
  let i = 1;
  while (i < n - 1 && input > inputRange[i]) i++;
  const a = inputRange[i - 1];
  let t = (input - a) / (inputRange[i] - a);
  if (ease !== undefined && t >= 0 && t <= 1) t = ease(t);
  return outputRange[i - 1] + (outputRange[i] - outputRange[i - 1]) * t;
}

/**
 * Closed-form damped harmonic oscillator, evaluated at `(frame - delay) / fps` seconds: O(1) per
 * call, so random access to any frame costs the same (no integration from frame 0).
 * `velocity` is in progress units per second (1 = the whole from→to distance per second).
 */
export function spring(options) {
  const {
    frame,
    fps,
    from = 0,
    to = 1,
    stiffness = 100,
    damping = 12,
    mass = 1,
    velocity = 0,
    delay = 0,
    clamp = false,
  } = options;
  if (!(fps > 0) || !(stiffness > 0) || !(mass > 0) || !(damping >= 0))
    throw new CompositionError(
      "CW_SPRING",
      "spring(): fps, stiffness, mass must be > 0 and damping >= 0",
    );
  const t = Math.max(0, frame - delay) / fps;
  const w0 = Math.sqrt(stiffness / mass);
  const zeta = damping / (2 * Math.sqrt(stiffness * mass));
  const v0 = -velocity; // x(t) = remaining fraction of the distance; x(0) = 1
  let x;
  if (zeta < 1) {
    const wd = w0 * Math.sqrt(1 - zeta * zeta);
    x = Math.exp(-zeta * w0 * t) * (Math.cos(wd * t) + ((v0 + zeta * w0) / wd) * Math.sin(wd * t));
  } else if (zeta === 1) {
    x = Math.exp(-w0 * t) * (1 + (v0 + w0) * t);
  } else {
    const s = Math.sqrt(zeta * zeta - 1);
    const r1 = -w0 * (zeta - s);
    const r2 = -w0 * (zeta + s);
    const c2 = (v0 - r1) / (r2 - r1);
    x = (1 - c2) * Math.exp(r1 * t) + c2 * Math.exp(r2 * t);
  }
  let progress = 1 - x;
  if (clamp) progress = Math.min(progress, 1);
  return from + (to - from) * progress;
}

function cubicBezier(x1, y1, x2, y2) {
  const ax = 3 * x1 - 3 * x2 + 1;
  const bx = 3 * x2 - 6 * x1;
  const cx = 3 * x1;
  const ay = 3 * y1 - 3 * y2 + 1;
  const by = 3 * y2 - 6 * y1;
  const cy = 3 * y1;
  const sampleX = (t) => ((ax * t + bx) * t + cx) * t;
  const slopeX = (t) => (3 * ax * t + 2 * bx) * t + cx;
  return (x) => {
    if (x <= 0) return 0;
    if (x >= 1) return 1;
    let t = x;
    for (let i = 0; i < 6; i++) {
      const err = sampleX(t) - x;
      const d = slopeX(t);
      if (Math.abs(err) < 1e-7 || Math.abs(d) < 1e-7) break;
      t -= err / d;
    }
    if (t < 0 || t > 1 || Math.abs(sampleX(t) - x) > 1e-5) {
      let lo = 0;
      let hi = 1;
      t = x;
      for (let i = 0; i < 40; i++) {
        if (sampleX(t) < x) lo = t;
        else hi = t;
        t = (lo + hi) / 2;
      }
    }
    return ((ay * t + by) * t + cy) * t;
  };
}

const outOf = (f) => (t) => 1 - f(1 - t);
const inOutOf = (f) => (t) => (t < 0.5 ? f(2 * t) / 2 : 1 - f(2 - 2 * t) / 2);
const quad = (t) => t * t;
const cubic = (t) => t * t * t;
const sine = (t) => 1 - Math.cos((t * Math.PI) / 2);
const expo = (t) => (t === 0 ? 0 : 2 ** (10 * t - 10));

/** Easing curves (t ∈ [0,1] → [0,1]) for `interpolate(..., { easing })`. */
export const easing = Object.freeze({
  linear: (t) => t,
  inQuad: quad,
  outQuad: outOf(quad),
  inOutQuad: inOutOf(quad),
  inCubic: cubic,
  outCubic: outOf(cubic),
  inOutCubic: inOutOf(cubic),
  inSine: sine,
  outSine: outOf(sine),
  inOutSine: inOutOf(sine),
  inExpo: expo,
  outExpo: outOf(expo),
  inOutExpo: inOutOf(expo),
  outBack: (t) => {
    const s = 1.70158;
    const u = t - 1;
    return 1 + (s + 1) * u * u * u + s * u * u;
  },
  bezier: cubicBezier,
});

/** 32-bit FNV-1a of `String(seed)`. */
export function hashSeed(seed) {
  const s = String(seed);
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return h >>> 0;
}

/** One mulberry32 step of a 32-bit state → [0, 1). */
export function mix32(state) {
  let t = (state + 0x6d2b79f5) | 0;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}

/** Stateless deterministic random in [0, 1) for a seed (string or number). */
export function random(seed) {
  return mix32(hashSeed(seed));
}

// Parsed colours are memoised (a pure cache: it cannot change any result).
const hexCache = new Map();
const HEX2 = Array.from({ length: 256 }, (_, i) => i.toString(16).padStart(2, "0"));

function parseHex(c) {
  let v = hexCache.get(c);
  if (v !== undefined) return v;
  if (typeof c !== "string" || !COLOR_RE.test(c))
    throw new CompositionError("CW_COLOR", `expected a hex colour, got ${String(c)}`);
  let h = c.slice(1);
  if (h.length <= 4) h = [...h].map((d) => d + d).join("");
  if (h.length === 6) h += "ff";
  v = [0, 2, 4, 6].map((i) => Number.parseInt(h.slice(i, i + 2), 16));
  if (hexCache.size < 4096) hexCache.set(c, v);
  return v;
}

/** Mixes two hex colours in sRGB (t = 0 → a, 1 → b); returns "#rrggbbaa". */
export function mixColor(a, b, t) {
  const ca = parseHex(a);
  const cb = parseHex(b);
  const u = t < 0 ? 0 : t > 1 ? 1 : t;
  return (
    "#" +
    HEX2[Math.round(ca[0] + (cb[0] - ca[0]) * u)] +
    HEX2[Math.round(ca[1] + (cb[1] - ca[1]) * u)] +
    HEX2[Math.round(ca[2] + (cb[2] - ca[2]) * u)] +
    HEX2[Math.round(ca[3] + (cb[3] - ca[3]) * u)]
  );
}

// ---------------------------------------------------------------------------------------------
// Scene nodes and the JSX factory (automatic runtime: `@cutweave/compose/jsx-runtime`).
// Nodes are plain JSON-able objects `{ type, ...props }`; groups carry `children`.
// ---------------------------------------------------------------------------------------------

export const Group = "group";
export const Rect = "rect";
export const Ellipse = "ellipse";
export const Text = "text";
export const Image = "image";
export const Fragment = Symbol.for("cutweave.fragment");

const NODE_TYPES = new Set([Group, Rect, Ellipse, Text, Image]);

function flattenInto(out, child) {
  if (child === null || child === undefined || child === false || child === true) return;
  if (Array.isArray(child)) {
    for (const c of child) flattenInto(out, c);
    return;
  }
  if (typeof child === "object" && NODE_TYPES.has(child.type)) {
    out.push(child);
    return;
  }
  throw new CompositionError(
    "CW_CHILD",
    `only scene nodes can be children of <Group> (wrap text in <Text>), got ${describeValue(child)}`,
  );
}

function textOf(children) {
  if (children === null || children === undefined || typeof children === "boolean") return "";
  if (typeof children === "string") return children;
  if (typeof children === "number") return String(children);
  if (Array.isArray(children)) return children.map(textOf).join("");
  throw new CompositionError("CW_CHILD", "<Text> children must be strings or numbers");
}

/** JSX factory. Function components are called immediately (no hooks, no state). */
export function jsx(type, props, key) {
  if (typeof type === "function") return type(props);
  if (type === Fragment) {
    const out = [];
    flattenInto(out, props.children);
    return out;
  }
  if (!NODE_TYPES.has(type))
    throw new CompositionError("CW_ELEMENT", `unknown element <${String(type)}>`);
  // One native copy (Object.assign) instead of a per-key loop: this is the per-node hot path.
  const node = Object.assign({ type }, props);
  if (node.key !== undefined) {
    if (key === undefined) key = node.key;
    delete node.key;
  }
  if (key !== undefined && key !== null && node.id === undefined) node.id = String(key);
  if (typeof node.scale === "number") node.scale = [node.scale, node.scale];
  if (typeof node.anchor === "number") node.anchor = [node.anchor, node.anchor];
  if (type === Group) {
    const children = [];
    flattenInto(children, props.children);
    node.children = children;
  } else if (props.children !== undefined) {
    if (type === Text) node.text = textOf(props.children);
    delete node.children;
  }
  return node;
}

export { jsx as jsxs };

/**
 * Classic-runtime factory `(type, props, ...children)`; oxc's automatic runtime falls back to it
 * for `<X {...spread} key={k} />`, and hand-written (JSX-free) compositions can use it as `h`.
 */
export function createElement(type, props, ...children) {
  const p2 = props === null || props === undefined ? {} : { ...props };
  if (children.length > 0) p2.children = children.length === 1 ? children[0] : children;
  const key = p2.key;
  delete p2.key;
  return jsx(type, p2, key);
}

export { createElement as h };

/** Host-internal accessors (not part of the authoring API). */
export const __internal = Object.freeze({ BRAND, NODE_TYPES, flattenInto });
