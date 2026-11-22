Dialog is a modal sheet for focused, multi-field tasks such as exporting; it sits on `surface-2` over a scrim with `shadow-dialog`.

**When to use**
- Export, project settings, relink media, confirm a destructive action.
- Tasks that must be finished or cancelled before returning to the timeline.
- Not for passive feedback (use Toast) or quick commands (use CommandPalette or Menu).

**Anatomy**
`.cw-dialog` (`surface-2`, `radius-lg`, `shadow-dialog`, `space-6` padding, `space-3` gaps, `z-dialog`) > `__title` (type: title) > rows (`.cw-row`: label + control; a `.cw-select` look-alike at `control-md`, NumberScrubbers at `control-sm`) > `.cw-note` (cost or info line, `text-muted`, icon) > `__footer` (Cancel ghost, Export primary, both `control-lg`). The host provides the `scrim` backdrop.

**The consumer provides**
Title, form fields and their values, the cost or "free" note, validation messages, and the confirm and cancel commands. Focus management and the scrim are the host's job.

**States**
Open (appears with `duration-slow`, `easing-enter`; none under reduced motion), valid, invalid (field shows a `danger` message with icon and words, primary disabled), submitting (primary shows progress and disables), closed. Escape cancels unless a change is unsaved.

**Keyboard & accessibility**
`role="dialog"` with `aria-modal="true"` and `aria-labelledby` pointing at the title. Focus moves to the first field and is trapped; Tab/Shift+Tab cycle; Enter triggers the primary only when no multi-line field has focus; Escape closes and returns focus to the opener.

**Command mapping**
Export dispatches `export.start {preset, width, height, fps, bitrate}`; the free-local note comes from `export.estimate`. Cancel dispatches nothing. Destructive dialogs use a danger button bound to the confirmed command.

**Do**
- Make Export (primary, marigold) the single emphasized action.
- Tell the user the cost, even when it is free.
- Keep value fields in mono via NumberScrubber.

**Don't**
- Don't use a Dialog for non-blocking progress; hand off to a job toast.
- Don't use agent blue in a dialog unless it presents agent-made changes.
- Don't exceed one scrollable region.
