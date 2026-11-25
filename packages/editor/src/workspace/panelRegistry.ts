// SPDX-License-Identifier: GPL-3.0-or-later
// The typed panel registry (WP-2H.01): every dockable panel is registered here with its title,
// component, default dock and minimum size. The dock, View > Panels and the "Reopen panels"
// empty state read it, so a panel added later (History 2H.11, Mixer, Export queue, plugin panels
// in P6) shows up in all of them by registering once. Contribution-point shaped: `registerPanel`
// returns an unregister function.
import type { FunctionComponent } from "react";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import type { TKey } from "../i18n/index.ts";

/** A panel's id: the dockview panel id, the layout key and the suffix of its toggle command. */
export type PanelId = string;

/** Where a panel docks, relative to another panel (or to the dock's edge when `ref` is null). */
export interface DockSpot {
  /** The panel it sits beside; `null` is the whole dock (its outer edge). */
  readonly ref: PanelId | null;
  readonly direction: "left" | "right" | "above" | "below" | "within";
}

export interface PanelMinSize {
  readonly width?: number;
  readonly height?: number;
}

export interface PanelDef {
  readonly id: PanelId;
  /** Catalog key of the panel's name (its tab and its View > Panels entry). */
  readonly title: TKey;
  readonly component: FunctionComponent;
  /** Where it docks when it has no remembered spot in this workspace. */
  readonly defaultDock: DockSpot;
  /**
   * Minimum size in pixels, read when the panel is added (a function because the numbers come
   * from layout tokens, which are only known once the stylesheet is loaded).
   */
  readonly minSize?: () => PanelMinSize;
}

interface RegistryState {
  readonly panels: readonly PanelDef[];
}

export const panelRegistry = createStore<RegistryState>()(() => ({ panels: [] }));

/** Registers a panel (replacing one with the same id); returns the function that removes it. */
export function registerPanel(def: PanelDef): () => void {
  panelRegistry.setState((s) => ({
    panels: [...s.panels.filter((p) => p.id !== def.id), def],
  }));
  return () => {
    panelRegistry.setState((s) => ({ panels: s.panels.filter((p) => p.id !== def.id) }));
  };
}

/** The registered panels, in registration order. */
export function registeredPanels(): readonly PanelDef[] {
  return panelRegistry.getState().panels;
}

export function getPanelDef(id: PanelId): PanelDef | undefined {
  return registeredPanels().find((p) => p.id === id);
}

export function useRegisteredPanels(): readonly PanelDef[] {
  return useStore(panelRegistry, (s) => s.panels);
}
