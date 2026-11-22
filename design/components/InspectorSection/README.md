InspectorSection is a collapsible group of property rows with an overline label and reset-to-default affordances.

**When to use**
- Grouping inspector properties: Transform, Crop, Audio, Speed, Keyframes.
- Any list of label + control rows that should be collapsible and resettable.
- Not for top-level regions (use Panel).

**Anatomy**
`.cw-section` (`surface-1`, `border-subtle`, `radius-md`) > `__head` (toggle button + section reset IconButton) > `__body` (`space-2` gap between rows, `space-3` padding). The toggle shows a chevron and the section name in the overline style (uppercase, `text-muted`). Rows are `.cw-row`: label (type: label) left, control right, optional `cw-row__reset` that appears when `data-modified="true"`.

**The consumer provides**
Section id and label, expanded state, rows (label + Slider, NumberScrubber, Switch or Select), which values differ from defaults, and the reset commands.

**States**
Expanded, collapsed (`aria-expanded="false"`, chevron turns -90deg and body is hidden), hover on header (`surface-3`), modified row (label moves to `text-primary`, row reset shown), focus-visible.

**Keyboard & accessibility**
The toggle is a `<button aria-expanded aria-controls>`; Enter/Space collapse. The section reset has an `aria-label` naming the section. Row reset buttons are only in the tab order while visible. Collapsed content is removed from the accessibility tree.

**Command mapping**
Row reset -> `clip.resetProperty {prop}`; section reset -> `clip.resetProperties {group}`. Collapse state is UI-only and persisted per workspace via `layout.section.set`.

**Do**
- Use the overline style only for section names, never for row labels.
- Keep rows `control-sm` tall and `space-2` apart for density.
- Show reset only when a value differs from its default.

**Don't**
- Don't use marigold to mark modified rows; modified is a neutral text shift.
- Don't nest sections.
- Don't make the reset the only way to return to defaults; double-click on a scrubber label also resets.
