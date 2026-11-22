Panel is the container for every docked region of the editor: a fixed-height header with title, tabs and actions, above a body on `surface-1`.

**When to use**
- Media bin, inspector, AI Studio, transcript, history and timeline header regions.
- Any region that can be docked, resized or collapsed.
- Not for floating content (use Menu, Dialog or popovers on `surface-2`).

**Anatomy**
`.cw-panel` (`surface-1`, `border-subtle`, `radius-md`) > `__header` (`panel-header` tall, `space-3` side padding, bottom `border-subtle`) containing `__title` (type: heading), optional Tabs, and `__actions` (IconButton `--sm`, pushed right) > `__body` (`space-3` padding). `__divider` is a 1px `border-subtle` rule between regions inside the body.

**The consumer provides**
Panel title, optional tab list and active tab, header actions, body content, and docking/resize behavior. Gutters between panels are `surface-0`.

**States**
Default. Collapsed (header only). Focused panel does not change color; keyboard focus shows `focus-ring` on the controls inside. Tabs switch bodies with `duration-base`.

**Keyboard & accessibility**
Render as `<section aria-label>` (or `role="region"`). Header tabs follow Tabs semantics. Panel actions are reachable by Tab in header order; F6 cycles panels at the app level.

**Command mapping**
Close/collapse and tab changes dispatch `layout.panel.close`, `layout.panel.collapse`, `layout.panel.setTab {panel, tab}` so agents can arrange the workspace over MCP.

**Do**
- Keep header height at `panel-header`; put only title, tabs and up to three actions there.
- Separate regions with `border-subtle`, not shadows.
- Use `space-3` for side padding so content aligns across panels.

**Don't**
- Don't tint a panel with marigold or agent blue; use InspectorSection or ProposalBar for emphasis.
- Don't nest Panels inside Panels.
- Don't put footage-adjacent colors on `surface-preview` panels.
