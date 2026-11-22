Menu is a floating list of commands for a clip, track or selection, with shortcuts shown at the right edge.

**When to use**
- Right-click context menus on clips, tracks and assets.
- Overflow ("more") menus from IconButtons.
- Not for choosing a value in a form (use a select) or searching commands (use CommandPalette).

**Anatomy**
`.cw-menu` (`surface-2`, `radius-md`, `shadow-popover`, `space-1` padding, `z-dropdown`) > `.cw-menu__item` (`control-md` tall, `radius-sm`, type: body) with leading `icon-md` icon, `__label`, and `__kbd` (type: numeric, `text-muted`) > `.cw-menu__sep` (1px `border-subtle`). Variants: `--agent` ("Ask agent..." in `agent-text` with sparkles), `--danger` (Delete in `danger`).

**The consumer provides**
Items with labels, icons, shortcut strings from the keymap, enabled state per selection, the destructive flag, and the position anchored to the pointer or button.

**States**
Default, hover or keyboard-active (`surface-3`, `data-active="true"`), focus-visible, disabled (`text-disabled`, skipped by arrows). Opens with `duration-base` and `easing-enter`; no motion under reduced motion.

**Keyboard & accessibility**
`role="menu"` and `role="menuitem"`; Up/Down move, Home/End jump, Enter activates, Escape closes and returns focus to the anchor, type-ahead matches labels. Shortcuts are real text. Danger and agent items carry words and icons, not color alone.

**Command mapping**
Each item dispatches a registered command: Split -> `clip.split`, Ripple delete -> `clip.rippleDelete`, Detach audio -> `clip.detachAudio`, Speed... -> `clip.speed.open`, Replace -> `clip.replace`, Ask agent... -> `agent.ask {context:selection}`, Delete -> `clip.delete`.

**Do**
- Group with `border-subtle` separators; keep danger last and separated.
- Show shortcuts from the live keymap, in mono.
- Use `agent-text` only for the agent entry point.

**Don't**
- Don't nest more than one submenu level.
- Don't use marigold for hover; hover is `surface-3`.
- Don't hide unavailable commands; disable them so users learn they exist.
