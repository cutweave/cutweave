// SPDX-License-Identifier: GPL-3.0-or-later
import { useEffect } from "react";
import { MenuBar, type MenuBarMenu } from "./MenuBar.tsx";

export default { title: "Overlays/MenuBar", component: MenuBar, tags: ["autodocs"] };

const noop = () => {};

/** The editor's application menu (WP-1M.09), with the shortcuts it shows on Windows. */
const MENUS: MenuBarMenu[] = [
  {
    id: "file",
    label: "File",
    items: [
      { id: "new", label: "New project…", shortcut: "Ctrl+N", onSelect: noop },
      { id: "open", label: "Open project…", shortcut: "Ctrl+O", onSelect: noop },
      {
        kind: "submenu",
        id: "recent",
        label: "Open recent",
        items: [
          { id: "r1", label: "Launch teaser.cutweave", icon: "folder-open", onSelect: noop },
          { id: "r2", label: "Interview cut.cutweave", icon: "folder-open", onSelect: noop },
        ],
      },
      { kind: "separator", id: "s1" },
      { id: "save", label: "Save", shortcut: "Ctrl+S", onSelect: noop },
      { id: "saveAs", label: "Save as…", shortcut: "Ctrl+Shift+S", onSelect: noop },
      { kind: "separator", id: "s2" },
      { id: "export", label: "Export…", shortcut: "Ctrl+M", disabled: true },
      { kind: "separator", id: "s3" },
      { id: "exit", label: "Exit", onSelect: noop },
    ],
  },
  {
    id: "edit",
    label: "Edit",
    items: [
      { id: "undo", label: "Undo Split clip", shortcut: "Ctrl+Z", onSelect: noop },
      { id: "redo", label: "Redo", shortcut: "Ctrl+Y", disabled: true },
      { kind: "separator", id: "s1" },
      { id: "cut", label: "Cut", shortcut: "Ctrl+X", onSelect: noop },
      { id: "copy", label: "Copy", shortcut: "Ctrl+C", onSelect: noop },
      { id: "paste", label: "Paste", shortcut: "Ctrl+V", onSelect: noop },
      { id: "selectAll", label: "Select all", shortcut: "Ctrl+A", onSelect: noop },
    ],
  },
  {
    id: "view",
    label: "View",
    items: [{ id: "fullScreen", label: "Toggle full screen", shortcut: "F11", onSelect: noop }],
  },
  {
    id: "help",
    label: "Help",
    items: [{ id: "about", label: "About Cutweave", onSelect: noop }],
  },
];

/** A title-bar strip: the menu bar sits on the app frame color, as in the editor's top bar. */
const bar = {
  display: "flex",
  alignItems: "center",
  boxSizing: "border-box",
  height: "var(--toolbar)",
  padding: "0 var(--space-2)",
  background: "var(--surface-0)",
  borderBottom: "var(--border-thin) solid var(--border-subtle)",
} as const;

const stage = {
  minHeight: "var(--layer-width-md)",
  background: "var(--surface-1)",
} as const;

/** At rest: the four menus as plain words. */
export const Closed = {
  render: () => (
    <div style={stage}>
      <div style={bar}>
        <MenuBar aria-label="Application menu" menus={MENUS} />
      </div>
    </div>
  ),
};

/** File opened with the pointer: the list hangs from the trigger's leading edge. */
export const Open = {
  render: () => (
    <div style={stage}>
      <div style={bar}>
        <MenuBar aria-label="Application menu" menus={MENUS} defaultValue="file" />
      </div>
    </div>
  ),
};

/** Alt pressed: the first menu has the keyboard focus and the access letters are underlined. */
function KeyboardFocusDemo() {
  useEffect(() => {
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Alt", altKey: true }));
    window.dispatchEvent(new KeyboardEvent("keyup", { key: "Alt" }));
  }, []);
  return (
    <div style={stage}>
      <div style={bar}>
        <MenuBar aria-label="Application menu" menus={MENUS} />
      </div>
    </div>
  );
}

export const KeyboardFocus = { render: () => <KeyboardFocusDemo /> };
