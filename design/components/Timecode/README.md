Timecode shows the current position and total duration in HH:MM:SS:FF, with the project frame rate in a chip; it is the transport readout above the timeline.

**When to use**
- The transport bar (large) and in/out, marker and clip-duration readouts (compact).
- Anywhere a frame-accurate time must be read or typed.
- Not for coarse durations like "2 min" in the UI chrome.

**Anatomy**
`.cw-timecode` > `__current` (type: timecode-lg, `font-mono`, `text-primary`) + `__sep` (`/`, `text-muted`) + `__total` (type: timecode, `text-secondary`); beside it a `.cw-chip.cw-timecode__fps` (`control-sm`, type: numeric) such as `23.976`. `--sm` renders current in timecode size. `.cw-timecode__bar` is the `toolbar`-tall transport container.

**The consumer provides**
Current and total time in frames, the project frame rate (and drop-frame flag), and the format function. Make the current value editable (click to type) in the consumer.

**States**
Playing and paused look identical (no animation during playback), editing (input replaces `__current`, marigold border), empty/disabled (`text-disabled`), focus-visible on the editable value.

**Keyboard & accessibility**
`role="timer"` with an `aria-label` reading the full time; do not announce every frame while playing. Click or Enter edits; typing digits fills from the right; Enter commits, Escape cancels. Numerals use `font-variant-numeric: tabular-nums` so width never jitters.

**Command mapping**
Committing a typed time dispatches `playhead.seek {time}`. The frame-rate chip opens project settings via `project.setFrameRate` (disabled once media is conformed).

**Do**
- Always use `var(--font-mono)` and tabular numerals.
- Keep separators in `text-muted` so frames read first.
- Show the exact project fps, including 23.976, never a rounded value.

**Don't**
- Don't tint the current time with marigold; the Playhead carries that color.
- Don't use the display or UI font for time.
- Don't hide the frame rate when the project is variable fps; show "VFR".
