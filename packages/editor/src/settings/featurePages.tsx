// SPDX-License-Identifier: GPL-3.0-or-later
// The pages of features that already exist (WP-1M.13): Export, which the Export dialog starts
// from, and Keyboard shortcuts, a read-only list of every key the app has. The shortcut rows are
// generated from `listBindings()` (shortcuts/bindings.ts), which reads the tables the handlers
// read, so this page cannot list a key that does nothing or miss one that does something.

import { EXPORT_FOLDER_CHOICES, EXPORT_HW_CHOICES, EXPORT_QUALITIES } from "@cutweave/preferences";
import { Fragment, type ReactNode } from "react";
import { type MenuPlatform, menuPlatform } from "../shell/appMenu.ts";
import {
  BINDING_GROUPS,
  type Binding,
  bindingKeysText,
  comboParts,
  distinctCombos,
  listBindings,
} from "../shortcuts/bindings.ts";
import {
  type ChoiceOption,
  registerSettingsPage,
  type SettingRowDef,
  type SettingsContext,
  type SettingsPageDef,
} from "./registry.ts";

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

// ---- Keyboard shortcuts -----------------------------------------------------------------------

/** The platform's own modifier names: `Ctrl` on Windows and Linux, `⌘` on macOS. */
function platform(): MenuPlatform {
  return menuPlatform(typeof navigator === "undefined" ? "" : navigator.platform);
}

/** The keys of one shortcut as key caps, alternatives joined with "or". */
function KeyCaps({
  binding,
  ctx,
  name,
}: {
  binding: Binding;
  ctx: SettingsContext;
  name: string;
}): ReactNode {
  const where = platform();
  const combos = distinctCombos(binding, where);
  return (
    // biome-ignore lint/a11y/useSemanticElements: a fieldset needs a visible legend; the name below says "Save: Ctrl+S"
    <span
      className="cw-settings__keys"
      role="group"
      aria-label={ctx.t("shortcuts.keysLabel", { name, keys: bindingKeysText(binding, where) })}
    >
      {combos.map((combo, index) => (
        <Fragment key={comboParts(combo, where).join("+")}>
          {index > 0 ? (
            <span className="caption cw-settings__or" aria-hidden="true">
              {ctx.t("shortcuts.or")}
            </span>
          ) : null}
          <span className="cw-settings__combo" aria-hidden="true">
            {comboParts(combo, where).map((part) => (
              <kbd key={part} className="cw-kbd numeric">
                {part}
              </kbd>
            ))}
          </span>
        </Fragment>
      ))}
    </span>
  );
}

const SECTION_OF = Object.fromEntries(BINDING_GROUPS.map((g) => [g.id, g.title] as const));

function shortcutRow(binding: Binding): SettingRowDef {
  return {
    id: `shortcuts.${binding.id}`,
    label: binding.label,
    description: binding.description,
    section: SECTION_OF[binding.group] as NonNullable<SettingRowDef["section"]>,
    // The keys are searchable too: "ctrl+s" finds Save.
    keywordsText: () => bindingKeysText(binding, platform()),
    noRowReset: true,
    control: {
      kind: "custom",
      render: (ctx) => <KeyCaps binding={binding} ctx={ctx} name={ctx.t(binding.label)} />,
    },
  };
}

const shortcutsPage: SettingsPageDef = {
  id: "shortcuts",
  title: "settings.pages.shortcuts",
  icon: "keyboard",
  group: "preferences",
  order: 100,
  note: "shortcuts.note",
  filterable: true,
  rows: listBindings(platform()).map(shortcutRow),
};

/** The pages of this work package. */
export const FEATURE_PAGES: readonly SettingsPageDef[] = [mediaPage, exportPage, shortcutsPage];

/** Registers them (idempotent). */
export function registerFeaturePages(): void {
  for (const page of FEATURE_PAGES) registerSettingsPage(page);
}
registerFeaturePages();
