// SPDX-License-Identifier: GPL-3.0-or-later
// The three questions a menu choice may ask before it acts (WP-2H.13): a name for a clip, a yes
// for deleting a track that holds clips, a yes for removing a media asset that clips use. One
// store, one component (`MenuDialogs.tsx`); the choice only opens it, and the dialog sends the one
// command when the person agrees.
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

export type MenuDialog =
  | { readonly kind: "rename-clip"; readonly item: string; readonly name: string }
  | {
      readonly kind: "delete-track";
      readonly track: string;
      readonly name: string;
      readonly clips: number;
    }
  | {
      readonly kind: "remove-media";
      readonly id: string;
      readonly name: string;
      readonly uses: number;
    };

export interface MenuDialogStore {
  readonly dialog: MenuDialog | null;
  open(dialog: MenuDialog): void;
  close(): void;
}

export const menuDialogStore = createStore<MenuDialogStore>()((set) => ({
  dialog: null,
  open: (dialog) => set({ dialog }),
  close: () => set({ dialog: null }),
}));

export function useMenuDialog(): MenuDialog | null {
  return useStore(menuDialogStore, (s) => s.dialog);
}
