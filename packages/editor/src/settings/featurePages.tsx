// SPDX-License-Identifier: GPL-3.0-or-later
// The pages of features that already exist (WP-1M.13): Export, which the Export dialog starts
// from, and Media and import. Keyboard shortcuts (WP-2H.05) is `shortcuts.tsx`.

import { EXPORT_FOLDER_CHOICES, EXPORT_HW_CHOICES, EXPORT_QUALITIES } from "@cutweave/preferences";
import { type ChoiceOption, registerSettingsPage, type SettingsPageDef } from "./registry.ts";
import { registerShortcutsPage } from "./shortcuts.tsx";

// ---- Export -----------------------------------------------------------------------------------

const FOLDER_LABELS = {
  project: "settings.rows.exportFolder.project",
  last: "settings.rows.exportFolder.last",
  chosen: "settings.rows.exportFolder.chosen",
} as const;
const QUALITY_LABELS = {
  low: "settings.rows.exportQuality.low",
  medium: "settings.rows.exportQuality.medium",
  high: "settings.rows.exportQuality.high",
} as const;
const HW_LABELS = {
  auto: "settings.rows.exportHw.auto",
  off: "settings.rows.exportHw.off",
} as const;

const choices = <T extends string>(
  values: readonly T[],
  labels: Readonly<Record<T, NonNullable<ChoiceOption["label"]>>>,
): readonly ChoiceOption[] => values.map((value) => ({ value, label: labels[value] }));

const exportPage: SettingsPageDef = {
  id: "export",
  title: "settings.pages.export",
  icon: "download",
  group: "preferences",
  order: 80,
  rows: [
    {
      id: "export.folder",
      key: "export.folder",
      label: "settings.rows.exportFolder.label",
      description: "settings.rows.exportFolder.description",
      keywords: "settings.rows.exportFolder.keywords",
      control: { kind: "select", options: choices(EXPORT_FOLDER_CHOICES, FOLDER_LABELS) },
    },
    {
      id: "export.folder_path",
      key: "export.folder_path",
      label: "settings.rows.exportFolderPath.label",
      description: "settings.rows.exportFolderPath.description",
      keywords: "settings.rows.exportFolderPath.keywords",
      control: { kind: "folder", choose: "settings.rows.exportFolderPath.choose" },
      disabledWhen: (values) => values["export.folder"] !== "chosen",
    },
    {
      id: "export.last_folder",
      key: "export.last_folder",
      label: "settings.rows.exportLastFolder.label",
      description: "settings.rows.exportLastFolder.description",
      keywords: "settings.rows.exportLastFolder.keywords",
      noRowReset: true,
      control: {
        kind: "action",
        label: "settings.rows.exportLastFolder.button",
        run: (ctx) => ctx.client.reset(["export.last_folder"]),
        disabled: (ctx) => ctx.values["export.last_folder"] === "",
        status: (ctx) =>
          ctx.values["export.last_folder"] === ""
            ? ctx.t("settings.rows.exportLastFolder.none")
            : ctx.values["export.last_folder"],
      },
    },
    {
      id: "export.quality",
      key: "export.quality",
      label: "settings.rows.exportQuality.label",
      description: "settings.rows.exportQuality.description",
      keywords: "settings.rows.exportQuality.keywords",
      control: { kind: "segmented", options: choices(EXPORT_QUALITIES, QUALITY_LABELS) },
    },
    {
      id: "export.hw",
      key: "export.hw",
      label: "settings.rows.exportHw.label",
      description: "settings.rows.exportHw.description",
      keywords: "settings.rows.exportHw.keywords",
      control: { kind: "segmented", options: choices(EXPORT_HW_CHOICES, HW_LABELS) },
    },
  ],
};

// ---- Media and import (WP-1M.14) --------------------------------------------------------------

const mediaPage: SettingsPageDef = {
  id: "media",
  title: "settings.pages.media",
  icon: "film",
  group: "preferences",
  order: 70,
  rows: [
    {
      id: "media.hover_scrub",
      key: "media.hover_scrub",
      label: "settings.rows.hoverScrub.label",
      description: "settings.rows.hoverScrub.description",
      keywords: "settings.rows.hoverScrub.keywords",
      control: { kind: "switch" },
    },
  ],
};

/** The pages of this work package. */
export const FEATURE_PAGES: readonly SettingsPageDef[] = [mediaPage, exportPage];

/** Registers them (idempotent). */
export function registerFeaturePages(): void {
  for (const page of FEATURE_PAGES) registerSettingsPage(page);
  registerShortcutsPage();
}
registerFeaturePages();
