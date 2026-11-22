@AGENTS.md

## Claude-specific

- When orchestrating, act as PM/integrator. Delegate work packages to Sonnet subagents in isolated worktrees, and review their diffs against the architecture contracts before merging (see docs/06-dev-process.md §5).
- Keep `docs/` consistent: when a decision changes, update the architecture/roadmap and add an ADR rather than leaving contradictions in WBS files.
