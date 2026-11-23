// SPDX-License-Identifier: GPL-3.0-or-later
// Transient notices shown as `@cutweave/ui` toasts (`shell/Toasts.tsx` renders them). Anything
// can push one: the timeline when the engine refuses an edit (WP-1M.06), imports and jobs later.
import type { ToastTone } from "@cutweave/ui";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

export interface Notice {
  readonly id: number;
  readonly tone: ToastTone;
  /** What happened, in a few words: "Clip not moved". */
  readonly title: string;
  /** Why, and the next step. */
  readonly message?: string;
}

/** Older notices are dropped beyond this many, so repeated refusals cannot pile up. */
export const MAX_NOTICES = 3;

export interface ToastStore {
  readonly notices: readonly Notice[];
  /** Shows a notice; an identical one already showing is replaced. Returns its id. */
  push(notice: Omit<Notice, "id">): number;
  dismiss(id: number): void;
  clear(): void;
}

export function createToastStore() {
  let next = 1;
  return createStore<ToastStore>()((set) => ({
    notices: [],
    push(notice) {
      const id = next++;
      set((s) => {
        const others = s.notices.filter(
          (n) => n.tone !== notice.tone || n.title !== notice.title || n.message !== notice.message,
        );
        return { notices: [...others, { ...notice, id }].slice(-MAX_NOTICES) };
      });
      return id;
    },
    dismiss(id) {
      set((s) => ({ notices: s.notices.filter((n) => n.id !== id) }));
    },
    clear() {
      set({ notices: [] });
    },
  }));
}

export const toastStore = createToastStore();

export function useToastStore<T>(selector: (state: ToastStore) => T): T {
  return useStore(toastStore, selector);
}
