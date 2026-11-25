// SPDX-License-Identifier: GPL-3.0-or-later
// How the media bin lays its cards out. Session-wide, like the other panel toggles: it survives
// switching asset tabs. The panel's layout control and the bin's context menu both set it.
import { createStore } from "zustand/vanilla";

export type MediaLayout = "grid" | "list";

export const layoutStore = createStore<{ layout: MediaLayout; set(layout: MediaLayout): void }>()(
  (set) => ({ layout: "grid", set: (layout) => set({ layout }) }),
);
