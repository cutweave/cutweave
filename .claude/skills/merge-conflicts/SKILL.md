---
name: merge-conflicts
description: Resolve Cutweave's recurring merge conflicts — generated files, the ADR index and architecture decision-log tables, user-flow rows, wbs.json — and allocate or renumber ADR and flow numbers. Use during any integration merge with conflicts, and before giving an ADR or flow a number.
---

# Resolve the usual conflicts

## Generated files: never hand-merge
`schema/`, `packages/protocol/src/generated/`, `docs/reference/commands.md`,
`crates/cutweave-commands/src/schema_hash.rs`, `docs/wbs/wbs.json`:
```bash
git checkout --ours <file>      # or --theirs; it doesn't matter, they're regenerated next
cargo xtask gen                 # schemas, TS types, commands.md, schema hash
just wbs-json                   # wbs.json (wbs-lint fails if it's stale)
git add -A
```
**Only these files.** Never `git checkout --ours/--theirs` a hand-written file (e.g. `testing/strategies.rs`, menus, i18n): it drops the other side's non-conflicting additions too. Resolve the conflict hunks only, keeping both sides' additions.

## Decision tables: keep every row, in number order
`docs/adr/README.md` (rows like `| [00NN] link | … | Status | Decided by |`) and `docs/01-architecture.md`
§23 (`| 0NN | … | Status |`). Resolve with a script (UTF-8, LF), then check each row's status:
```python
import pathlib, re
def key(l):
    m = re.match(r"^\| \[?(\d{3,4})", l); return int(m.group(1)) if m else 9999
for f in ["docs/adr/README.md", "docs/01-architecture.md"]:
    p = pathlib.Path(f); t = p.read_text(encoding="utf-8")
    pat = re.compile(r"<<<<<<< [^\n]*\n(.*?)=======\n(.*?)>>>>>>> [^\n]*\n", re.S)
    def fix(m):
        rows = {}
        for l in (m.group(1) + m.group(2)).splitlines():
            if l.strip(): rows.setdefault(key(l), l)   # prefer the first side's version of a row
        return "\n".join(rows[k] for k in sorted(rows)) + "\n"
    p.write_text(pat.sub(fix, t), encoding="utf-8", newline="\n")
```
- Prefer the row with the newer status: **Accepted** beats Proposed.
- Afterwards, re-sort the whole §23 block. Rows outside the conflict hunk can be out of order.
- `docs/user-flows.md`: keep both rows, ordered by F-number.

## Numbers: check every branch before assigning
ADR numbers and flow numbers (F-n) collide when parallel agents pick "the next free one".
- **Before an agent gets a number,** look at main and every open branch:
  ```bash
  for b in $(git for-each-ref --format='%(refname:short)' refs/heads); do
    git ls-tree -r --name-only $b docs/adr | grep -oE '[0-9]{4}' ; done | sort -u | tail
  git grep -hoE '\| F[0-9]+ \|' $(git for-each-ref --format='%(refname:short)' refs/heads) -- docs/user-flows.md | sort -u | tail
  ```
- **On a collision,** the branch that lands later is renumbered:
  - `git mv docs/adr/00NN-x.md docs/adr/00MM-x.md`;
  - replace `ADR-00NN` and the file name everywhere on that branch: ADR text, README row, arch §23 row (written `| 0NN |`), WBS, specs, code comments;
  - flows: rename the `F-n` in `docs/user-flows.md`, the spec's title, and any references.

## Other conflicts
- **WBS text:** keep the merged WP's "as built" lines and any later additions. If a WP landed before a dependency it was planned on, move that piece into the later WP rather than keeping a dependency on finished work.
- **Docs-only changes carried on a branch with unrelated commits:** `git cherry-pick -x <sha>` instead of merging the branch.

## Editing safely
- **Edit docs with Python:** `read_text(encoding="utf-8")` / `write_text(..., encoding="utf-8", newline="\n")`.
- **Never use `sed` on text containing Windows paths:** `\U` and `\c` in the replacement mangle it.
- **Keep LF line endings.** Biome and the gates fail on CRLF.
