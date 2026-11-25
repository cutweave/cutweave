// SPDX-License-Identifier: GPL-3.0-or-later
// The built-in panels, registered once when the dock is imported. Minimum sizes come from layout
// tokens (design/tokens.json), so no pixel literal lives here.
import { tokenPx } from "../dock/theme.ts";
import { AssetBrowserPanel } from "../panels/AssetBrowserPanel.tsx";
import { InspectorPanel } from "../panels/InspectorPanel.tsx";
import { PreviewPanel } from "../panels/PreviewPanel.tsx";
import { TimelinePanel } from "../panels/TimelinePanel.tsx";
import { registerPanel } from "./panelRegistry.ts";

export function registerBuiltinPanels(): void {
  registerPanel({
    id: "assets",
    title: "panel.assets",
    component: AssetBrowserPanel,
    defaultDock: { ref: "preview", direction: "left" },
    minSize: () => ({ width: tokenPx("--layer-width-sm") }),
  });
  registerPanel({
    id: "preview",
    title: "panel.preview",
    component: PreviewPanel,
    defaultDock: { ref: "timeline", direction: "above" },
    minSize: () => ({ width: tokenPx("--layer-width-md") }),
  });
  registerPanel({
    id: "inspector",
    title: "panel.inspector",
    component: InspectorPanel,
    defaultDock: { ref: "preview", direction: "right" },
    minSize: () => ({ width: tokenPx("--layer-width-sm") }),
  });
  registerPanel({
    id: "timeline",
    title: "panel.timeline",
    component: TimelinePanel,
    defaultDock: { ref: null, direction: "below" },
    minSize: () => ({ height: tokenPx("--track-md") * 3 }),
  });
}

registerBuiltinPanels();
