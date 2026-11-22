#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Checks that relative links in tracked Markdown files resolve to existing files.

External (http/https/mailto) links and pure anchors are ignored; anchors on file links are
stripped. Run from anywhere: paths resolve against the repository root.
"""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LINK = re.compile(r"(?<!\!)\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
FENCE = re.compile(r"^\s*(```|~~~)")


def tracked_markdown() -> list[Path]:
    out = subprocess.run(
        ["git", "ls-files", "*.md"], cwd=ROOT, check=True, capture_output=True, text=True
    ).stdout
    return [ROOT / line for line in out.splitlines() if not line.startswith("spikes/")]


def main() -> int:
    bad = 0
    for md in tracked_markdown():
        in_fence = False
        for no, line in enumerate(md.read_text(encoding="utf-8").splitlines(), 1):
            if FENCE.match(line):
                in_fence = not in_fence
                continue
            if in_fence:
                continue
            for target in LINK.findall(line):
                if re.match(r"^([a-z][a-z0-9+.-]*:|#)", target, re.I):
                    continue
                path = target.split("#", 1)[0]
                if not path:
                    continue
                resolved = (ROOT / path.lstrip("/")) if path.startswith("/") else (md.parent / path)
                if not resolved.exists():
                    print(f"{md.relative_to(ROOT)}:{no}: broken link `{target}`")
                    bad += 1
    print(f"docs-lint: {'ok' if bad == 0 else f'{bad} broken link(s)'}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
