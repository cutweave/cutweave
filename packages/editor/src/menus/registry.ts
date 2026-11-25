// SPDX-License-Identifier: GPL-3.0-or-later
// The surface registry and the contribution registry (WP-2H.12, ADR-0026). A surface owner calls
// `registerMenuSurface` with its table; first-party feature WPs and, later, plugins
// (WP-6B.03) call `MenuContributionRegistry.register` to add rows to a surface they do not own.
import type { MenuContribution, MenuSurfaceDef } from "./types.ts";

export class MenuSurfaceRegistry {
  readonly #surfaces = new Map<string, MenuSurfaceDef>();

  /** Registers (or replaces) a surface's table. Returns an unregister function. */
  register(def: MenuSurfaceDef): () => void {
    this.#surfaces.set(def.id, def);
    return () => {
      if (this.#surfaces.get(def.id) === def) this.#surfaces.delete(def.id);
    };
  }

  get(id: string): MenuSurfaceDef | undefined {
    return this.#surfaces.get(id);
  }

  list(): readonly MenuSurfaceDef[] {
    return [...this.#surfaces.values()];
  }
}

/** Items a plugin adds before the rest go into a submenu named for the plugin. */
export const MAX_PLUGIN_ITEMS = 3;
/** First-party rows order within 0..999; a plugin's within 0..99 of its own group. */
export const ORDER_MAX = 999;
export const PLUGIN_ORDER_MAX = 99;
/** A plugin's label is plain text up to this many characters. */
export const LABEL_MAX = 48;

/** Plain text: no markup, no control characters, one line, bounded. */
export function plainLabel(raw: string): string {
  const noMarkup = raw.replace(/<[^>]*>/g, "");
  let text = "";
  for (const ch of noMarkup) {
    const code = ch.codePointAt(0) ?? 0;
    // Control characters, line and paragraph separators become spaces (then one line).
    const control =
      code < 0x20 || (code >= 0x7f && code <= 0x9f) || code === 0x2028 || code === 0x2029;
    text += control ? " " : ch;
  }
  text = text.replace(/\s+/g, " ").trim();
  return text.length > LABEL_MAX ? `${text.slice(0, LABEL_MAX - 1).trimEnd()}…` : text;
}

export function clampOrder(order: number, max: number): number {
  if (!Number.isFinite(order)) return max;
  return Math.min(max, Math.max(0, Math.round(order)));
}

/** A contribution as the menu builder reads it: order clamped, label made plain. */
export interface NormalizedContribution extends MenuContribution {
  readonly order: number;
  readonly label?: string;
}

/** Which commands a plugin may run; the first-party default allows all of them. */
export type PluginPermit = (pluginId: string, command: string) => boolean;

export class MenuContributionRegistry {
  readonly #items: NormalizedContribution[] = [];
  #permit: PluginPermit = () => true;

  /** Plugins only reach commands their manifest permits (WP-6B.03 supplies the check). */
  setPermit(permit: PluginPermit): void {
    this.#permit = permit;
  }

  /** Registers a row on `surface`. Returns an unregister function. */
  register(contribution: MenuContribution): () => void {
    const plugin = contribution.plugin;
    const label = contribution.label === undefined ? undefined : plainLabel(contribution.label);
    const normalized: NormalizedContribution = {
      ...contribution,
      order: clampOrder(contribution.order, plugin ? PLUGIN_ORDER_MAX : ORDER_MAX),
      ...(label !== undefined && label !== "" && { label }),
    };
    this.#items.push(normalized);
    return () => {
      const at = this.#items.indexOf(normalized);
      if (at >= 0) this.#items.splice(at, 1);
    };
  }

  /** What `surface` has been given, in order, with plugin items the plugin may not run left out. */
  forSurface(surface: string): readonly NormalizedContribution[] {
    return this.#items
      .filter((c) => c.surface === surface)
      .filter((c) => c.plugin === undefined || this.#permit(c.plugin.id, c.command))
      .sort((a, b) => a.order - b.order);
  }
}
