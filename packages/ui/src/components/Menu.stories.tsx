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

const REASON_ITEMS: MenuEntry[] = [
  { id: "split", label: "Split at playhead", icon: "scissors", shortcut: "S", onSelect: noop },
  {
    id: "rename",
    label: "Rename…",
    shortcut: "F2",
    disabled: true,
    disabledReason: "Select one clip to rename it",
  },
  {
    kind: "submenu",
    id: "move",
    label: "Move to",
    items: [
      { id: "left", label: "Left", onSelect: noop },
      { id: "right", label: "Right", onSelect: noop },
      { id: "bottom", label: "Bottom", onSelect: noop },
    ],
  },
  { kind: "separator", id: "s1" },
  {
    id: "delete",
    label: "Delete 3 clips",
    icon: "trash-2",
    variant: "danger",
    shortcut: "Backspace",
    disabled: true,
    disabledReason: "Track is locked",
  },
];

/** A row that cannot run says why on hover; "Move to" opens the one allowed nested list. */
export const SubmenuAndReason = {
  render: () => (
    <div style={stage}>
      <Menu
        mode="dropdown"
        defaultOpen
        items={REASON_ITEMS}
        aria-label="Clip actions for interview_A.mov"
      >
        <IconButton icon="ellipsis" label="Clip actions" />
      </Menu>
    </div>
  ),
};

/** The editor's host opens every context menu like this: controlled, at a viewport point. */
export const AtAPoint = {
  render: () => (
    <div style={stage}>
      <Menu
        mode="point"
        open
        anchor={{ x: 48, y: 48 }}
        items={CLIP_ITEMS}
        aria-label="Clip actions for interview_A.mov"
      />
    </div>
  ),
};

const BIN_CARD_ITEMS: MenuEntry[] = [
  { id: "insert", label: "Insert at playhead", shortcut: "Enter", onSelect: noop },
  { id: "append", label: "Append to timeline", onSelect: noop },
  { kind: "separator", id: "s1" },
  { id: "play", label: "Play in Source view", icon: "play", shortcut: "Space", onSelect: noop },
  { kind: "separator", id: "s2" },
  { id: "reveal", label: "Reveal in Explorer", icon: "folder-open", onSelect: noop },
  { id: "copy", label: "Copy path", icon: "copy", onSelect: noop },
  { kind: "separator", id: "s3" },
  {
    id: "remove",
    label: "Remove from project",
    icon: "trash-2",
    variant: "danger",
    shortcut: "Delete",
    onSelect: noop,
  },
];

/** The media bin's card menu as the editor builds it (WP-2H.13). */
export const BinCardActions = {
  render: () => (
    <div style={stage}>
      <Menu
        mode="point"
        open
        anchor={{ x: 48, y: 48 }}
        items={BIN_CARD_ITEMS}
        aria-label="Media actions for interview_A.mov"
      />
    </div>
  ),
};

const TIMELINE_CLIP_ITEMS: MenuEntry[] = [
  { id: "split", label: "Split at playhead", icon: "scissors", shortcut: "S", onSelect: noop },
  {
    id: "delete",
    label: "Delete clip",
    icon: "trash-2",
    variant: "danger",
    shortcut: "Delete",
    onSelect: noop,
  },
  { kind: "separator", id: "s1" },
  { id: "disable", label: "Disable clip", onSelect: noop },
  {
    id: "rename",
    label: "Rename…",
    disabled: true,
    disabledReason: "Select one clip",
  },
  { kind: "separator", id: "s2" },
  { id: "reveal", label: "Reveal in media bin", icon: "film", onSelect: noop },
  { id: "copy", label: "Copy path", icon: "copy", onSelect: noop },
];

/** The timeline clip's menu as the editor builds it (WP-2H.13), with a row that says why not. */
export const TimelineClipActions = {
  render: () => (
    <div style={stage}>
      <Menu
        mode="point"
        open
        anchor={{ x: 48, y: 48 }}
        items={TIMELINE_CLIP_ITEMS}
        aria-label="Clip actions for interview_A.mov"
      />
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
