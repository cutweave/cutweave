# wbs-to-issues

Parses every work package in [`docs/wbs/*.md`](../../docs/wbs/), lints the breakdown, writes the normalized snapshot [`docs/wbs/wbs.json`](../../docs/wbs/wbs.json), and syncs GitHub issues (WP-0A.07). The WBS markdown stays the source of truth ([06-dev-process §4](../../docs/06-dev-process.md)).

**Why Python, not TypeScript.** The WP text suggests a TS CLI. This implementation is a single stdlib-only Python file: no `pnpm install`, no network, no build step, so `just wbs-lint` runs offline in any CI job and on a fresh checkout. `scripts/check-md-links.py` already sets the precedent, and Python 3 is on every runner. Node's type stripping would work too, but a Python script needs no flag and no transpile step. The JSON snapshot is the stable interface, so a port stays cheap.

## Usage

```
python scripts/wbs-to-issues/wbs_to_issues.py                 # lint (offline), exit 1 on problems
python scripts/wbs-to-issues/wbs_to_issues.py --check         # lint + fail if wbs.json is stale  (just wbs-lint)
python scripts/wbs-to-issues/wbs_to_issues.py --write         # lint + regenerate wbs.json       (just wbs-json)
python scripts/wbs-to-issues/wbs_to_issues.py --sync --repo cutweave/cutweave [--phase P0] [--dry-run]
```

`just wbs-lint` also runs the unit tests (`python -m unittest discover -s scripts/wbs-to-issues`) and is part of `just check`.

## What it parses

A block starts at `### WP-<id> — <title>` and runs to the next heading. Fields are `- **Name:**` bullets (a parenthetical after the name, as in `**Acceptance criteria (security tests):**`, is accepted): Area, Depends on, Size, Executor, Scope, Interfaces, Acceptance criteria, Tests, Out of scope. Executors may carry a note (`opus (security review by human)`): the first word is `executor`, the rest is `executor_note`.

Dependencies accept `WP-1A.01`, bare `5A.02`, ranges `5A.02–5A.06`, epic codes `E1B` / `epic 1B`, and ignore parenthetical remarks and prose references. **An epic code expands to every WP of that epic** (excluding the WP itself); `depends` in `wbs.json` is the expanded list, `depends_wps` / `depends_epics` / `depends_raw` keep the original.

## Lint rules (exit 1)

Missing id/title/Area/Depends/Size/Executor/Scope/Acceptance, empty acceptance criteria, id not `WP-<phase><epic>.<nn>`, duplicate id, size not `S`/`M`, executor not `sonnet|opus|human`, unknown WP id or epic in dependencies, self-dependency, dependency cycles (after epic expansion; reported as a concrete `a -> b -> a` path), and a `phase:`, `epic:`, `size:` or `exec:` label missing from `.github/labels.yml`. Errors are printed as `file:line`. Missing Interfaces / Tests / Out of scope are **warnings** only.

## Sync

Issues are matched by the hidden marker `<!-- wp:WP-0A.07 -->` in the body, so it is idempotent and independent of the title. Each body ends with `<!-- wbs-hash:... -->`; an issue is edited only when the rendered body, the title (`WP-0A.07: <title>`) or the label set differs.

- Labels: `type:work-package`, `phase:Pn`, `epic:nX`, `size:S|M`, `exec:*`. Labels missing on the repo are created from `.github/labels.yml`, **never in `--dry-run`**. Sync only adds labels to existing issues; it never removes any.
- Dependencies: a `### Blocked by` checklist of issue references (`- [ ] #12 WP-1A.01: title`; ticked when that issue is closed). Pass 1 creates missing issues, pass 2 re-renders bodies once every number is known.
- The body also has a Phase / Epic / Size / Executor / Status table for the project board. Setting Project v2 fields is not automated (needs the `project` token scope); it is out of scope here.
- Nothing is ever closed or deleted. `--dry-run` reads issues and labels with `gh` (read-only; if that fails it assumes an empty repo) and prints creates, updates and no-ops. Requires an authenticated `gh`.
