Switch toggles a setting on or off with immediate effect; on is marigold.

**When to use**
- Preferences and project settings that apply instantly (snap to clips, auto-ripple, burn in captions).
- Binary states with a clear label on the left.
- Not for choices that need an explicit Apply (use a checkbox) or for tool modes in a toolbar (use IconButton with `aria-pressed`).

**Anatomy**
`<button role="switch" class="cw-switch">` > `.cw-switch__thumb`. The track is a 32x16 pill (`space-8` x `space-4`, `radius-full`, `border-control` outline) inside a `hit-min` tall button; the thumb is `space-3`. Pair with a `.cw-row__label` to its left.

**The consumer provides**
A visible label, `aria-checked`, the setting key, and the command it dispatches. Pass `disabled` when the setting is locked by project policy.

**States**
Off (`surface-3` track, `text-secondary` thumb), on (`marigold` track, `on-marigold` thumb), hover, focus-visible (`focus-ring`), disabled on/off (`border-subtle` track, `text-disabled` thumb).

**Keyboard & accessibility**
Tab focuses; Space toggles (Enter also works on button). Uses `role="switch"` and `aria-checked`. The state is communicated by thumb position and fill, not color alone. The label is a real element associated by `aria-label` or `aria-labelledby`.

**Command mapping**
Toggling dispatches `settings.set {key, value}` (for example `timeline.snapping` or `export.burnCaptions`); MCP changes reflect back in `aria-checked`.

**Do**
- Make the label state the setting, not the action ("Snap to clips", not "Enable snapping").
- Apply changes immediately; no confirm step.
- Keep the on color `marigold` so it reads as the active thread.

**Don't**
- Don't use a Switch where the result needs Save/Apply.
- Don't rely on the thumb color alone to show disabled; use `text-disabled` plus the muted track.
- Don't put the label inside the thumb.
