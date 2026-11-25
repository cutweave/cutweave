// SPDX-License-Identifier: GPL-3.0-or-later
// The one text-field menu (WP-2H.12, research §5.8): Undo, Cut, Copy, Paste and Select all for
// every input, textarea and contenteditable, installed by the host's document-level handler so no
// field is missed. Electron has no default text menu, so without this a field has none. The four
// clipboard actions are the existing `edit:run` IPC (main runs them on the focused element); Undo
// is the page's own. Spelling suggestions are out (they need Electron's `context-menu` event).

import type { TKey } from "../i18n/index.ts";
import { desktopFromWindow, type EditAction } from "../session/desktop.ts";
import type { UiCommand } from "./commands.ts";
import type { OpenRequest } from "./host.tsx";
import type { MenuContext, MenuSurfaceDef, OpenVia, ScreenPoint } from "./types.ts";

/** Input types that hold text. Everything else (checkbox, range, button...) has no text menu. */
const TEXT_INPUT_TYPES = new Set([
  "",
  "text",
  "search",
  "url",
  "tel",
  "email",
  "password",
  "number",
]);

export type TextField = HTMLInputElement | HTMLTextAreaElement | HTMLElement;

/** The text field `source` is, or is inside (a contenteditable's child), or `null`. */
export function textFieldOf(source: Element): TextField | null {
  const editable = source.closest("[contenteditable]");
  if (editable instanceof HTMLElement && editable.isContentEditable) return editable;
  const field = source.closest("input, textarea");
  if (field instanceof HTMLTextAreaElement) return field;
  if (field instanceof HTMLInputElement && TEXT_INPUT_TYPES.has(field.type.toLowerCase())) {
    return field;
  }
  return null;
}

/** What the menu needs to know about the field it opened on. */
export interface FieldState {
  readonly readOnly: boolean;
  readonly hasSelection: boolean;
  readonly hasValue: boolean;
}

export function fieldState(field: TextField): FieldState {
  if (field instanceof HTMLInputElement || field instanceof HTMLTextAreaElement) {
    const start = field.selectionStart;
    const end = field.selectionEnd;
    return {
      readOnly: field.readOnly || field.disabled,
      hasSelection: start !== null && end !== null && end > start,
      hasValue: field.value.length > 0,
    };
  }
  const selection = field.ownerDocument.getSelection();
  return {
    readOnly: false,
    hasSelection:
      selection !== null && !selection.isCollapsed && field.contains(selection.anchorNode),
    hasValue: (field.textContent ?? "").length > 0,
  };
}

/** The field's name for the menu's label: its aria-label, its `<label>`, its placeholder. */
function fieldName(field: TextField): string {
  const aria = field.getAttribute("aria-label");
  if (aria !== null && aria.trim() !== "") return aria.trim();
  const labels = (field as HTMLInputElement).labels;
  const text = labels?.[0]?.textContent?.trim();
  if (text !== undefined && text !== "") return text;
  return field.getAttribute("placeholder")?.trim() ?? "";
}

/** The request for a text field's menu. */
export function fieldRequest(field: TextField, point: ScreenPoint, via: OpenVia): OpenRequest {
  return {
    surface: "field.text",
    point,
    element: field,
    via,
    target: { name: fieldName(field), data: fieldState(field) },
  };
}

// ---- the commands and the surface -----------------------------------------------------------------

const state = (ctx: MenuContext): FieldState => ctx.target.data as FieldState;

/** Runs the clipboard action on the focused field: main when it can, else the page. */
function runEdit(action: EditAction): void {
  const bridge = desktopFromWindow();
  if (bridge?.edit !== undefined) {
    void bridge.edit.run(action).catch(() => undefined);
    return;
  }
  const command = action === "selectAll" ? "selectAll" : action;
  document.execCommand(command);
}

export const FIELD_COMMANDS: readonly UiCommand[] = [
  {
    id: "edit.text.undo",
    hidden: true,
    label: "contextMenu.field.undo",
    icon: "undo-2",
    run: () => void document.execCommand("undo"),
  },
  {
    id: "edit.text.cut",
    hidden: true,
    label: "contextMenu.field.cut",
    host: true,
    run: () => runEdit("cut"),
  },
  {
    id: "edit.text.copy",
    hidden: true,
    label: "contextMenu.field.copy",
    icon: "copy",
    host: true,
    run: () => runEdit("copy"),
  },
  {
    id: "edit.text.paste",
    hidden: true,
    label: "contextMenu.field.paste",
    icon: "clipboard",
    host: true,
    run: () => runEdit("paste"),
  },
  {
    id: "edit.text.select_all",
    hidden: true,
    label: "contextMenu.field.selectAll",
    host: true,
    run: () => runEdit("selectAll"),
  },
];

const readOnlyReason = (): TKey => "contextMenu.reason.fieldReadOnly";
const nothingSelected = (ctx: MenuContext): TKey =>
  state(ctx).readOnly ? readOnlyReason() : "contextMenu.reason.noSelection";

export const FIELD_SURFACE: MenuSurfaceDef = {
  id: "field.text",
  name: (target) =>
    target.name !== undefined && target.name !== ""
      ? { key: "contextMenu.field.nameFor", vars: { name: target.name } }
      : { key: "contextMenu.field.name" },
  groups: ["history", "clipboard", "select"],
  rows: [
    {
      command: "edit.text.undo",
      group: "history",
      order: 0,
      enabledIf: (ctx) => !state(ctx).readOnly,
      reason: readOnlyReason,
    },
    {
      command: "edit.text.cut",
      group: "clipboard",
      order: 0,
      enabledIf: (ctx) => !state(ctx).readOnly && state(ctx).hasSelection,
      reason: nothingSelected,
    },
    {
      command: "edit.text.copy",
      group: "clipboard",
      order: 1,
      enabledIf: (ctx) => state(ctx).hasSelection,
      reason: "contextMenu.reason.noSelection",
    },
    {
      command: "edit.text.paste",
      group: "clipboard",
      order: 2,
      enabledIf: (ctx) => !state(ctx).readOnly,
      reason: readOnlyReason,
    },
    {
      command: "edit.text.select_all",
      group: "select",
      order: 0,
      enabledIf: (ctx) => state(ctx).hasValue,
      reason: "contextMenu.reason.emptyField",
    },
  ],
};

/** Development builds: Shift+right-click on dead space offers "Inspect element". */
export const INSPECT_COMMAND: UiCommand = {
  id: "host.inspect_element",
  hidden: true,
  label: "contextMenu.inspect",
  icon: "search",
  host: true,
  run: (_args, ctx) => {
    const point = (ctx.target.data as { point: ScreenPoint }).point;
    void desktopFromWindow()
      ?.window?.inspectElement?.(point.x, point.y)
      .catch(() => undefined);
  },
};

export const INSPECT_SURFACE: MenuSurfaceDef = {
  id: "dev.inspect",
  name: () => ({ key: "contextMenu.devName" }),
  groups: ["dev"],
  rows: [{ command: "host.inspect_element", group: "dev", order: 0 }],
};
