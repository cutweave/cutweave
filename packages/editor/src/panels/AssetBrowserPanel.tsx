// SPDX-License-Identifier: GPL-3.0-or-later
import { type IconName, type TabItem, TabList, TabPanel, Tabs } from "@cutweave/ui";
import { useTranslation } from "react-i18next";
import type { TKey } from "../i18n/index.ts";
import { ASSET_TABS, type AssetTabId, useViewStore, viewStore } from "../state/viewStore.ts";
import { EmptyState } from "./EmptyState.tsx";
import { MediaPanel } from "./MediaPanel.tsx";

interface AssetTabDef {
  labelKey: TKey;
  titleKey: TKey;
  bodyKey: TKey;
  icon: IconName;
}

export const ASSET_TAB_DEFS: Record<AssetTabId, AssetTabDef> = {
  media: {
    labelKey: "assets.tabs.media",
    titleKey: "assets.empty.media.title",
    bodyKey: "assets.empty.media.body",
    icon: "film",
  },
  audio: {
    labelKey: "assets.tabs.audio",
    titleKey: "assets.empty.audio.title",
    bodyKey: "assets.empty.audio.body",
    icon: "audio-waveform",
  },
  text: {
    labelKey: "assets.tabs.text",
    titleKey: "assets.empty.text.title",
    bodyKey: "assets.empty.text.body",
    icon: "type",
  },
  stickers: {
    labelKey: "assets.tabs.stickers",
    titleKey: "assets.empty.stickers.title",
    bodyKey: "assets.empty.stickers.body",
    icon: "star",
  },
  effects: {
    labelKey: "assets.tabs.effects",
    titleKey: "assets.empty.effects.title",
    bodyKey: "assets.empty.effects.body",
    icon: "sliders-horizontal",
  },
  transitions: {
    labelKey: "assets.tabs.transitions",
    titleKey: "assets.empty.transitions.title",
    bodyKey: "assets.empty.transitions.body",
    icon: "layers",
  },
  filters: {
    labelKey: "assets.tabs.filters",
    titleKey: "assets.empty.filters.title",
    bodyKey: "assets.empty.filters.body",
    icon: "palette",
  },
  templates: {
    labelKey: "assets.tabs.templates",
    titleKey: "assets.empty.templates.title",
    bodyKey: "assets.empty.templates.body",
    icon: "layout-grid",
  },
  captions: {
    labelKey: "assets.tabs.captions",
    titleKey: "assets.empty.captions.title",
    bodyKey: "assets.empty.captions.body",
    icon: "captions",
  },
  "ai-studio": {
    labelKey: "assets.tabs.ai-studio",
    titleKey: "assets.empty.ai-studio.title",
    bodyKey: "assets.empty.ai-studio.body",
    icon: "sparkles",
  },
};

/** Left dock panel: the asset browser tabs (arch §16). Media is the bin (WP-1M.03); the rest are empty states. */
export function AssetBrowserPanel() {
  const { t } = useTranslation();
  const tab = useViewStore((s) => s.assetTab);
  const tabs: TabItem[] = ASSET_TABS.map((id) => ({
    id,
    label: t(ASSET_TAB_DEFS[id].labelKey),
    ...(id === "ai-studio" ? { icon: "sparkles" as const } : {}),
  }));
  return (
    <div className="cw-assets">
      <Tabs
        value={tab}
        onValueChange={(next) => viewStore.getState().setAssetTab(next as AssetTabId)}
      >
        <TabList tabs={tabs} aria-label={t("assets.tablistLabel")} />
        {ASSET_TABS.map((id) => {
          const def = ASSET_TAB_DEFS[id];
          if (id === "media") {
            return (
              <TabPanel key={id} value={id} className="cw-assets__panel cw-assets__panel--media">
                <MediaPanel />
              </TabPanel>
            );
          }
          return (
            <TabPanel key={id} value={id} className="cw-assets__panel">
              <EmptyState
                icon={def.icon}
                title={t(def.titleKey)}
                body={t(def.bodyKey)}
                tone={id === "ai-studio" ? "agent" : "neutral"}
              />
            </TabPanel>
          );
        })}
      </Tabs>
    </div>
  );
}
