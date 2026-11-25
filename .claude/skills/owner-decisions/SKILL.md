---
name: owner-decisions
description: Put pending product decisions (ADRs, open spec questions, milestone sign-offs) to the Cutweave product owner with recommendations, and record the answers consistently across ADRs, the index, the architecture log, specs and the WBS. Use when the owner asks to go through decisions, or after they accept or reject something.
---

# Owner decisions

Only maintainers (the product owner) accept ADRs. Agents draft them as **Proposed** and build only up to their spike; a contract change merges only after acceptance.

## Asking
- **On autopilot,** don't ask. Queue the decision in the integrator memory's owner list and move on to unblocked work.
- **When the owner is present and wants to go through them:** collect the open questions from the ADRs, specs ("Open questions" sections, decision tables D-n) and reports.
  - Use `AskUserQuestion` with **at most 4 questions per call**.
  - Each question gets one line of context in plain words and 2–3 options, the recommended one first, labelled "(Recommended)", with a one-line description of what each choice means for the product.
- **Group by topic:** milestone sign-off; one ADR's questions, then "approve the ADR?"; and so on.
- **Save the answers immediately.** Write them to memory first: a session restart loses unsaved answers.

## Recording an acceptance (all of these, in one commit)
- **ADR file:** `- **Status:** Accepted (product owner, YYYY-MM-DD)`. Under `## Decision`, add one line listing the confirmed choices.
- **`docs/adr/README.md`:** the row's status becomes `Accepted` and decided-by `Product owner (YYYY-MM-DD)`.
- **`docs/01-architecture.md` §23:** the row's last cell becomes `**Accepted** (product owner, YYYY-MM-DD)`. Add the row if it's missing.
- **References:** search for `ADR-00NN, Proposed`, `proposed in ADR-00NN` and "must be Accepted before". Update them in the architecture doc, the WBS and specs.
- **Spec header:** e.g. `> **Status:** approved by the product owner on YYYY-MM-DD (ADR-00NN accepted)`. Resolve its open-questions section ("Owner decisions").
- **Consequences:** add new scope to the WP that implements it, e.g. a new param in that WP's Scope. Run `just wbs-json`.
- **Milestone sign-off:** tick the box in `docs/milestones/M<n>-exit.md` with the date and the owner's notes.
- **Check and commit:** `python scripts/check-md-links.py`, `just wbs-lint`, `just spec-check`, then `git commit -s -m "docs(adr): ADR-00NN accepted by the product owner"`.

If the ADR lives on an unmerged branch, commit the acceptance on that branch, so it lands with the code.

## After
- Unblocked WPs go to the front of the agent queue.
- Tell the owner in a few lines what is now unblocked.
