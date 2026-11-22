JobProgress is a row for a background job (export, generation, transcription, proxy render) with name, progress bar, percent, ETA and cancel.

**When to use**
- The jobs list in the status bar or AI Studio queue.
- Any long-running operation the user can leave and return to.
- Not for the short outcome message (use Toast).

**Anatomy**
`.cw-job` (`surface-1`, `border-subtle` bottom rule, `space-3` gaps) > `__main`: `__name` (type: body, ellipsis) + `.cw-bar` (`border-control` track, `marigold` fill via `--value`) > `__pct` (type: numeric, mono, tabular, right-aligned) > `__eta` (type: caption, `text-muted`) > cancel IconButton `--sm`. Failed rows show `__err` (`danger`, icon + words) and a Retry Button.

**The consumer provides**
Job id, display name with real file and model names, progress 0-100, ETA text, state (`running`, `queued`, `failed`, `done`), error message, and the cancel/retry commands.

**States**
Running (marigold fill), queued (StatusChip "Queued", `text-muted` percent), failed (fill turns `danger`, error line with icon and words, Retry), done (collapses to a success Toast). Updates are not animated beyond the bar width.

**Keyboard & accessibility**
List uses `role="list"`; the bar has `role="progressbar"` with `aria-valuenow/min/max` and a label. Percent is also shown as text. Cancel has an `aria-label` that names the job. Failed state is announced through an alert, not color.

**Command mapping**
Cancel -> `job.cancel {id}`, Retry -> `job.retry {id}`. Progress arrives from engine events (`job.progress`), so jobs started by an agent over MCP appear identically.

**Do**
- Show percent in `font-mono` with `tabular-nums` so it does not jitter.
- Name jobs with the real asset (Shorts_final.mp4) and model (Kling 3.0 via fal).
- Keep marigold for the fill only.

**Don't**
- Don't show an indeterminate bar for jobs that report progress.
- Don't hide failed rows; keep them until dismissed or retried.
- Don't use agent blue for jobs started by agents; use ActorBadge in the details view.
