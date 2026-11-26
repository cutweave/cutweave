// SPDX-License-Identifier: GPL-3.0-or-later
// Where floating layers open (WP-2H.15, ADR-0031). The editor can move a panel into its own OS
// window (a popout that shares the main window's React tree), and a menu or tooltip opened from
// that panel must open in that window, not in the main one. A panel sets the container to its
// window's `<body>` and changes it when the panel moves; `Menu` and `Tooltip` portal into it, and
// components that observe their own size read the window from it. Without a provider, layers open
// in the main document as before. (A provider, not a component: it draws nothing, so it has no
// story.)
import { createContext, createElement, type ReactNode, useContext } from "react";

const Container = createContext<HTMLElement | null>(null);

export interface PortalContainerProviderProps {
  /** The element floating layers open in (a window's `<body>`); `null` is the main document. */
  container: HTMLElement | null;
  children?: ReactNode;
}

/** Floating layers (`Menu`, `Tooltip`) under this open in `container`. */
export function PortalContainerProvider({ container, children }: PortalContainerProviderProps) {
  return createElement(Container.Provider, { value: container }, children);
}

/** The element floating layers open in here, or `null` for the main document. */
export function usePortalContainer(): HTMLElement | null {
  return useContext(Container);
}

/**
 * The window this part of the tree is shown in: the container's window, else the main one. A
 * component that observes its size or schedules frames takes them from it (a `ResizeObserver`
 * from another window fires unreliably for an element in this one).
 */
export function useOwnerWindow(): (Window & typeof globalThis) | null {
  const container = usePortalContainer();
  const own = container?.ownerDocument.defaultView;
  if (own !== null && own !== undefined) return own as Window & typeof globalThis;
  return typeof globalThis.window === "undefined" ? null : globalThis.window;
}
