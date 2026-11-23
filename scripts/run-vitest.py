#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Runs the Vitest suite (`pnpm test`) for the local gate, retrying once on worker-start timeouts.

Vitest 5 hard-codes a 60 s worker start timeout. When the gate runs while other builds saturate
the machine, jsdom workers sometimes take longer to start and the run reports
"[vitest-pool]: Failed to start ... worker" although no test failed. This wrapper reruns the
suite once in exactly that case: the run failed, a worker failed to start, and no test or file
failed. Any real failure is reported as is, without a retry.
"""

import re
import subprocess
import sys

START_FAILURE = re.compile(r"\[vitest-pool\]: (Failed to start|Timeout starting)")
TEST_FAILURE = re.compile(r"(Tests|Test Files)\s+\d+ failed")


def run() -> tuple[int, str]:
    proc = subprocess.Popen(
        "pnpm test",
        shell=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    assert proc.stdout is not None
    lines = []
    for line in proc.stdout:
        sys.stdout.write(line)
        lines.append(line)
    return proc.wait(), "".join(lines)


def main() -> int:
    # Vitest prints box-drawing and check-mark characters; Windows defaults a redirected stdout
    # to cp1252, which cannot encode them.
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    code, out = run()
    if code == 0:
        return 0
    if START_FAILURE.search(out) and not TEST_FAILURE.search(out):
        print(
            "\nrun-vitest: only worker-start timeouts (machine under load); retrying once\n",
            flush=True,
        )
        code, _ = run()
    return code


if __name__ == "__main__":
    sys.exit(main())
