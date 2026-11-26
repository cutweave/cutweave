// SPDX-License-Identifier: GPL-3.0-or-later
// The `when` language of menus (ADR-0026, WP-2H.12): a small boolean expression over the context
// keys below. The command palette (WP-2H.04) reads the same clauses and plugins write them in
// their manifest (WP-6B.03), so the key list is a published contract: a key is added freely and
// renamed or removed only by ADR.
//
//   expr    := or
//   or      := and ( "||" and )*
//   and     := not ( "&&" not )*
//   not     := "!" not | compare
//   compare := atom ( ("==" | "!=" | "<" | "<=" | ">" | ">=") atom )?
//   atom    := "(" expr ")" | number | "true" | "false" | 'text' | key
//
// Precedence, tightest first: `!`, comparison, `&&`, `||`. A bare word on the right of a
// comparison that is not a key is text (`selection.kind == video`). A key that is not in the
// list is false, and it is logged once. A clause that does not parse is false and logged.

/** The context keys a `when` clause may read (ADR-0026). */
export const CONTEXT_KEYS = [
  "selection.count",
  /** `video`, `audio`, `image`, `text`, `compound` or `mixed`. */
  "selection.kind",
  /** How many of the selected clips or assets are pictures (video clips, videos, images). */
  "selection.videoCount",
  "item.linked",
  "item.hasAudio",
  "item.enabled",
  /** Every target clip plays moving pictures: video, not a still or a freeze frame (WP-2D.01). */
  "item.movingPicture",
  "track.locked",
  "playhead.inItem",
  "clipboard.hasItems",
  "media.used",
  "media.offline",
  "engine.live",
  "project.readOnly",
  /** The panel is in a window of its own (a detached panel, WP-2H.15). */
  "panel.detached",
] as const;

export type ContextKey = (typeof CONTEXT_KEYS)[number];

export type ContextValue = string | number | boolean | undefined;

/** The value of each key at the moment a menu opens. */
export type ContextValues = Partial<Record<ContextKey, ContextValue>>;

const KNOWN: ReadonlySet<string> = new Set(CONTEXT_KEYS);

export function isContextKey(name: string): name is ContextKey {
  return KNOWN.has(name);
}

type Logger = (message: string) => void;
let logger: Logger = (message) => console.warn(message);
const logged = new Set<string>();

/** Where unknown keys and bad clauses are reported (tests collect them). */
export function setWhenLogger(next: Logger | null): void {
  logger = next ?? ((message) => console.warn(message));
  logged.clear();
}

function report(message: string): void {
  if (logged.has(message)) return;
  logged.add(message);
  logger(message);
}

// ---- parsing ------------------------------------------------------------------------------------

type Node =
  | { readonly t: "or" | "and"; readonly l: Node; readonly r: Node }
  | { readonly t: "not"; readonly e: Node }
  | { readonly t: "cmp"; readonly op: string; readonly l: Node; readonly r: Node }
  | { readonly t: "lit"; readonly v: string | number | boolean }
  | { readonly t: "key"; readonly name: string };

type Token = { readonly k: "op" | "num" | "str" | "word"; readonly v: string };

const TOKEN =
  /\s*(?:(&&|\|\||==|!=|<=|>=|<|>|!|\(|\))|(-?\d+(?:\.\d+)?)|'([^']*)'|([A-Za-z_][A-Za-z0-9_.]*))/y;

function tokenize(source: string): Token[] {
  const out: Token[] = [];
  TOKEN.lastIndex = 0;
  while (TOKEN.lastIndex < source.length) {
    const at = TOKEN.lastIndex;
    const m = TOKEN.exec(source);
    if (m === null) {
      if (source.slice(at).trim() === "") break;
      throw new Error(`unexpected "${source.slice(at).trim().slice(0, 12)}"`);
    }
    if (m[1] !== undefined) out.push({ k: "op", v: m[1] });
    else if (m[2] !== undefined) out.push({ k: "num", v: m[2] });
    else if (m[3] !== undefined) out.push({ k: "str", v: m[3] });
    else out.push({ k: "word", v: m[4] ?? "" });
  }
  return out;
}

const COMPARE = new Set(["==", "!=", "<", "<=", ">", ">="]);

function parse(source: string): Node {
  const tokens = tokenize(source);
  let i = 0;
  const peek = (): Token | undefined => tokens[i];
  const isOp = (v: string): boolean => peek()?.k === "op" && peek()?.v === v;

  const orExpr = (): Node => {
    let left = andExpr();
    while (isOp("||")) {
      i++;
      left = { t: "or", l: left, r: andExpr() };
    }
    return left;
  };
  const andExpr = (): Node => {
    let left = notExpr();
    while (isOp("&&")) {
      i++;
      left = { t: "and", l: left, r: notExpr() };
    }
    return left;
  };
  const notExpr = (): Node => {
    if (isOp("!")) {
      i++;
      return { t: "not", e: notExpr() };
    }
    return compare();
  };
  const compare = (): Node => {
    const left = atom(false);
    const next = peek();
    if (next?.k === "op" && COMPARE.has(next.v)) {
      i++;
      return { t: "cmp", op: next.v, l: left, r: atom(true) };
    }
    return left;
  };
  const atom = (rhs: boolean): Node => {
    const token = tokens[i++];
    if (token === undefined) throw new Error("unexpected end");
    if (token.k === "op") {
      if (token.v !== "(") throw new Error(`unexpected "${token.v}"`);
      const inner = orExpr();
      if (!isOp(")")) throw new Error('missing ")"');
      i++;
      return inner;
    }
    if (token.k === "num") return { t: "lit", v: Number(token.v) };
    if (token.k === "str") return { t: "lit", v: token.v };
    if (token.v === "true") return { t: "lit", v: true };
    if (token.v === "false") return { t: "lit", v: false };
    // A word after a comparison operator is text unless it names a key.
    if (rhs && !isContextKey(token.v)) return { t: "lit", v: token.v };
    return { t: "key", name: token.v };
  };

  const tree = orExpr();
  if (i < tokens.length) throw new Error(`unexpected "${tokens[i]?.v}"`);
  return tree;
}

// ---- evaluation ---------------------------------------------------------------------------------

const UNKNOWN = Symbol("unknown key");

function value(node: Node, ctx: ContextValues, source: string): ContextValue | typeof UNKNOWN {
  switch (node.t) {
    case "lit":
      return node.v;
    case "key": {
      if (!isContextKey(node.name)) {
        report(`when: unknown context key "${node.name}" in "${source}" (treated as false)`);
        return UNKNOWN;
      }
      return ctx[node.name];
    }
    default:
      return test(node, ctx, source);
  }
}

function test(node: Node, ctx: ContextValues, source: string): boolean {
  switch (node.t) {
    case "or":
      return test(node.l, ctx, source) || test(node.r, ctx, source);
    case "and":
      return test(node.l, ctx, source) && test(node.r, ctx, source);
    case "not":
      return !test(node.e, ctx, source);
    case "cmp": {
      const l = value(node.l, ctx, source);
      const r = value(node.r, ctx, source);
      if (l === UNKNOWN || r === UNKNOWN) return false;
      if (l === undefined || r === undefined) return node.op === "!=" ? l !== r : false;
      switch (node.op) {
        case "==":
          return l === r;
        case "!=":
          return l !== r;
        default: {
          if (typeof l !== "number" || typeof r !== "number") return false;
          if (node.op === "<") return l < r;
          if (node.op === "<=") return l <= r;
          if (node.op === ">") return l > r;
          return l >= r;
        }
      }
    }
    default: {
      const v = value(node, ctx, source);
      return (
        v === true || (typeof v === "number" && v !== 0) || (typeof v === "string" && v !== "")
      );
    }
  }
}

const cache = new Map<string, Node | null>();

/** Whether `clause` holds in `ctx`. No clause (or a blank one) holds. */
export function evaluateWhen(clause: string | undefined, ctx: ContextValues): boolean {
  if (clause === undefined || clause.trim() === "") return true;
  let tree = cache.get(clause);
  if (tree === undefined) {
    try {
      tree = parse(clause);
    } catch (error) {
      report(`when: cannot read "${clause}": ${(error as Error).message} (treated as false)`);
      tree = null;
    }
    cache.set(clause, tree);
  }
  return tree === null ? false : test(tree, ctx, clause);
}
