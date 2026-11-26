// SPDX-License-Identifier: GPL-3.0-or-later
// The shared math module's ABI as its binary declares it (WP-2C.06): every exported function with
// its WebAssembly signature, read from the type, import, function and export sections. The loader
// checks a module against the generated table before it uses it, and `just wasm` renders the
// generated typings (`generated/exports.ts`) with the same reader, so a Rust signature that
// changes without regenerating them fails the typings gate and the loader.
//
// Plain erasable TypeScript: `wasm/build.ts` runs it with Node directly.

/** A WebAssembly value type. */
export type ValType = "i32" | "i64" | "f32" | "f64" | "v128" | "funcref" | "externref";

/** One function type. */
export interface FuncSig {
  readonly params: readonly ValType[];
  readonly results: readonly ValType[];
}

/** What a module exports and imports, as far as the loader cares. */
export interface ModuleAbi {
  /** Exported functions by name. */
  readonly functions: Readonly<Record<string, FuncSig>>;
  /** Names of the exported memories. */
  readonly memories: readonly string[];
  /** How many imports the module needs (the math module needs none). */
  readonly imports: number;
}

const VAL_TYPES: Readonly<Record<number, ValType>> = {
  127: "i32",
  126: "i64",
  125: "f32",
  124: "f64",
  123: "v128",
  112: "funcref",
  111: "externref",
};

class Reader {
  at = 0;
  readonly bytes: Uint8Array;
  readonly end: number;

  constructor(bytes: Uint8Array, start = 0, end = bytes.length) {
    this.bytes = bytes;
    this.at = start;
    this.end = end;
  }

  byte(): number {
    if (this.at >= this.end) throw new Error("wasm: unexpected end of section");
    return this.bytes[this.at++] as number;
  }

  /** An unsigned LEB128 number of at most 32 bits. */
  u32(): number {
    let value = 0;
    for (let shift = 0; shift < 35; shift += 7) {
      const b = this.byte();
      value += (b & 0x7f) * 2 ** shift;
      if ((b & 0x80) === 0) return value;
    }
    throw new Error("wasm: LEB128 number too long");
  }

  name(): string {
    const len = this.u32();
    if (this.at + len > this.end) throw new Error("wasm: name runs past its section");
    const text = new TextDecoder("utf-8", { fatal: true }).decode(
      this.bytes.subarray(this.at, this.at + len),
    );
    this.at += len;
    return text;
  }

  valType(): ValType {
    const b = this.byte();
    const t = VAL_TYPES[b];
    if (t === undefined) throw new Error(`wasm: unknown value type 0x${b.toString(16)}`);
    return t;
  }

  limits(): void {
    const flags = this.byte();
    this.u32();
    if (flags & 1) this.u32();
  }
}

/** Reads a module's exported functions and their signatures (throws on a malformed binary). */
export function readAbi(bytes: Uint8Array): ModuleAbi {
  const magic = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
  if (bytes.length < 8 || magic.some((b, i) => bytes[i] !== b)) {
    throw new Error("not a WebAssembly 1.0 module");
  }
  const types: FuncSig[] = [];
  const funcTypes: number[] = []; // type index of every function, imported ones first
  const exports: { name: string; kind: number; index: number }[] = [];
  let imports = 0;
  const top = new Reader(bytes, 8);
  while (top.at < top.end) {
    const id = top.byte();
    const size = top.u32();
    const r = new Reader(bytes, top.at, top.at + size);
    if (r.end > bytes.length) throw new Error("wasm: section runs past the end");
    top.at = r.end;
    if (id === 1) {
      for (let n = r.u32(); n > 0; n--) {
        const form = r.byte();
        if (form !== 0x60) throw new Error(`wasm: unsupported type form 0x${form.toString(16)}`);
        const params = Array.from({ length: r.u32() }, () => r.valType());
        const results = Array.from({ length: r.u32() }, () => r.valType());
        types.push({ params, results });
      }
    } else if (id === 2) {
      for (let n = r.u32(); n > 0; n--) {
        r.name();
        r.name();
        imports++;
        const kind = r.byte();
        if (kind === 0) funcTypes.push(r.u32());
        else if (kind === 1) {
          r.byte();
          r.limits();
        } else if (kind === 2) r.limits();
        else if (kind === 3) {
          r.valType();
          r.byte();
        } else if (kind === 4) {
          r.byte();
          r.u32();
        } else throw new Error(`wasm: unknown import kind ${kind}`);
      }
    } else if (id === 3) {
      for (let n = r.u32(); n > 0; n--) funcTypes.push(r.u32());
    } else if (id === 7) {
      for (let n = r.u32(); n > 0; n--) {
        const name = r.name();
        const kind = r.byte();
        exports.push({ name, kind, index: r.u32() });
      }
    }
  }
  const functions: Record<string, FuncSig> = {};
  const memories: string[] = [];
  for (const e of exports) {
    if (e.kind === 2) memories.push(e.name);
    if (e.kind !== 0) continue;
    const sig = types[funcTypes[e.index] ?? -1];
    if (sig === undefined) throw new Error(`wasm: export ${e.name} has no function type`);
    functions[e.name] = sig;
  }
  return { functions, memories, imports };
}

/** `"(i64, i32, i32) -> i64"`; `-> ()` when there is no result. */
export function signatureText(sig: FuncSig): string {
  const results = sig.results.length === 1 ? sig.results[0] : `(${sig.results.join(", ")})`;
  return `(${sig.params.join(", ")}) -> ${results}`;
}

const TS_TYPES: Readonly<Record<ValType, string>> = {
  i32: "number",
  i64: "bigint",
  f32: "number",
  f64: "number",
  v128: "never",
  funcref: "unknown",
  externref: "unknown",
};

/** The generated typings module of a math module binary (sorted, stable text). */
export function renderExportsModule(abi: ModuleAbi, version: string): string {
  const names = Object.keys(abi.functions).sort();
  const lines = [
    "// @generated by `just wasm` (packages/protocol/wasm/build.ts) from the built module; do not edit.",
    "// The C ABI of crates/cutweave-wasm (WP-2C.06). A Rust signature that changes without",
    "// regenerating this file fails the typings gate, and the loader refuses the module.",
    "",
    "/** The workspace version the module was built from; `@cutweave/protocol` has the same. */",
    `export const MODULE_VERSION = ${JSON.stringify(version)};`,
    "",
    "/** Every exported function and its WebAssembly signature. */",
    "export const EXPORT_SIGNATURES: Readonly<Record<string, string>> = {",
    ...names.map((n) => `  ${n}: ${JSON.stringify(signatureText(abi.functions[n] as FuncSig))},`),
    "};",
    "",
    "/** The module's exports: i64 is a BigInt, i32 and f64 are numbers. */",
    "export interface RawExports {",
    ...abi.memories.map((m) => `  readonly ${m}: WebAssembly.Memory;`),
    ...names.map((n) => {
      const sig = abi.functions[n] as FuncSig;
      const params = sig.params.map((t, i) => `a${i}: ${TS_TYPES[t]}`).join(", ");
      const result =
        sig.results.length === 0
          ? "void"
          : sig.results.length === 1
            ? TS_TYPES[sig.results[0] as ValType]
            : `[${sig.results.map((t) => TS_TYPES[t]).join(", ")}]`;
      return `  ${n}(${params}): ${result};`;
    }),
    "}",
    "",
  ];
  return lines.join("\n");
}

/** Differences between a module's exports and the expected table; empty when they match. */
export function abiMismatches(
  abi: ModuleAbi,
  expected: Readonly<Record<string, string>>,
): string[] {
  const problems: string[] = [];
  for (const [name, want] of Object.entries(expected)) {
    const sig = abi.functions[name];
    if (sig === undefined) problems.push(`${name} is missing`);
    else if (signatureText(sig) !== want) {
      problems.push(`${name} is ${signatureText(sig)}, expected ${want}`);
    }
  }
  for (const name of Object.keys(abi.functions)) {
    if (!(name in expected)) problems.push(`${name} is not in the typings`);
  }
  if (abi.imports !== 0) problems.push(`the module needs ${abi.imports} import(s)`);
  if (!abi.memories.includes("memory")) problems.push("the module exports no memory");
  return problems;
}
