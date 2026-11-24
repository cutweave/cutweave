// SPDX-License-Identifier: GPL-3.0-or-later
import { type ReactNode, useState } from "react";
import { Button } from "./Button.tsx";
import { Dialog } from "./Dialog.tsx";
import { SearchField } from "./SearchField.tsx";
import { SegmentedControl } from "./SegmentedControl.tsx";
import { SettingRow } from "./SettingRow.tsx";
import { SettingsLayout, type SettingsPageGroup, SettingsResultGroup } from "./SettingsLayout.tsx";
import { Switch } from "./Switch.tsx";
import { TextField } from "./TextField.tsx";
import { TooltipProvider } from "./Tooltip.tsx";

export default {
  title: "Overlays/SettingsLayout",
  component: SettingsLayout,
  tags: ["autodocs"],
  parameters: { layout: "fullscreen" },
};

const GROUPS: SettingsPageGroup[] = [
  {
    id: "preferences",
    label: "Preferences",
    pages: [
      { id: "general", label: "General", icon: "settings" },
      { id: "appearance", label: "Appearance", icon: "palette" },
      { id: "saving", label: "Saving and backups", icon: "history" },
    ],
  },
  { id: "other", label: "Advanced", pages: [{ id: "advanced", label: "Advanced" }] },
];

const mark = (text: string, word: string): ReactNode => {
  const at = text.toLowerCase().indexOf(word.toLowerCase());
  if (at < 0) return text;
  return (
    <>
      {text.slice(0, at)}
      <mark>{text.slice(at, at + word.length)}</mark>
      {text.slice(at + word.length)}
    </>
  );
};

function GeneralRows() {
  const [startup, setStartup] = useState("empty");
  const [recent, setRecent] = useState("10");
  return (
    <>
      <SettingRow
        id="startup"
        label="On startup"
        description="What Cutweave opens when it starts."
        modified={startup !== "empty"}
        onReset={() => setStartup("empty")}
      >
        <SegmentedControl
          aria-label="On startup"
          value={startup}
          onChange={setStartup}
          options={[
            { value: "empty", label: "Empty project" },
            { value: "last", label: "Last project" },
          ]}
        />
      </SettingRow>
      <SettingRow
        id="folder"
        label="Folder for new projects"
        description="New project dialogs start here."
      >
        <TextField
          aria-label="Folder for new projects"
          value="C:\Users\You\Documents\Cutweave"
          readOnly
          onChange={() => {}}
        />
        <Button size="sm">Choose…</Button>
      </SettingRow>
      <SettingRow
        id="recent"
        label="Recent projects to list"
        description="How many projects File > Open recent shows."
        htmlFor="recent-count"
        modified={recent !== "10"}
        onReset={() => setRecent("10")}
      >
        <TextField id="recent-count" type="number" numeric value={recent} onChange={setRecent} />
        <span className="caption">projects</span>
      </SettingRow>
      <SettingRow
        id="clear"
        label="Recent projects"
        description="Forget the projects listed in File > Open recent."
      >
        <Button size="sm">Clear recent projects</Button>
      </SettingRow>
    </>
  );
}

function Frame({ initialQuery = "", page = "general" }: { initialQuery?: string; page?: string }) {
  const [query, setQuery] = useState(initialQuery);
  const [active, setActive] = useState(page);
  const [autosave, setAutosave] = useState(false);
  const searching = query.trim() !== "";
  const results = query.toLowerCase().includes("auto");
  return (
    <TooltipProvider>
      <div style={{ minHeight: "100vh", background: "var(--surface-1)" }}>
        <Dialog
          open
          onOpenChange={() => {}}
          title="Settings"
          className="cw-dialog--settings"
          footer={
            <Button variant="primary" size="lg">
              Done
            </Button>
          }
        >
          <SettingsLayout
            navLabel="Settings pages"
            search={
              <SearchField
                value={query}
                onChange={setQuery}
                label="Search settings"
                clearLabel="Clear search"
                count={searching ? (results ? "1 result" : "0 results") : undefined}
              />
            }
            groups={GROUPS}
            activeId={searching ? null : active}
            onSelect={setActive}
            heading={
              searching
                ? `Results for “${query}”`
                : (GROUPS.flatMap((g) => g.pages).find((p) => p.id === active)?.label ?? "")
            }
            actions={
              searching ? undefined : (
                <Button variant="ghost" size="sm">
                  Reset page
                </Button>
              )
            }
            resultsLabel="Search results"
          >
            {searching ? (
              results ? (
                <SettingsResultGroup label="Saving and backups">
                  <SettingRow
                    id="autosave"
                    label="Save automatically"
                    labelContent={mark("Save automatically", "auto")}
                    description="Saves your project while you edit, and keeps backups."
                    descriptionContent={mark(
                      "Saves your project while you edit, and keeps backups.",
                      "auto",
                    )}
                    htmlFor="autosave-switch"
                    modified={!autosave}
                    onReset={() => setAutosave(true)}
                  >
                    <Switch id="autosave-switch" checked={autosave} onCheckedChange={setAutosave} />
                  </SettingRow>
                </SettingsResultGroup>
              ) : (
                <p className="cw-settings__empty body">No settings match “{query}”</p>
              )
            ) : active === "general" ? (
              <GeneralRows />
            ) : (
              <p className="cw-settings__empty body">This page's rows go here.</p>
            )}
          </SettingsLayout>
        </Dialog>
      </div>
    </TooltipProvider>
  );
}

/** The General page, with a changed value showing its reset button. */
export const GeneralPage = { render: () => <Frame /> };

/** A search across every page: the matching rows under their page, the match marked. */
export const SearchResult = { render: () => <Frame initialQuery="auto" /> };

/** A search with no match says so. */
export const EmptySearch = { render: () => <Frame initialQuery="zzz" /> };

/** One row in each state: default, modified, with a condition, disabled and as a deep link target. */
export const RowStates = {
  render: () => (
    <TooltipProvider>
      <div
        style={{
          padding: "var(--space-4)",
          width: "var(--layer-width-lg)",
          background: "var(--surface-2)",
        }}
      >
        <SettingRow id="a" label="Default" description="The value is the default.">
          <Switch aria-label="Default" checked onCheckedChange={() => {}} />
        </SettingRow>
        <SettingRow
          id="b"
          label="Modified"
          description="The value differs, so the reset button shows."
          modified
          onReset={() => {}}
        >
          <Switch aria-label="Modified" checked={false} onCheckedChange={() => {}} />
        </SettingRow>
        <SettingRow
          id="c"
          label="Needs the engine"
          description="Applied by the engine, so it waits for one."
          note="Needs the engine"
          disabled
        >
          <Switch aria-label="Needs the engine" checked disabled onCheckedChange={() => {}} />
        </SettingRow>
        <SettingRow
          id="d"
          label="Deep link target"
          description="Opened from the save status."
          highlighted
        >
          <Switch aria-label="Deep link target" checked onCheckedChange={() => {}} />
        </SettingRow>
      </div>
    </TooltipProvider>
  ),
};
