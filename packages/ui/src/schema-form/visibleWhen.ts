// SPDX-License-Identifier: GPL-3.0-or-later
/**
 * `x-ui.visibleWhen`: a tiny boolean expression over sibling values (SCHEMA_HINTS.md).
 *
 *   expr  := and ('||' and)*
 *   and   := not ('&&' not)*
 *   not   := '!' not | '(' expr ')' | atom
 *   atom  := path [('==' | '!=') literal]
 *   literal := 'text' | "text" | number | true | false | null
 *
 * A bare `path` is true when the value is truthy. Paths are dot paths resolved from the object
 * that holds the field, then from the form root.
 */
export type VisibleExpr =
  | { op: "or" | "and"; left: VisibleExpr; right: VisibleExpr }
  | { op: "not"; expr: VisibleExpr }
  | { op: "truthy"; path: string }
  | { op: "eq" | "ne"; path: string; literal: unknown };

type Token =
  | { t: "path"; v: string }
  | { t: "lit"; v: unknown }
  | { t: "op"; v: "&&" | "||" | "!" | "(" | ")" | "==" | "!=" };

const TOKEN =
  /\s*(?:(&&|\|\||==|!=|!|\(|\))|'([^']*)'|"([^"]*)"|(-?\d+(?:\.\d+)?)(?![\w.])|([A-Za-z_$][\w$.]*))/y;

function tokenize(source: string): Token[] | null {
  const out: Token[] = [];
  TOKEN.lastIndex = 0;
  let pos = 0;
  while (pos < source.length) {
    if (/^\s*$/.test(source.slice(pos))) break;
    TOKEN.lastIndex = pos;
    const m = TOKEN.exec(source);
    if (!m) return null;
    pos = TOKEN.lastIndex;
    if (m[1] !== undefined) out.push({ t: "op", v: m[1] as Extract<Token, { t: "op" }>["v"] });
    else if (m[2] !== undefined) out.push({ t: "lit", v: m[2] });
    else if (m[3] !== undefined) out.push({ t: "lit", v: m[3] });
    else if (m[4] !== undefined) out.push({ t: "lit", v: Number(m[4]) });
    else if (m[5] !== undefined) {
      if (m[5] === "true") out.push({ t: "lit", v: true });
      else if (m[5] === "false") out.push({ t: "lit", v: false });
      else if (m[5] === "null") out.push({ t: "lit", v: null });
      else out.push({ t: "path", v: m[5] });
    }
  }
  return out;
}

/** Returns `null` for a malformed expression (the field then stays visible). */
export function parseVisibleWhen(source: string): VisibleExpr | null {
  const found = tokenize(source);
  if (!found || found.length === 0) return null;
  const tokens: readonly Token[] = found;
  let i = 0;
  const isOp = (v: string) => {
    const tok = tokens[i];
    return tok?.t === "op" && tok.v === v;
  };

  function parseOr(): VisibleExpr | null {
    let left = parseAnd();
    while (left && isOp("||")) {
      i++;
      const right = parseAnd();
      if (!right) return null;
      left = { op: "or", left, right };
    }
    return left;
  }
  function parseAnd(): VisibleExpr | null {
    let left = parseNot();
    while (left && isOp("&&")) {
      i++;
      const right = parseNot();
      if (!right) return null;
      left = { op: "and", left, right };
    }
    return left;
  }
  function parseNot(): VisibleExpr | null {
    if (isOp("!")) {
      i++;
      const expr = parseNot();
      return expr ? { op: "not", expr } : null;
    }
    if (isOp("(")) {
      i++;
      const expr = parseOr();
      if (!expr || !isOp(")")) return null;
      i++;
      return expr;
    }
    const tok = tokens[i];
    if (tok?.t !== "path") return null;
    i++;
    if (isOp("==") || isOp("!=")) {
      const eq = isOp("==");
      i++;
      const lit = tokens[i];
      if (lit?.t !== "lit") return null;
      i++;
      return { op: eq ? "eq" : "ne", path: tok.v, literal: lit.v };
    }
    return { op: "truthy", path: tok.v };
  }

  const expr = parseOr();
  return expr && i === tokens.length ? expr : null;
}

export function evalVisible(expr: VisibleExpr, lookup: (path: string) => unknown): boolean {
  switch (expr.op) {
    case "or":
      return evalVisible(expr.left, lookup) || evalVisible(expr.right, lookup);
    case "and":
      return evalVisible(expr.left, lookup) && evalVisible(expr.right, lookup);
    case "not":
      return !evalVisible(expr.expr, lookup);
    case "truthy":
      return Boolean(lookup(expr.path));
    case "eq":
      return lookup(expr.path) === expr.literal;
    case "ne":
      return lookup(expr.path) !== expr.literal;
  }
}
