Playhead is the marigold line and flag that marks the current time across the ruler and every track, alongside the ruler's ticks, timecode labels and in/out range.

**When to use**
- Always, once per timeline: the single "here" marker for seek, split and paste.
- The ruler and range shading appear wherever a time axis is shown.
- Not for clip selection or markers (those use their own marks).

**Anatomy**
`.cw-timeline` > `.cw-ruler` (`control-md` tall) with `__tick` (1px `text-muted`, `--major` taller) and `__label` (type: numeric, mono, `text-muted`) > `.cw-ruler__range` and `.cw-range-shade` (`marigold-subtle` fill, marigold in/out edges) > `.cw-playhead` (2px `playhead` line, `z-playhead`) with `__flag` (`radius-full`, `on-marigold` mono timecode).

**The consumer provides**
Current time (`--x`), zoom level (tick spacing and which labels show), in/out points, and timecode formatting at the project frame rate.

**States**
Idle, dragging the flag (scrubbing), snapped (flag shows the snap target time), in/out range set or unset. The line ignores pointer events; only the flag and ruler are draggable.

**Keyboard & accessibility**
The ruler is `role="slider"` with `aria-valuetext` of the timecode; Left/Right = 1 frame, Shift = 1 second, Home/End = start/end, I/O set in and out. Timecodes are real text. Nothing about the playhead animates during playback.

**Command mapping**
Seek -> `playhead.seek {time}`, I/O -> `range.setIn` / `range.setOut`, Clear -> `range.clear`. Agents read and move the playhead through the same commands.

**Do**
- Use `playhead` (marigold) only for the playhead line and flag.
- Draw ticks in `text-muted` and keep labels in mono with `tabular-nums`.
- Keep the line above clips with `z-playhead`.

**Don't**
- Don't color the in/out range with agent blue or status colors.
- Don't add a gradient or shadow to the line.
- Don't show more than one playhead; use markers for other positions.
