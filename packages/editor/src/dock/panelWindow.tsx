// SPDX-License-Identifier: GPL-3.0-or-later
// The window a docked panel is in (WP-2H.15, ADR-0031). The dock moves a detached panel's DOM into
// its own window while React keeps rendering it from the main one, so nothing in the panel knows
// it moved. Each panel (and each group's header) is wrapped here: it reads its window from the
// dock (`api.getWindow()`), follows it when the panel moves, and sets the design system's portal
// container to that window's `<body>`, so the panel's menus and tooltips open where the person is
// and the components that observe their size (`useOwnerWindow`) re-observe there.
import { Icon, IconButton, PortalContainerProvider, Tooltip } from "@cutweave/ui";
import type { IDockviewPanelProps } from "dockview-react";
import {
  Component,
  type ErrorInfo,
  type FunctionComponent,
  type ReactNode,
  useEffect,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import type { TKey } from "../i18n/index.ts";
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

/** The compact message a crashed panel shows in its own room, with the way to try it again. */
function PanelCrashed({ name, onReload }: { name: string; onReload: () => void }) {
  const { t } = useTranslation();
  return (
    <div className="cw-empty" role="alert" data-panel-crashed="">
      <Icon name="triangle-alert" size="lg" className="cw-empty__icon" />
      <p className="cw-empty__title body-strong">{t("dock.panelCrashed.title", { name })}</p>
      <p className="cw-empty__body body">{t("dock.panelCrashed.body")}</p>
      <Tooltip label={t("dock.panelCrashed.reload")}>
        <IconButton icon="rotate-cw" label={t("dock.panelCrashed.reload")} onClick={onReload} />
      </Tooltip>
    </div>
  );
}

interface BoundaryProps {
  readonly name: string;
  readonly children?: ReactNode;
}

/**
 * Contains a crash to its panel (#191): the rest of the editor, and the other panels, keep
 * working while this one shows a short message and a Reload panel button.
 */
export class PanelErrorBoundary extends Component<BoundaryProps, { failed: boolean }> {
  override state = { failed: false };

  static getDerivedStateFromError(): { failed: boolean } {
    return { failed: true };
  }

  override componentDidCatch(error: unknown, info: ErrorInfo): void {
    console.error(`[panel] ${this.props.name} crashed`, error, info.componentStack);
  }

  override render(): ReactNode {
    if (this.state.failed) {
      return (
        <PanelCrashed name={this.props.name} onReload={() => this.setState({ failed: false })} />
      );
    }
    return this.props.children;
  }
}

const wrapped = new WeakMap<FunctionComponent, FunctionComponent<IDockviewPanelProps>>();

/**
 * The panel component the dock renders for `component`: the same panel, with its menus and
 * tooltips in its own window. One wrapper per component, so the dock never remounts a panel.
 */
export function inPanelWindow(
  component: FunctionComponent,
): FunctionComponent<IDockviewPanelProps> {
  const known = wrapped.get(component);
  if (known !== undefined) return known;
  const Panel = component;
  function PanelInWindow({ api, params }: IDockviewPanelProps) {
    const { t } = useTranslation();
    const win = useLocationWindow(api as unknown as LocatedApi);
    const titleKey = (params as { titleKey?: TKey } | undefined)?.titleKey;
    return (
      <WindowLayers win={win}>
        <PanelErrorBoundary name={titleKey === undefined ? api.id : t(titleKey)}>
          <Panel />
        </PanelErrorBoundary>
      </WindowLayers>
    );
  }
  PanelInWindow.displayName = `InPanelWindow(${component.displayName ?? component.name})`;
  wrapped.set(component, PanelInWindow);
  return PanelInWindow;
}
