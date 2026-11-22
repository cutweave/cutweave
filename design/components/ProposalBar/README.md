ProposalBar is the banner pinned above the timeline when an AI agent has proposed a set of edits that the user can review, accept or reject.

**When to use**
- While an open `proposal` transaction exists (the timeline shows ghost clips).
- Whenever an agent action spends money or changes many clips at once.
- Not for finished agent edits (they appear in history with an ActorBadge) or simple tips.

**Anatomy**
`.cw-proposal` (`agent-subtle` fill, dashed `agent` border, `radius-md`, `space-3` padding) > agent badge (`.cw-badge--agent`: `agent` fill, `on-agent` text, sparkles) > `__text`: `__title` (type: body with body-strong lead-in) + `__cost` line (type: caption, `text-secondary`) > `__actions`: Review (secondary), Reject (secondary), Accept (`.cw-btn--agent`, `on-agent` text).

**The consumer provides**
The proposal id, a human summary ("Agent proposes 6 changes: remove 14 silences, add captions"), a change count, time saved, a cost estimate with provider, and whether approval is required.

**States**
Pending (default), reviewing (Review steps through changes; Accept stays enabled), approval-needed (cost line shows warning icon and "Needs approval"), accepting (buttons disabled while committing), resolved (bar collapses with `duration-slow`).

**Keyboard & accessibility**
`role="region"` with a label; focus moves to Review when a proposal arrives without stealing focus from text fields. Enter accepts only when Accept is focused; Escape rejects only from the bar. Dashed outline plus the word "Agent" carries meaning beyond color.

**Command mapping**
Accept -> `proposal.accept {id}` (commits the whole transaction as one undo step); Reject -> `proposal.reject {id}` (discards ghosts); Review -> `proposal.review {id}`. The agent created it via the `proposal` transaction over MCP; costs are shown via `ai.estimate`.

**Do**
- Keep the agent fill on Accept only, so one button owns the decision.
- Always show a cost line, even when it is "Free (local)".
- Pair `agent-subtle` with the dashed `agent` border every time.

**Don't**
- Don't use marigold here; agent blue means AI-proposed.
- Don't let Accept run without showing a cost for paid models.
- Don't stack multiple ProposalBars; merge into one with a count.
