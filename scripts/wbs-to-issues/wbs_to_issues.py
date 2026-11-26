#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""wbs-to-issues: parse docs/wbs/*.md, lint the work breakdown, emit docs/wbs/wbs.json,
and (optionally) sync GitHub issues through the `gh` CLI. Standard library only.

    python scripts/wbs-to-issues/wbs_to_issues.py                # lint (default, offline)
    ... --write                                                  # lint + regenerate docs/wbs/wbs.json
    ... --check                                                  # lint + fail if wbs.json is stale
    ... --sync --repo cutweave/cutweave [--phase P0] [--dry-run]

See scripts/wbs-to-issues/README.md.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WBS_DIR = ROOT / "docs" / "wbs"
SNAPSHOT = WBS_DIR / "wbs.json"
LABELS_YML = ROOT / ".github" / "labels.yml"

FIELDS = {
    "Area": "area",
    "Depends on": "depends",
    "Size": "size",
    "Executor": "executor",
    "Scope": "scope",
    "Interfaces": "interfaces",
    "Acceptance criteria": "acceptance",
    "Tests": "tests",
    "Out of scope": "out_of_scope",
}
EXECUTORS = ("sonnet", "opus", "human")
HEADING_RE = re.compile(r"^### (WP-[^\s]+)\s+[—–-]\s+(.+?)\s*$")
# A WP split after it was filed keeps its number plus a letter (WP-2G.03a, WP-2G.03b).
ID_RE = re.compile(r"^WP-(\d)([A-Z])\.(\d{2})([a-z]?)$")
FIELD_RE = re.compile(r"^- \*\*([A-Za-z ]+?)(?: \([^)]*\))?:\*\*\s*(.*)$")
OPTIONAL = {"interfaces", "tests", "out_of_scope"}  # missing => warning, not error
MARKER_RE = re.compile(r"<!-- wp:(WP-[^\s]+) -->")
HASH_RE = re.compile(r"<!-- wbs-hash:([0-9a-f]+) -->")


# ---------------------------------------------------------------- parsing


def parse_file(path: Path) -> list[dict]:
    rel = path.relative_to(ROOT).as_posix()
    lines = path.read_text(encoding="utf-8").splitlines()
    blocks: list[tuple[int, str, str, list[tuple[int, str]]]] = []
    cur = None
    for i, line in enumerate(lines, 1):
        if line.startswith("### WP-"):
            m = HEADING_RE.match(line)
            cur = (i, m.group(1), m.group(2), []) if m else (i, line[4:].split()[0], "", [])
            blocks.append(cur)
        elif re.match(r"^#{1,3} ", line):
            cur = None
        elif cur is not None:
            cur[3].append((i, line))
    return [parse_block(rel, *b) for b in blocks]


def parse_block(rel: str, line_no: int, wid: str, title: str, body: list[tuple[int, str]]) -> dict:
    errors: list[str] = []
    fields: dict[str, tuple[int, list[str]]] = {}
    current = None
    for n, line in body:
        m = FIELD_RE.match(line)
        if m and m.group(1) in FIELDS:
            key = FIELDS[m.group(1)]
            if key in fields:
                errors.append(f"{rel}:{n}: duplicate field '{m.group(1)}'")
            current = key
            fields[key] = (n, [m.group(2)])
        elif line.startswith("- **") or line.strip() == "---":
            current = None
        elif current is not None:
            fields[current][1].append(line)
    rec: dict = {"id": wid, "title": title, "source": {"file": rel, "line": line_no}, "_errors": errors}
    mid = ID_RE.match(wid)
    if not mid:
        errors.append(f"{rel}:{line_no}: id '{wid}' does not match WP-<phase><epic>.<nn>")
    if not title:
        errors.append(f"{rel}:{line_no}: malformed heading (expected '### WP-<id> — <title>')")
    rec["phase"] = f"P{mid.group(1)}" if mid else ""
    rec["epic"] = f"{mid.group(1)}{mid.group(2)}" if mid else ""
    for label, key in FIELDS.items():
        if key not in fields:
            tag = "warning: " if key in OPTIONAL else ""
            errors.append(f"{tag}{rel}:{line_no}: {wid} is missing field '{label}'")

    def text(key: str) -> str:
        return dedent_md(fields[key][1]) if key in fields else ""

    rec["area"] = text("area")
    size_raw = text("size").strip().strip("*`").strip()
    rec["size"] = size_raw
    if "size" in fields and size_raw not in ("S", "M"):
        errors.append(f"{rel}:{fields['size'][0]}: {wid} size '{size_raw}' is not S or M")
    ex_raw = text("executor").strip()
    first = re.match(r"[A-Za-z]+", ex_raw)
    rec["executor"] = first.group(0).lower() if first else ""
    rec["executor_note"] = ex_raw[first.end():].strip(" :;,-") if first else ""
    if "executor" in fields and rec["executor"] not in EXECUTORS:
        errors.append(f"{rel}:{fields['executor'][0]}: {wid} executor '{ex_raw}' must start with sonnet|opus|human")
    rec["depends_raw"] = text("depends").strip()
    rec["depends_wps"], rec["depends_epics"] = parse_deps(rec["depends_raw"])
    rec["depends"] = []  # filled by expand_deps
    rec["scope"] = text("scope")
    rec["interfaces"] = text("interfaces")
    rec["acceptance"] = bullets(fields["acceptance"][1]) if "acceptance" in fields else []
    if "acceptance" in fields and not rec["acceptance"]:
        errors.append(f"{rel}:{fields['acceptance'][0]}: {wid} has empty acceptance criteria")
    rec["tests"] = text("tests")
    rec["out_of_scope"] = text("out_of_scope")
    return rec


def dedent_md(raw: list[str]) -> str:
    first, rest = raw[0].strip(), list(raw[1:])
    while rest and not rest[-1].strip():
        rest.pop()
    indents = [len(l) - len(l.lstrip()) for l in rest if l.strip()]
    cut = min(indents) if indents else 0
    parts = ([first] if first else []) + [l[cut:].rstrip() for l in rest]
    return "\n".join(parts).strip()


def bullets(raw: list[str]) -> list[str]:
    """Top-level bullets of a field (continuations and nested bullets stay in their item)."""
    items: list[list[str]] = []
    first = raw[0].strip()
    if first:
        items.append([first])
    for line in raw[1:]:
        if not line.strip():
            continue
        if re.match(r"^ {0,3}[-*] ", line):
            items.append([re.sub(r"^ {0,3}[-*] ", "", line).rstrip()])
        elif items:
            items[-1].append(line.strip())
        else:
            items.append([line.strip()])
    return ["\n".join(i) for i in items]


def strip_parens(s: str) -> str:
    out, depth = [], 0
    for ch in s:
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth = max(0, depth - 1)
        elif depth == 0:
            out.append(ch)
    return "".join(out)


DEP_RE = re.compile(
    r"(?:WP-)?(\d[A-Z])\.(\d+)(?:\s*[–-]\s*(?:WP-)?(?:(\d[A-Z])\.)?(\d+))?([a-z](?![A-Za-z0-9]))?"
    r"|\bE(\d[A-Z])\b|\b[Ee]pic\s+(\d[A-Z])\b"
)


def parse_deps(raw: str) -> tuple[list[str], list[str]]:
    """Return (explicit WP ids, epic codes). Accepts WP-1A.01, bare 5A.02, split WPs such as
    WP-2G.03a, ranges such as `5A.02–5A.06`, and epic codes `E1B` / `epic 1B`. Parenthetical
    remarks are ignored."""
    s = strip_parens(raw).replace("*", "").replace("`", "")
    wps: list[str] = []
    epics: list[str] = []
    for m in DEP_RE.finditer(s):
        if m.group(1):
            ep, a = m.group(1), int(m.group(2))
            if m.group(5) and not m.group(4):  # a split work package: WP-2H.03a
                wps.append(f"WP-{ep}.{a:02d}{m.group(5)}")
                continue
            b = int(m.group(4)) if m.group(4) and (not m.group(3) or m.group(3) == ep) else a
            wps.extend(f"WP-{ep}.{n:02d}" for n in range(a, b + 1))
        else:
            epics.append(m.group(6) or m.group(7))
    return dedupe(wps), dedupe(epics)


def dedupe(xs: list[str]) -> list[str]:
    return list(dict.fromkeys(xs))


# ------------------------------------------------------- expansion / lint


def load_wbs() -> list[dict]:
    wps: list[dict] = []
    for f in sorted(WBS_DIR.glob("*.md")):
        wps.extend(parse_file(f))
    return wps


def wp_key(wid: str):
    m = ID_RE.match(wid)
    return (m.group(1), m.group(2), m.group(3), m.group(4)) if m else (wid,)


def expand_deps(wps: list[dict]) -> list[str]:
    """Fill w['depends'] with explicit WP ids (epic deps expanded). Returns problems."""
    by_epic: dict[str, list[str]] = {}
    for w in wps:
        by_epic.setdefault(w["epic"], []).append(w["id"])
    problems = []
    for w in wps:
        deps: list[str] = []
        for e in w["depends_epics"]:
            if e not in by_epic:
                problems.append(f"{w['source']['file']}:{w['source']['line']}: {w['id']} depends on epic E{e}, which has no WPs")
                continue
            deps.extend(x for x in by_epic[e] if x != w["id"])
        deps.extend(w["depends_wps"])
        w["depends"] = sorted(dedupe(deps), key=wp_key)
    return problems


def find_cycles(graph: dict[str, list[str]]) -> list[list[str]]:
    """Strongly connected components with more than one node (iterative Tarjan)."""
    index: dict[str, int] = {}
    low: dict[str, int] = {}
    on: set[str] = set()
    stack: list[str] = []
    counter = 0
    comps: list[list[str]] = []
    for root in graph:
        if root in index:
            continue
        index[root] = low[root] = counter
        counter += 1
        stack.append(root)
        on.add(root)
        work = [(root, iter(graph[root]))]
        while work:
            v, it = work[-1]
            advanced = False
            for u in it:
                if u not in index:
                    index[u] = low[u] = counter
                    counter += 1
                    stack.append(u)
                    on.add(u)
                    work.append((u, iter(graph[u])))
                    advanced = True
                    break
                if u in on:
                    low[v] = min(low[v], index[u])
            if advanced:
                continue
            work.pop()
            if work:
                low[work[-1][0]] = min(low[work[-1][0]], low[v])
            if low[v] == index[v]:
                comp = []
                while True:
                    x = stack.pop()
                    on.discard(x)
                    comp.append(x)
                    if x == v:
                        break
                if len(comp) > 1:
                    comps.append(sorted(comp, key=wp_key))
    return comps


def cycle_path(comp: list[str], graph: dict[str, list[str]]) -> str:
    """One concrete cycle inside a strongly connected set, rendered a -> b -> ... -> a."""
    members = set(comp)
    start = comp[0]
    # BFS from start back to start within the component
    prev: dict[str, str] = {}
    queue = [start]
    seen = {start}
    while queue:
        cur = queue.pop(0)
        for d in graph[cur]:
            if d not in members:
                continue
            if d == start:
                path = [cur]
                while path[-1] != start:
                    path.append(prev[path[-1]])
                # path runs from cur back to start; each node depends on the next one printed
                return " -> ".join([*reversed(path), start])
            if d not in seen:
                seen.add(d)
                prev[d] = cur
                queue.append(d)
    return " -> ".join(comp)


def read_labels() -> list[dict]:
    out = []
    for line in LABELS_YML.read_text(encoding="utf-8").splitlines():
        if line.lstrip().startswith("#"):
            continue
        m = re.search(r'name:\s*"([^"]+)".*?color:\s*"([^"]+)".*?description:\s*"([^"]*)"', line)
        if m:
            out.append({"name": m.group(1), "color": m.group(2), "description": m.group(3)})
    return out


def lint(wps: list[dict]) -> list[str]:
    errs: list[str] = []
    for w in wps:
        errs.extend(e for e in w["_errors"] if not e.startswith("warning: "))
    seen: dict[str, dict] = {}
    for w in wps:
        if w["id"] in seen:
            o = seen[w["id"]]["source"]
            errs.append(f"{w['source']['file']}:{w['source']['line']}: duplicate id {w['id']} (first at {o['file']}:{o['line']})")
        else:
            seen[w["id"]] = w
    errs.extend(expand_deps(wps))
    for w in wps:
        for d in w["depends_wps"]:
            if d not in seen:
                errs.append(f"{w['source']['file']}:{w['source']['line']}: {w['id']} depends on unknown id {d}")
        if w["id"] in w["depends_wps"]:
            errs.append(f"{w['source']['file']}:{w['source']['line']}: {w['id']} depends on itself")
    labels = {l["name"] for l in read_labels()}
    for w in wps:
        for lab in (f"phase:{w['phase']}", f"epic:{w['epic']}", f"size:{w['size']}", f"exec:{w['executor']}"):
            if lab not in labels:
                errs.append(f"{w['source']['file']}:{w['source']['line']}: {w['id']} needs label '{lab}', missing from .github/labels.yml")
    graph = {w["id"]: [d for d in w["depends"] if d in seen and d != w["id"]] for w in wps}
    for comp in find_cycles(graph):
        errs.append(f"dependency cycle ({len(comp)} WPs): " + cycle_path(comp, graph))
    return errs


# --------------------------------------------------------------- snapshot


def to_json(wps: list[dict]) -> str:
    recs = [{k: v for k, v in w.items() if not k.startswith("_")} for w in wps]
    doc = {"version": 1, "generated_by": "scripts/wbs-to-issues", "count": len(recs), "work_packages": recs}
    return json.dumps(doc, indent=2, ensure_ascii=False) + "\n"


# ------------------------------------------------------------- issue body


def render_body(w: dict, numbers: dict[str, int], states: dict[str, str], titles: dict[str, str]) -> str:
    def sec(h: str, t: str) -> str:
        return f"### {h}\n\n{t.strip()}\n" if t.strip() else ""

    ex = f" ({w['executor_note']})" if w["executor_note"] else ""
    parts = [
        f"<!-- wp:{w['id']} -->",
        f"<!-- Generated by scripts/wbs-to-issues. Edit {w['source']['file']} (source of truth), not this body. -->",
        "",
        "| Phase | Epic | Size | Executor | Status |",
        "|---|---|---|---|---|",
        f"| {w['phase']} | {w['epic']} | {w['size']} | {w['executor']}{ex} | Backlog |",
        "",
        f"**Area:** {w['area']}  ",
        f"**Source:** `{w['source']['file']}` line {w['source']['line']}",
        "",
    ]
    dep_lines = []
    for d in w["depends"]:
        t = titles.get(d, "")
        if d in numbers:
            box = "x" if states.get(d) == "CLOSED" else " "
            dep_lines.append(f"- [{box}] #{numbers[d]} {d}" + (f": {t}" if t else ""))
        else:
            dep_lines.append(f"- [ ] {d}" + (f": {t}" if t else "") + " (issue not created yet)")
    dep_txt = "\n".join(dep_lines) if dep_lines else "None."
    note = ""
    if w["depends_epics"]:
        note = (
            "\n\nEpic dependencies ("
            + ", ".join("E" + e for e in w["depends_epics"])
            + ") are expanded to every WP of that epic. Original text: "
            + w["depends_raw"].replace("\n", " ")
        )
    parts.append(f"### Blocked by\n\n{dep_txt}{note}\n")
    parts.append(sec("Scope", w["scope"]))
    parts.append(sec("Interfaces", w["interfaces"]))
    parts.append(sec("Acceptance criteria", "\n".join(f"- [ ] {a}" for a in w["acceptance"])))
    parts.append(sec("Tests", w["tests"]))
    parts.append(sec("Out of scope", w["out_of_scope"]))
    body = "\n".join(parts).rstrip() + "\n"
    digest = hashlib.sha256(body.encode()).hexdigest()[:16]
    return body + f"\n<!-- wbs-hash:{digest} -->\n"


def wp_labels(w: dict) -> list[str]:
    return ["type:work-package", f"phase:{w['phase']}", f"epic:{w['epic']}", f"size:{w['size']}", f"exec:{w['executor']}"]


def wp_title(w: dict) -> str:
    return f"{w['id']}: {w['title']}"


# ------------------------------------------------------------------- sync


def gh(args: list[str], stdin: str | None = None) -> str:
    r = subprocess.run(["gh", *args], input=stdin, capture_output=True, text=True, encoding="utf-8")
    if r.returncode != 0:
        raise RuntimeError(f"gh {' '.join(args[:2])} failed: {r.stderr.strip()}")
    return r.stdout


def needs_update(w: dict, ex: dict, body: str) -> bool:
    old = HASH_RE.search(ex.get("body") or "")
    new = HASH_RE.search(body).group(1)
    have = {l["name"] for l in ex.get("labels", [])}
    return (not old) or old.group(1) != new or ex["title"] != wp_title(w) or any(l not in have for l in wp_labels(w))


def sync(wps: list[dict], repo: str, phase: str | None, dry: bool) -> int:
    selected = [w for w in wps if not phase or w["phase"] == phase]
    titles = {w["id"]: w["title"] for w in wps}
    existing: dict[str, dict] = {}
    labels_present: set[str] = set()
    try:
        raw = gh(["issue", "list", "--repo", repo, "--state", "all", "--limit", "5000",
                  "--json", "number,title,body,state,labels"])
        for it in json.loads(raw):
            m = MARKER_RE.search(it.get("body") or "")
            if m:
                existing[m.group(1)] = it
        labels_present = {l["name"] for l in json.loads(gh(["label", "list", "--repo", repo, "--limit", "1000", "--json", "name"]))}
    except (RuntimeError, OSError, json.JSONDecodeError) as e:
        if not dry:
            print(f"error: cannot read {repo}: {e}", file=sys.stderr)
            return 2
        print(f"warning: could not read {repo} ({e}); dry-run assumes no issues and no labels exist", file=sys.stderr)
    numbers = {k: v["number"] for k, v in existing.items()}
    states = {k: v["state"] for k, v in existing.items()}

    used = {n for w in selected for n in wp_labels(w)}
    missing = [l for l in read_labels() if l["name"] in used and l["name"] not in labels_present]
    for l in missing:
        print(f"{'would create' if dry else 'create'} label {l['name']}")
        if not dry:
            gh(["label", "create", l["name"], "--repo", repo, "--color", l["color"], "--description", l["description"]])

    plan = []
    noops = 0
    for w in selected:
        ex = existing.get(w["id"])
        body = render_body(w, numbers, states, titles)
        if ex is None:
            plan.append(("create", w, None))
        elif needs_update(w, ex, body):
            plan.append(("update", w, ex))
        else:
            noops += 1
    for action, w, ex in plan:
        extra = f" #{ex['number']}" if ex else ""
        print(f"{'would ' if dry else ''}{action}{extra}: {w['id']} {w['title']}  [{', '.join(wp_labels(w))}]  deps={len(w['depends'])}")
    creates = sum(1 for p in plan if p[0] == "create")
    updates = len(plan) - creates
    print(f"summary ({repo}, {phase or 'all phases'}): {creates} create, {updates} update, {noops} no-op, "
          f"{len(missing)} label(s) {'would be ' if dry else ''}created")
    if dry:
        return 0
    # pass 1: create missing issues (so every WP has a number)
    for action, w, _ in plan:
        if action == "create":
            body = render_body(w, numbers, states, titles)
            url = gh(["issue", "create", "--repo", repo, "--title", wp_title(w), "--body-file", "-",
                      *[x for l in wp_labels(w) for x in ("--label", l)]], stdin=body).strip()
            numbers[w["id"]] = int(url.rsplit("/", 1)[1])
            states[w["id"]] = "OPEN"
    # pass 2: re-render with all issue numbers known; edit only what changed
    for w in selected:
        n = numbers.get(w["id"])
        ex = existing.get(w["id"])
        if n is None:
            continue
        body = render_body(w, numbers, states, titles)
        if ex is None:
            gh(["issue", "edit", str(n), "--repo", repo, "--body-file", "-"], stdin=body)
        elif needs_update(w, ex, body):
            have = {l["name"] for l in ex.get("labels", [])}
            add = [x for l in wp_labels(w) if l not in have for x in ("--add-label", l)]
            gh(["issue", "edit", str(n), "--repo", repo, "--title", wp_title(w), "--body-file", "-", *add], stdin=body)
    return 0


# -------------------------------------------------------------------- cli


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--write", action="store_true", help="lint, then regenerate docs/wbs/wbs.json")
    ap.add_argument("--check", action="store_true", help="lint, then fail if docs/wbs/wbs.json is stale")
    ap.add_argument("--sync", action="store_true", help="create/update GitHub issues (needs --repo)")
    ap.add_argument("--repo")
    ap.add_argument("--phase", help="restrict --sync to one phase, e.g. P0")
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args(argv)
    for stream in (sys.stdout, sys.stderr):
        stream.reconfigure(encoding="utf-8", errors="replace")
    if a.sync and not a.repo:
        ap.error("--sync requires --repo")

    wps = load_wbs()
    for w in wps:
        for e in w["_errors"]:
            if e.startswith("warning: "):
                print("wbs-lint " + e, file=sys.stderr)
    errs = lint(wps)
    if errs:
        print(f"wbs-lint: {len(errs)} problem(s) in {len(wps)} work packages\n", file=sys.stderr)
        for e in errs:
            print("  " + e, file=sys.stderr)
        return 1
    per_phase: dict[str, int] = {}
    for w in wps:
        per_phase[w["phase"]] = per_phase.get(w["phase"], 0) + 1
    print(f"wbs-lint: ok, {len(wps)} work packages (" + ", ".join(f"{k}:{v}" for k, v in sorted(per_phase.items())) + ")")
    text = to_json(wps)
    if a.write:
        SNAPSHOT.write_text(text, encoding="utf-8", newline="\n")
        print(f"wrote {SNAPSHOT.relative_to(ROOT).as_posix()}")
    if a.check:
        cur = SNAPSHOT.read_text(encoding="utf-8") if SNAPSHOT.exists() else ""
        if cur != text:
            print("wbs-lint: docs/wbs/wbs.json is stale; run `just wbs-json` and commit it", file=sys.stderr)
            return 1
        print("wbs-lint: docs/wbs/wbs.json is up to date")
    if a.sync:
        return sync(wps, a.repo, a.phase, a.dry_run)
    return 0


if __name__ == "__main__":
    sys.exit(main())
