CostChip is a pill that states what an AI action will cost before it runs: an estimate, free/local, over budget, or needing approval.

**When to use**
- Next to Generate buttons, in ModelCard, and in ProposalBar cost lines.
- Anywhere an action spends money or credits.
- Not for already-incurred spend history (use a list row).

**Anatomy**
`.cw-cost` (`control-sm` tall, `radius-full`, `border-default`, `surface-2`) > optional `.cw-icon.cw-icon--sm` + label (type: label). Amounts use `.cw-mono` (type: numeric, tabular). `data-state` sets tone: default (`text-secondary`), `warning` (`warning` text and border, alert icon, "Over budget"), `approval` (`agent-subtle` fill, `agent` border, `agent-text`, sparkles, "Needs approval").

**The consumer provides**
The estimate (amount and currency, or `free`), the budget comparison, whether an agent action requires user approval, and a tooltip explaining the estimate basis.

**States**
Estimate (neutral), free/local (check icon + words), warning (over budget), approval (agent-originated action waiting on the user), pending estimate (use "Estimating" label). The chip is static; link actions live in adjacent buttons.

**Keyboard & accessibility**
Non-interactive text; if it needs a tooltip, wrap it in a focusable element. Every state has words, and warning/approval also have icons, so color is never the only signal. Amount reads as "approximately 0.40 US dollars" via `aria-label` when needed.

**Command mapping**
Values come from `ai.estimate {model, params}`. "Needs approval" appears when a `proposal` transaction or `ai.generate` exceeds the agent's spending policy; approval is granted by the user accepting the ProposalBar or confirming in a dialog.

**Do**
- Prefix estimates with `~` and show real amounts like `~$0.40`.
- Use `warning` for budget issues and `agent` only for agent-origin approvals.
- Keep the amount in `font-mono` with tabular numerals.

**Don't**
- Don't use `danger` for cost; reserve it for errors.
- Don't show a chip without a number or an explicit free/approval word.
- Don't make it clickable; pair it with a Button.
