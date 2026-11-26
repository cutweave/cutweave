// SPDX-License-Identifier: GPL-3.0-or-later
// Transient notices shown as `@cutweave/ui` toasts (`shell/Toasts.tsx` renders them). Anything
// can push one: the timeline when the engine refuses an edit (WP-1M.06), imports and jobs later.
import type { ToastTone } from "@cutweave/ui";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import { activeWindow, registeredWindows } from "../shell/windows.ts";

export interface Notice {
  readonly id: number;
  readonly tone: ToastTone;
  /** What happened, in a few words: "Clip not moved". */
  readonly title: string;
  /** Why, and the next step. */
  readonly message?: string;
  /** Buttons on the toast ("Reveal in folder"); the toast closes after one is pressed. */
  readonly actions?: readonly { readonly label: string; readonly onClick: () => void }[];
  /** Milliseconds before it goes away, instead of the tone's default. */
  readonly duration?: number;
  /**
   * The detached panel's window it was raised from; it shows there (#191). Absent for the main
   * window. Taken from the window the person last worked in when pushed.
   */
  readonly window?: Window;
}

/** Older notices are dropped beyond this many, so repeated refusals cannot pile up. */
export const MAX_NOTICES = 3;

export interface ToastStore {
  readonly notices: readonly Notice[];
  /** Shows a notice; an identical one already showing is replaced. Returns its id. */
  push(notice: Omit<Notice, "id">): number;
  /** Changes a showing notice in place (a progress toast); nothing if it is gone. */
  update(id: number, patch: Partial<Pick<Notice, "title" | "message">>): void;
  dismiss(id: number): void;
  clear(): void;
}

export function createToastStore() {
  let next = 1;
  return createStore<ToastStore>()((set) => ({
    notices: [],
    push(notice) {
      const id = next++;
      const raisedIn = notice.window ?? activeWindow();
      const inPopout = registeredWindows().some(
        (w) => w.role === "popout" && w.window === raisedIn,
      );
      const stamped = inPopout ? { ...notice, window: raisedIn } : notice;
      set((s) => {
        const others = s.notices.filter(
          (n) => n.tone !== notice.tone || n.title !== notice.title || n.message !== notice.message,
        );
        return { notices: [...others, { ...stamped, id }].slice(-MAX_NOTICES) };
      });
      return id;
    },
    update(id, patch) {
      set((s) => ({ notices: s.notices.map((n) => (n.id === id ? { ...n, ...patch } : n)) }));
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
