#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Pinned sources of the Cutweave FFmpeg bundle (WP-0A.06).

    sources.py check                       validate versions.toml
    sources.py version                     print the pinned FFmpeg version
    sources.py fetch [--out DIR] [--os OS]  download / clone every pinned source, verify it against
                                           its sha256 or commit, and write the source tree that
                                           build-deps.sh / build-ffmpeg.sh consume

The source tree (default: third_party/ffmpeg/cutweave-ffmpeg-<version>-source) is also the GPL
"corresponding source" published next to every bundle:

    upstream/      pristine upstream tarballs and `git archive` snapshots of pinned commits
    scripts/       the build scripts and workflow that produced the bundle
    versions.toml  the pins;  pins.env  the same pins for the shell scripts
    SOURCES.txt    human-readable list (origin, pin, license);  SHA256SUMS  for upstream/

Standard library only. Needs Python >= 3.11 (tomllib), curl and git on PATH.
"""

from __future__ import annotations

import argparse
import hashlib
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
VERSIONS = HERE / "versions.toml"
OSES = ("windows", "macos", "linux")

HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
NAME = re.compile(r"^[a-z][a-z0-9_]*$")
FILENAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._+-]*$")

TARBALL_KEYS = {"version", "url", "sha256", "mirrors", "license", "os", "strip"}
GIT_KEYS = {"version", "git", "tag", "branch", "commit", "license", "os", "strip"}


class PinError(Exception):
    """A pin is malformed or a fetched source does not match its pin."""


def load(path: Path = VERSIONS) -> dict:
    with open(path, "rb") as f:
        return tomllib.load(f)


def components(data: dict) -> list[tuple[str, dict]]:
    """FFmpeg first, then the dependencies in file order."""
    return [("ffmpeg", data.get("ffmpeg", {})), *data.get("deps", {}).items()]


def validate(data: dict) -> list[str]:
    errors: list[str] = []
    if data.get("schema") != 1:
        errors.append("`schema = 1` is required")
    if "ffmpeg" not in data:
        errors.append("missing [ffmpeg] table")
    elif "url" not in data["ffmpeg"]:
        errors.append("[ffmpeg] must be pinned as a release tarball (url + sha256)")
    files: dict[str, str] = {}
    for name, c in components(data):
        where = "[ffmpeg]" if name == "ffmpeg" else f"[deps.{name}]"
        if not NAME.match(name):
            errors.append(f"{where}: name must match {NAME.pattern}")
        for key in ("version", "license", "os"):
            if key not in c:
                errors.append(f"{where}: missing `{key}`")
        oses = c.get("os", [])
        if not isinstance(oses, list) or not oses or any(o not in OSES for o in oses):
            errors.append(f"{where}: `os` must be a non-empty list of {', '.join(OSES)}")
        strip = c.get("strip", 1)
        if not isinstance(strip, int) or strip < 0:
            errors.append(f"{where}: `strip` must be a non-negative integer")
        is_tar, is_git = "url" in c, "git" in c
        if is_tar == is_git:
            errors.append(f"{where}: needs exactly one of `url` (tarball) or `git`")
            continue
        if is_tar:
            unknown = set(c) - TARBALL_KEYS
            if not HEX64.match(str(c.get("sha256", ""))):
                errors.append(f"{where}: `sha256` must be 64 lowercase hex digits")
            urls = [c["url"], *c.get("mirrors", [])]
            if any(not str(u).startswith("https://") for u in urls):
                errors.append(f"{where}: every url and mirror must use https")
            fname = archive_name(name, c)
            if not FILENAME.match(fname):
                errors.append(f"{where}: unusable file name `{fname}` in url")
            if fname in files:
                errors.append(f"{where}: file name `{fname}` collides with [{files[fname]}]")
            files[fname] = name
        else:
            unknown = set(c) - GIT_KEYS
            if not HEX40.match(str(c.get("commit", ""))):
                errors.append(f"{where}: `commit` must be a full 40-hex commit id")
            if ("tag" in c) == ("branch" in c):
                errors.append(f"{where}: needs exactly one of `tag` or `branch`")
            if not str(c["git"]).startswith("https://"):
                errors.append(f"{where}: `git` must be an https url")
        if unknown:
            errors.append(f"{where}: unknown key(s) {', '.join(sorted(unknown))}")
    meson = data.get("tools", {}).get("meson_pip")
    if meson is not None and not re.match(r"^[0-9][0-9.]*$", str(meson)):
        errors.append("[tools] meson_pip must be a plain version number")
    return errors


def archive_name(name: str, c: dict) -> str:
    if "url" in c:
        return str(c["url"]).rsplit("/", 1)[-1]
    return f"{name}-{c['commit'][:12]}.tar.gz"


def sha256_of(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def run(cmd: list[str], **kwargs) -> subprocess.CompletedProcess:
    print("+ " + " ".join(shlex.quote(c) for c in cmd), flush=True)
    return subprocess.run(cmd, check=True, **kwargs)


def output(cmd: list[str]) -> str:
    return subprocess.run(cmd, check=True, capture_output=True, text=True).stdout.strip()


def fetch_tarball(name: str, c: dict, dest_dir: Path) -> Path:
    dest = dest_dir / archive_name(name, c)
    if dest.exists() and sha256_of(dest) == c["sha256"]:
        print(f"{name}: {dest.name} already present and verified")
        return dest
    part = dest.with_name(dest.name + ".part")
    for url in [c["url"], *c.get("mirrors", [])]:
        part.unlink(missing_ok=True)
        try:
            run(["curl", "-fL", "--retry", "5", "--retry-delay", "5", "--connect-timeout", "30",
                 "--silent", "--show-error", "-o", str(part), url])
        except subprocess.CalledProcessError:
            print(f"warning: {name}: download failed from {url}", file=sys.stderr)
            continue
        got = sha256_of(part)
        if got == c["sha256"]:
            part.replace(dest)
            print(f"{name}: {dest.name} sha256 ok")
            return dest
        print(f"warning: {name}: {url} has sha256 {got}, pinned {c['sha256']}", file=sys.stderr)
    part.unlink(missing_ok=True)
    raise PinError(f"{name}: no url produced a file matching the pinned sha256")


def fetch_git(name: str, c: dict, dest_dir: Path, scratch: Path) -> Path:
    commit = c["commit"]
    dest = dest_dir / archive_name(name, c)
    repo = scratch / name
    # Blob-less clone: full history for the tag/branch checks, file contents only for the commit.
    run(["git", "clone", "--quiet", "--filter=blob:none", "--no-checkout", c["git"], str(repo)])
    git = ["git", "-C", str(repo), "-c", "advice.detachedHead=false"]
    run([*git, "checkout", "--quiet", "--detach", commit])
    head = output([*git, "rev-parse", "HEAD"])
    if head != commit:
        raise PinError(f"{name}: checked out {head}, pinned {commit}")
    if "tag" in c:
        tagged = output([*git, "rev-parse", "--verify", f"refs/tags/{c['tag']}^{{commit}}"])
        if tagged != commit:
            raise PinError(f"{name}: tag {c['tag']} points at {tagged}, pinned {commit}")
    else:
        ref = f"refs/remotes/origin/{c['branch']}"
        if subprocess.run([*git, "merge-base", "--is-ancestor", commit, ref]).returncode != 0:
            raise PinError(f"{name}: {commit} is not on branch {c['branch']}")
    # `git archive` is reproducible (mtime = commit time, internal gzip without a timestamp).
    run([*git, "archive", "--format=tar.gz", f"--prefix={name}/", "-o", str(dest), commit])
    print(f"{name}: {dest.name} from {c['git']} @ {commit}")
    return dest


def origin(c: dict) -> str:
    if "url" in c:
        return f"{c['url']} (sha256 {c['sha256']})"
    ref = f"tag {c['tag']}" if "tag" in c else f"branch {c['branch']}"
    return f"{c['git']} ({ref}, commit {c['commit']})"


def shell_assign(var: str, value: str) -> str:
    return f"{var}={shlex.quote(value)}"


def write_text(path: Path, text: str) -> None:
    path.write_text(text, encoding="utf-8", newline="\n")


def copy_scripts(out: Path) -> None:
    """The scripts that control compilation are part of the GPL corresponding source."""
    target = out / "scripts" / "ffmpeg"
    shutil.rmtree(out / "scripts", ignore_errors=True)
    shutil.copytree(HERE, target, ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
    extra = {
        ROOT / "scripts" / "check-glibc.sh": out / "scripts" / "check-glibc.sh",
        ROOT / ".github" / "workflows" / "ffmpeg.yml": out / "scripts" / "ffmpeg.yml",
    }
    for src, dst in extra.items():
        if src.exists():
            shutil.copy2(src, dst)


def cmd_fetch(args: argparse.Namespace) -> int:
    data = load()
    errors = validate(data)
    if errors:
        for e in errors:
            print(f"versions.toml: {e}", file=sys.stderr)
        return 1
    version = data["ffmpeg"]["version"]
    out = Path(args.out or ROOT / "third_party" / "ffmpeg" / f"cutweave-ffmpeg-{version}-source")
    upstream = out / "upstream"
    upstream.mkdir(parents=True, exist_ok=True)
    wanted = [(n, c) for n, c in components(data) if not args.os or args.os in c["os"]]

    fetched: list[tuple[str, dict, Path]] = []
    # Git marks pack files read-only; tolerate that when cleaning up on Windows.
    with tempfile.TemporaryDirectory(prefix="cw-ffmpeg-git-", ignore_cleanup_errors=True) as scratch:
        for name, c in wanted:
            if "url" in c:
                path = fetch_tarball(name, c, upstream)
            else:
                path = fetch_git(name, c, upstream, Path(scratch))
            fetched.append((name, c, path))
    keep = {p.name for _, _, p in fetched}
    for stale in upstream.iterdir():
        if stale.name not in keep:
            stale.unlink()

    sums = {p.name: sha256_of(p) for _, _, p in fetched}
    write_text(out / "SHA256SUMS", "".join(f"{sums[p.name]}  upstream/{p.name}\n"
                                           for _, _, p in fetched))

    lines = [
        f"Corresponding source of the Cutweave FFmpeg {version} bundles",
        "",
        "Generated from scripts/ffmpeg/versions.toml by scripts/ffmpeg/sources.py. Every file",
        "listed here is in upstream/ of the source archive",
        f"cutweave-ffmpeg-{version}-source.tar.zst, next to the build scripts (scripts/) that",
        "configure, compile and package the bundles. Rebuild instructions:",
        "scripts/ffmpeg/README.md.",
        "",
    ]
    for name, c, path in fetched:
        lines += [
            f"{name} {c['version']}",
            f"  license:  {c['license']}",
            f"  bundles:  {', '.join(c['os'])}",
            f"  origin:   {origin(c)}",
            f"  file:     upstream/{path.name}",
            f"  sha256:   {sums[path.name]}",
            "",
        ]
    write_text(out / "SOURCES.txt", "\n".join(lines))

    env = [
        "# Generated by scripts/ffmpeg/sources.py from versions.toml; do not edit.",
        shell_assign("CW_FFMPEG_VERSION", version),
        shell_assign("CW_COMPONENTS", " ".join(n for n, _, _ in fetched)),
    ]
    for name, c, path in fetched:
        env += [
            shell_assign(f"CW_ARCHIVE_{name}", f"upstream/{path.name}"),
            shell_assign(f"CW_STRIP_{name}", str(c.get("strip", 1))),
            shell_assign(f"CW_OS_{name}", " ".join(c["os"])),
            shell_assign(f"CW_VERSION_{name}", str(c["version"])),
            shell_assign(f"CW_LICENSE_{name}", str(c["license"])),
            shell_assign(f"CW_ORIGIN_{name}", origin(c)),
        ]
    write_text(out / "pins.env", "\n".join(env) + "\n")

    shutil.copy2(VERSIONS, out / "versions.toml")
    copy_scripts(out)
    write_text(out / "README.txt", "\n".join([
        f"Cutweave FFmpeg {version} corresponding source",
        "",
        "See SOURCES.txt for every component, its license and upstream origin, and",
        "scripts/ffmpeg/README.md for how to rebuild the bundles from this tree:",
        "",
        "  CW_SOURCES=$PWD bash scripts/ffmpeg/build-deps.sh",
        "  CW_SOURCES=$PWD bash scripts/ffmpeg/build-ffmpeg.sh",
        "  CW_SOURCES=$PWD bash scripts/ffmpeg/package.sh",
        "",
    ]))
    print(f"source tree: {out}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="cmd", required=True)
    sub.add_parser("check", help="validate versions.toml")
    sub.add_parser("version", help="print the pinned FFmpeg version")
    fetch = sub.add_parser("fetch", help="download and verify every pinned source")
    fetch.add_argument("--out", help="output directory (default: third_party/ffmpeg/...-source)")
    fetch.add_argument("--os", choices=OSES, help="only the components this bundle needs")
    args = parser.parse_args()

    try:
        if args.cmd == "check":
            errors = validate(load())
            for e in errors:
                print(f"versions.toml: {e}", file=sys.stderr)
            if not errors:
                print(f"versions.toml: ok ({len(components(load()))} components)")
            return 1 if errors else 0
        if args.cmd == "version":
            print(load()["ffmpeg"]["version"])
            return 0
        return cmd_fetch(args)
    except (PinError, subprocess.CalledProcessError, OSError) as e:
        print(f"error: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
