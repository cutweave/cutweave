#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Machine-wide GPU lock for running several gates on one machine (agents in parallel).

Only one run at a time may use the graphics card: several gates hitting the GPU driver at once
crashed a Windows laptop (2026-11-25). Usage:

    python scripts/gpu-lock.py -- just check-affected --base <rev>
    python scripts/gpu-lock.py -- bash -c ". scripts/ffmpeg/env.sh && cargo nextest run -p cutweave-engine --features ffmpeg"

* A gate (a command mentioning `check-affected`, or a `gate-*.sh` script) is NOT locked as a
  whole: it gets CUTWEAVE_GPU_LOCK_FILE and `scripts/check-affected.py` locks only its GPU steps,
  after building them outside the lock.
* Any other command is GPU work: it holds the lock for its whole run, with
  CUTWEAVE_GPU_LOCK_HELD=1 so nothing inside it waits for the lock again.
* Runs start at below-normal priority so the desktop stays responsive. The OS releases the lock
  when the holder exits or dies; `<lock>.holder` names the holder for anyone waiting.

* On Windows the command's temp dir is `<checkout>/.tmp` (see `scripts/repo_temp.py`).

The lock file is `$CUTWEAVE_GPU_LOCK_FILE` if set, else `<system temp dir>/cutweave-gate.lock`.
"""

import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from datetime import datetime
from pathlib import Path

from repo_temp import use_repo_temp

LOCK =os.environ.get("CUTWEAVE_GPU_LOCK_FILE") or os.path.join(tempfile.gettempdir(), "cutweave-gate.lock")
INFO = LOCK + ".holder"
# On Windows a bare `bash` can resolve to WSL's System32\bash.exe: use Git Bash.
GIT_BASH = r"C:\Program Files\Git\bin\bash.exe"


def say(text: str) -> None:
    try:
        print(text, flush=True)
    except Exception:  # noqa: BLE001 - a status line must never end the wait
        pass


def run(args: list[str], env: dict[str, str]) -> int:
    if os.name == "nt":
        flags = subprocess.BELOW_NORMAL_PRIORITY_CLASS  # type: ignore[attr-defined]
        return subprocess.run(args, creationflags=flags, env=env).returncode
    return subprocess.run(args, env=env, preexec_fn=lambda: os.nice(10)).returncode  # noqa: PLW1509


def try_lock(fh) -> bool:
    fh.seek(0)
    try:
        if os.name == "nt":
            import msvcrt
            msvcrt.locking(fh.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl
            fcntl.flock(fh.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        return True
    except OSError:
        return False


def unlock(fh) -> None:
    fh.seek(0)
    try:
        if os.name == "nt":
            import msvcrt
            msvcrt.locking(fh.fileno(), msvcrt.LK_UNLCK, 1)
        else:
            import fcntl
            fcntl.flock(fh.fileno(), fcntl.LOCK_UN)
    except OSError:
        pass


def main() -> int:
    # A holder's command line can contain any character; the console may be cp1252.
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(errors="replace")
        except (AttributeError, ValueError):
            pass
    args = sys.argv[1:]
    if args and args[0] == "--":
        args = args[1:]
    if not args:
        print(__doc__)
        return 2
    if os.name == "nt" and args[0] in ("bash", "sh", "bash.exe") and os.path.isfile(GIT_BASH):
        args = [GIT_BASH, *args[1:]]
    elif os.name == "nt":
        # `pnpm`, `npx` and friends are .cmd shims that CreateProcess finds only by their full path.
        found = shutil.which(args[0])
        if found:
            args = [found, *args[1:]]
    joined = " ".join(args)
    # LOCK was resolved from the system temp dir above; only what we start moves to `.tmp`.
    use_repo_temp(Path(__file__).resolve().parent.parent)
    env = {**os.environ, "CUTWEAVE_GPU_LOCK_FILE": LOCK}

    if os.environ.get("CUTWEAVE_GPU_LOCK_HELD") == "1":  # already inside a locked run
        return run(args, env)
    if "check-affected" in joined or re.search(r"[\\/]gate-[\w.-]*\.sh\b", joined):
        say("[gpu-lock] gate: only its GPU steps take the lock")
        return run(args, env)

    with open(LOCK, "a+") as fh:
        waited = 0
        while not try_lock(fh):
            if waited % 60 == 0:
                try:
                    holder = open(INFO, encoding="utf-8", errors="replace").read().strip()
                except OSError:
                    holder = "unknown"
                say(f"[gpu-lock] waiting for the GPU ({waited}s); held by: {holder}")
            time.sleep(5)
            waited += 5
        try:
            try:
                with open(INFO, "w", encoding="utf-8") as f:
                    f.write(f"pid {os.getpid()} since {datetime.now():%H:%M:%S} in {os.getcwd()}: {joined}")
            except OSError:
                pass
            say(f"[gpu-lock] acquired after {waited}s")
            env["CUTWEAVE_GPU_LOCK_HELD"] = "1"
            return run(args, env)
        finally:
            unlock(fh)


if __name__ == "__main__":
    sys.exit(main())
