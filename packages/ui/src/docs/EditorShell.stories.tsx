// SPDX-License-Identifier: GPL-3.0-or-later
// The editor's shell, built from the design system's own components: the top bar (which is the
// window's title bar), the media bin, the preview with its transport bar, the inspector and the
// track headers. It is the page the density and interface scale baselines are taken of
// (WP-2H.09, visual/density.spec.ts): every size here is a token, so `data-density` on the root
// and the browser's zoom are all that change between them.
import { useState } from "react";
import { Button } from "../components/Button.tsx";
import { FolderTile } from "../components/FolderTile.tsx";
import { IconButton, Toolbar, ToolbarSeparator } from "../components/IconButton.tsx";
import { InspectorRow, InspectorSection } from "../components/InspectorSection.tsx";
import { MenuBar, type MenuBarMenu } from "../components/MenuBar.tsx";
import { NumberScrubber } from "../components/NumberScrubber.tsx";
import { Panel } from "../components/Panel.tsx";
import { SearchField } from "../components/SearchField.tsx";
import { SegmentedControl } from "../components/SegmentedControl.tsx";
import { Slider } from "../components/Slider.tsx";
import { Switch } from "../components/Switch.tsx";
import { TabList, TabPanel, Tabs } from "../components/Tabs.tsx";

export default {
  title: "Foundations/Editor shell",
  parameters: { layout: "fullscreen" },
};

const noop = () => {};

const MENUS: MenuBarMenu[] = [
  { id: "file", label: "File", items: [{ id: "new", label: "New project…", onSelect: noop }] },
  { id: "edit", label: "Edit", items: [{ id: "undo", label: "Undo", onSelect: noop }] },
  {
    id: "view",
    label: "View",
    items: [{ id: "full", label: "Toggle full screen", onSelect: noop }],
  },
  { id: "help", label: "Help", items: [{ id: "about", label: "About Cutweave", onSelect: noop }] },
];

const app = {
  display: "grid",
  gridTemplateRows: "var(--toolbar) minmax(0, 1fr) auto",
  height: "100vh",
  background: "var(--surface-0)",
  color: "var(--text-primary)",
} as const;
const topBar = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  gap: "var(--space-3)",
  boxSizing: "border-box",
  height: "var(--toolbar)",
  padding: "0 var(--space-2)",
  borderBottom: "var(--border-thin) solid var(--border-subtle)",
} as const;
const group = { display: "flex", alignItems: "center", gap: "var(--space-2)" } as const;
const dock = {
  display: "grid",
  gridTemplateColumns: "var(--layer-width-sm) minmax(0, 1fr) var(--layer-width-sm)",
  gap: "var(--space-1)",
  minHeight: 0,
  padding: "var(--space-1)",
} as const;
const column = {
  display: "flex",
  flexDirection: "column",
  gap: "var(--space-3)",
  padding: "var(--space-3)",
} as const;
const tiles = {
  display: "grid",
  gridTemplateColumns: "repeat(2, minmax(0, 1fr))",
  gap: "var(--space-2)",
} as const;
const preview = {
  display: "flex",
  flexDirection: "column",
  minHeight: 0,
  background: "var(--surface-1)",
  borderRadius: "var(--radius-md)",
} as const;
const stage = {
  flex: 1,
  minHeight: 0,
  margin: "var(--space-4)",
  background: "var(--surface-preview)",
  border: "var(--border-thin) solid var(--border-default)",
  borderRadius: "var(--radius-sm)",
} as const;
const transport = {
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  gap: "var(--space-2)",
  padding: "var(--space-2)",
} as const;
const timeline = {
  display: "grid",
  gridTemplateColumns: "calc(4 * var(--space-12)) minmax(0, 1fr)",
  margin: "0 var(--space-1) var(--space-1)",
  background: "var(--surface-1)",
  borderRadius: "var(--radius-md)",
  overflow: "hidden",
} as const;
const ruler = {
  gridColumn: "1 / -1",
  display: "flex",
  alignItems: "center",
  boxSizing: "border-box",
  height: "var(--control-md)",
  padding: "0 var(--space-3)",
  color: "var(--text-muted)",
  borderBottom: "var(--border-thin) solid var(--border-subtle)",
} as const;
const header = (height: string) =>
  ({
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    boxSizing: "border-box",
    height: `calc(var(${height}) + 2 * var(--space-1))`,
    padding: "0 var(--space-2)",
    borderBottom: "var(--border-thin) solid var(--border-subtle)",
  }) as const;
const lane = (alt: boolean) =>
  ({
    display: "flex",
    alignItems: "center",
    padding: "var(--space-1) 0",
    background: alt ? "var(--timeline-lane-alt)" : "var(--timeline-lane)",
    borderBottom: "var(--border-thin) solid var(--border-subtle)",
  }) as const;
const clip = (family: "media" | "audio" | "captions", start: string, width: string) =>
  ({
    display: "flex",
    alignItems: "center",
    boxSizing: "border-box",
    height: "100%",
    width,
    marginInlineStart: start,
    padding: "0 var(--space-2)",
    overflow: "hidden",
    whiteSpace: "nowrap",
    background: `var(--clip-${family}-body)`,
    borderTop: `var(--clip-strip) solid var(--clip-${family})`,
    borderRadius: "var(--radius-sm)",
    color: "var(--text-primary)",
  }) as const;

const TRACKS = [
  { name: "Video V1", height: "--track-lg", family: "media", label: "interview_A.mov" },
  { name: "Audio A1", height: "--track-md", family: "audio", label: "music_bed.wav" },
  { name: "Captions", height: "--track-sm", family: "captions", label: "So here is the thing" },
] as const;

function Shell() {
  const [tab, setTab] = useState("media");
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState(true);
  const [scale, setScale] = useState(112);
  const [opacity, setOpacity] = useState(80);
  const [blend, setBlend] = useState("normal");
  const [flip, setFlip] = useState(false);
  return (
    <div style={app}>
      <header style={topBar}>
        <div style={group}>
          <MenuBar aria-label="Application menu" menus={MENUS} />
          <Toolbar aria-label="Project actions">
            <IconButton icon="undo-2" label="Undo" />
            <IconButton icon="redo-2" label="Redo" disabled />
            <ToolbarSeparator />
            <IconButton icon="save" label="Save" />
          </Toolbar>
        </div>
        <span className="label">Launch teaser</span>
        <div style={group}>
          <Button size="sm">Share</Button>
          <Button variant="primary">Export video</Button>
        </div>
      </header>
      <main style={dock}>
        <Tabs value={tab} onValueChange={setTab}>
          <Panel
            title="Assets"
            tabs={
              <TabList
                aria-label="Asset browser"
                tabs={[
                  { id: "media", label: "Media" },
                  { id: "audio", label: "Audio" },
                  { id: "text", label: "Text" },
                ]}
              />
            }
            actions={<IconButton size="sm" icon="plus" label="Import media" />}
          >
            <TabPanel value="media">
              <div style={column}>
                <SearchField
                  value={query}
                  onChange={setQuery}
                  label="Search media"
                  clearLabel="Clear search"
                />
                <div role="listbox" aria-label="Folders" style={tiles}>
                  <FolderTile name="Interviews" countLabel="12 items" active />
                  <FolderTile name="B-roll" countLabel="48 items" selected />
                </div>
              </div>
            </TabPanel>
          </Panel>
        </Tabs>
        <section aria-label="Preview" style={preview}>
          <div style={stage} />
          <div style={transport}>
            <IconButton icon="skip-back" label="Go to start" />
            <IconButton icon="play" label="Play" />
            <IconButton icon="skip-forward" label="Go to end" />
            <span className="timecode">00:00:12:05</span>
          </div>
        </section>
        <Panel title="Inspector" actions={<IconButton size="sm" icon="x" label="Close panel" />}>
          <InspectorSection label="Transform" expanded={open} onExpandedChange={setOpen}>
            <InspectorRow label="Scale">
              <NumberScrubber aria-label="Scale" value={scale} onChange={setScale} unit="%" />
            </InspectorRow>
            <InspectorRow label="Opacity">
              <Slider
                label="Opacity"
                labelHidden
                value={opacity}
                min={0}
                max={100}
                onChange={setOpacity}
              />
            </InspectorRow>
            <InspectorRow label="Blend">
              <SegmentedControl
                aria-label="Blend"
                value={blend}
                onChange={setBlend}
                options={[
                  { value: "normal", label: "Normal" },
                  { value: "screen", label: "Screen" },
                ]}
              />
            </InspectorRow>
            <InspectorRow label="Flip">
              <Switch aria-label="Flip" checked={flip} onCheckedChange={setFlip} />
            </InspectorRow>
          </InspectorSection>
        </Panel>
      </main>
      <section aria-label="Timeline" style={timeline}>
        <div className="numeric" style={ruler}>
          00:00:00:00
        </div>
        {TRACKS.map((track, index) => (
          <div key={track.name} style={{ display: "contents" }}>
            <div className="label" style={header(track.height)}>
              {track.name}
              <IconButton size="sm" icon="lock" label={`Lock ${track.name}`} />
            </div>
            <div style={{ ...lane(index % 2 === 1), height: `var(${track.height})` }}>
              <div className="caption" style={clip(track.family, "var(--space-6)", "40%")}>
                {track.label}
              </div>
            </div>
          </div>
        ))}
      </section>
    </div>
  );
}

/** The whole shell at rest. Density comes from the toolbar; the tests also take it at 125%. */
export const AtRest = { render: () => <Shell /> };
