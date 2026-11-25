Menu is a floating list of commands for a clip, track or selection, with shortcuts shown at the right edge.

**When to use**
- Right-click context menus on clips, tracks, assets, panels, the preview and text fields (opened by the editor's context-menu host, see "Opening").
- Overflow ("more") menus from IconButtons.
- Not for choosing a value in a form (use a select) or searching commands (use CommandPalette).

**Anatomy**
`.cw-menu` (`surface-2`, `radius-md`, `shadow-popover`, `space-1` padding, `z-dropdown`) > `.cw-menu__item` (`control-md` tall, `radius-sm`, type: body) with leading `icon-md` icon, `__label`, and `__kbd` (type: numeric, `text-muted`) > `.cw-menu__sep` (1px `border-subtle`). Variants: `--agent` ("Ask agent..." in `agent-text` with sparkles), `--danger` (Delete in `danger`). A submenu row ends in a `chevron-right` icon and opens a second `.cw-menu`.

**The consumer provides**
Items with labels, icons, shortcut strings from the keymap, enabled state per selection, the destructive flag, a one-line reason for each disabled item, and the position anchored to the pointer or button.

**Entry kinds**
`item` (a command; `checked` makes it a choice), `separator`, `label` (a heading, not focusable) and `submenu` (a row with its own list of items, separators and headings). One submenu level only.

**Opening**
- `mode="context"`: right-click or the context-menu key over the children (Radix opens it at the pointer).
- `mode="dropdown"`: a click on the trigger button.
- `mode="point"`: controlled (`open`, `onOpenChange`), no children, hangs from `anchor` (a viewport point) through a zero-size, hidden anchor. This is what the editor's `ContextMenuHost` uses for every surface (DOM, canvas, keyboard, touch), so one code path serves them all. It is not modal: a second right-click elsewhere closes the menu and still reaches the surface under it. `onCloseAutoFocus` lets the host return focus to the surface instead of the anchor.

**States**
Default, hover or keyboard-active (`surface-3`, `data-active="true"`), focus-visible, disabled (`text-disabled`, skipped by arrows). A disabled item carries its reason: it is the row's description (`aria-describedby`, so a screen reader hears it) and a tooltip shows it on hover. Opens with `duration-base` and `easing-enter`; no motion under reduced motion.

**Keyboard & accessibility**
`role="menu"` and `role="menuitem"`; Up/Down move, Home/End jump, Enter activates, Right opens a submenu and Left closes it, Escape closes and returns focus to the anchor (or to the surface, with `onCloseAutoFocus`), type-ahead matches labels. Shift+F10 and the Menu key open a context menu from the focused surface. The menu is named for its target: `aria-label` "Clip actions for interview_A.mov". Shortcuts are real text. Danger and agent items carry words and icons, not color alone.

**Command mapping**
Each item dispatches a registered command by its registry name, or an editor-side UI command (`workspace.*`, copy, reveal) in the same shape. Label, icon and the destructive flag come from the command; the shortcut hint comes from the keymap. Split -> `clip.split`, Delete -> `item.delete`, Ripple delete -> `timeline.ripple_delete`, Mute and Lock -> `track.set`, Insert at playhead -> `timeline.insert`, Move playhead here -> `view.set_playhead`, Undo -> `edit.undo`, Close panel -> `workspace.close_panel`, Ask agent... -> `agent.ask {context:selection}` (shown once the chat exists). The surfaces and their context keys are in ADR-0026.

**Do**
- Group with `border-subtle` separators; keep danger last and separated.
- Show shortcuts from the live keymap, in mono.
- Use `agent-text` only for the agent entry point.
- Disable an item that applies but cannot run now, and say why in one line ("Track is locked"). Leave out an item that makes no sense for the target (audio fades on a text clip).
- Name the object when it helps: "Delete 3 clips".

**Don't**
- Don't nest more than one submenu level.
- Don't use marigold for hover; hover is `surface-3`.
- Don't hide unavailable commands that apply to the target; disable them so users learn they exist.
- Don't show more than about 12 rows at once; move the long tail into a submenu.
- Don't put markup in a label (plugin-supplied labels are plain text).
