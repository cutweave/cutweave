// SPDX-License-Identifier: GPL-3.0-or-later
// What a menu row names (WP-2H.12): a catalog command, or an editor-side UI command. Both give
// a label, an icon and the flags; neither is a closure over several mutations (ADR-0002).
import { needsRequestId } from "@cutweave/protocol";
import type { IconName } from "@cutweave/ui";
import type { TKey } from "../i18n/index.ts";
import type { MenuTarget } from "./types.ts";

/**
 * How a menu shows a registry command. The generated catalog (`schema/commands.json`) carries
 * the name, `undoable` and the destructive hint but no label or icon yet, so the menu keeps its
 * own table for those and a test (`menus.test.ts`) holds it to the catalog: a name that is not
 * registered, or a flag that disagrees, fails.
 */
export interface CommandInfo {
  /** The registry name: `clip.split`. */
  readonly name: string;
  /** Verb phrase for the row; a plural key gets `count`. */
  readonly label: TKey;
  /** The label for several targets ("Delete {{count}} clips"); `count` is passed. */
  readonly labelMany?: TKey;
  readonly icon?: IconName;
  /** Mirrors the catalog's `destructiveHint`: shown as danger and placed last. */
  readonly destructive?: boolean;
  /** Mirrors the catalog's `undoable`. */
  readonly undoable?: boolean;
  /**
   * When the command makes sense (a `when` clause, see `when.ts`): the palette lists it only
   * while it holds, and a surface row without a clause of its own takes it as its enabled state.
   */
  readonly when?: string;
}

export class CommandInfoTable {
  readonly #byName = new Map<string, CommandInfo>();

  register(info: CommandInfo): void {
    this.#byName.set(info.name, info);
  }

  get(name: string): CommandInfo | undefined {
    return this.#byName.get(name);
  }

  list(): readonly CommandInfo[] {
    return [...this.#byName.values()];
  }
}

/** Raised when one menu choice would apply two mutating commands: it needs one composite. */
export class MultipleMutationsError extends Error {
  constructor(first: string, second: string) {
    super(
      `a menu choice ran two mutating commands (${first}, then ${second}); ` +
        "add one composite command so the choice is one undo step",
    );
    this.name = "MultipleMutationsError";
  }
}

/** What a UI command's `run` may use. */
export interface RunContext {
  readonly target: MenuTarget;
  readonly ids: readonly string[];
  /** Calls a registry command. At most one mutating call per menu choice. */
  call(name: string, args?: unknown): Promise<unknown>;
}

/** An editor-side action: layout, copy text, reveal. It never changes the document. */
export interface UiCommand {
  /** `workspace.close_panel`, `edit.text.copy`. */
  readonly id: string;
  readonly label: TKey;
  readonly icon?: IconName;
  readonly danger?: boolean;
  /** Touches the OS (clipboard, file manager, developer tools): it must be on the exemption list. */
  readonly host?: boolean;
  /** Not listed in the palette: it only makes sense inside the menu that offers it (a text field's Cut). */
  readonly hidden?: boolean;
  /** When the command makes sense; see {@link CommandInfo.when}. */
  readonly when?: string;
  run(args: unknown, ctx: RunContext): void | Promise<void>;
}

export class UiCommandTable {
  readonly #byId = new Map<string, UiCommand>();

  register(command: UiCommand): void {
    this.#byId.set(command.id, command);
  }

  get(id: string): UiCommand | undefined {
    return this.#byId.get(id);
  }

  list(): readonly UiCommand[] {
    return [...this.#byId.values()];
  }
}

/**
 * Wraps the real call so a single menu choice cannot apply two mutating commands. `view.*` and
 * reads are free (selection, playhead and queries are not undo steps).
 */
export function guardedCall(
  call: (name: string, args?: unknown) => Promise<unknown>,
): (name: string, args?: unknown) => Promise<unknown> {
  let mutated: string | null = null;
  return (name, args) => {
    if (needsRequestId(name)) {
      if (mutated !== null) {
        return Promise.reject(new MultipleMutationsError(mutated, name));
      }
      mutated = name;
    }
    return call(name, args);
  };
}
