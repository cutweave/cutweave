# ADR-NNNN: Short title in the imperative ("Use X for Y")

- **Status:** Proposed | Accepted | Accepted with fallback: <what> | Rejected | Superseded by ADR-MMMM
- **Date:** YYYY-MM-DD (date of the status change)
- **Deciders:** names or roles; "Cutweave maintainers" for project-level decisions
- **Supersedes:** none | ADR-LLLL
- **Architecture refs:** `docs/01-architecture.md` §N, §M

> Process: [docs/06-dev-process.md](../06-dev-process.md) §9. The orchestrator may draft an ADR; only maintainers accept it.
> A **Proposed** ADR names the spike or work package that decides it and its pass criteria. It does not pre-empt the result.
> Delete these hint lines and every `<…>` placeholder before opening the PR. No section may be empty.

## Context

What forces are at play? State the problem, the constraints (product brief, architecture, budgets) and why a decision is needed now. Link evidence rather than restating it.

## Decision drivers

- Driver 1 (for example "Linux/Wayland must work", "contributor and agent familiarity", "determinism")
- Driver 2

## Considered options

1. **Option A (chosen or leading):** one-line description.
2. **Option B:** one-line description.
3. **Option C:** one-line description.

At least two options besides "do nothing" for an Accepted ADR. Say why each rejected option was rejected.

## Decision

State the decision in full sentences, in the active voice ("We will …"). For a Proposed ADR, write **Decided by:** the spike or WP and its pass criteria, and describe the fallback. Do not decide it here.

## Consequences

**Positive**

- What gets easier or becomes possible.

**Negative / costs**

- What gets harder, what we now owe (migrations, tests, docs), and the risks we accept.

**Follow-ups**

- Work packages, tests or later ADRs this decision creates.

## Links

- Architecture: `../01-architecture.md` §N
- Research / spike report: `../research/NN-name.md`, `spikes/<id>/REPORT.md`
- Related ADRs and work packages
- Evidence (measurements, benchmarks, legal review) for Accepted ADRs
