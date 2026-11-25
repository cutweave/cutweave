#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""The M1 exit gate (WP-1O.04, `just m1-gate`): runs the M1 acceptance suite on this machine and
reports pass or fail per exit criterion (docs/wbs/P1-skeleton.md, "Exit criteria (M1)"; the
report is docs/milestones/M1-exit.md).

  1. Through the UI: import 3 clips, arrange, trim, split, export a 1080p MP4 that plays.
     `e2e/m1/m1-exit.spec.ts` (the M1 corpus clips, exact inputs), the user flow F14 (pointer
     drags), and the UI's export checked here: it decodes without errors, its sound is not
     silent, and its frames match `cutweave render` of the saved project (PSNR >= 38 dB in the
     export's 4:2:0 colour space, the tolerance policy's bound for a lossy MP4 against a
     rendered frame; the RGB round trip is reported too).
  2. Through MCP headless: a scripted client does the same and exports an identical-timeline file.
     The MCP conformance suite with the M1 scripted agent (`mcp_conformance`, built with FFmpeg),
     then the two saved projects compared by normalized structure (ids, machine paths and
     timestamps replaced or dropped, `normalize_project`) and the two exports by streams, length
     (within a frame) and decoded frames (equal hashes at every 10th frame, else PSNR >= 45 dB).
  3. Kill the engine mid-edit: the UI recovers with no lost commands. `e2e/m1/m1-crash.spec.ts`
     (`--trials` engine kills mid-command and mid-drag, `--trials` whole-app kills restored from
     the journal) and the user flows F11 and F12.
  4. CI green (R34: on Windows, with local gates), golden frames pass, the parity test passes.
     4a `just check` and `just ui-snapshots` (only with `--full`: they take long), 4b `cargo
     xtask golden` (the software adapter), 4c the CLI = MCP = RPC parity test.

Artefacts go to `target/m1/` (logs/, ui/, mcp/, crash/, compare.json, report.json, report.md);
nothing there is committed. `--evidence DIR` also writes the text evidence (the report, the
normalized timeline, ffprobe output, trial tables, the tail of each log) to DIR for the milestone
report. Media is never written there.

Usage:
  python scripts/m1-gate.py [--trials N] [--full] [--skip-build] [--criteria 1,2,3,4]
                            [--evidence DIR] [--golden-base REV]
  python scripts/m1-gate.py compare UI_DIR MCP_DIR   # criterion 2's comparison of two runs
  python scripts/m1-gate.py --self-test              # the normalizer's own checks

Exit code 0 when every criterion that ran passed.
"""

from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "target" / "m1"
EXE = ".exe" if os.name == "nt" else ""
CLI = ROOT / "target" / "debug" / f"cutweave{EXE}"

PROJECT = "M1 cut"
CLIPS = ("h264_1080p60.mp4", "hevc_10bit_sdr.mp4", "vfr_phone_rotated90.mp4")
FPS = 30
LENGTH_S = 12
# Frames of the cut checked against `cutweave render`: A's first half, the HEVC clip, the phone
# clip (rotated, pillarboxed).
RENDER_FRAMES = (30, 150, 255)
# Tolerance policy (docs/wbs/P1-skeleton.md): a decoded lossy MP4 against a rendered frame.
LOSSY_PSNR_DB = 38.0
# WP-1O.04 exit 2: the UI's and the agent's exports are frame-equivalent.
EQUIVALENT_PSNR_DB = 45.0
# A sound track whose loudest sample is below this is silent.
SILENT_DB = -60.0

CRITERIA = {
    "1": "UI: import 3 clips, arrange, trim, split, export a 1080p MP4 that plays",
    "2": "MCP headless: a scripted client does the same and exports an identical-timeline file",
    "3": "Kill the engine mid-edit: the UI recovers with no lost commands",
    "4": "CI green (Windows, local gates, R34); golden frames pass; parity (CLI = MCP = RPC) passes",
}


# ---- normalization ------------------------------------------------------------------------------

ID_RE = re.compile(r"^(med|seq|trk|itm|grp)_[0-9a-f]{32}$")
# Fields that say where and when a document was made, not what it is.
DROPPED_META = ("created", "modified", "app_version")
DROPPED_MEDIA = ("path_abs", "path_rel")


def file_name(path: str) -> str:
    return re.split(r"[\\/]", path)[-1] if path else ""


def normalize_project(doc: dict) -> dict:
    """The document's structure without what depends on the run: every entity id is replaced by a
    name derived from the structure (media by file name, tracks by their place in the stack,
    items by track and start, linked groups by first appearance), media paths become file names,
    and `meta.created`, `meta.modified` and `meta.app_version` are dropped. Two documents with
    the same content normalize to equal values. An id this does not know stays as it is, so the
    comparison fails on it."""
    labels: dict[str, str] = {}
    media = doc.get("media", {})
    names = {mid: file_name(m.get("path_abs") or m.get("path_rel") or "") for mid, m in media.items()}
    for mid in sorted(media, key=lambda i: (names[i], media[i].get("content_hash") or "")):
        label, n = f"media:{names[mid]}", 2
        while label in labels.values():
            label, n = f"media:{names[mid]}#{n}", n + 1
        labels[mid] = label

    seqs = sorted(doc.get("sequences", {}).values(), key=lambda s: str(s.get("name")))
    ordered: list[tuple[dict, list[tuple[dict, list[dict]]]]] = []
    for si, s in enumerate(seqs):
        labels[s["id"]] = f"sequence:{si}:{s.get('name')}"
        tracks = s.get("tracks", {})
        order = list(s.get("track_order", []))
        order += sorted((t for t in tracks if t not in order), key=lambda t: str(tracks[t].get("name")))
        per_track = []
        for ti, tid in enumerate(order):
            t = tracks[tid]
            labels[tid] = f"track:{ti}:{t.get('name')}"
            items = sorted(t.get("items", {}).values(),
                           key=lambda i: (i.get("start", 0), i.get("duration", 0), i.get("kind", "")))
            for ii, item in enumerate(items):
                labels[item["id"]] = f"item:{t.get('name')}:{ii}"
            per_track.append((t, items))
        ordered.append((s, per_track))
    groups = 0
    for _, per_track in ordered:
        for _, items in per_track:
            for item in items:
                g = item.get("linked_group")
                if isinstance(g, str) and g not in labels:
                    labels[g] = f"group:{groups}"
                    groups += 1

    def relabel(v):
        if isinstance(v, str):
            return labels.get(v, v)
        if isinstance(v, list):
            return [relabel(x) for x in v]
        if isinstance(v, dict):
            return {labels.get(k, k): relabel(x) for k, x in v.items()}
        return v

    out = {k: v for k, v in doc.items() if k not in ("meta", "media", "sequences")}
    out["meta"] = {k: v for k, v in doc.get("meta", {}).items() if k not in DROPPED_META}
    out["media"] = [
        {"file": names[mid], **{k: v for k, v in media[mid].items() if k not in DROPPED_MEDIA}}
        for mid in sorted(media, key=lambda i: labels[i])
    ]
    out["sequences"] = [
        {
            **{k: v for k, v in s.items() if k not in ("tracks", "track_order")},
            "tracks": [
                {**{k: v for k, v in t.items() if k != "items"}, "items": items}
                for t, items in per_track
            ],
        }
        for s, per_track in ordered
    ]
    return relabel(out)


def leftover_ids(value) -> list[str]:
    """Entity ids still in a normalized value (none when the normalizer knew them all)."""
    found: list[str] = []
    if isinstance(value, str) and ID_RE.match(value):
        found.append(value)
    elif isinstance(value, list):
        for x in value:
            found += leftover_ids(x)
    elif isinstance(value, dict):
        for k, x in value.items():
            found += leftover_ids(k) + leftover_ids(x)
    return found


def self_test() -> bool:
    def doc(ids: dict[str, str], folder: str, created: str, duration: int) -> dict:
        m, s, v, a, i1, i2, g = (ids[k] for k in ("m", "s", "v", "a", "i1", "i2", "g"))
        clip = {"kind": "clip", "media": m, "linked_group": g, "start": 0, "duration": duration}
        return {
            "meta": {"schema_version": 1, "name": "T", "created": created, "app_version": None},
            "media": {m: {"id": m, "path_abs": f"{folder}\\media\\a.mp4",
                          "path_rel": "../media/a.mp4", "content_hash": "h"}},
            "sequences": {s: {"id": s, "name": "Sequence 1", "track_order": [v, a], "tracks": {
                a: {"id": a, "kind": "audio", "name": "A1", "items": {i2: {**clip, "id": i2}}},
                v: {"id": v, "kind": "video", "name": "V1", "items": {i1: {**clip, "id": i1}}},
            }}},
            "custom": {},
        }

    def ids(seed: str) -> dict[str, str]:
        prefixes = {"m": "med", "s": "seq", "v": "trk", "a": "trk", "i1": "itm", "i2": "itm", "g": "grp"}
        return {k: f"{p}_{(seed + k).encode().hex()[:32].ljust(32, '0')}" for k, p in prefixes.items()}

    one = normalize_project(doc(ids("a"), "C:\\x", "1970-01-01T00:00:00Z", 10))
    two = normalize_project(doc(ids("b"), "/tmp/y", "2026-11-25T10:00:00Z", 10))
    other = normalize_project(doc(ids("b"), "/tmp/y", "2026-11-25T10:00:00Z", 11))
    checks = {
        "same content, other ids, paths and times: equal": one == two,
        "another duration: different": one != other,
        "no ids left": not leftover_ids(one),
        "tracks in stack order": [t["name"] for t in one["sequences"][0]["tracks"]] == ["V1", "A1"],
        "items keep their linked group": one["sequences"][0]["tracks"][0]["items"][0]["linked_group"]
        == one["sequences"][0]["tracks"][1]["items"][0]["linked_group"] == "group:0",
        "media by file name": one["media"][0]["file"] == "a.mp4" and "path_abs" not in one["media"][0],
    }
    for name, ok in checks.items():
        print(f"  {'ok  ' if ok else 'FAIL'} {name}")
    return all(checks.values())


# ---- the environment ----------------------------------------------------------------------------


def main_checkout() -> Path | None:
    try:
        common = subprocess.run(
            ["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
            cwd=ROOT, check=True, capture_output=True, text=True).stdout.strip()
        return Path(common).parent
    except (OSError, subprocess.CalledProcessError):
        return None


def ffmpeg_env(env: dict[str, str]) -> str | None:
    """Puts the FFmpeg dev bundle in `env` as scripts/ffmpeg/env.sh does; returns its folder."""
    candidates = [env.get("FFMPEG_DIR")]
    for root in (ROOT, main_checkout()):
        f = root / "third_party" / "ffmpeg" / "ffmpeg.env" if root else None
        if f and f.is_file():
            m = re.search(r'^FFMPEG_DIR="(.*)"', f.read_text(encoding="utf-8"), re.M)
            candidates.append(m.group(1) if m else None)
    bundle = next((c for c in candidates
                   if c and (Path(c) / "include" / "libavcodec" / "avcodec.h").is_file()), None)
    if bundle is None:
        return None
    env["FFMPEG_DIR"] = bundle
    pkg = str(Path(bundle) / "lib" / "pkgconfig")
    env["PKG_CONFIG_PATH"] = pkg + (os.pathsep + env["PKG_CONFIG_PATH"] if env.get("PKG_CONFIG_PATH") else "")
    if os.name == "nt":
        llvm = Path("C:/Program Files/LLVM/bin")
        if not env.get("LIBCLANG_PATH") and (llvm / "libclang.dll").is_file():
            env["LIBCLANG_PATH"] = str(llvm)
        env["PATH"] = str(Path(bundle) / "bin") + os.pathsep + env.get("PATH", "")
    elif sys.platform == "darwin":
        env["DYLD_FALLBACK_LIBRARY_PATH"] = str(Path(bundle) / "lib")
    else:
        env["LD_LIBRARY_PATH"] = str(Path(bundle) / "lib")
    return bundle


def corpus_dir() -> Path | None:
    candidates = [os.environ.get("CUTWEAVE_CORPUS_DIR"), os.environ.get("CUTWEAVE_CORPUS"),
                  str(ROOT / "testdata" / "corpus")]
    main = main_checkout()
    if main:
        candidates.append(str(main / "testdata" / "corpus"))
    for c in candidates:
        if c and all((Path(c) / clip).is_file() for clip in CLIPS):
            return Path(c)
    return None


def tool(name: str) -> str:
    """A program on the PATH (`pnpm` is `pnpm.cmd` on Windows)."""
    return shutil.which(name) or name


def ffmpeg_tool(name: str, env: dict[str, str]) -> str:
    bundle = env.get("FFMPEG_DIR")
    if bundle and (Path(bundle) / "bin" / f"{name}{EXE}").is_file():
        return str(Path(bundle) / "bin" / f"{name}{EXE}")
    return shutil.which(name, path=env.get("PATH")) or name


def run_capture(cmd: list[str], env: dict[str, str] | None = None, cwd: Path = ROOT) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True,
                          encoding="utf-8", errors="replace")


# ---- media checks -------------------------------------------------------------------------------


def probe(file: Path, env: dict[str, str]) -> dict:
    out = run_capture([ffmpeg_tool("ffprobe", env), "-v", "error", "-show_format", "-show_streams",
                       "-of", "json", str(file)], env)
    if out.returncode != 0:
        raise RuntimeError(f"ffprobe refused {file}: {out.stderr.strip()}")
    return json.loads(out.stdout)


STREAM_FIELDS = ("codec_type", "codec_name", "profile", "width", "height", "pix_fmt", "r_frame_rate",
                 "color_space", "color_range", "sample_rate", "channels", "channel_layout", "nb_frames")


def streams(probed: dict) -> list[dict]:
    return [{k: s.get(k) for k in STREAM_FIELDS if k in s} for s in probed.get("streams", [])]


def frame_hashes(file: Path, env: dict[str, str], every: int = 10) -> list[str]:
    """MD5 of every `every`th decoded video frame."""
    out = run_capture([ffmpeg_tool("ffmpeg", env), "-v", "error", "-i", str(file), "-map", "0:v:0",
                       "-vf", f"select=not(mod(n\\,{every}))", "-fps_mode", "passthrough",
                       "-f", "framemd5", "-"], env)
    if out.returncode != 0:
        raise RuntimeError(f"ffmpeg could not decode {file}: {out.stderr.strip()}")
    return [line.rsplit(",", 1)[-1].strip() for line in out.stdout.splitlines()
            if line and not line.startswith("#")]


def audio_hash(file: Path, env: dict[str, str]) -> str | None:
    out = run_capture([ffmpeg_tool("ffmpeg", env), "-v", "error", "-i", str(file), "-map", "0:a:0",
                       "-f", "md5", "-"], env)
    return out.stdout.strip() if out.returncode == 0 else None


def psnr(a: Path, b: Path, env: dict[str, str]) -> float:
    """Average PSNR (dB) of the video of `b` against `a`, both as RGB; inf when identical."""
    return _psnr([ffmpeg_tool("ffmpeg", env), "-v", "info", "-nostats", "-i", str(a), "-i", str(b),
                  "-lavfi", "[0:v]format=rgb24[a];[1:v]format=rgb24[b];[a][b]psnr",
                  "-f", "null", "-"], env)


def _psnr(cmd: list[str], env: dict[str, str]) -> float:
    out = run_capture(cmd, env)
    m = re.search(r"PSNR .*average:(inf|[0-9.]+)", out.stderr)
    if m is None:
        raise RuntimeError(f"no PSNR from {' '.join(cmd)}: {out.stderr[-500:]}")
    return float("inf") if m.group(1) == "inf" else float(m.group(1))


# The export's colour: BT.709, limited range, 4:2:0. `setparams` pins the decoded frame's tags so
# FFmpeg 8's colour negotiation never converts it twice (as crates/cutweave-engine/tests/
# export_ffmpeg.rs does).
PIN_709 = "setparams=range=tv:colorspace=bt709:color_primaries=bt709:color_trc=bt709"
EXACT = "accurate_rnd+full_chroma_int"


def frame_psnr(rendered: Path, export: Path, n: int, env: dict[str, str]) -> dict:
    """Frame `n` of the export against `cutweave render`'s PNG of it, measured two ways:

    - `yuv_db`, the bound: the render converted to the export's BT.709 limited-range 4:2:0, as
      the exporter does before encoding, against the decoded frame. That is the encoder's loss,
      which the tolerance policy's lossy-MP4 row allows for (>= 38 dB).
    - `rgb_db`, reported: the decoded frame converted back to RGB with the BT.709 matrix (as
      export_ffmpeg.rs does) against the render. On saturated synthetic pictures the 4:2:0 round
      trip alone costs a few dB in red and blue, with any encoder.
    """
    ffmpeg = ffmpeg_tool("ffmpeg", env)
    pick = f"select=eq(n\\,{n}),{PIN_709}"
    yuv = _psnr([ffmpeg, "-v", "info", "-nostats", "-i", str(rendered), "-i", str(export), "-lavfi",
                 f"[0:v]scale=out_color_matrix=bt709:out_range=tv:flags={EXACT},format=yuv420p[a];"
                 f"[1:v]{pick},format=yuv420p[b];[a][b]psnr", "-frames:v", "1", "-f", "null", "-"], env)
    rgb = _psnr([ffmpeg, "-v", "info", "-nostats", "-i", str(rendered), "-i", str(export), "-lavfi",
                 f"[0:v]format=rgb24[a];[1:v]{pick},scale=in_color_matrix=bt709:in_range=tv:"
                 f"out_range=pc:flags={EXACT},format=rgb24[b];[a][b]psnr", "-frames:v", "1",
                 "-f", "null", "-"], env)
    return {"frame": n, "yuv_db": round(yuv, 2), "rgb_db": round(rgb, 2)}


def validate_mp4(project_dir: Path, file: Path, env: dict[str, str], work: Path) -> dict:
    """WP-1O.04 exit 1's checks of the UI's export beyond ffprobe (the test did that): it decodes
    to the end without an error, its sound is not silent, and frames decoded from it match the
    same frames rendered by `cutweave render` from the saved project (`frame_psnr`)."""
    result: dict = {"file": file.name}
    decoded = run_capture([ffmpeg_tool("ffmpeg", env), "-v", "error", "-i", str(file), "-f", "null", "-"], env)
    result["decode_errors"] = decoded.stderr.strip().splitlines()
    volume = run_capture([ffmpeg_tool("ffmpeg", env), "-nostats", "-i", str(file), "-map", "0:a:0",
                          "-af", "volumedetect", "-f", "null", "-"], env)
    m = re.search(r"max_volume:\s*(-?[0-9.]+|-inf) dB", volume.stderr)
    result["max_volume_db"] = None if m is None or m.group(1) == "-inf" else float(m.group(1))
    frames_dir = work / "render"
    shutil.rmtree(frames_dir, ignore_errors=True)
    spec = ",".join(f"f:{n}" for n in RENDER_FRAMES)
    rendered = run_capture([str(CLI), "render", "--project", str(project_dir), "--frames", spec,
                            "--out-dir", str(frames_dir), "--allow-root", str(project_dir.parent)], env)
    result["render"] = {"exit": rendered.returncode, "stdout": rendered.stdout.strip()[-2000:],
                        "stderr": rendered.stderr.strip()[-2000:]}
    result["frames"] = []
    for n in RENDER_FRAMES:
        png = frames_dir / f"f{n:05d}.png"
        result["frames"].append(frame_psnr(png, file, n, env) if png.is_file()
                                else {"frame": n, "yuv_db": None, "rgb_db": None})
    result["ok"] = (
        decoded.returncode == 0 and not result["decode_errors"]
        and result["max_volume_db"] is not None and result["max_volume_db"] > SILENT_DB
        and rendered.returncode == 0
        and all(f["yuv_db"] is not None and f["yuv_db"] >= LOSSY_PSNR_DB for f in result["frames"])
    )
    return result


def sha256(file: Path) -> str:
    digest = hashlib.sha256()
    with file.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def compare_runs(ui: Path, mcp: Path, env: dict[str, str]) -> dict:
    """Exit 2: the UI's and the agent's projects and exports (`<dir>/M1 cut/M1 cut.{cutweave,mp4}`)."""
    result: dict = {"ui": str(ui), "mcp": str(mcp)}
    docs = {}
    for side, root in (("ui", ui), ("mcp", mcp)):
        path = root / PROJECT / f"{PROJECT}.cutweave"
        docs[side] = normalize_project(json.loads(path.read_text(encoding="utf-8")))
    a = json.dumps(docs["ui"], indent=2, sort_keys=True).splitlines()
    b = json.dumps(docs["mcp"], indent=2, sort_keys=True).splitlines()
    result["timeline"] = {
        "equal": a == b,
        "leftover_ids": leftover_ids(docs["ui"]) + leftover_ids(docs["mcp"]),
        "items": sum(len(t["items"]) for s in docs["ui"]["sequences"] for t in s["tracks"]),
        "diff": list(difflib.unified_diff(a, b, "ui (normalized)", "mcp (normalized)", lineterm=""))[:200],
    }
    result["normalized"] = docs["ui"]
    files = {side: root / PROJECT / f"{PROJECT}.mp4" for side, root in (("ui", ui), ("mcp", mcp))}
    probed = {side: probe(f, env) for side, f in files.items()}
    durations = {side: float(p["format"]["duration"]) for side, p in probed.items()}
    hashes = {side: frame_hashes(f, env) for side, f in files.items()}
    same_frames = hashes["ui"] == hashes["mcp"] and len(hashes["ui"]) > 0
    result["export"] = {
        "streams": {side: streams(p) for side, p in probed.items()},
        "streams_equal": streams(probed["ui"]) == streams(probed["mcp"]),
        "durations_s": durations,
        "duration_within_a_frame": abs(durations["ui"] - durations["mcp"]) <= 1 / FPS,
        "frames_hashed": len(hashes["ui"]),
        "frame_hashes_equal": same_frames,
        "psnr_db": None if same_frames else psnr(files["ui"], files["mcp"], env),
        "audio_md5_equal": audio_hash(files["ui"], env) == audio_hash(files["mcp"], env),
        "bytes": {side: f.stat().st_size for side, f in files.items()},
        "files_identical": sha256(files["ui"]) == sha256(files["mcp"]),
    }
    e = result["export"]
    frames_ok = same_frames or (e["psnr_db"] is not None and e["psnr_db"] >= EQUIVALENT_PSNR_DB)
    result["ok"] = (result["timeline"]["equal"] and not result["timeline"]["leftover_ids"]
                    and e["streams_equal"] and e["duration_within_a_frame"] and frames_ok)
    return result


# ---- the gate -----------------------------------------------------------------------------------


@dataclass
class Step:
    criterion: str
    name: str
    cmd: list[str] | None = None
    check: object | None = None  # a callable returning (ok, detail) instead of a command
    env: dict[str, str] = field(default_factory=dict)
    unset: tuple[str, ...] = ()
    status: str = "not run"
    seconds: float = 0.0
    log: str = ""
    detail: str = ""


def run_step(step: Step, env: dict[str, str], index: int) -> bool:
    logs = OUT / "logs"
    logs.mkdir(parents=True, exist_ok=True)
    slug = re.sub(r"[^a-z0-9]+", "-", step.name.lower()).strip("-")
    log = logs / f"{index:02d}-{slug}.log"
    step.log = str(log.relative_to(OUT)).replace("\\", "/")
    print(f"[m1-gate] {step.criterion:>3} {step.name} ...", flush=True)
    started = time.monotonic()
    with log.open("w", encoding="utf-8", errors="replace") as out:
        if step.cmd is not None:
            out.write(f"$ {' '.join(step.cmd)}\n\n")
            out.flush()
            child_env = {k: v for k, v in {**env, **step.env}.items() if k not in step.unset}
            code = subprocess.run(step.cmd, cwd=ROOT, env=child_env, stdout=out,
                                  stderr=subprocess.STDOUT).returncode
            ok = code == 0
            step.detail = f"exit {code}"
        else:
            try:
                ok, detail = step.check()  # type: ignore[misc]
            except Exception as error:  # noqa: BLE001 - a broken check is a failed step
                ok, detail = False, f"{type(error).__name__}: {error}"
            step.detail = detail
            out.write(detail + "\n")
    step.seconds = time.monotonic() - started
    step.status = "pass" if ok else "fail"
    print(f"[m1-gate] {step.criterion:>3} {step.name}: {step.status.upper()} "
          f"({step.seconds:.0f} s, {step.detail}; {step.log})", flush=True)
    return ok


def machine(env: dict[str, str]) -> dict:
    def first_line(cmd: list[str]) -> str:
        try:
            out = run_capture(cmd, env)
            return (out.stdout or out.stderr).strip().splitlines()[0]
        except (OSError, IndexError):
            return "unknown"
    head = first_line(["git", "rev-parse", "HEAD"])
    dirty = bool(run_capture(["git", "status", "--porcelain"], env).stdout.strip())
    info = {
        "commit": head + (" (with uncommitted changes)" if dirty else ""),
        "os": platform.platform(),
        "cpu": platform.processor() or platform.machine(),
        "ffmpeg": first_line([ffmpeg_tool("ffmpeg", env), "-version"]),
        "node": first_line([tool("node"), "--version"]),
        "rustc": first_line(["rustc", "--version"]),
    }
    if os.name == "nt":
        gpus = run_capture(["powershell", "-NoProfile", "-Command",
                            "(Get-CimInstance Win32_VideoController).Name"], env)
        info["gpus"] = [g.strip() for g in gpus.stdout.splitlines() if g.strip()]
    return info


def write_report(steps: list[Step], verdicts: dict[str, str], info: dict, args) -> None:
    report = {
        "machine": info,
        "trials": args.trials,
        "full": args.full,
        "criteria": {c: {"title": CRITERIA[c], "result": verdicts[c]} for c in CRITERIA},
        "steps": [{"criterion": s.criterion, "name": s.name, "status": s.status,
                   "seconds": round(s.seconds, 1), "detail": s.detail, "log": s.log} for s in steps],
    }
    (OUT / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    lines = ["# M1 exit gate", "",
             f"- Commit: `{info['commit']}`", f"- Machine: {info['os']}; {info['cpu']}",
             f"- GPUs: {', '.join(info.get('gpus', [])) or 'unknown'}",
             f"- FFmpeg: {info['ffmpeg']}", f"- Node: {info['node']}; {info['rustc']}",
             f"- Crash trials per kind: {args.trials}", "",
             "| Criterion | Result |", "|---|---|"]
    lines += [f"| {c}. {CRITERIA[c]} | **{verdicts[c]}** |" for c in CRITERIA]
    lines += ["", "| Criterion | Step | Result | Time | Detail |", "|---|---|---|---|---|"]
    lines += [f"| {s.criterion} | {s.name} | {s.status} | {s.seconds:.0f} s | "
              f"{s.detail.replace('|', '/')[:200]} |" for s in steps]
    (OUT / "report.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


def write_evidence(dest: Path, steps: list[Step]) -> None:
    """The text evidence for the milestone report: no media, logs cut to their last lines."""
    dest.mkdir(parents=True, exist_ok=True)
    for rel in ("report.md", "report.json", "compare.json", "validate-mp4.json",
                "ui/session.json", "mcp/session.json", "crash/engine-kill.json", "crash/app-kill.json"):
        src = OUT / rel
        if src.is_file():
            target = dest / rel.replace("/", "-")
            target.write_text(src.read_text(encoding="utf-8"), encoding="utf-8")
    logs = dest / "logs"
    shutil.rmtree(logs, ignore_errors=True)
    logs.mkdir()
    for s in steps:
        if not s.log:
            continue
        text = (OUT / s.log).read_text(encoding="utf-8", errors="replace").splitlines()
        # Progress bars and compiler chatter are not evidence: keep the end of each log.
        tail = [re.sub(r"\x1b\[[0-9;]*m", "", line) for line in text[-80:]]
        (logs / Path(s.log).name.replace(".log", ".txt")).write_text("\n".join(tail) + "\n", encoding="utf-8")


def gate(args) -> int:
    env = dict(os.environ)
    if ffmpeg_env(env) is None:
        print("[m1-gate] no FFmpeg dev bundle: run `just ffmpeg-fetch` (docs/dev/ffmpeg.md)")
        return 2
    corpus = corpus_dir()
    if corpus is None:
        print(f"[m1-gate] the corpus lacks {', '.join(CLIPS)}: run `just corpus`")
        return 2
    shutil.rmtree(OUT, ignore_errors=True)
    OUT.mkdir(parents=True)
    env.update({
        "CUTWEAVE_M1_OUT": str(OUT),
        "CUTWEAVE_M1_TRIALS": str(args.trials),
        "CUTWEAVE_CORPUS": str(corpus),
        "CUTWEAVE_CORPUS_DIR": str(corpus),
    })
    env.pop("CUTWEAVE_DETERMINISTIC_IDS", None)
    env.pop("CUTWEAVE_DEV_STUB_MEDIA", None)
    pnpm, cargo, just = tool("pnpm"), tool("cargo"), tool("just")
    playwright = [pnpm, "--filter", "@cutweave/desktop", "exec", "playwright", "test"]
    wanted = set(args.criteria.split(","))

    steps: list[Step] = []
    if not args.skip_build:
        steps += [
            Step("0", "build the engine with FFmpeg", [cargo, "build", "-p", "cutweave-engine", "--bin",
                                                       "cutweave-engine-serve", "--features", "ffmpeg"]),
            Step("0", "build the CLI with FFmpeg", [cargo, "build", "-p", "cutweave-cli", "--bin",
                                                    "cutweave", "--features", "ffmpeg"]),
            Step("0", "build the preview addon", [pnpm, "--filter", "@cutweave/desktop", "build:bridge"]),
            Step("0", "build the app (main, preload)", [pnpm, "--filter", "@cutweave/desktop", "build:node"]),
            Step("0", "build the app (renderer)", [pnpm, "--filter", "@cutweave/desktop", "build:renderer"]),
            Step("0", "Electron binary", [tool("node"), "apps/desktop/scripts/ensure-electron.ts"]),
        ]
    validated: dict = {}
    compared: dict = {}

    def check_validate():
        work = OUT / "validate"
        work.mkdir(parents=True, exist_ok=True)
        project = OUT / "ui" / PROJECT
        validated.update(validate_mp4(project, project / f"{PROJECT}.mp4", env, work))
        (OUT / "validate-mp4.json").write_text(json.dumps(validated, indent=2) + "\n", encoding="utf-8")
        frames = ", ".join(f"f{f['frame']} {f['yuv_db']:.1f} dB (RGB {f['rgb_db']:.1f})"
                           if f["yuv_db"] is not None else f"f{f['frame']} missing"
                           for f in validated["frames"])
        return validated["ok"], (f"decode errors {len(validated['decode_errors'])}, max volume "
                                 f"{validated['max_volume_db']} dB, render vs export: {frames}")

    def check_compare():
        compared.update(compare_runs(OUT / "ui", OUT / "mcp", env))
        (OUT / "compare.json").write_text(json.dumps(compared, indent=2) + "\n", encoding="utf-8")
        e = compared["export"]
        frames = ("decoded frames identical" if e["frame_hashes_equal"]
                  else f"PSNR {e['psnr_db']:.2f} dB")
        if e["files_identical"]:
            frames += "; the two files are byte-identical"
        return compared["ok"], (
            f"timeline {'equal' if compared['timeline']['equal'] else 'DIFFERENT'} "
            f"({compared['timeline']['items']} items), streams "
            f"{'equal' if e['streams_equal'] else 'DIFFERENT'}, durations {e['durations_s']}, "
            f"{e['frames_hashed']} frames hashed: {frames}")

    if "1" in wanted:
        steps += [
            Step("1", "UI cut and export (e2e/m1/m1-exit)", playwright + ["e2e/m1/m1-exit.spec.ts"]),
            Step("1", "the UI export decodes, has sound, matches the render", check=check_validate),
            Step("1", "user flow F14 export (pointer drags)", playwright + ["e2e/flows/f14-export.spec.ts"]),
        ]
    if "2" in wanted:
        steps.append(Step("2", "MCP conformance and the M1 scripted agent",
                          [cargo, "nextest", "run", "-p", "cutweave-cli", "--features", "ffmpeg",
                           "--test", "mcp_conformance", "--no-fail-fast"]))
        if "1" in wanted:
            steps.append(Step("2", "UI and agent: same timeline, equivalent exports", check=check_compare))
    if "3" in wanted:
        steps += [
            Step("3", f"crash trials ({args.trials} engine kills, {args.trials} app kills)",
                 playwright + ["e2e/m1/m1-crash.spec.ts"]),
            Step("3", "user flows F11 and F12", playwright + ["e2e/flows/f11-engine-crash.spec.ts",
                                                             "e2e/flows/f12-app-crash-recovery.spec.ts"]),
        ]
    if "4" in wanted:
        steps += [
            Step("4b", "golden frames (cargo xtask golden)", [cargo, "xtask", "golden"] + (
                ["--base", args.golden_base] if args.golden_base else [])),
            Step("4c", "parity CLI = MCP = RPC", [cargo, "nextest", "run", "-p", "cutweave-api",
                                                  "--test", "parity"]),
        ]
        if args.full:
            # Last: the workspace build has no FFmpeg and replaces the binaries built above, and
            # outside the gate the M1 agent may skip (it needs FFmpeg), so CUTWEAVE_M1_OUT goes.
            plain = ("CUTWEAVE_M1_OUT", "CUTWEAVE_M1_TRIALS")
            steps += [Step("4a", "the full local gate (just check)", [just, "check"], unset=plain),
                      Step("4a", "Storybook visual, axe, fonts (just ui-snapshots)",
                           [just, "ui-snapshots"], unset=plain)]

    info = machine(env)
    built = True
    for index, step in enumerate(steps, start=1):
        if step.criterion == "0":
            built = run_step(step, env, index) and built
            continue
        if not built:
            step.status, step.detail = "fail", "not run: a build step failed"
            continue
        run_step(step, env, index)

    verdicts: dict[str, str] = {}
    for c in CRITERIA:
        mine = [s for s in steps if s.criterion.startswith(c)]
        if not mine:
            verdicts[c] = "not run"
        elif all(s.status == "pass" for s in mine):
            verdicts[c] = "pass" if c != "4" or args.full else "pass (4a not run: --full)"
        else:
            verdicts[c] = "FAIL"
    write_report(steps, verdicts, info, args)
    if args.evidence:
        write_evidence(Path(args.evidence), steps)
    print("\n[m1-gate] M1 exit criteria:")
    for c in CRITERIA:
        print(f"  {c}. {verdicts[c]:<28} {CRITERIA[c]}")
    print(f"[m1-gate] report: {OUT / 'report.md'}")
    return 0 if all(v != "FAIL" for v in verdicts.values()) and not any(
        s.status == "fail" for s in steps) else 1


def main() -> int:
    if len(sys.argv) > 1 and sys.argv[1] == "compare":
        p = argparse.ArgumentParser(prog="m1-gate.py compare")
        p.add_argument("ui", type=Path)
        p.add_argument("mcp", type=Path)
        a = p.parse_args(sys.argv[2:])
        env = dict(os.environ)
        ffmpeg_env(env)
        result = compare_runs(a.ui, a.mcp, env)
        result.pop("normalized", None)
        print(json.dumps(result, indent=2))
        return 0 if result["ok"] else 1
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--trials", type=int, default=20, help="crash trials of each kind (default 20, WP-1O.04)")
    p.add_argument("--full", action="store_true", help="also run `just check` and `just ui-snapshots` (4a)")
    p.add_argument("--skip-build", action="store_true", help="use the binaries and app already built")
    p.add_argument("--criteria", default="1,2,3,4", help="which criteria to run, e.g. 1,2")
    p.add_argument("--evidence", help="also write the text evidence to this folder")
    p.add_argument("--golden-base", help="the golden frames' reference-update guard base "
                   "(default: the merge base with main)")
    p.add_argument("--self-test", action="store_true", help="check the normalizer and exit")
    args = p.parse_args()
    if args.self_test:
        return 0 if self_test() else 1
    return gate(args)


if __name__ == "__main__":
    sys.exit(main())
