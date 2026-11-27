// SPDX-License-Identifier: GPL-3.0-or-later
// The pages of features that already exist (WP-1M.13): Export, which the Export dialog starts
// from, and Media and import. Keyboard shortcuts (WP-2H.05) is `shortcuts.tsx`. Editing
// (WP-2F.05b) and Playback (WP-2H.10) are here too.

import {
  EXPORT_FOLDER_CHOICES,
  EXPORT_HW_CHOICES,
  EXPORT_QUALITIES,
  IMAGE_SEQUENCE_FPS_CHOICES,
  PROXY_POLICIES,
  PROXY_SIZES,
  STILL_DURATION_MAX,
} from "@cutweave/preferences";
import { DefaultPresetControl, FileNameControl } from "../export/ExportSettingRows.tsx";
import { formatWords, parseFormatPreference } from "../project/newSequenceFormat.ts";
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

const PROXY_POLICY_LABELS = {
  off: "settings.rows.proxyPolicy.off",
  auto: "settings.rows.proxyPolicy.auto",
  all: "settings.rows.proxyPolicy.all",
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
    // WP-2G.04: the preset and the file name the dialog starts at.
    {
      id: "export.preset",
      key: "export.preset",
      label: "settings.rows.exportPreset.label",
      description: "settings.rows.exportPreset.description",
      keywords: "settings.rows.exportPreset.keywords",
      control: {
        kind: "custom",
        render: (ctx, id) => <DefaultPresetControl ctx={ctx} id={id} />,
      },
    },
    {
      id: "export.last_preset",
      key: "export.last_preset",
      label: "settings.rows.exportLastPreset.label",
      description: "settings.rows.exportLastPreset.description",
      keywords: "settings.rows.exportLastPreset.keywords",
      noRowReset: true,
      control: {
        kind: "action",
        label: "settings.rows.exportLastPreset.button",
        run: (ctx) => ctx.client.reset(["export.last_preset"]),
        disabled: (ctx) => ctx.values["export.last_preset"] === "",
        status: (ctx) =>
          ctx.values["export.last_preset"] === ""
            ? ctx.t("settings.rows.exportLastPreset.none")
            : ctx.values["export.last_preset"],
      },
    },
    {
      id: "export.filename",
      key: "export.filename",
      label: "settings.rows.exportFileName.label",
      description: "settings.rows.exportFileName.description",
      keywords: "settings.rows.exportFileName.keywords",
      control: { kind: "custom", render: (ctx, id) => <FileNameControl ctx={ctx} id={id} /> },
    },
  ],
};

// ---- Editing (WP-2F.05b; more rows come with WP-2A.13) ------------------------------------------

const editingPage: SettingsPageDef = {
  id: "editing",
  title: "settings.pages.editing",
  icon: "scissors",
  group: "preferences",
  order: 30,
  rows: [
    {
      id: "editing.still_duration",
      key: "editing.still_duration",
      label: "settings.rows.stillDuration.label",
      description: "settings.rows.stillDuration.description",
      keywords: "settings.rows.stillDuration.keywords",
      control: {
        kind: "number",
        min: 1,
        max: STILL_DURATION_MAX,
        unit: "settings.rows.stillDuration.unit",
      },
    },
    // The format new projects start with (#221): File > Project settings… > "Use as defaults
    // for new projects" sets it; this row says what it is and puts Cutweave's own back.
    {
      id: "editing.sequence_format",
      key: "editing.sequence_format",
      label: "settings.rows.sequenceFormat.label",
      description: "settings.rows.sequenceFormat.description",
      keywords: "settings.rows.sequenceFormat.keywords",
      noRowReset: true,
      control: {
        kind: "action",
        label: "settings.rows.sequenceFormat.button",
        run: (ctx) => ctx.client.reset(["editing.sequence_format"]),
        disabled: (ctx) => ctx.values["editing.sequence_format"] === "",
        status: (ctx) => {
          const format = parseFormatPreference(ctx.values["editing.sequence_format"]);
          return format === null
            ? ctx.t("settings.rows.sequenceFormat.none")
            : formatWords(format, ctx.t);
        },
      },
    },
  ],
};

// ---- Playback (WP-2H.10) ------------------------------------------------------------------------
// The rows the engine applies (preview quality, proxies, scrub audio, loop) join the page with
// the work that gives the engine each behaviour; see docs/reference/settings.md.

const playbackPage: SettingsPageDef = {
  id: "playback",
  title: "settings.pages.playback",
  icon: "play",
  group: "preferences",
  order: 50,
  rows: [
    {
      id: "playback.return_to_start",
      key: "playback.return_to_start",
      label: "settings.rows.returnToStart.label",
      description: "settings.rows.returnToStart.description",
      keywords: "settings.rows.returnToStart.keywords",
      control: { kind: "switch" },
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
    {
      id: "media.keep_structure",
      key: "media.keep_structure",
      label: "settings.rows.keepStructure.label",
      description: "settings.rows.keepStructure.description",
      keywords: "settings.rows.keepStructure.keywords",
      control: { kind: "switch" },
    },
    {
      id: "media.copy_on_import",
      key: "media.copy_on_import",
      label: "settings.rows.copyOnImport.label",
      description: "settings.rows.copyOnImport.description",
      keywords: "settings.rows.copyOnImport.keywords",
      control: { kind: "switch" },
    },
    {
      id: "media.image_sequence_fps",
      key: "media.image_sequence_fps",
      label: "settings.rows.imageSequenceFps.label",
      description: "settings.rows.imageSequenceFps.description",
      keywords: "settings.rows.imageSequenceFps.keywords",
      control: {
        kind: "select",
        // Frame rates are numbers: only "the sequence's frame rate" is a phrase to translate.
        options: IMAGE_SEQUENCE_FPS_CHOICES.map((value) =>
          value === "sequence"
            ? { value, label: "settings.rows.imageSequenceFps.sequence" as const }
            : { value, text: `${value} fps` },
        ),
      },
    },
    // Proxies (WP-2F.03b, ADR-0027): the policy new projects start with, and this computer's
    // proxy size (applied by the engine).
    {
      id: "media.proxy_policy",
      key: "media.proxy_policy",
      label: "settings.rows.proxyPolicy.label",
      description: "settings.rows.proxyPolicy.description",
      keywords: "settings.rows.proxyPolicy.keywords",
      control: { kind: "segmented", options: choices(PROXY_POLICIES, PROXY_POLICY_LABELS) },
    },
    {
      id: "media.proxy_size",
      key: "media.proxy_size",
      label: "settings.rows.proxySize.label",
      description: "settings.rows.proxySize.description",
      keywords: "settings.rows.proxySize.keywords",
      // "960p" is a size, not a phrase: nothing to translate.
      control: {
        kind: "select",
        options: PROXY_SIZES.map((value) => ({ value, text: `${value}p` })),
      },
      applies: "engine",
    },
  ],
};

/** The pages of this work package. */
export const FEATURE_PAGES: readonly SettingsPageDef[] = [
  editingPage,
  playbackPage,
  mediaPage,
  exportPage,
];

/** Registers them (idempotent). */
export function registerFeaturePages(): void {
  for (const page of FEATURE_PAGES) registerSettingsPage(page);
  registerShortcutsPage();
}
registerFeaturePages();
