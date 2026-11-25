// SPDX-License-Identifier: GPL-3.0-or-later
// Which Inspector sections are open. They were each panel's own state; one store lets the section
// header's context menu collapse or expand all of them (WP-2H.13) and keeps a section's state when
// the selection moves from a clip to another clip.
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

/** The sections the Inspector has: a clip's four, a media asset's, the project's. */
export const INSPECTOR_SECTIONS = [
  "clip",
  "timing",
  "transform",
  "audio",
  "media",
  "project",
] as const;
export type InspectorSectionId = (typeof INSPECTOR_SECTIONS)[number];

export interface SectionsStore {
  /** Sections the person closed; everything else is open. */
  readonly closed: ReadonlySet<InspectorSectionId>;
  set(id: InspectorSectionId, expanded: boolean): void;
  collapseAll(): void;
  expandAll(): void;
}

export const sectionsStore = createStore<SectionsStore>()((set) => ({
  closed: new Set(),
  set: (id, expanded) =>
    set((s) => {
      const closed = new Set(s.closed);
      if (expanded) closed.delete(id);
      else closed.add(id);
      return { closed };
    }),
  collapseAll: () => set({ closed: new Set(INSPECTOR_SECTIONS) }),
  expandAll: () => set({ closed: new Set() }),
}));

/** `[expanded, setExpanded]` of one section. */
export function useSection(id: InspectorSectionId): [boolean, (expanded: boolean) => void] {
  const expanded = useStore(sectionsStore, (s) => !s.closed.has(id));
  return [expanded, (next) => sectionsStore.getState().set(id, next)];
}
