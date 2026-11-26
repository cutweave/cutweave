// SPDX-License-Identifier: GPL-3.0-or-later
// The window a docked panel is in (WP-2H.15, ADR-0031). The dock moves a detached panel's DOM into
// its own window while React keeps rendering it from the main one, so nothing in the panel knows
// it moved. Each panel (and each group's header) is wrapped here: it reads its window from the
// dock (`api.getWindow()`), follows it when the panel moves, and sets the design system's portal
// container to that window's `<body>`, so the panel's menus and tooltips open where the person is
// and the components that observe their size (`useOwnerWindow`) re-observe there.
import { PortalContainerProvider } from "@cutweave/ui";
import type { IDockviewPanelProps } from "dockview-react";
import { type FunctionComponent, type ReactNode, useEffect, useState } from "react";
import { isMainWindow } from "../shell/windows.ts";

interface Disposable {
  dispose(): void;
}

/** The part of a dockview panel or group API that says where it is. */
export interface LocatedApi {
  getWindow(): Window;
  readonly onDidLocationChange: (listener: () => void) => Disposable;
  readonly onDidGroupChange?: (listener: () => void) => Disposable;
}

function windowOfApi(api: LocatedApi): Window | null {
  try {
    return api.getWindow();
  } catch {
    // Between groups (a move in progress): the next event says where it landed.
    return null;
  }
}

/** The window a panel or group is in, kept current as it moves between windows. */
export function useLocationWindow(api: LocatedApi): Window | null {
  const [win, setWin] = useState<Window | null>(() => windowOfApi(api));
  useEffect(() => {
    let live = true;
    // The dock moves the DOM first and says so after: read the window once the move is done.
    const update = (): void =>
      queueMicrotask(() => {
        if (live) setWin(windowOfApi(api));
      });
    update();
    const subs: Disposable[] = [api.onDidLocationChange(update)];
    if (api.onDidGroupChange !== undefined) subs.push(api.onDidGroupChange(update));
    return () => {
      live = false;
      for (const sub of subs) sub.dispose();
    };
  }, [api]);
  return win;
}

/** Menus and tooltips under this open in `win` (nothing to set for the main window). */
export function WindowLayers({ win, children }: { win: Window | null; children?: ReactNode }) {
  const container = isMainWindow(win) ? null : (win?.document.body ?? null);
  return <PortalContainerProvider container={container}>{children}</PortalContainerProvider>;
}

const wrapped = new WeakMap<FunctionComponent, FunctionComponent<IDockviewPanelProps>>();

/**
 * The panel component the dock renders for `component`: the same panel, with its menus and
 * tooltips in its own window. One wrapper per component, so the dock never remounts a panel.
 */
export function inPanelWindow(component: FunctionComponent): FunctionComponent<IDockviewPanelProps> {
  const known = wrapped.get(component);
  if (known !== undefined) return known;
  const Panel = component;
  function PanelInWindow({ api }: IDockviewPanelProps) {
    const win = useLocationWindow(api as unknown as LocatedApi);
    return (
      <WindowLayers win={win}>
        <Panel />
      </WindowLayers>
    );
  }
  PanelInWindow.displayName = `InPanelWindow(${component.displayName ?? component.name})`;
  wrapped.set(component, PanelInWindow);
  return PanelInWindow;
}
