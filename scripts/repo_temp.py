# SPDX-License-Identifier: GPL-3.0-or-later
"""Point the temp dir of a test run at `<checkout>/.tmp` on Windows.

Test runs write thousands of temp files: an Electron profile per user flow, engine caches and
proxy videos, Node and Playwright caches. In the system temp dir Microsoft Defender scans each
of them as it is written; dev machines exclude the checkout from real-time scanning, so files
under `.tmp` are not scanned (docs/06-dev-process.md §8). `scripts/gpu-lock.py` calls this
before starting its command, so a gate run through it (every step) inherits it.

* Windows only: the cost is Defender's, and on Unix a deep temp path could break socket paths.
* `CUTWEAVE_KEEP_TEMP=1` keeps the system temp dir.
* Entries untouched for two days are deleted when a run starts.
"""

import os
import shutil
import tempfile
import time
from pathlib import Path

ENV = "CUTWEAVE_REPO_TEMP"
KEEP_ENV = "CUTWEAVE_KEEP_TEMP"
KEEP_DAYS = 2


def use_repo_temp(checkout: Path) -> str | None:
    """Sets TMP, TEMP and TMPDIR to `<checkout>/.tmp`; returns it, or None when left alone."""
    if os.name != "nt" or os.environ.get(KEEP_ENV) == "1":
        return None
    if os.environ.get(ENV):  # an outer run already chose one; keep it
        return os.environ[ENV]
    tmp = checkout / ".tmp"
    try:
        tmp.mkdir(exist_ok=True)
    except OSError:
        return None
    prune(tmp)
    for key in ("TMP", "TEMP", "TMPDIR"):
        os.environ[key] = str(tmp)
    os.environ[ENV] = str(tmp)
    tempfile.tempdir = None  # this process's own temp files follow too
    return str(tmp)


def prune(tmp: Path, days: float = KEEP_DAYS) -> None:
    """Deletes entries of `tmp` untouched for `days`; files still in use are skipped."""
    cutoff = time.time() - days * 86400
    try:
        entries = list(tmp.iterdir())
    except OSError:
        return
    for entry in entries:
        try:
            if entry.lstat().st_mtime >= cutoff:
                continue
            if entry.is_dir() and not entry.is_symlink():
                shutil.rmtree(entry, ignore_errors=True)
            else:
                entry.unlink()
        except OSError:
            pass
