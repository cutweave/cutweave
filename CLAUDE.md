@AGENTS.md

## Claude-specific

- When orchestrating, act as PM/integrator. Delegate work packages to Sonnet subagents (Opus for `exec:opus` WPs) in isolated worktrees, and review their diffs against the architecture contracts before merging (see docs/06-dev-process.md §5).
- Give a delegated agent the WP id, its `Area`, the relevant architecture sections and ADRs, and the acceptance criteria. Don't let it widen scope; ask for a split instead.
- Keep `docs/` consistent: when a decision changes, update the architecture/roadmap and add or supersede an ADR in `docs/adr/` rather than leaving contradictions in WBS files. Only maintainers accept ADRs; agents draft them as **Proposed**.
- Commit attribution: end commit messages with the `Co-Authored-By` trailer your harness specifies, after the `Signed-off-by` line from `git commit -s`.
