#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Run the MCP Inspector CLI against Cutweave on both transports (WP-1O.03, `just mcp-inspect`).

- stdio: the Inspector spawns `cutweave mcp --headless --no-log --dev-stub-media`;
- Streamable HTTP: this script starts `cutweave engine --headless` and passes the run-file's
  `mcp` token as a bearer header.

For each transport it runs `tools/list` (at most 19 tools, the P1 gate) and one read-only
`tools/call` that needs no project (`cutweave_search_commands`; the CLI opens one connection per
invocation, so a multi-step edit is the rmcp suite's job). Any non-zero exit, unparsable
output or budget breach fails the run (exit 1). Needs Node (npx) and network access to fetch
the pinned Inspector; the rmcp-based suite (`cargo test -p cutweave-cli --test mcp_conformance`)
needs neither.

Usage: python scripts/mcp-inspect.py [--bin target/debug/cutweave] [--only stdio|http]
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

# v1 line: the v2 line needs Node >= 24.22 while .nvmrc pins 24.19. v1 only gets security fixes
# (npm tag `v1-latest`); move to v2 when .nvmrc moves.
INSPECTOR = "@modelcontextprotocol/inspector@1.0.6"
MAX_TOOLS = 19


def inspector(*args: str) -> dict:
    npx = shutil.which("npx") or "npx"
    cmd = [npx, "--yes", INSPECTOR, "--cli", *args]
    shown = ["Authorization: Bearer <token>" if a.startswith("Authorization:") else a for a in cmd]
    print("+", " ".join(shown), flush=True)
    out = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=180)
    try:
        result = json.loads(out.stdout)
    except json.JSONDecodeError as e:
        sys.exit(f"inspector failed ({out.returncode}), output is not JSON ({e}):\n{out.stdout}\n{out.stderr}")
    # Node on Windows aborts at exit after printing the result when the transport is HTTP (libuv
    # `UV_HANDLE_CLOSING` assertion, 0xC0000409; the npx wrapper then exits 1): the complete JSON
    # result on stdout is the verdict in that one case.
    if out.returncode != 0:
        if "UV_HANDLE_CLOSING" not in out.stderr + out.stdout:
            sys.exit(f"inspector failed ({out.returncode}):\n{out.stdout}\n{out.stderr}")
        print(f"(inspector aborted at exit, code {out.returncode}, after a complete result)")
    return result


def check(label: str, target: list[str]) -> None:
    listed = inspector(*target, "--method", "tools/list")
    tools = listed.get("tools", [])
    print(f"{label}: tools/list -> {len(tools)} tools")
    if not 0 < len(tools) <= MAX_TOOLS:
        sys.exit(f"{label}: {len(tools)} tools, expected 1..={MAX_TOOLS}")
    called = inspector(
        *target,
        "--method",
        "tools/call",
        "--tool-name",
        "cutweave_search_commands",
        "--tool-arg",
        "query=split",
    )
    if called.get("isError"):
        sys.exit(f"{label}: tools/call failed: {called}")
    if "clip.split" not in json.dumps(called.get("structuredContent", called)):
        sys.exit(f"{label}: tools/call result has no clip.split: {called}")
    print(f"{label}: tools/call ok")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--bin", default="target/debug/cutweave")
    parser.add_argument("--only", choices=["stdio", "http"])
    args = parser.parse_args()
    binary = str(Path(args.bin).resolve())
    work = tempfile.mkdtemp(prefix="cutweave-inspect-")
    env = {k: v for k, v in os.environ.items() if not k.startswith("CUTWEAVE_")}

    if args.only in (None, "stdio"):
        check(
            "stdio",
            [binary, "mcp", "--headless", "--no-log", "--dev-stub-media", "--allow-root", work],
        )

    if args.only in (None, "http"):
        run_dir = Path(work) / "run"
        engine = subprocess.Popen(
            [binary, "engine", "--headless", "--port", "0", "--no-log", "--dev-stub-media",
             "--run-dir", str(run_dir), "--allow-root", work],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            env=env,
        )
        try:
            ready = json.loads(engine.stdout.readline())
            run = json.loads((run_dir / f"{ready['pid']}.json").read_text())
            token = next(t["token"] for t in run["agent_tokens"] if t["label"] == "mcp")
            check(
                "http",
                [
                    f"http://127.0.0.1:{run['port']}/mcp",
                    "--transport",
                    "http",
                    "--header",
                    f"Authorization: Bearer {token}",
                ],
            )
        finally:
            engine.kill()
            engine.wait()
    shutil.rmtree(work, ignore_errors=True)
    print("mcp-inspect: ok")


if __name__ == "__main__":
    main()
