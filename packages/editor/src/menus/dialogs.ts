// SPDX-License-Identifier: GPL-3.0-or-later
// The questions a menu choice may ask before it acts (WP-2H.13): a name for a clip, a speed for
// clips (WP-2D.01), a yes for
// deleting a track that holds clips, a yes for removing a media asset that clips use, and (WP-2F.08)
// what to do with a folder's contents and where to move tiles to. One
// store, one component (`MenuDialogs.tsx`); the choice only opens it, and the dialog sends the one
// command when the person agrees.
import type { MediaFilter } from "@cutweave/protocol";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

export type MenuDialog =
  | { readonly kind: "rename-clip"; readonly item: string; readonly name: string }
  // Speed ▸ Custom… (WP-2D.01): a speed for the clips, prefilled with their shared one.
  | { readonly kind: "custom-speed"; readonly items: readonly string[]; readonly speed: string }
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
    }
  // Deleting a folder that holds something (WP-2F.08); `rest` are folders still to ask about.
  | { readonly kind: "delete-folder"; readonly folder: string; readonly rest: readonly string[] }
  // Paste attributes… (WP-2A.06): which groups of `from` go onto the clips `to`.
  | { readonly kind: "paste-attributes"; readonly from: string; readonly to: readonly string[] }
  // Replace clip… (WP-2A.11): which media takes the clip's place, and how long the clip stays.
  | { readonly kind: "replace-clip"; readonly item: string }
  // Import as slideshow… (WP-2F.05d): seconds per picture and the order, for the files chosen.
  | { readonly kind: "slideshow"; readonly files: readonly string[] }
  // Duration… on an image card (#215): the length the next insert of that picture uses.
  | { readonly kind: "insert-duration"; readonly media: string; readonly name: string }
  // Add tag… on a bin card (WP-2F.06b): the tag editor for the assets chosen.
  | { readonly kind: "tags"; readonly media: readonly string[] }
  // Set rating from the command palette (WP-2F.06b): the stars for the assets chosen.
  | { readonly kind: "rating"; readonly media: readonly string[] }
  // New collection… and Save as smart collection… (WP-2F.06c): a name; with `filter` the
  // collection is smart, else it is manual and `media` are its first members.
  | {
      readonly kind: "new-collection";
      readonly media: readonly string[];
      readonly filter: MediaFilter | null;
    }
  // Add to collection… from the command palette: which manual collection the assets go onto.
  | { readonly kind: "add-to-collection"; readonly media: readonly string[] }
  // Deleting a collection that lists something: a yes (the media stay).
  | { readonly kind: "delete-collection"; readonly collection: string }
  // The folder picker of "Move to…" for the assets and folders chosen.
  | {
      readonly kind: "move-to";
      readonly media: readonly string[];
      readonly folders: readonly string[];
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
