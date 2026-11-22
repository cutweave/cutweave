// SPDX-License-Identifier: GPL-3.0-or-later
import { IconButton } from "./IconButton.tsx";
import { Menu, type MenuEntry } from "./Menu.tsx";

export default { title: "Overlays/Menu", component: Menu, tags: ["autodocs"] };

const noop = () => {};

const CLIP_ITEMS: MenuEntry[] = [
  { id: "split", label: "Split at playhead", icon: "scissors", shortcut: "S", onSelect: noop },
  {
    id: "ripple",
    label: "Ripple delete",
    icon: "ripple",
    shortcut: "Shift+Backspace",
    onSelect: noop,
  },
  { id: "detach", label: "Detach audio", icon: "volume-2", onSelect: noop },
  { id: "speed", label: "Speed…", icon: "gauge", shortcut: "Ctrl+R", onSelect: noop },
  { id: "replace", label: "Replace clip", icon: "film", onSelect: noop },
  { kind: "separator", id: "s1" },
  { id: "ask", label: "Ask agent…", variant: "agent", onSelect: noop },
  { kind: "separator", id: "s2" },
  {
    id: "delete",
    label: "Delete",
    icon: "trash-2",
    variant: "danger",
    shortcut: "Backspace",
    onSelect: noop,
  },
];

const DISABLED_ITEMS: MenuEntry[] = [
  { id: "paste", label: "Paste", shortcut: "Ctrl+V", onSelect: noop },
  { id: "paste-attrs", label: "Paste attributes", shortcut: "Ctrl+Shift+V", disabled: true },
];

const stage = {
  padding: "var(--space-4)",
  minHeight: "var(--layer-width-md)",
  display: "flex",
  gap: "var(--space-8)",
  alignItems: "flex-start",
  background: "var(--surface-0)",
} as const;

/** Open on load so the visual-regression job captures the floating list. */
export const ClipActions = {
  render: () => (
    <div style={stage}>
      <Menu
        mode="dropdown"
        defaultOpen
        items={CLIP_ITEMS}
        aria-label="Clip actions for interview_A.mov"
      >
        <IconButton icon="ellipsis" label="Clip actions" />
      </Menu>
    </div>
  ),
};

export const DisabledItems = {
  render: () => (
    <div style={stage}>
      <Menu mode="dropdown" defaultOpen items={DISABLED_ITEMS} aria-label="Edit actions">
        <IconButton icon="ellipsis" label="Edit actions" />
      </Menu>
    </div>
  ),
};

/** Right-click the clip: the same items open at the pointer. */
export const ContextMenu = {
  render: () => (
    <div style={stage}>
      <Menu items={CLIP_ITEMS} aria-label="Clip actions for interview_A.mov">
        <button
          type="button"
          className="body"
          style={{
            padding: "var(--space-4)",
            background: "var(--clip-media-body)",
            border: "var(--border-thin) solid var(--clip-media)",
            borderRadius: "var(--radius-sm)",
            color: "var(--text-primary)",
            textAlign: "left",
          }}
        >
          interview_A.mov: right-click for actions
        </button>
      </Menu>
    </div>
  ),
};
