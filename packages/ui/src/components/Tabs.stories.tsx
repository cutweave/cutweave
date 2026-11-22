// SPDX-License-Identifier: GPL-3.0-or-later
import { useState } from "react";
import { type TabItem, TabList, TabPanel, Tabs } from "./Tabs.tsx";

export default { title: "Structure/Tabs", component: Tabs, tags: ["autodocs"] };

const ASSET_TABS: TabItem[] = [
  { id: "media", label: "Media" },
  { id: "audio", label: "Audio" },
  { id: "text", label: "Text" },
  { id: "stickers", label: "Stickers" },
  { id: "effects", label: "Effects" },
  { id: "transitions", label: "Transitions" },
  { id: "captions", label: "Captions" },
  { id: "ai", label: "AI Studio", icon: "sparkles" },
];

const col = {
  padding: "var(--space-4)",
  display: "flex",
  flexDirection: "column",
  gap: "var(--space-4)",
  background: "var(--surface-1)",
} as const;

function AssetBrowser() {
  const [tab, setTab] = useState("media");
  return (
    <div style={{ width: "var(--layer-width-lg)" }}>
      <Tabs value={tab} onValueChange={setTab}>
        <TabList tabs={ASSET_TABS} aria-label="Asset browser" />
        {ASSET_TABS.map((t) => (
          <TabPanel key={t.id} value={t.id}>
            <p className="body" style={{ margin: 0, padding: "var(--space-3)" }}>
              {t.label} panel
            </p>
          </TabPanel>
        ))}
      </Tabs>
    </div>
  );
}

const WITH_DISABLED: TabItem[] = [
  { id: "text", label: "Text" },
  { id: "captions", label: "Captions" },
  { id: "stickers", label: "Stickers", disabled: true },
];

function CaptionsSelected() {
  const [tab, setTab] = useState("captions");
  return (
    <div style={{ width: "var(--layer-width-md)" }}>
      <Tabs value={tab} onValueChange={setTab}>
        <TabList tabs={WITH_DISABLED} aria-label="Asset browser, Captions selected" />
        {WITH_DISABLED.map((t) => (
          <TabPanel key={t.id} value={t.id}>
            <p className="body" style={{ margin: 0, padding: "var(--space-3)" }}>
              {t.label} panel
            </p>
          </TabPanel>
        ))}
      </Tabs>
    </div>
  );
}

export const AssetBrowserTabs = {
  render: () => (
    <div style={col}>
      <AssetBrowser />
    </div>
  ),
};

export const WithDisabledTab = {
  render: () => (
    <div style={col}>
      <CaptionsSelected />
    </div>
  ),
};
