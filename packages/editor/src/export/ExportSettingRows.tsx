// SPDX-License-Identifier: GPL-3.0-or-later
// Settings > Export rows of WP-2G.04: the preset File > Export… starts at (the last one used, or
// a preset the engine lists) and the file name pattern (`{project}`, `{sequence}`, `{date}`,
// `{preset}`), with what the pattern makes today.

import { validatePreference } from "@cutweave/preferences";
import { TextField } from "@cutweave/ui";
import { useEffect, useState } from "react";
import type { SettingsContext } from "../settings/registry.ts";
import { dateToken, expandFileName, groupPresets } from "./exportModel.ts";
import { useExportStore } from "./exportStore.ts";
import { exportController } from "./useExport.ts";

const LAST_USED = "";

/** The default preset: "Last used", or one of the engine's presets, grouped as in the dialog. */
export function DefaultPresetControl({
  ctx,
  id,
}: {
  readonly ctx: SettingsContext;
  readonly id: string;
}) {
  const presets = useExportStore((s) => s.presets);
  useEffect(() => {
    void exportController().loadPresets();
  }, []);
  const value = ctx.values["export.preset"];
  const entries = presets.status === "ready" ? presets.value : [];
  // The grouping of a 16:9 sequence: the platforms, then General and yours.
  const groups = groupPresets(entries, { width: 0, height: 0 });
  const known = entries.some((e) => e.preset.id === value);
  return (
    <select
      id={id}
      className="cw-field body"
      aria-label={ctx.t("settings.rows.exportPreset.label")}
      value={value}
      onChange={(event) => void ctx.client.set("export.preset", event.target.value)}
    >
      <option value={LAST_USED}>{ctx.t("settings.rows.exportPreset.lastUsed")}</option>
      {value !== LAST_USED && !known ? <option value={value}>{value}</option> : null}
      {groups.map((group) => (
        <optgroup
          key={group.id}
          label={
            "name" in group.label ? group.label.name : ctx.t(group.label.key, group.label.vars)
          }
        >
          {group.options
            .filter((o) => o.supported)
            .map((o) => (
              <option key={o.id} value={o.id}>
                {o.name}
              </option>
            ))}
        </optgroup>
      ))}
    </select>
  );
}

/** The file name pattern, saved when it is valid, with the name it makes today. */
export function FileNameControl({
  ctx,
  id,
}: {
  readonly ctx: SettingsContext;
  readonly id: string;
}) {
  const stored = ctx.values["export.filename"];
  const [text, setText] = useState(stored);
  useEffect(() => setText(stored), [stored]);
  const isValid = (value: string): boolean =>
    validatePreference("export.filename", value).ok && value.trim() !== "";
  const valid = isValid(text);
  // A valid pattern is saved as it is typed; an invalid one stays in the field, saying why.
  const type = (value: string): void => {
    setText(value);
    if (isValid(value) && value !== stored) void ctx.client.set("export.filename", value);
  };
  const example = expandFileName(text, {
    project: ctx.t("settings.rows.exportFileName.project"),
    sequence: ctx.t("settings.rows.exportFileName.sequence"),
    preset: "TikTok",
    date: dateToken(new Date()),
  });
  return (
    <span className="cw-settings__filename">
      <TextField
        id={id}
        value={text}
        onChange={type}
        aria-label={ctx.t("settings.rows.exportFileName.label")}
        invalid={!valid}
        aria-describedby={`${id}-example`}
      />
      <span id={`${id}-example`} className="caption cw-settings__status">
        {valid
          ? ctx.t("settings.rows.exportFileName.example", { name: `${example}.mp4` })
          : ctx.t("settings.rows.exportFileName.invalid")}
      </span>
    </span>
  );
}
