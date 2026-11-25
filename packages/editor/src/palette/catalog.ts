// SPDX-License-Identifier: GPL-3.0-or-later
// The command catalog the palette lists (WP-2H.04): every registered engine command, from the
// generated `schema/commands.json`. A store, not a constant, so a fixture command (a test, and
// later a plugin's) registers once and shows in the palette and the menus with no UI code
// change. `hidden` entries (the engine's plumbing, `mcp: "hidden"`) and `view.*` queries are
// never listed.
import { createStore } from "zustand/vanilla";
import raw from "../../../../schema/commands.json";

/** A JSON Schema fragment, as the catalog's `input_schema` has it. */
export interface JsonSchema {
  readonly type?: string | readonly string[];
  readonly description?: string;
  readonly properties?: Readonly<Record<string, JsonSchema>>;
  readonly required?: readonly string[];
  readonly enum?: readonly unknown[];
  readonly anyOf?: readonly JsonSchema[];
  readonly oneOf?: readonly JsonSchema[];
  readonly items?: JsonSchema;
  readonly pattern?: string;
  readonly default?: unknown;
  readonly $ref?: string;
  readonly $defs?: Readonly<Record<string, JsonSchema>>;
  readonly definitions?: Readonly<Record<string, JsonSchema>>;
  /** Display names of `enum` values, for schemas the editor builds itself. */
  readonly "x-enum-labels"?: Readonly<Record<string, string>>;
}

export interface CatalogCommand {
  /** `clip.split`. */
  readonly name: string;
  /** `clip`: the chip the palette shows. */
  readonly namespace: string;
  /** The engine's one-line summary; the title when the editor has no label for it. */
  readonly summary: string;
  readonly undoable: boolean;
  readonly destructive: boolean;
  readonly readOnly: boolean;
  /** The engine's plumbing (`mcp: "hidden"`): not listed. */
  readonly hidden: boolean;
  /** A `view.*` query or selection call: not listed. */
  readonly view: boolean;
  readonly inputSchema: JsonSchema;
}

interface RawCommand {
  readonly name: string;
  readonly namespace: string;
  readonly summary: string;
  readonly undoable: boolean;
  readonly mcp: string;
  readonly view: boolean;
  readonly annotations: { readonly destructiveHint: boolean; readonly readOnlyHint: boolean };
  readonly input_schema: JsonSchema;
}

function fromRaw(c: RawCommand): CatalogCommand {
  return {
    name: c.name,
    namespace: c.namespace,
    summary: c.summary,
    undoable: c.undoable,
    destructive: c.annotations.destructiveHint,
    readOnly: c.annotations.readOnlyHint,
    hidden: c.mcp === "hidden",
    view: c.view,
    inputSchema: c.input_schema,
  };
}

interface CatalogState {
  readonly commands: readonly CatalogCommand[];
  /** Adds a command (or replaces the one with its name). */
  register(command: CatalogCommand): void;
}

export const commandCatalog = createStore<CatalogState>()((set) => ({
  commands: (raw as unknown as { commands: RawCommand[] }).commands.map(fromRaw),
  register: (command) =>
    set((s) => ({
      commands: [...s.commands.filter((c) => c.name !== command.name), command],
    })),
}));

/** The commands the palette lists. */
export function listedCatalog(commands: readonly CatalogCommand[]): CatalogCommand[] {
  return commands.filter((c) => !c.hidden && !c.view);
}
