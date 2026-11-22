Slider adjusts a bounded value along a track, with a numeric readout and an optional keyframe diamond at the right.

**When to use**
- Bounded values where the range matters: opacity, volume, blur, feather.
- Animatable properties that need a keyframe toggle beside them.
- For unbounded or precise numbers use NumberScrubber.

**Anatomy**
`.cw-slider` row: `__label` (type: label) + `__track` (`control-sm` tall hit area) containing `__rail` (`border-control`, 4px), `__fill` (`marigold`) and `__thumb` (`surface-2` with marigold ring) + `__value` (type: numeric, mono, tabular) + KeyframeToggle. Progress is driven by the `--value` custom property.

**The consumer provides**
Label, value, min/max/step, unit formatting for the readout, keyframe state (`off`, `on`, `elsewhere`), and the commit command.

**States**
Default, hover/drag, focus-visible on the track, disabled (`aria-disabled="true"`: fill and thumb in `text-disabled`). Keyframe: off (`text-muted` outline), on at playhead (`marigold-text` filled), has keyframes elsewhere (`marigold-text` outlined).

**Keyboard & accessibility**
`role="slider"` with `aria-valuenow/min/max`; Left/Right step, Shift = x10, Home/End jump to bounds. The readout is always visible so value never relies on thumb position alone. Keyframe button has a state-specific `aria-label`.

**Command mapping**
Dragging dispatches `clip.setProperty {prop, value}` (coalesced to one undo step); the diamond dispatches `clip.keyframe.toggle {prop, time:playhead}`.

**Do**
- Keep the fill in `marigold` and the rail in `border-control` so the 3:1 contrast holds on `surface-1`.
- Show the readout in `font-mono` with `tabular-nums`.
- Reserve the keyframe slot even when the property is not animatable (disabled).

**Don't**
- Don't use Slider for values where the exact number is the point; add a NumberScrubber.
- Don't color the fill with agent blue unless the value was set by an agent and shown elsewhere as such.
- Don't make the track shorter than `control-sm` in height.
