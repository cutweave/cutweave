#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Checks a Cutweave project archive against its file list (`archive.json`, WP-2F.04b).

`project.archive` (File > Archive project…) writes a folder or a zip with the project, its media
and `archive.json`, which gives the size and SHA-256 of every file. This script reads every file
again and compares. Usage:

    python scripts/verify-archive.py <archive folder | archive.zip>

It prints one line per problem and exits 1 if there is any: a missing file, a wrong size or hash,
a file the list does not name (the engine's own `.cutweave/` folder aside), or (in a zip) a
compressed entry. It needs only Python's standard library and writes nothing.
"""

import hashlib
import json
import sys
import zipfile
from pathlib import Path

MANIFEST = "archive.json"
# The engine's bookkeeping folder of a project folder (its lock and journal). It is written
# whenever the project is opened, so it is not part of the archive's list.
BOOKKEEPING = ".cutweave/"
CHUNK = 1 << 20


def sha256(stream) -> tuple[int, str]:
    digest = hashlib.sha256()
    size = 0
    while chunk := stream.read(CHUNK):
        digest.update(chunk)
        size += len(chunk)
    return size, digest.hexdigest()


def check(manifest: dict, names: set[str], open_file) -> list[str]:
    """Compares the manifest with the files `names` (paths with `/`), read through `open_file`."""
    problems = []
    if manifest.get("format") != "cutweave-archive" or manifest.get("hash") != "sha256":
        return [f"{MANIFEST} is not a Cutweave archive list this script knows"]
    listed = set()
    for file in manifest["files"]:
        path = file["path"]
        listed.add(path)
        if path not in names:
            problems.append(f"missing: {path}")
            continue
        with open_file(path) as stream:
            size, digest = sha256(stream)
        if size != file["size"]:
            problems.append(f"wrong size: {path} is {size} bytes, the list says {file['size']}")
        elif digest != file["hash"]:
            problems.append(f"wrong content: {path}")
    if manifest.get("project") not in listed:
        problems.append("the list does not name the project document")
    problems += [f"not in the list: {name}" for name in sorted(names - listed - {MANIFEST})]
    return problems


def verify_folder(folder: Path) -> list[str]:
    if not (folder / MANIFEST).is_file():
        return [f"no {MANIFEST} in {folder}"]
    manifest = json.loads((folder / MANIFEST).read_text(encoding="utf-8"))
    names = {p.relative_to(folder).as_posix() for p in folder.rglob("*") if p.is_file()}
    names = {name for name in names if not name.startswith(BOOKKEEPING)}
    return check(manifest, names, lambda path: (folder / path).open("rb"))


def verify_zip(path: Path) -> list[str]:
    with zipfile.ZipFile(path) as archive:
        files = [i for i in archive.infolist() if not i.is_dir()]
        tops = {i.filename.split("/", 1)[0] for i in files}
        if len(tops) != 1 or any("/" not in i.filename for i in files):
            return ["the zip does not hold exactly one folder"]
        top = tops.pop() + "/"
        problems = [
            f"compressed entry: {i.filename}" for i in files if i.compress_type != zipfile.ZIP_STORED
        ]
        names = {i.filename[len(top) :] for i in files}
        if MANIFEST not in names:
            return [f"no {MANIFEST} in the zip"]
        manifest = json.loads(archive.read(top + MANIFEST).decode("utf-8"))
        return problems + check(manifest, names, lambda p: archive.open(top + p))


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    target = Path(sys.argv[1])
    if target.is_dir():
        problems = verify_folder(target)
    elif target.is_file():
        problems = verify_zip(target)
    else:
        print(f"{target} is neither a folder nor a file", file=sys.stderr)
        return 2
    for problem in problems:
        print(problem)
    if problems:
        print(f"{target}: {len(problems)} problem(s)")
        return 1
    print(f"{target}: every file matches {MANIFEST}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
