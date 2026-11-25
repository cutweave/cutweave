// SPDX-License-Identifier: GPL-3.0-or-later
import { SettingRow } from "./SettingRow.tsx";
import { Switch } from "./Switch.tsx";

export default { title: "Structure/SettingRow", component: SettingRow, tags: ["autodocs"] };

const noop = () => {};

const column = {
  padding: "var(--space-4)",
  display: "flex",
  flexDirection: "column",
  gap: "var(--space-2)",
  background: "var(--surface-1)",
} as const;

/** The Settings dialog's row: label, description and control; a changed value can be reset. */
export const States = {
  render: () => (
    <div style={column}>
      <SettingRow
        id="hover-scrub"
        label="Hover scrub"
        description="Move the pointer across a video card to see its frames."
        htmlFor="hover-scrub-switch"
      >
        <Switch id="hover-scrub-switch" checked onCheckedChange={noop} />
      </SettingRow>
      <SettingRow
        id="autosave"
        label="Autosave"
        description="Save edits to the project file as you work."
        htmlFor="autosave-switch"
        modified
        onReset={noop}
        resetLabel="Reset Autosave to its default"
      >
        <Switch id="autosave-switch" checked={false} onCheckedChange={noop} />
      </SettingRow>
      <SettingRow
        id="hw-decode"
        label="Hardware decoding"
        description="Decode video on the graphics card when it can."
        note="Not available on this machine."
        htmlFor="hw-decode-switch"
        disabled
      >
        <Switch id="hw-decode-switch" checked={false} onCheckedChange={noop} disabled />
      </SettingRow>
      <SettingRow
        id="theme"
        label="Theme"
        description="A search match is highlighted."
        htmlFor="theme-switch"
        highlighted
      >
        <Switch id="theme-switch" checked onCheckedChange={noop} />
      </SettingRow>
    </div>
  ),
};
