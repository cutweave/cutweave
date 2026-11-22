SegmentedControl picks exactly one value from a short, always-visible set, such as the project aspect ratio.

**When to use**
- 2 to 5 mutually exclusive options that fit on one line (16:9, 9:16, 1:1, 4:5).
- Settings where seeing every option at once beats a dropdown.
- Not for navigation between panels (use Tabs) or independent toggles (use Switch).

**Anatomy**
`.cw-seg` (`control-sm` tall, `radius-sm`, `border-control` outline) > `.cw-seg__item` buttons separated by `space-1`. Each item is at least `hit-min` wide.

**The consumer provides**
The option list, the selected value, an `aria-label` for the group, and an `onChange` that dispatches the setting command.

**States**
Default (`text-secondary`), hover (`surface-3`), selected (`aria-checked="true"`: `marigold-subtle` fill, 1px marigold edge, `text-primary` label), focus-visible, disabled item (`text-disabled`).

**Keyboard & accessibility**
`role="radiogroup"` with `role="radio"` items; Tab enters the group, Arrow keys move and select, selection follows focus. Selection is shown by fill plus edge, not color alone. Labels are real text, not icons.

**Command mapping**
Changing aspect ratio dispatches `project.setAspectRatio {ratio:"9:16"}`; an agent calling the same command over MCP updates the selected segment.

**Do**
- Keep labels under about 8 characters; use mono only for numeric ratios if tabular alignment matters.
- Use `marigold-subtle` + marigold edge for the selected segment, never `surface-3`.
- Show a disabled item with `text-disabled` rather than hiding it.

**Don't**
- Don't exceed 5 segments; switch to a select.
- Don't mix text and icon-only segments in one group.
- Don't use agent blue for a selected segment.
