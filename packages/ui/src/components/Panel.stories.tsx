// SPDX-License-Identifier: GPL-3.0-or-later
import { useState } from "react";
import { IconButton } from "./IconButton.tsx";
import { Panel, PanelDivider } from "./Panel.tsx";
import { TabList, Tabs } from "./Tabs.tsx";

export default { title: "Structure/Panel", component: Panel, tags: ["autodocs"] };

const col = {
  padding: "var(--space-4)",
  display: "flex",
  flexDirection: "column",
  gap: "var(--space-4)",
  background: "var(--surface-0)",
} as const;

const width = { width: "var(--layer-width-md)" } as const;

function Fact({ label, value }: { label: string; value: string }) {
  return (
    <div className="cw-row">
      <span className="cw-row__label label">{label}</span>
      <span className="cw-mono numeric">{value}</span>
    </div>
  );
}

function InspectorPanel() {
  const [tab, setTab] = useState("video");
  return (
    <Tabs value={tab} onValueChange={setTab}>
      <Panel
        title="Inspector"
        tabs={
          <TabList
            aria-label="Inspector groups"
            tabs={[
              { id: "video", label: "Video" },
              { id: "audio", label: "Audio" },
              { id: "speed", label: "Speed" },
            ]}
          />
        }
        actions={
          <>
            <IconButton size="sm" icon="search" label="Search properties" />
            <IconButton size="sm" icon="x" label="Close panel" />
          </>
        }
      >
        <Fact label="Clip" value="interview_A.mov" />
        <PanelDivider />
        <Fact label="Duration" value="00:00:12:05" />
        <Fact label="Frame rate" value="23.976 fps" />
      </Panel>
    </Tabs>
  );
}

export const WithTabsAndActions = {
  render: () => (
    <div style={col}>
      <div style={width}>
        <InspectorPanel />
      </div>
    </div>
  ),
};

export const Collapsed = {
  render: () => (
    <div style={col}>
      <div style={width}>
        <Panel
          title="Media bin"
          collapsed
          actions={<IconButton size="sm" icon="plus" label="Import media" />}
        />
      </div>
    </div>
  ),
};
