#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Runs only the gate steps that the changed files can affect (`just check-affected`).

`just check` runs everything. This script diffs against a base commit, maps the changed files
to workspace crates and packages, adds every crate that depends on a changed crate (normal,
dev and build edges, transitively, from `cargo metadata`), and runs the same steps as
`just check` restricted to that set. Changes to shared configuration (root Cargo.toml,
Cargo.lock, toolchain, justfile, this script) fall back to the full gate. When the desktop app,
the editor (or a package it is built from, the UI kit among them), the engine or the CLI is
affected, the user-flow suite runs last (`just e2e-flows`, ADR-0018); the full gate runs it too.

Base commit:
  * on `main`: the first parent of HEAD (the pre-merge main after a merge commit);
  * on any other branch: the merge base with `main`;
  * `--base <rev>` overrides both. Uncommitted changes are always included.

Usage: python scripts/check-affected.py [--base REV] [--dry-run]
"""

import argparse
import json
import subprocess
import sys
from pathlib import PurePosixPath

# Crates that must compile to wasm32 (keep in sync with xtask/src/main.rs WASM_CRATES).
WASM_CRATES = {"cutweave-model", "cutweave-commands", "cutweave-render", "cutweave-text", "cutweave-tokens"}
# Crates whose registry, types or events feed `cargo xtask gen` (schema/, packages/protocol/src/generated).
GEN_INPUT_CRATES = {
    "cutweave-model", "cutweave-commands", "cutweave-commands-derive", "cutweave-ops",
    "cutweave-jobs", "cutweave-engine", "xtask",
}
# Crates the Electron bridge fixture build depends on.
BRIDGE_CRATES = {"cutweave-bridge", "cutweave-playback", "cutweave-model"}
# The user-flow suite (ADR-0018) runs the app against the engine and the CLI; a change to either
# (or to any crate they depend on, through `affected`) runs it.
FLOW_CRATES = {"cutweave-engine", "cutweave-cli"}
# The desktop app, the editor and the packages it is built from (the UI kit among them).
FLOW_PATHS = (
    "apps/desktop/", "packages/editor/", "packages/ui/", "packages/timeline/",
    "packages/protocol/", "packages/tokens/", "design/",
)
FLOW_STEP = "just e2e-flows"

# Any change here can affect everything: run the full gate.
FULL_GATE_FILES = {
    "Cargo.toml", "rust-toolchain.toml", "rust-toolchain", "justfile",
    "deny.toml", "clippy.toml", "rustfmt.toml", ".cargo/config.toml",
    "scripts/check-affected.py",
}
JS_ROOT_FILES = {
    "package.json", "pnpm-lock.yaml", "pnpm-workspace.yaml", "biome.json", "biome.jsonc",
    "tsconfig.json", "tsconfig.base.json", "vitest.config.ts", "scripts/run-vitest.py",
}
JS_EXTS = {".ts", ".tsx", ".js", ".mjs", ".cjs", ".css", ".json", ".html"}


def git(*args: str) -> str:
    return subprocess.run(["git", *args], check=True, capture_output=True, text=True,
                          encoding="utf-8").stdout.strip()


def base_rev(explicit: str | None) -> str:
    if explicit:
        return explicit
    branch = git("rev-parse", "--abbrev-ref", "HEAD")
    if branch == "main":
        return "HEAD~1"
    return git("merge-base", "HEAD", "main")


def changed_files(base: str, committed_only: bool = False) -> list[str]:
    committed = git("diff", "--name-only", f"{base}", "HEAD").splitlines()
    worktree = [] if committed_only else git("diff", "--name-only", "HEAD").splitlines()
    untracked = [] if committed_only else git("ls-files", "--others", "--exclude-standard").splitlines()
    return sorted({p.replace("\\", "/") for p in committed + worktree + untracked if p})


def workspace_graph() -> tuple[dict[str, str], dict[str, set[str]], set[str]]:
    """Returns (crate dir → package name, package name → workspace packages that depend on it,
    packages with a library target)."""
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--offline"],
        check=True, capture_output=True, text=True, encoding="utf-8").stdout)
    root = PurePosixPath(meta["workspace_root"].replace("\\", "/"))
    members = set(meta["workspace_members"])
    by_id = {p["id"]: p for p in meta["packages"] if p["id"] in members}
    dirs = {}
    for p in by_id.values():
        manifest = PurePosixPath(p["manifest_path"].replace("\\", "/"))
        dirs[str(manifest.parent.relative_to(root))] = p["name"]
    libs = {p["name"] for p in by_id.values()
            if any(k in ("lib", "rlib", "proc-macro") for t in p["targets"] for k in t["kind"])}
    rdeps: dict[str, set[str]] = {p["name"]: set() for p in by_id.values()}
    for node in meta["resolve"]["nodes"]:
        if node["id"] not in by_id:
            continue
        dependent = by_id[node["id"]]["name"]
        for dep in node["deps"]:
            if dep["pkg"] in by_id:
                rdeps[by_id[dep["pkg"]]["name"]].add(dependent)
    return dirs, rdeps, libs


def crate_of(path: str, dirs: dict[str, str]) -> str | None:
    best = None
    for d, name in dirs.items():
        if d != "." and (path == d or path.startswith(d + "/")):
            if best is None or len(d) > len(best[0]):
                best = (d, name)
    return best[1] if best else None


def plan(files: list[str]) -> tuple[list[tuple[str, str]], str]:
    """Returns the steps to run as (command, reason) and a summary line."""
    if not files:
        return [], "no changes"
    full = sorted(f for f in files if f in FULL_GATE_FILES or f.startswith(".cargo/"))
    # A lockfile change without any crate manifest change (e.g. `cargo update`) can move any
    # third-party version under any crate: full gate. With a manifest change, the crate that
    # added the dependency is in the changed set and deny/check-deps run below.
    if "Cargo.lock" in files and not any(f.endswith("/Cargo.toml") for f in files):
        full.append("Cargo.lock (without a crate manifest change)")
    if full:
        return [
            ("just check", f"shared config changed: {', '.join(full)}"),
            (FLOW_STEP, "full gate: the app, the editor and the engine are all affected"),
        ], "full gate"

    dirs, rdeps, libs = workspace_graph()
    changed_crates = {c for f in files if (c := crate_of(f, dirs))}
    affected = set(changed_crates)
    stack = list(changed_crates)
    while stack:
        for dependent in rdeps.get(stack.pop(), ()):
            if dependent not in affected:
                affected.add(dependent)
                stack.append(dependent)

    manifests = any(f.endswith("Cargo.toml") or f == "Cargo.lock" for f in files)
    js = any(
        f in JS_ROOT_FILES
        or ((f.startswith("packages/") or f.startswith("apps/")) and PurePosixPath(f).suffix in JS_EXTS)
        for f in files
    )
    md = any(f.endswith(".md") for f in files)
    wbs = any(f.startswith("docs/wbs/") or f.startswith("scripts/wbs-to-issues/") for f in files)
    tokens = any(f.startswith("design/") or f.startswith("packages/tokens/") for f in files) \
        or "cutweave-tokens" in affected
    literals = any(
        (f.startswith("packages/") or f.startswith("apps/") or f == "scripts/check-no-literals.py")
        and PurePosixPath(f).suffix in JS_EXTS | {".py"}
        for f in files
    )
    gen = bool(affected & GEN_INPUT_CRATES) or any(
        f.startswith("schema/") or f.startswith("packages/protocol/") or f == "docs/reference/commands.md"
        for f in files
    )
    bridge = bool(affected & BRIDGE_CRATES) or any(f.startswith("apps/desktop/") for f in files)

    steps: list[tuple[str, str]] = []
    if affected:
        pkgs = " ".join(f"-p {c}" for c in sorted(affected))
        why = f"changed {', '.join(sorted(changed_crates))} (+{len(affected) - len(changed_crates)} dependents)"
        steps += [
            ("cargo fmt --all --check", "Rust sources changed"),
            (f"cargo clippy {pkgs} --all-targets -- -D warnings", why),
            (f"cargo nextest run {pkgs} --no-tests=pass", why),
        ]
        doc_pkgs = sorted(affected & libs)
        if doc_pkgs:
            steps.append((f"cargo test {' '.join(f'-p {c}' for c in doc_pkgs)} --doc", why))
        if affected & WASM_CRATES:
            steps.append(("cargo xtask check-wasm", f"wasm-safe crate affected: {', '.join(sorted(affected & WASM_CRATES))}"))
        if manifests:
            steps.append(("cargo xtask check-deps", "a Cargo.toml changed"))
            steps.append(("cargo deny check", "a Cargo.toml changed"))
    if any(not f.endswith((".md", ".json", ".snap", ".lock", ".yaml", ".yml", ".txt")) for f in files):
        steps.append(("cargo xtask license-headers --check", "source files changed"))
    if tokens:
        steps.append(("cargo xtask tokens --check", "design tokens or their generator affected"))
    if wbs:
        steps.append(("just wbs-lint", "WBS docs or scripts changed"))
    if literals:
        steps.append(("just lint-literals", "UI sources changed"))
    if md:
        steps.append(("python scripts/check-md-links.py", "Markdown changed"))
    if js:
        steps += [
            ("pnpm lint", "TS/JS sources or config changed"),
            ("pnpm format:check", "TS/JS sources or config changed"),
            ("pnpm typecheck", "TS/JS sources or config changed"),
        ]
    if bridge:
        steps.append(("just bridge --fixture", "bridge, playback or desktop app affected"))
    if js:
        steps.append(("python scripts/run-vitest.py", "TS/JS sources or config changed"))
    if gen:
        steps.append(("cargo xtask gen --check", "schema/codegen inputs affected"))
    flow_paths = sorted({f.split("/")[0] + "/" + f.split("/")[1] for f in files
                         if f.startswith(FLOW_PATHS) and not f.endswith(".md")})
    if affected & FLOW_CRATES or flow_paths:
        why = ", ".join(sorted(affected & FLOW_CRATES) + flow_paths)
        steps.append((FLOW_STEP, f"user flows affected: {why}"))
    return steps, f"{len(affected)} crate(s) affected"


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--base", help="commit to diff against")
    ap.add_argument("--dry-run", action="store_true", help="print the plan without running it")
    ap.add_argument("--head", default="HEAD", help="commit to check instead of HEAD (implies --committed-only; with --dry-run, for planning past merges)")
    ap.add_argument("--committed-only", action="store_true", help="ignore uncommitted and untracked files")
    args = ap.parse_args()

    if args.head != "HEAD" and not args.dry_run:
        ap.error("--head only plans (use it with --dry-run); checks always run on the working tree")
    base = base_rev(args.base)
    if args.head != "HEAD":
        files = sorted(git("diff", "--name-only", base, args.head).splitlines())
    else:
        files = changed_files(base, args.committed_only)
    steps, summary = plan(files)
    print(f"check-affected: base {git('rev-parse', '--short', base)}, {len(files)} file(s) changed, {summary}")
    for cmd, why in steps:
        print(f"  - {cmd}    [{why}]")
    if not steps:
        print("check-affected: nothing to check")
        return 0
    if args.dry_run:
        return 0
    for cmd, _ in steps:
        print(f"\n==> {cmd}", flush=True)
        if subprocess.run(cmd, shell=True).returncode != 0:
            print(f"\ncheck-affected: FAILED at: {cmd}", flush=True)
            return 1
    print("\ncheck-affected: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
