// SPDX-License-Identifier: GPL-3.0-or-later
// Which Inspector sections are open. They were each panel's own state; one store lets the section
// header's context menu collapse or expand all of them (WP-2H.13) and keeps a section's state when
// the selection moves from a clip to another clip.
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

/**
 * The sections the Inspector has: a clip's (the Video ones are named by the engine's `x-ui.group`
 * of the property, so `transform` to `video` here are the groups it knows), a media asset's and
 * the project's. "Collapse all" closes these.
 */
export const INSPECTOR_SECTIONS = [
  "clip",
  "timing",
  "track",
  "transform",
  "crop",
  "flip",
  "compositing",
  "video",
  "audio",
  "media",
  "project",
] as const;
/** A section id: one of the above, or a group a newer schema brings. */
export type InspectorSectionId = string;

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
