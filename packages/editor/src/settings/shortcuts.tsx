// SPDX-License-Identifier: GPL-3.0-or-later
// Settings > Keyboard shortcuts (WP-1M.13 listed the keys; WP-2H.05 makes them editable): a preset
// switcher, "Reset all shortcuts", export and import of the shortcuts as a file, and one row per
// command with its keys, a Record button, and a menu (`shortcuts.row`). A row is made from the
// command table (`keymap/commands.ts`), the engine catalog's commands a key can run, and the
// workspace commands, which are the palette's list: every command the palette lists is bindable.
//
// Storing only the differences: a row never writes a key list of its own, it asks the keymap model
// for the result and stores what differs from the preset (`shortcuts.bindings`).

import { SHORTCUT_PRESETS, type ShortcutOverrides } from "@cutweave/preferences";
import { Button, IconButton, Tooltip } from "@cutweave/ui";
import { Fragment, type ReactNode, useEffect, useRef, useState } from "react";
import { i18n, type TKey } from "../i18n/index.ts";
import { KEY_COMMANDS } from "../keymap/commands.ts";
import {
  bindingId,
  type Conflict,
  type KeymapModel,
  overridesFor,
  withKey,
  withoutKey,
  withPresetKeys,
} from "../keymap/model.ts";
import { reservedReason } from "../keymap/reserved.ts";
import { comboText, keymapModel } from "../keymap/store.ts";
import {
  type KeyCombo,
  type KeyPlatform,
  parseKeys,
  spell,
  strokeOfEvent,
  strokeParts,
} from "../keymap/stroke.ts";
import { menuServices } from "../menus/defaults.ts";
import { registerContextSurface } from "../menus/host.tsx";
import { needsArguments } from "../palette/argForm.ts";
import { type CatalogCommand, commandCatalog, listedCatalog } from "../palette/catalog.ts";
import { preferences } from "../prefs/prefsStore.ts";
import { menuPlatform } from "../shell/appMenu.ts";
import { BINDING_GROUPS } from "../shortcuts/bindings.ts";
import { WORKSPACES } from "../state/viewStore.ts";
import { panelRegistry, registeredPanels } from "../workspace/panelRegistry.ts";
import {
  type ChoiceOption,
  registerSettingRow,
  registerSettingsPage,
  type SettingRowDef,
  type SettingsContext,
  type SettingsPageDef,
} from "./registry.ts";
import { registerShortcutRowMenu } from "./shortcutRowMenu.ts";

const platform = (): KeyPlatform =>
  menuPlatform(typeof navigator === "undefined" ? "" : navigator.platform);

// ---- the names commands go by -----------------------------------------------------------------

interface Title {
  readonly label?: TKey;
  readonly text?: string;
}

/** What each row's command is called, by binding id (conflict prompts name the other one). */
const TITLES = new Map<string, Title>();

/** A command's name as a person reads it. */
export function titleOf(id: string, t: (key: TKey) => string): string {
  const known = TITLES.get(id);
  if (known?.text !== undefined) return known.text;
  if (known?.label !== undefined) return t(known.label);
  return id;
}

const SECTION_OF = Object.fromEntries(BINDING_GROUPS.map((g) => [g.id, g.title] as const));

// ---- the row's cell ---------------------------------------------------------------------------

type Mode =
  | { readonly kind: "idle" }
  | { readonly kind: "recording" }
  | { readonly kind: "conflict"; readonly keys: string; readonly conflicts: readonly Conflict[] }
  | { readonly kind: "refused"; readonly keys: string; readonly reason: TKey };

function KeyCaps({
  combos,
  name,
  ctx,
  onRemove,
}: {
  combos: readonly KeyCombo[];
  name: string;
  ctx: SettingsContext;
  onRemove: ((combo: KeyCombo) => void) | undefined;
}): ReactNode {
  const where = platform();
  const text = combos.map((c) => comboText(c, where)).join(", ");
  return (
    // biome-ignore lint/a11y/useSemanticElements: a fieldset needs a visible legend; the name below says "Save: Ctrl+S"
    <span
      className="cw-settings__keys"
      role="group"
      aria-label={
        combos.length === 0
          ? ctx.t("shortcuts.keysLabel", { name, keys: ctx.t("shortcuts.cell.none") })
          : ctx.t("shortcuts.keysLabel", { name, keys: text })
      }
    >
      {combos.length === 0 ? <span className="cw-settings__keyslot" aria-hidden="true" /> : null}
      {combos.map((combo, index) => {
        const label = comboText(combo, where);
        return (
          <Fragment key={label}>
            {index > 0 ? (
              <span className="caption cw-settings__or" aria-hidden="true">
                {ctx.t("shortcuts.or")}
              </span>
            ) : null}
            <span className="cw-settings__combo">
              <span className="cw-settings__combo" aria-hidden="true">
                {strokeParts(combo, where).map((part) => (
                  <kbd key={part} className="cw-kbd numeric">
                    {part}
                  </kbd>
                ))}
                {combo.next === undefined
                  ? null
                  : strokeParts(combo.next, where).map((part) => (
                      <kbd key={`n-${part}`} className="cw-kbd numeric">
                        {part}
                      </kbd>
                    ))}
              </span>
              {onRemove === undefined ? null : (
                <Tooltip label={ctx.t("shortcuts.cell.removeTip")}>
                  <IconButton
                    size="sm"
                    icon="x"
                    label={ctx.t("shortcuts.cell.remove", { keys: label, name })}
                    onClick={() => onRemove(combo)}
                  />
                </Tooltip>
              )}
            </span>
          </Fragment>
        );
      })}
    </span>
  );
}

function ShortcutCell({
  id,
  name,
  fixed,
  ctx,
}: {
  id: string;
  name: string;
  fixed: boolean;
  ctx: SettingsContext;
}): ReactNode {
  const model = keymapModel();
  const [mode, setMode] = useState<Mode>({ kind: "idle" });
  const [said, setSaid] = useState("");
  const anchor = useRef<HTMLSpanElement>(null);
  const keys = model.keysOf(id);
  const changed = model.isChanged(id);

  const store = (wanted: Readonly<Record<string, readonly string[]>>): void => {
    void ctx.client.set("shortcuts.bindings", overridesFor(model, wanted)).catch(() => undefined);
  };

  const assign = (spelled: string, reassign: readonly Conflict[]): void => {
    store(withKey(model, model.effective(), id, spelled, reassign));
    setMode({ kind: "idle" });
    const combo = parseKeys(spelled, model.platform);
    setSaid(
      ctx.t("shortcuts.cell.assigned", {
        keys: combo === null ? spelled : comboText(combo, model.platform),
        name,
      }),
    );
  };

  // The row's own context menu (`shortcuts.row`) acts on this row's binding.
  useEffect(() => {
    const row = anchor.current?.closest<HTMLElement>("[data-setting]");
    if (row === null || row === undefined) return;
    return registerContextSurface(
      row,
      () => ({
        surface: "shortcuts.row",
        target: { ids: [], name, data: { id, changed, hasKeys: keys.length > 0 } },
      }),
      { name: "shortcuts.row" },
    );
  }, [id, name, changed, keys.length]);

  // While recording, the next key press is the shortcut: nothing else sees it.
  const recording = mode.kind === "recording";
  useEffect(() => {
    if (!recording) return;
    const onKey = (event: KeyboardEvent): void => {
      if (event.isComposing || event.keyCode === 229) return;
      const stroke = strokeOfEvent(event, model.platform);
      event.preventDefault();
      event.stopPropagation();
      if (stroke === null) return; // a modifier on its own: keep waiting
      if (stroke.key === "Escape" && stroke.mod !== true && stroke.alt !== true) {
        setMode({ kind: "idle" });
        return;
      }
      const spelled = spell(stroke);
      const reason = reservedReason(stroke, model.platform);
      if (reason !== null) {
        setMode({ kind: "refused", keys: comboText(stroke, model.platform), reason });
        return;
      }
      const conflicts = model.conflicts(id, spelled);
      if (conflicts.length > 0) setMode({ kind: "conflict", keys: spelled, conflicts });
      else assign(spelled, []);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [recording, id, model]);

  const caps = (
    <KeyCaps
      combos={keys}
      name={name}
      ctx={ctx}
      onRemove={
        fixed ? undefined : (combo) => store(withoutKey(model.effective(), id, spell(combo)))
      }
    />
  );
  const unverified = model.isUnverified(id);
  return (
    <span className="cw-shortcut" ref={anchor}>
      <span className="cw-shortcut__line">
        {caps}
        {unverified ? <span className="caption">{ctx.t("shortcuts.cell.unverified")}</span> : null}
        {fixed ? <span className="caption">{ctx.t("shortcuts.cell.fixed")}</span> : null}
        {changed && !fixed ? (
          <Tooltip label={ctx.t("shortcuts.cell.resetTip")}>
            <IconButton
              size="sm"
              icon="undo-2"
              label={ctx.t("shortcuts.cell.reset", { name })}
              onClick={() => store(withPresetKeys(model, model.effective(), id))}
            />
          </Tooltip>
        ) : null}
        {fixed ? null : (
          <>
            {recording ? (
              <span className="caption">{ctx.t("shortcuts.cell.recording")}</span>
            ) : null}
            <Tooltip label={ctx.t("shortcuts.cell.add")}>
              <IconButton
                size="sm"
                icon="plus"
                label={ctx.t("shortcuts.cell.addLabel", { name })}
                pressed={recording}
                onClick={() => setMode(recording ? { kind: "idle" } : { kind: "recording" })}
              />
            </Tooltip>
          </>
        )}
      </span>
      {mode.kind === "conflict" ? (
        <span
          className="cw-shortcut__prompt"
          role="alertdialog"
          aria-label={ctx.t("shortcuts.cell.conflictTitle")}
        >
          <span className="body">
            {ctx.t("shortcuts.cell.conflict", {
              keys: comboText(
                parseKeys(mode.keys, model.platform) ?? { key: mode.keys },
                model.platform,
              ),
              name: mode.conflicts.map((c) => titleOf(c.binding.id, ctx.t)).join(", "),
            })}
          </span>
          <Button size="sm" variant="primary" onClick={() => assign(mode.keys, mode.conflicts)}>
            {ctx.t("shortcuts.cell.reassign")}
          </Button>
          <Button size="sm" onClick={() => setMode({ kind: "idle" })}>
            {ctx.t("shortcuts.cell.cancel")}
          </Button>
        </span>
      ) : null}
      {mode.kind === "refused" ? (
        <span className="caption cw-shortcut__refused" role="alert">
          {ctx.t("shortcuts.cell.refused", { keys: mode.keys, reason: ctx.t(mode.reason) })}
        </span>
      ) : null}
      <span className="caption" role="status">
        {said}
      </span>
    </span>
  );
}

// ---- rows -------------------------------------------------------------------------------------

function keysText(id: string): string {
  const where = platform();
  return keymapModel()
    .keysOf(id)
    .map((c) => comboText(c, where))
    .join(", ");
}

function commandRow(
  id: string,
  title: Title,
  description: { label?: TKey; text?: string },
  section: TKey,
  fixed: boolean,
): SettingRowDef {
  TITLES.set(id, title);
  const nameFor = (t: (key: TKey) => string): string =>
    title.text ?? (title.label === undefined ? id : t(title.label));
  return {
    id: `shortcuts.${id}`,
    label: title.label ?? "shortcuts.cell.command",
    description: description.label ?? "shortcuts.cell.commandHelp",
    ...(title.text !== undefined && { labelText: title.text }),
    ...(description.text !== undefined && { descriptionText: description.text }),
    section,
    keywordsText: () => `${keysText(id)} ${id}`,
    noRowReset: true,
    control: {
      kind: "custom",
      render: (ctx) => <ShortcutCell id={id} name={nameFor(ctx.t)} fixed={fixed} ctx={ctx} />,
    },
  };
}

const tableRows = (): SettingRowDef[] =>
  KEY_COMMANDS.filter((c) => c.devOnly !== true).map((c) =>
    commandRow(
      c.id,
      { label: c.label },
      { label: c.description },
      SECTION_OF[c.group] as TKey,
      c.fixed === true,
    ),
  );

/** The arguments a key can supply: the selection and the playhead (the palette's prefill). */
const PREFILLED = new Set(["item", "items", "at"]);

/** Catalog commands a key can run with no form, that have no row of the table's. */
function catalogRows(): SettingRowDef[] {
  const taken = new Set(KEY_COMMANDS.map((c) => c.id));
  const rows: SettingRowDef[] = [];
  for (const command of listedCatalog(commandCatalog.getState().commands)) {
    if (taken.has(command.name)) continue;
    if (needsArguments(command.inputSchema) && !onlyPrefilled(command)) continue;
    const info = menuServices.commands.get(command.name);
    rows.push(
      commandRow(
        command.name,
        info === undefined ? { text: command.summary } : { label: info.label },
        { text: command.summary },
        "shortcuts.sections.commands",
        false,
      ),
    );
  }
  return rows;
}

function onlyPrefilled(command: CatalogCommand): boolean {
  return (command.inputSchema.required ?? []).every((name) => PREFILLED.has(name));
}

const WORKSPACE_ROWS: readonly { id: string; label: TKey }[] = [
  { id: "workspace.reopen_panels", label: "workspace.commands.reopenPanels" },
  { id: "workspace.reset_layout", label: "workspace.reset" },
  { id: "workspace.reset_all", label: "workspace.resetAll" },
];

function workspaceRows(): SettingRowDef[] {
  const section: TKey = "shortcuts.sections.workspace";
  return [
    ...WORKSPACE_ROWS.map((r) =>
      commandRow(r.id, { label: r.label }, { label: "shortcuts.cell.commandHelp" }, section, false),
    ),
    ...WORKSPACES.map((workspace) =>
      commandRow(
        bindingId("workspace.switch_workspace", { workspace }),
        { text: i18n.t("palette.switchTo", { name: i18n.t(`workspace.${workspace}`) }) },
        { label: "shortcuts.cell.commandHelp" },
        section,
        false,
      ),
    ),
  ];
}

function panelRow(id: string, title: TKey): SettingRowDef {
  return commandRow(
    `workspace.toggle_panel.${id}`,
    { text: i18n.t("workspace.commands.togglePanelNamed", { name: i18n.t(title) }) },
    { label: "shortcuts.cell.commandHelp" },
    "shortcuts.sections.workspace",
    false,
  );
}

// ---- the preset, reset and files --------------------------------------------------------------

const PRESET_LABELS = {
  nle: "shortcuts.rows.preset.nle",
  capcut: "shortcuts.rows.preset.capcut",
} as const;

const presetOptions: readonly ChoiceOption[] = SHORTCUT_PRESETS.map((id) => ({
  value: id,
  label: PRESET_LABELS[id],
}));

async function resetAllShortcuts(ctx: SettingsContext): Promise<void> {
  const yes = await ctx.confirm({
    title: "shortcuts.rows.reset.confirmTitle",
    message: "shortcuts.rows.reset.confirmMessage",
    confirm: "shortcuts.rows.reset.confirm",
    danger: true,
  });
  if (!yes) return;
  await ctx.client.reset(["shortcuts.preset", "shortcuts.bindings"]);
  ctx.say(ctx.t("shortcuts.notices.reset"));
}

async function exportShortcuts(ctx: SettingsContext): Promise<void> {
  const result = await ctx.client.exportToFile("shortcuts");
  if (result.status === "saved") {
    ctx.say(ctx.t("shortcuts.notices.exported", { path: result.path }));
  }
}

async function importShortcuts(ctx: SettingsContext): Promise<void> {
  const result = await ctx.client.importFromFile("shortcuts");
  if (result.status === "cancelled") return;
  if (result.status === "invalid") {
    ctx.say(
      ctx.t(
        result.reason === "unreadable"
          ? "settings.notices.unreadable"
          : result.reason === "newer"
            ? "settings.notices.newer"
            : "settings.notices.notSettings",
      ),
    );
    return;
  }
  ctx.say(ctx.t("shortcuts.notices.imported"));
}

const FILES_SECTION: TKey = "shortcuts.sections.preset";

const presetRows: SettingRowDef[] = [
  {
    id: "shortcuts.preset",
    key: "shortcuts.preset",
    label: "shortcuts.rows.preset.label",
    description: "shortcuts.rows.preset.description",
    keywords: "shortcuts.rows.preset.keywords",
    section: FILES_SECTION,
    control: { kind: "segmented", options: presetOptions },
  },
  {
    id: "shortcuts.reset_all",
    // The row resets `shortcuts.bindings` (and the preset) itself: it names the key, with no reset of its own.
    key: "shortcuts.bindings",
    label: "shortcuts.rows.reset.label",
    description: "shortcuts.rows.reset.description",
    keywords: "shortcuts.rows.reset.keywords",
    section: FILES_SECTION,
    noRowReset: true,
    control: {
      kind: "action",
      label: "shortcuts.rows.reset.button",
      run: resetAllShortcuts,
      disabled: (ctx) =>
        ctx.values["shortcuts.preset"] ===
          ctx.client.store.getState().defaults["shortcuts.preset"] &&
        Object.keys(ctx.values["shortcuts.bindings"]).length === 0,
      danger: true,
    },
  },
  {
    id: "shortcuts.export",
    label: "shortcuts.rows.export.label",
    description: "shortcuts.rows.export.description",
    keywords: "shortcuts.rows.export.keywords",
    section: FILES_SECTION,
    noRowReset: true,
    control: { kind: "action", label: "shortcuts.rows.export.button", run: exportShortcuts },
  },
  {
    id: "shortcuts.import",
    label: "shortcuts.rows.import.label",
    description: "shortcuts.rows.import.description",
    keywords: "shortcuts.rows.import.keywords",
    section: FILES_SECTION,
    noRowReset: true,
    control: { kind: "action", label: "shortcuts.rows.import.button", run: importShortcuts },
  },
];

export const SHORTCUTS_PAGE_ID = "shortcuts";

const shortcutsPage = (): SettingsPageDef => ({
  id: SHORTCUTS_PAGE_ID,
  title: "settings.pages.shortcuts",
  icon: "keyboard",
  group: "preferences",
  order: 100,
  note: "shortcuts.note",
  filterable: true,
  rows: [...presetRows, ...tableRows(), ...catalogRows(), ...workspaceRows()],
});

let registered = false;

/** Registers the page, its row menu, and a toggle row for each panel as it is registered. */
export function registerShortcutsPage(): void {
  if (registered) return;
  registered = true;
  registerShortcutRowMenu(menuServices, {
    reset: (id) => resetBinding(id),
    remove: (id) => removeBinding(id),
  });
  registerSettingsPage(shortcutsPage());
  const seen = new Set<string>();
  const addPanels = (): void => {
    for (const panel of registeredPanels()) {
      if (seen.has(panel.id)) continue;
      seen.add(panel.id);
      registerSettingRow(SHORTCUTS_PAGE_ID, panelRow(panel.id, panel.title));
    }
  };
  addPanels();
  panelRegistry.subscribe(addPanels);
}

// ---- what the row menu does -------------------------------------------------------------------

function write(model: KeymapModel, wanted: Readonly<Record<string, readonly string[]>>): void {
  const overrides: ShortcutOverrides = overridesFor(model, wanted);
  void preferences.set("shortcuts.bindings", overrides).catch(() => undefined);
}

function resetBinding(id: string): void {
  const model = keymapModel();
  write(model, withPresetKeys(model, model.effective(), id));
}

function removeBinding(id: string): void {
  const model = keymapModel();
  write(model, { ...model.effective(), [id]: [] });
}
