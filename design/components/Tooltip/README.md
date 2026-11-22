Tooltip names a control on hover or focus and shows its keyboard shortcut in a mono key cap.

**When to use**
- Every IconButton and any control whose label is abbreviated or hidden.
- Surfacing shortcuts so users learn the keyboard-first workflow.
- Not for essential instructions, errors or long explanations.

**Anatomy**
`.cw-tooltip` (`surface-2`, `radius-sm`, `shadow-popover`, `space-1` / `space-2` padding, type: label, `z-tooltip`) > label text + one or more `.cw-kbd` caps (type: numeric, `font-mono`, `surface-3` with `border-default`, `radius-xs`). `.cw-tipwrap` positions it under or beside its anchor with a `space-1` gap.

**The consumer provides**
The label text (verb phrase), the shortcut keys (platform-aware), the anchor element and placement, and the command id used to look the shortcut up in the keymap.

**States**
Hidden, shown on hover after a short delay or on keyboard focus, hidden on Escape or blur. It never contains interactive content. Appears with `duration-fast`; no motion under reduced motion.

**Keyboard & accessibility**
`role="tooltip"` referenced by `aria-describedby` on the anchor; shown on focus-visible as well as hover. Do not rely on a tooltip as the only accessible name; the anchor still needs `aria-label`. Shortcut text is real characters (S, N, shift + backspace).

**Command mapping**
The shortcut shown is read from the keymap for the command the control dispatches (`clip.split` -> S, `timeline.snapping.toggle` -> N), so rebinding updates the tooltip.

**Do**
- Write the action as a verb phrase ("Split at playhead").
- Show the current shortcut in `.cw-kbd` using the mono token.
- Use `shadow-popover` since it floats above the workspace.

**Don't**
- Don't put buttons or links inside a tooltip.
- Don't use agent or marigold color in tooltips.
- Don't show tooltips on touch/long-press without a visible alternative.
