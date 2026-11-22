IconButton is a square, label-less button for toolbars and dense panels; toggles use `aria-pressed` and turn marigold when active.

**When to use**
- Tool selection and mode toggles in the main toolbar (select, razor, ripple, snapping).
- Per-row actions in panels and track headers (lock, hide, close, reset).
- Never for a first-time or ambiguous action; add a Tooltip or use Button with a label.

**Anatomy**
`.cw-iconbtn` > one `.cw-icon`. Default is `control-lg` with an `icon-lg` (20px) glyph for the `toolbar` bar; `--sm` is `control-sm` with an `icon-md` glyph. `.cw-toolbar` supplies the `toolbar` height, `space-1` gaps and `.cw-toolbar__sep` dividers.

**The consumer provides**
An inline SVG, an `aria-label` that includes the shortcut, `aria-pressed` for toggles, and the command id.

**States**
Default (`text-secondary`), hover (`surface-3`, `text-primary`), pressed/active (`marigold-subtle` fill, `marigold-text` icon), focus-visible (`focus-ring`), disabled (`text-disabled`).

**Keyboard & accessibility**
Native `<button>`; Space/Enter toggles. Toolbars use `role="toolbar"` with arrow-key roving focus. Every button needs an accessible name; hit area never drops below `hit-min`. Active state is conveyed by fill and `aria-pressed`, not color alone.

**Command mapping**
Select -> `tool.set {tool:"select"}`, Razor -> `tool.set {tool:"razor"}`, Snapping -> `timeline.snapping.toggle`. The pressed state mirrors engine state, so an agent toggling snapping over MCP updates the button.

**Do**
- Use `marigold-subtle` + `marigold-text` only for the active tool or on-state.
- Keep glyphs 16x16 viewBox 24, stroke 1.75, `currentColor`.
- Group related tools and separate groups with `.cw-toolbar__sep`.

**Don't**
- Don't tint inactive icons with marigold; marigold means "here, this will act".
- Don't shrink the hit area to the glyph size.
- Don't use IconButton for destructive actions without confirmation or undo.
