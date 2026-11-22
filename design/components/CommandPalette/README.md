CommandPalette is the keyboard-first search overlay for running any registered command or handing a request to the agent.

**When to use**
- Opened with Cmd/Ctrl+K from anywhere in the editor.
- Finding commands, panels, assets and agent suggestions without leaving the keyboard.
- Not for choosing a value in a form or for a clip-specific context menu.

**Anatomy**
`.cw-palette` (`surface-2`, `radius-lg`, `shadow-popover`, `z-dialog`, over a `scrim` supplied by the host) > `__search` (`toolbar` tall; search icon, `__input` in type: body, Esc key cap) > grouped results: `__group` (type: overline, `text-muted`) + `__item` rows (`control-md` tall, type: body, icon, `__label`, `.cw-kbd` shortcut). "Agent" group rows use `.cw-palette__item--agent` (sparkles in `agent-text`).

**The consumer provides**
The query, ranked results by group (Commands, Agent suggestions, recent), each command's id, label, icon and shortcut, and the highlighted index.

**States**
Empty (recent commands), typing (live filter), highlighted row (`aria-selected="true"`: `marigold-subtle` fill + 2px marigold edge), hover (`surface-3`), no results ("Ask the agent" fallback), input focus (focus ring as bottom edge). Opens with `duration-base`; no motion under reduced motion.

**Keyboard & accessibility**
`role="dialog"` with a combobox-style input controlling a `role="listbox"`; Up/Down move the highlight, Enter runs, Escape closes and restores focus, Tab does not leave the dialog. Group headings are announced; the selected row is exposed through `aria-activedescendant`.

**Command mapping**
Command rows dispatch the same engine commands as menus and buttons (`clip.split`, `clip.rippleDelete`). Agent rows dispatch `agent.run {prompt}` which may open a `proposal` transaction; the user then sees ProposalBar. No UI-only actions.

**Do**
- Show shortcuts in `.cw-kbd` so the palette teaches the keymap.
- Keep exactly one highlighted row, using marigold-subtle plus the edge.
- Label agent rows with sparkles and `agent-text`.

**Don't**
- Don't use agent blue for the highlight; it is for agent meaning only.
- Don't run an agent action without showing its cost in the proposal.
- Don't build a separate command list; read the engine registry.
