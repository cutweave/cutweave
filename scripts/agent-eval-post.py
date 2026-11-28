#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""One HTTPS POST to the Anthropic Messages API for the agent evaluation (WP-2I.09).

`cutweave-bench agent-eval` builds every request and reads every response itself; this
script only carries one request over HTTPS, because the workspace has no HTTP client with
TLS (reqwest is built without it, for loopback only) and the evaluation must not add one.
Python's standard library only: no pip installs.

    python scripts/agent-eval-post.py [--url URL] [--timeout SECONDS]  < request.json

- The request body is read from stdin and sent as it is.
- The key comes from the environment variable ANTHROPIC_API_KEY and from nowhere else. It is
  sent in the `x-api-key` header and never printed.
- stdout is one JSON object: {"status": 200, "retry_after": null, "request_id": "...",
  "body": {...}}. A request that never got an answer is {"status": 0, "error": "..."}.
- The exit code is 0 whenever such an object was printed, 2 when the script was misused
  (no key, an address that is neither https nor this machine).

It never runs in the gate or in CI: a test only ever starts it against a listener on
127.0.0.1 with a made-up key.
"""

import argparse
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request

DEFAULT_URL = "https://api.anthropic.com/v1/messages"
# The API version the request shapes in crates/cutweave-bench/src/agent_eval/anthropic.rs
# were written against (https://platform.claude.com/docs/en/api/versioning).
API_VERSION = "2026-10-21"
LOOPBACK = {"127.0.0.1", "localhost", "::1"}


def allowed(url: str) -> bool:
    """The key only ever goes to an https address, or to this machine (the test)."""
    parts = urllib.parse.urlsplit(url)
    if parts.scheme == "https":
        return bool(parts.hostname)
    return parts.scheme == "http" and parts.hostname in LOOPBACK


def answer(status: int, headers, raw: bytes) -> dict:
    text = raw.decode("utf-8", errors="replace")
    try:
        body = json.loads(text)
    except ValueError:
        body = text
    return {
        "status": status,
        "retry_after": headers.get("retry-after") if headers else None,
        "request_id": headers.get("request-id") if headers else None,
        "body": body,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--url", default=DEFAULT_URL)
    parser.add_argument("--timeout", type=float, default=600.0)
    args = parser.parse_args()

    key = os.environ.get("ANTHROPIC_API_KEY", "")
    if not key:
        print("agent-eval-post: ANTHROPIC_API_KEY is not set", file=sys.stderr)
        return 2
    if not allowed(args.url):
        print("agent-eval-post: the address must be https (or this machine)", file=sys.stderr)
        return 2

    request = urllib.request.Request(
        args.url,
        data=sys.stdin.buffer.read(),
        method="POST",
        headers={
            "content-type": "application/json",
            "x-api-key": key,
            "anthropic-version": API_VERSION,
            "user-agent": "cutweave-agent-eval",
        },
    )
    # No proxy from the environment for this machine's own address (the test).
    if urllib.parse.urlsplit(args.url).hostname in LOOPBACK:
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    else:
        opener = urllib.request.build_opener()
    try:
        with opener.open(request, timeout=args.timeout) as response:
            out = answer(response.status, response.headers, response.read())
    except urllib.error.HTTPError as e:
        out = answer(e.code, e.headers, e.read())
    except (urllib.error.URLError, OSError, ValueError) as e:
        # The message of a failed connection names the host, never the key.
        out = {"status": 0, "error": f"{type(e).__name__}: {e}"}
    sys.stdout.write(json.dumps(out))
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
