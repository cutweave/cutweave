NumberScrubber is a compact numeric field you edit by dragging the label or value horizontally, or by clicking to type.

**When to use**
- Transform, audio, speed and export values in the inspector and dialogs.
- Any number where fast, coarse-to-fine adjustment beats typing.
- Not for a bounded 0-100 value that benefits from a visible track (use Slider).

**Anatomy**
`.cw-num` (`control-sm`, `radius-sm`, `border-control`) > `__label` (type: label, `text-secondary`) + `__value` (type: numeric, `font-mono`, tabular) + optional `__unit` (`text-muted`). Cursor is `ew-resize`; while typing it becomes a text cursor and the border turns marigold.

**The consumer provides**
Label, current value, unit, min/max/step, drag sensitivity, a `mixed` flag when the selection disagrees, and the command that commits the value.

**States**
Default, hover (`surface-3`), editing (`data-state="editing"`, marigold border, native `<input>`), mixed (`data-state="mixed"`, value shown as an em dash in `text-muted`), disabled (`aria-disabled="true"`, `text-disabled`).

**Keyboard & accessibility**
`role="spinbutton"` with `aria-valuenow/min/max`; Up/Down step, Shift+Up/Down step x10, Enter starts editing and commits, Escape cancels. Dragging is never the only path. Holding Alt while dragging gives fine steps. Mixed values announce "mixed".

**Command mapping**
Commits dispatch the property command (for example `clip.setProperty {id, prop:"scale", value:100}`); a drag coalesces into one undo step.

**Do**
- Right-align values in `font-mono` with `tabular-nums` so digits line up in a column.
- Show units in `text-muted` so the value stays scannable.
- Keep the label short; the row label can carry detail.

**Don't**
- Don't use marigold except for the editing border.
- Don't clear a mixed value on focus; require explicit input.
- Don't hide the unit when it prevents ambiguity (px vs %).
