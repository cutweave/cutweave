#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""No-literals lint (WP-0K.05, docs/07-design-system.md): UI code uses design tokens only.

Rejects, in packages/*/src (and packages/*/.storybook) CSS, TS and TSX:
  * literal colors: #hex, rgb(/rgba(/hsl(/hsla(/hwb(/lab(/lch(/oklab(/oklch(/color(<space>
  * raw px font sizes: `font-size: 12px`, `fontSize: 12` / `fontSize: "12px"`, `text-[12px]`,
    and `font:` shorthands containing a px size
  * font-family stacks: any `font-family` / `fontFamily` value that is not a `var(--token)`
    (or inherit/initial/unset/revert)

Exempt: generated token files (packages/ui/src/tokens/**, packages/*/src/generated/**), the
bundled-font manifest (packages/ui/src/fonts/fonts.css) and *.test.* files (they assert token
values). Comments are ignored. A line carrying `no-literals: allow` is skipped; use it sparingly
and say why.

Usage:
  python scripts/check-no-literals.py              # scan the repository
  python scripts/check-no-literals.py --self-test  # prove the rules fire on planted fixtures
  python scripts/check-no-literals.py PATH...      # scan specific files (no exemptions applied)
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIXTURES = Path(__file__).resolve().parent / "check-no-literals" / "fixtures"
EXTS = {".css", ".ts", ".tsx"}
ALLOW_MARKER = "no-literals: allow"

COLOR_FUNCS = r"(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch)\("
HEX = r"(?<![\w&#/-])#(?:[0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{4}|[0-9a-fA-F]{3})(?![\w-])"
FAMILY_OK = r"(?:var\(\s*--[\w-]+\s*\)|inherit|initial|unset|revert)"

RULES = [
    ("literal-color", re.compile(HEX)),
    ("literal-color", re.compile(r"\b" + COLOR_FUNCS, re.I)),
    ("literal-color", re.compile(r"\bcolor\(\s*(?:srgb|display-p3|a98-rgb|rec2020|xyz)", re.I)),
    ("raw-font-size", re.compile(r"\bfont-size\s*:\s*[^;}]*?\d(?:\.\d+)?px", re.I)),
    ("raw-font-size", re.compile(r"\bfontSize\s*:\s*(?:\d|[\"'`][^\"'`]*\d(?:\.\d+)?px)")),
    ("raw-font-size", re.compile(r"\btext-\[\d+(?:\.\d+)?px\]")),
    ("raw-font-size", re.compile(r"\bfont\s*:\s*[^;}]*?\d(?:\.\d+)?px", re.I)),
    # `font-family: <value>` where the value is not a var(--x) / CSS-wide keyword.
    (
        "font-family-stack",
        re.compile(r"\bfont-family\s*:\s*(?!" + FAMILY_OK + r"\s*(?:;|}|$))\S", re.I),
    ),
    ("font-family-stack", re.compile(r"\bfontFamily\s*:\s*[\"'`](?!var\(--)")),
]


def strip_comments(text: str, ext: str) -> str:
    """Blank out comments (keeping newlines so line numbers hold). String-aware for TS."""
    out: list[str] = []
    i, n = 0, len(text)
    strings = ("'", '"', "`") if ext != ".css" else ("'", '"')
    line_comments = ext != ".css"
    while i < n:
        c = text[i]
        nxt = text[i + 1] if i + 1 < n else ""
        if c == "/" and nxt == "*":
            j = text.find("*/", i + 2)
            j = n if j < 0 else j + 2
            out.append(re.sub(r"[^\n]", " ", text[i:j]))
            i = j
        elif line_comments and c == "/" and nxt == "/":
            j = text.find("\n", i)
            j = n if j < 0 else j
            out.append(" " * (j - i))
            i = j
        elif c in strings:
            j = i + 1
            while j < n and text[j] != c:
                if c != "`" and text[j] == "\n":
                    break
                j += 2 if text[j] == "\\" else 1
            out.append(text[i : j + 1])
            i = j + 1
        else:
            out.append(c)
            i += 1
    return "".join(out)


def scan_text(text: str, ext: str) -> list[tuple[int, str, str]]:
    raw_lines = text.splitlines()
    findings = []
    for lineno, line in enumerate(strip_comments(text, ext).splitlines(), 1):
        if ALLOW_MARKER in (raw_lines[lineno - 1] if lineno <= len(raw_lines) else ""):
            continue
        for rule, rx in RULES:
            if rx.search(line):
                findings.append((lineno, rule, line.strip()))
                break
    return findings


def exempt(rel: str) -> bool:
    return (
        rel.startswith("packages/ui/src/tokens/")
        or "/src/generated/" in rel
        or rel == "packages/ui/src/fonts/fonts.css"
        or re.search(r"\.test\.[cm]?[jt]sx?$", rel) is not None
    )


def repo_files() -> list[Path]:
    files = []
    for pattern in ("packages/*/src", "packages/*/.storybook"):
        for base in ROOT.glob(pattern):
            for p in sorted(base.rglob("*")):
                if p.suffix in EXTS and p.is_file() and "node_modules" not in p.parts:
                    files.append(p)
    return files


def scan_paths(paths: list[Path], apply_exemptions: bool) -> int:
    bad = 0
    for p in paths:
        try:
            rel = p.resolve().relative_to(ROOT).as_posix()
        except ValueError:
            rel = p.as_posix()
        if apply_exemptions and exempt(rel):
            continue
        for lineno, rule, line in scan_text(p.read_text(encoding="utf-8"), p.suffix):
            print(f"{rel}:{lineno}: [{rule}] {line}")
            bad += 1
    return bad


def self_test() -> int:
    bad_files = sorted(FIXTURES.glob("bad.*"))
    good_files = sorted(FIXTURES.glob("good.*"))
    if not bad_files or not good_files:
        print("self-test: fixtures missing", file=sys.stderr)
        return 1
    failures = []
    # Every `expect: <rule>` marker in a bad fixture must produce that rule on that line.
    for f in bad_files:
        src = f.read_text(encoding="utf-8")
        found = {(ln, rule) for ln, rule, _ in scan_text(src, f.suffix)}
        for ln, line in enumerate(src.splitlines(), 1):
            m = re.search(r"expect: ([\w-]+)", line)
            if m and (ln, m.group(1)) not in found:
                failures.append(f"{f.name}:{ln}: expected [{m.group(1)}] was NOT reported")
    # The headline case from the acceptance criteria.
    planted = scan_text("a { color: #fff; }\n", ".css")
    if [r for _, r, _ in planted] != ["literal-color"]:
        failures.append("planted `color: #fff` was not rejected")
    for f in good_files:
        for ln, rule, line in scan_text(f.read_text(encoding="utf-8"), f.suffix):
            failures.append(f"{f.name}:{ln}: false positive [{rule}] {line}")
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print(f"no-literals self-test ok ({len(bad_files)} bad, {len(good_files)} good fixtures)")
    return 0


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return self_test()
    if argv:
        return 1 if scan_paths([Path(a) for a in argv], False) else 0
    files = repo_files()
    bad = scan_paths(files, True)
    if bad:
        print(
            f"\ncheck-no-literals: {bad} literal(s). Use var(--token) values "
            "(design/tokens.json); add a token if one is missing.",
            file=sys.stderr,
        )
        return 1
    print(f"check-no-literals: ok ({len(files)} files scanned)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
