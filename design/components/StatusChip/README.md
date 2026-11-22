StatusChip is a small pill that states a status as icon plus word in success, warning, danger or neutral tone.

**When to use**
- Export, render, proxy, transcription and connection status in lists and headers.
- Per-item health in the media bin and job list.
- Not for AI provenance (use the AI badge or ActorBadge) or costs (use CostChip).

**Anatomy**
`.cw-status` (`control-sm` tall, `radius-full`, 1px border in the tone color, transparent fill, type: label) > `.cw-icon.cw-icon--sm` + word. Tone via `data-tone`: `success` (check), `warning` (alert-triangle), `danger` (alert-circle), `neutral` (info; `text-secondary`).

**The consumer provides**
The tone, a short word or phrase ("Exported", "Missing media", "Failed", "Proxy ready"), and optionally the object it describes in surrounding text.

**States**
Static. Tone changes when the underlying state changes (for example Rendering -> Exported). It does not hover or take focus unless wrapped by an interactive parent.

**Keyboard & accessibility**
Always icon plus word; never color-only. Tone colors (`success`, `warning`, `danger`) are for text and icons on `surface-1` and `surface-2` where contrast is guaranteed. Status changes should also be announced by a live region elsewhere (for example Toast), not by the chip alone.

**Command mapping**
Status is derived from engine state (`export.status`, `media.status`). Chips do not dispatch commands; actions such as Relink or Retry are separate Buttons beside them.

**Do**
- Keep words to one to three, sentence case.
- Use `warning` for recoverable issues (missing media) and `danger` for failures.
- Use `neutral` for informational state with no urgency.

**Don't**
- Don't use a status color without an icon and a word.
- Don't invent extra tones; use agent styling only for AI meaning, elsewhere.
- Don't fill the chip with solid status color; it is outlined on surfaces.
