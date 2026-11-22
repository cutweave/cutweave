# SPDX-License-Identifier: GPL-3.0-or-later
"""Checks one WP-0H.03 agent run against the S5 pass criteria (see ../MCP-AGENT-RUN.md).

Usage:
    python check_run.py --state run-1.state.json [--transcript run-1.claude.jsonl] [--server cutweave]

--state       the file `s5-mcp stdio --state-file` keeps current (client-independent).
--transcript  Claude Code `--output-format stream-json --verbose` output (optional; other clients:
              check their transcript by hand against the same criteria).

Exit code 0 when every applicable check passes.
"""

import argparse
import json
import re
import sys

FLICKS = 705_600_000
FPS = 30  # the spike's default sequence rate


def to_flicks(value):
    """Mirror of the spike's Time parser (integer flicks, "2.5s", "HH:MM:SS:FF", "f:N")."""
    if isinstance(value, int):
        return value
    if not isinstance(value, str):
        return None
    if m := re.fullmatch(r"(\d+(?:\.\d+)?)s", value):
        return round(float(m.group(1)) * FLICKS)
    if m := re.fullmatch(r"(\d+):([0-5]\d):([0-5]\d):(\d+)", value):
        h, mi, s, f = (int(g) for g in m.groups())
        return ((h * 3600 + mi * 60 + s) * FPS + f) * (FLICKS // FPS)
    if m := re.fullmatch(r"f:(\d+)", value):
        return int(m.group(1)) * (FLICKS // FPS)
    return None


def check_state(path, results):
    state = json.load(open(path, encoding="utf-8"))
    timeline = state["timeline"]
    media = timeline["media"]
    tracks = [t for t in timeline["tracks"] if t["items"]]
    results.append(("exactly one media imported", len(media) == 1, f"{len(media)} media"))
    ok_shape = len(tracks) == 1 and len(tracks[0]["items"]) == 2
    results.append(("one track with exactly two items", ok_shape, f"{[len(t['items']) for t in tracks]}"))
    if ok_shape:
        a, b = sorted(tracks[0]["items"], key=lambda i: i["start"])
        start = a["start"]
        cut_ok = a["end"] == b["start"] == start + 2 * FLICKS
        results.append(("cut at 2 s into the clip", cut_ok, f"left [{a['start']}, {a['end']}), right [{b['start']}, {b['end']})"))
        same = a["media"] == b["media"] and b["source_in"] == 2 * FLICKS
        results.append(("both parts come from the imported media", same, ""))
    actors = sorted({c["actor"] for c in state["changes"]})
    results.append(("every change made by one agent actor", len(actors) == 1 and actors[0].startswith("agent:"), f"{actors}"))
    labels = [c["label"] for c in state["changes"]]
    results.append(("change log", True, " -> ".join(labels)))


def check_transcript(path, server, results):
    prefix = f"mcp__{server}__"
    tool_uses, tool_results, init, final = {}, {}, None, None
    for line in open(path, encoding="utf-8"):
        line = line.strip()
        if not line:
            continue
        msg = json.loads(line)
        if msg.get("type") == "system" and msg.get("subtype") == "init":
            init = msg
        elif msg.get("type") == "result":
            final = msg
        for block in (msg.get("message") or {}).get("content") or []:
            if not isinstance(block, dict):
                continue
            if block.get("type") == "tool_use":
                tool_uses[block["id"]] = block
            elif block.get("type") == "tool_result":
                tool_results[block["tool_use_id"]] = block
    names = [u["name"] for u in tool_uses.values()]
    foreign = [n for n in names if not n.startswith(prefix)]
    if init is not None:
        servers = {s.get("name"): s.get("status") for s in init.get("mcp_servers", [])}
        results.append((f"MCP server `{server}` connected", servers.get(server) == "connected", f"{servers}"))
    results.append(("only Cutweave MCP tools used (no screenshots / computer use / shell)", not foreign, f"other tools: {foreign}" if foreign else f"{len(names)} calls"))
    shown = False
    for use_id, use in tool_uses.items():
        name, args = use["name"], use.get("input") or {}
        if name == prefix + "cutweave_query" and args.get("name") == "render.frame":
            args = args.get("args") or {}
        elif name != prefix + "render_frame":
            continue
        if to_flicks(args.get("at")) != 3 * FLICKS:
            continue
        content = (tool_results.get(use_id) or {}).get("content") or []
        if isinstance(content, list) and any(isinstance(c, dict) and c.get("type") == "image" for c in content):
            shown = True
    results.append(("frame at 3 s rendered and returned as an image", shown, ""))
    if final is not None:
        results.append(("run finished without error", final.get("subtype") == "success" and not final.get("is_error"), f"turns={final.get('num_turns')} cost_usd={final.get('total_cost_usd')}"))
    results.append(("tool sequence", True, " -> ".join(n.removeprefix(prefix) for n in names)))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--state", required=True)
    parser.add_argument("--transcript")
    parser.add_argument("--server", default="cutweave")
    args = parser.parse_args()
    results = []
    check_state(args.state, results)
    if args.transcript:
        check_transcript(args.transcript, args.server, results)
    failed = 0
    for name, ok, detail in results:
        failed += not ok
        print(f"{'PASS' if ok else 'FAIL'}  {name}" + (f"  ({detail})" if detail else ""))
    print("VERDICT:", "PASS" if failed == 0 else f"FAIL ({failed})")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
