// SPDX-License-Identifier: GPL-3.0-or-later
// The context-menu host (WP-2H.12, ADR-0026): one `ContextMenuHost` in `EditorShell` draws the
// design system's `Menu` in controlled `point` mode, anchored at a point, for every surface:
// DOM surfaces (a document-level `contextmenu` listener finds the surface under the pointer),
// the timeline canvas (it calls `openContextMenu`), the keyboard (Shift+F10 and the Menu key) and
// text fields. A right-click outside every surface shows nothing, and not the browser's menu.
import { Menu } from "@cutweave/ui";
import { type ReactNode, type RefCallback, useCallback, useEffect, useRef } from "react";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import { desktopFromWindow } from "../session/desktop.ts";
import { buildMenu, type ResolvedMenu } from "./build.ts";
import { menuServices } from "./defaults.ts";
import { type MenuServices, setDevBuild } from "./services.ts";
import { applyTargetRules } from "./target.ts";
import { fieldRequest, textFieldOf } from "./textField.ts";
import type { MenuTarget, OpenVia, ScreenPoint } from "./types.ts";

// ---- opening a menu -----------------------------------------------------------------------------

export interface OpenRequest {
  readonly surface: string;
  readonly point: ScreenPoint;
  /** What was hit; `ids` is empty for a place. */
  readonly target?: Partial<Omit<MenuTarget, "surface">>;
  /** Where the focus returns when the menu closes: the surface element. */
  readonly element?: HTMLElement | null;
  readonly via?: OpenVia;
}

export interface OpenMenu {
  readonly id: number;
  readonly menu: ResolvedMenu;
  readonly point: ScreenPoint;
  readonly element: HTMLElement | null;
  readonly via: OpenVia;
  readonly openedAt: number;
}

export const contextMenuStore = createStore<{ open: OpenMenu | null }>()(() => ({ open: null }));

let nextId = 1;
/** The work of the row just chosen; it runs once the menu has closed and the focus is back. */
let pending: (() => void | Promise<void>) | null = null;

/**
 * Opens the menu of `request.surface` at `request.point`, after applying the selection rules.
 * Returns `false` (and shows nothing) for a surface with no table or no rows.
 */
export function openContextMenu(
  request: OpenRequest,
  services: MenuServices = menuServices,
): boolean {
  const def = services.surfaces.get(request.surface);
  if (def === undefined) {
    closeContextMenu();
    return false;
  }
  pending = null;
  const target = applyTargetRules(
    { ids: [], ...request.target, surface: request.surface },
    def.selection,
  );
  const menu = buildMenu(services, target, (run) => {
    pending = run;
  });
  if (menu === null) {
    closeContextMenu();
    return false;
  }
  contextMenuStore.setState({
    open: {
      id: nextId++,
      menu,
      point: request.point,
      element: request.element ?? null,
      via: request.via ?? "pointer",
      openedAt: services.now(),
    },
  });
  return true;
}

export function closeContextMenu(): void {
  if (contextMenuStore.getState().open !== null) contextMenuStore.setState({ open: null });
}

// ---- DOM surfaces ---------------------------------------------------------------------------------

/** What a surface element says about a right-click, a key press or a long-press on it. */
export interface SurfaceHit {
  readonly surface: string;
  readonly target?: Partial<Omit<MenuTarget, "surface">>;
  /** Overrides the point (the keyboard anchors at the element, not the pointer). */
  readonly point?: ScreenPoint;
}

export interface SurfaceQuery {
  readonly via: OpenVia;
  /** The pointer, or the lower-left of the focused element for the keyboard. */
  readonly point: ScreenPoint;
  /** The element the event came from. */
  readonly source: Element;
}

export type SurfaceResolver = (query: SurfaceQuery) => SurfaceHit | null;

interface Registered {
  readonly resolve: SurfaceResolver;
  /** The element opens its own menu from the pointer (the canvas); the host only does keys. */
  readonly pointerOwned: boolean;
}

const registered = new WeakMap<Element, Registered>();

/** Marks `element` as a surface. Returns the unregister function. */
export function registerContextSurface(
  element: HTMLElement,
  resolve: SurfaceResolver,
  options: { readonly pointerOwned?: boolean; readonly name?: string } = {},
): () => void {
  registered.set(element, { resolve, pointerOwned: options.pointerOwned === true });
  element.dataset.contextSurface = options.name ?? "";
  return () => {
    registered.delete(element);
    delete element.dataset.contextSurface;
  };
}

function surfaceOf(source: Element | null): { element: HTMLElement; entry: Registered } | null {
  for (let el = source; el !== null; el = el.parentElement) {
    const entry = registered.get(el);
    if (entry !== undefined) return { element: el as HTMLElement, entry };
  }
  return null;
}

/**
 * A ref for a DOM surface: `ref={useContextMenu("bin.card", () => ({ ids: [card.id] }))}`. The
 * target is read when the menu opens, so it is never stale.
 */
export function useContextMenu<T extends HTMLElement>(
  surface: string,
  target: (query: SurfaceQuery) => SurfaceHit["target"],
): RefCallback<T> {
  const latest = useRef(target);
  latest.current = target;
  const off = useRef<(() => void) | null>(null);
  return useCallback(
    (element: T | null) => {
      off.current?.();
      off.current = null;
      if (element === null) return;
      off.current = registerContextSurface(
        element,
        (query) => ({ surface, target: latest.current(query) ?? {} }),
        { name: surface },
      );
    },
    [surface],
  );
}

/** A DOM surface as a wrapper element, for a surface with no element of its own. */
export function ContextSurface(props: {
  surface: string;
  target?: (query: SurfaceQuery) => SurfaceHit["target"];
  className?: string;
  children?: ReactNode;
}) {
  const ref = useContextMenu<HTMLDivElement>(props.surface, props.target ?? (() => ({})));
  return (
    <div ref={ref} className={props.className}>
      {props.children}
    </div>
  );
}

// ---- the document-level listeners -----------------------------------------------------------------

/** After a key opened a menu, the contextmenu event the platform adds is not a second request. */
const DUPLICATE_WINDOW_MS = 400;

function lowerLeft(el: Element): ScreenPoint {
  const r = el.getBoundingClientRect();
  return { x: Math.round(r.left), y: Math.round(r.bottom) };
}

const FOCUSABLE =
  'button, a[href], input, select, textarea, summary, [tabindex]:not([tabindex="-1"])';

/**
 * Where the focus goes when a menu opened on `surface` closes: what the right-click focused
 * (a button takes focus on press), else the surface's first focusable part, else the surface.
 */
function focusReturn(doc: Document, surface: HTMLElement): HTMLElement {
  const active = doc.activeElement;
  if (active instanceof HTMLElement && active !== doc.body && surface.contains(active)) {
    return active;
  }
  return surface.matches(FOCUSABLE)
    ? surface
    : (surface.querySelector<HTMLElement>(FOCUSABLE) ?? surface);
}

function activeElement(doc: Document): Element | null {
  let active: Element | null = doc.activeElement;
  // Into an open shadow root or iframe-less composition: keep it simple, the editor has none.
  if (active === doc.body) active = null;
  return active;
}

/** Whether `event` is Shift+F10 or the Menu key. */
export function isMenuKey(event: KeyboardEvent): boolean {
  return event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey);
}

/**
 * Installs the listeners that find a surface for a right-click, a key or a long-press. Returns
 * the uninstall function. The host component does this on mount.
 */
export function installContextMenuHandlers(
  doc: Document,
  services: MenuServices = menuServices,
): () => void {
  const duplicateOfKey = (surface: Element): boolean => {
    const open = contextMenuStore.getState().open;
    return (
      open !== null &&
      open.via === "keyboard" &&
      open.element !== null &&
      surface.contains(open.element) &&
      services.now() - open.openedAt < DUPLICATE_WINDOW_MS
    );
  };

  const onContextMenu = (event: MouseEvent): void => {
    const source = event.target instanceof Element ? event.target : null;
    event.preventDefault(); // never the browser's menu, and nothing where no surface claims it
    if (source === null || source.closest(".cw-menu") !== null) return;
    const point = { x: event.clientX, y: event.clientY };

    const field = textFieldOf(source);
    if (field !== null) {
      if (duplicateOfKey(field)) return;
      openContextMenu(fieldRequest(field, point, "pointer"), services);
      return;
    }
    const found = surfaceOf(source);
    if (found !== null) {
      if (found.entry.pointerOwned) return; // the canvas opens its own from its pointer stream
      if (duplicateOfKey(found.element)) return;
      const hit = found.entry.resolve({ via: "pointer", point, source });
      if (hit !== null) {
        openContextMenu(
          {
            ...hit,
            point: hit.point ?? point,
            element: focusReturn(doc, found.element),
            via: "pointer",
          },
          services,
        );
        return;
      }
    }
    // Dead space: nothing, except a development build's Shift+right-click.
    if (event.shiftKey && services.isDev()) {
      openContextMenu(
        { surface: "dev.inspect", point, target: { data: { point } }, via: "pointer" },
        services,
      );
      return;
    }
    closeContextMenu();
  };

  const onKeyDown = (event: KeyboardEvent): void => {
    if (!isMenuKey(event) || event.defaultPrevented) return;
    const active = activeElement(doc);
    if (active === null) return;
    const field = textFieldOf(active);
    if (field !== null) {
      event.preventDefault();
      openContextMenu(fieldRequest(field, lowerLeft(field), "keyboard"), services);
      return;
    }
    const found = surfaceOf(active);
    if (found === null) return;
    const point = lowerLeft(active);
    const hit = found.entry.resolve({ via: "keyboard", point, source: active });
    event.preventDefault();
    if (hit === null) return;
    openContextMenu(
      { ...hit, point: hit.point ?? point, element: active as HTMLElement, via: "keyboard" },
      services,
    );
  };

  // A menu closes when the page scrolls or resizes under it: its anchor has moved.
  const closeOnMove = (event: Event): void => {
    if (contextMenuStore.getState().open === null) return;
    if (event.target instanceof Element && event.target.closest(".cw-menu") !== null) return;
    closeContextMenu();
  };

  const win = doc.defaultView;
  doc.addEventListener("contextmenu", onContextMenu, true);
  doc.addEventListener("keydown", onKeyDown, true);
  doc.addEventListener("scroll", closeOnMove, true);
  doc.addEventListener("wheel", closeOnMove, { capture: true, passive: true });
  win?.addEventListener("resize", closeOnMove);
  return () => {
    doc.removeEventListener("contextmenu", onContextMenu, true);
    doc.removeEventListener("keydown", onKeyDown, true);
    doc.removeEventListener("scroll", closeOnMove, true);
    doc.removeEventListener("wheel", closeOnMove, true);
    win?.removeEventListener("resize", closeOnMove);
  };
}

// ---- the component ---------------------------------------------------------------------------------

/** Mounted once in `EditorShell`. */
export function ContextMenuHost({ services = menuServices }: { services?: MenuServices }) {
  const open = useStore(contextMenuStore, (s) => s.open);
  // The menu stays drawn through its close, so keep what it was showing.
  const last = useRef<OpenMenu | null>(null);
  if (open !== null) last.current = open;
  const shown = open ?? last.current;

  useEffect(() => installContextMenuHandlers(document, services), [services]);
  // Development builds offer "Inspect element" on dead space (Shift+right-click).
  useEffect(() => {
    void desktopFromWindow()
      ?.app?.getInfo()
      .then((info) => setDevBuild(info.dev === true))
      .catch(() => undefined);
  }, []);

  const onCloseAutoFocus = useCallback(
    (event: Event) => {
      // Focus goes back to the surface, not the zero-size anchor.
      event.preventDefault();
      // A menu replaced by another (a second right-click) leaves the focus to its successor.
      if (contextMenuStore.getState().open !== null) return;
      shown?.element?.focus({ preventScroll: true });
      const run = pending;
      pending = null;
      if (run !== null) {
        void Promise.resolve()
          .then(run)
          .catch((error: unknown) => services.report(error));
      }
    },
    [shown, services],
  );

  if (shown === null) return null;
  return (
    <Menu
      key={shown.id}
      mode="point"
      open={open !== null}
      anchor={shown.point}
      items={shown.menu.entries}
      aria-label={shown.menu.name}
      onOpenChange={(next) => {
        // A replaced menu closing as it unmounts must not close its successor.
        if (!next && contextMenuStore.getState().open?.id === shown.id) closeContextMenu();
      }}
      onCloseAutoFocus={onCloseAutoFocus}
    />
  );
}
