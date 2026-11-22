Toast is a transient notification that reports the outcome of a command or the progress of a background job without blocking the editor.

**When to use**
- Export finished, media missing, generation failed, long job running.
- Outcomes that happened out of view or from an agent over MCP.
- Not for validation errors beside a field, or decisions (use Dialog or ProposalBar).

**Anatomy**
`.cw-toast` (`surface-2`, `radius-md`, `shadow-popover`, `space-3` padding, `z-toast`) > tone icon (`success`, `warning`, `danger`, or `marigold-text` for jobs) > `__main`: `__title` (type: body-strong), `__text` (type: body, `text-secondary`), optional `.cw-bar` progress (marigold fill on `border-control`) and `__actions` (Button `--sm`) > dismiss IconButton `--sm`. Set tone via `data-tone`.

**The consumer provides**
Tone, title, message with real values (file names, timecodes), optional actions with their commands, progress and ETA for jobs, and auto-dismiss timing.

**States**
Success and info auto-dismiss; warning and error persist until dismissed; job toast updates in place and becomes success or error when done; hover/focus pauses dismissal. Appears with `duration-slow` and `easing-enter`, disabled under reduced motion.

**Keyboard & accessibility**
`role="status"` for success, warning and job; `role="alert"` for errors. The icon is always paired with a title, so color is never alone. Actions are reachable via a landmark shortcut; Escape dismisses the focused toast. The progress bar has `role="progressbar"` and values.

**Command mapping**
Toasts are subscribed to engine events (`export.finished`, `ai.generate.failed`, `job.progress`). Actions dispatch commands: Retry -> `job.retry {id}`, Cancel -> `job.cancel {id}`, Show in folder -> `fs.reveal {path}`.

**Do**
- Use real file names and timecodes ("Exported 00:00:58 to Shorts_final.mp4").
- Keep one primary action; use ghost for secondary.
- Use `marigold` only for the job progress fill.

**Don't**
- Don't stack more than three toasts; collapse into a job list.
- Don't auto-dismiss errors.
- Don't use agent blue for toasts; show agent messages in the copilot panel.
