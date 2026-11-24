#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Generates the deterministic synthetic test media for Spike S3 (WP-0E.01).

Every clip is made from FFmpeg lavfi sources with the FFmpeg bundle's own `ffmpeg`, so nothing is
downloaded. Video frames carry a 16-bit **frame-number barcode** in the top two bands (bit k is a
white block in band 0 when set, in band 1 when clear), so the harness can read the presentation
index straight off the decoded luma plane, independently of ffprobe. A mild temporal noise layer
keeps the bitrate (and thus the decode cost) closer to camera footage than a flat test pattern.

Usage:
    python gen_media.py [--out media] [--ffmpeg PATH] [--only NAME[,NAME..]] [--force] [--list]

`--ffmpeg` defaults to $FFMPEG_DIR/bin/ffmpeg(.exe), then `ffmpeg` on PATH. The output folder is
git-ignored; `manifest.tsv` lists every file with its kind and the properties the harness checks.
"""

import argparse
import hashlib
import os
import random
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
BITS = 16  # barcode bits: up to 65535 frames
BANDS = 18  # each barcode band is H/18 tall; blocks are W/16 wide

# x264: closed GOP of a fixed length, 3 B-frames with a B-pyramid, no scene-cut keyframes.
X264 = "-c:v libx264 -preset veryfast -profile:v high -pix_fmt yuv420p -bf 3 -sc_threshold 0"
X264P = "b-pyramid=normal:scenecut=0"
NOISE = "noise=alls=6:allf=t"


def barcode(w: int, h: int) -> str:
    """drawbox chain that burns the frame number `n` into the top two bands (see module doc)."""
    bw, bh = w // BITS, h // BANDS
    parts = [f"drawbox=x=0:y=0:w={w}:h={2 * bh}:color=black:t=fill"]
    for k in range(BITS):
        bit = f"mod(floor(n/{2 ** k}),2)"
        parts.append(f"drawbox=x={k * bw}:y=0:w={bw}:h={bh}:color=white:t=fill:enable='eq({bit},1)'")
        parts.append(f"drawbox=x={k * bw}:y={bh}:w={bw}:h={bh}:color=white:t=fill:enable='eq({bit},0)'")
    return ",".join(parts)


def pattern(w: int, h: int, rate: str, seconds: float, extra: str = "") -> str:
    """lavfi graph: moving test pattern + temporal noise + barcode (+ optional extra filters)."""
    graph = f"testsrc2=size={w}x{h}:rate={rate}:duration={seconds},{NOISE},{barcode(w, h)}"
    return graph + ("," + extra if extra else "")


# (name, kind, expectations for the harness, list of ffmpeg argument strings run in order).
# `{out}` is the output path, `{media}` the media folder (for derived clips).
def clips() -> list[tuple[str, str, str, list[str]]]:
    c = []

    def cfr(name, w, h, rate, secs, gop, extra_x264="", container_args="", expect="", audio=""):
        afl = f"-f lavfi -i {audio}" if audio else ""
        acodec = "-c:a aac -b:a 128k -shortest" if audio else ""
        c.append((name, "video", f"barcode=1;gop={gop};{expect}".rstrip(";"), [
            f"-f lavfi -i \"{pattern(w, h, rate, secs)}\" {afl} {X264} -g {gop} -keyint_min {gop} "
            f"-x264-params {X264P}{extra_x264} -b:v {bitrate(w, h)} -maxrate {bitrate(w, h, 1.5)} "
            f"-bufsize {bitrate(w, h, 2)} {acodec} {container_args} {{out}}"]))

    # CFR H.264 at the common rates, known GOP sizes, several containers.
    cfr("h264_1080p23976_gop48.mp4", 1920, 1080, "24000/1001", 10, 48)
    cfr("h264_1080p25_gop50.mkv", 1920, 1080, "25", 10, 50)
    cfr("h264_1080p2997_gop60_opengop.mp4", 1920, 1080, "30000/1001", 10, 60, ":open-gop=1", expect="opengop=1")
    cfr("h264_1080p30_gop30.mov", 1920, 1080, "30", 10, 30)
    cfr("h264_1080p60_gop120.mp4", 1920, 1080, "60", 10, 120)
    # MPEG-TS: no container index, non-zero start time (1.4 s), seek must bisect by timestamps.
    cfr("h264_1080p25_gop25.ts", 1920, 1080, "25", 8, 25)
    # 4K30: camera-like 1 s GOP, and x264's default 250-frame GOP (the long-GOP worst case).
    cfr("h264_2160p30_gop30.mp4", 3840, 2160, "30", 10, 30, audio="sine=frequency=1000:sample_rate=44100")
    cfr("h264_2160p30_gop250.mp4", 3840, 2160, "30", 20, 250)

    # VFR, phone-like: ~30 fps with +-4 ms jitter, a dropped frame every 37 frames, a low-light
    # stretch at 24 fps; 90 kHz timescale; rotated 90 degrees (display matrix) like portrait
    # phone footage; AAC 48 kHz.
    phone_pts = ("if(eq(N,0),st(0,0),st(0,ld(0)+1/30+0.004*sin(N*1.7)+if(eq(mod(N,37),0),1/30,0)"
                 "+if(between(N,120,150),1/24-1/30,0)))/TB")
    c.append(("h264_vfr_phone_1080p_rot90.mp4", "video", "barcode=1;vfr=1;rotation=90", [
        f"-f lavfi -i \"{pattern(1920, 1080, '30', 10, 'settb=1/90000,setpts=' + chr(39) + phone_pts + chr(39))}\" "
        f"-f lavfi -i sine=frequency=440:sample_rate=48000 {X264} -g 30 -keyint_min 30 -x264-params {X264P} "
        f"-b:v 16M -fps_mode passthrough -enc_time_base:v 1/90000 -video_track_timescale 90000 "
        f"-c:a aac -b:a 128k -shortest {{media}}/_phone_tmp.mp4",
        "-display_rotation:v:0 90 -i {media}/_phone_tmp.mp4 -map 0 -c copy {out}",
    ]))
    # VFR by concatenated rates: 90 frames @30, 120 @60, 90 @24 (Matroska, 1 ms timebase).
    rates_pts = "if(lt(N,90),N/30,if(lt(N,210),3+(N-90)/60,5+(N-210)/24))/TB"
    c.append(("h264_vfr_rates_1080p.mkv", "video", "barcode=1;vfr=1", [
        f"-f lavfi -i \"{pattern(1920, 1080, '60', 5, 'settb=1/1000,setpts=' + chr(39) + rates_pts + chr(39))}\" "
        f"-frames:v 300 {X264} -g 30 -keyint_min 30 -x264-params {X264P} -b:v 16M -fps_mode passthrough "
        f"-enc_time_base:v 1/1000 {{out}}",
    ]))
    # Rotated CFR clip (270 degrees), 720p.
    c.append(("h264_720p30_rot270.mp4", "video", "barcode=1;rotation=270", [
        f"-f lavfi -i \"{pattern(1280, 720, '30', 6, '')}\" {X264} -g 30 -keyint_min 30 -x264-params {X264P} "
        f"-b:v 8M {{media}}/_rot_tmp.mp4",
        "-display_rotation:v:0 270 -i {media}/_rot_tmp.mp4 -map 0 -c copy {out}",
    ]))

    # HEVC Main10 (x265 defaults: open GOP with CRA + RASL pictures, B-frames).
    x265 = "-c:v libx265 -preset ultrafast -pix_fmt yuv420p10le -tag:v hvc1"
    c.append(("hevc_main10_2160p30_gop60.mp4", "video", "barcode=1;gop=60;opengop=1;bitdepth=10", [
        f"-f lavfi -i \"{pattern(3840, 2160, '30', 6, '')}\" {x265} -g 60 -keyint_min 60 -b:v 40M "
        f"-x265-params \"bframes=4:scenecut=0:open-gop=1:log-level=error\" {{out}}",
    ]))
    # HEVC Main10 HDR10 (PQ, BT.2020, mastering display + MaxCLL SEI).
    hdr = ("colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:range=limited:hdr10=1:repeat-headers=1:"
           "master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=1000,400:"
           "bframes=4:scenecut=0:log-level=error")
    c.append(("hevc_main10_pq_1080p24.mp4", "video", "barcode=1;gop=48;bitdepth=10;trc=smpte2084", [
        f"-f lavfi -i \"{pattern(1920, 1080, '24', 6, '')}\" {x265} -g 48 -keyint_min 48 -b:v 12M "
        f"-color_primaries bt2020 -color_trc smpte2084 -colorspace bt2020nc -color_range tv "
        f"-x265-params \"{hdr}\" {{out}}",
    ]))
    # ProRes 422 HQ, 10-bit 4:2:2, intra-only.
    c.append(("prores_hq_1080p25.mov", "video", "barcode=1;gop=1;bitdepth=10", [
        f"-f lavfi -i \"{pattern(1920, 1080, '25', 4, '')}\" -c:v prores_ks -profile:v 3 -pix_fmt yuv422p10le "
        f"-vendor apl0 {{out}}",
    ]))

    # Intra proxies of the 4K long-GOP clip (same frames and timestamps, <= 960 wide).
    c.append(("proxy_prores_540p.mov", "proxy", "barcode=1;gop=1;proxy_of=h264_2160p30_gop250.mp4", [
        "-i {media}/h264_2160p30_gop250.mp4 -vf scale=960:540:flags=bicubic -c:v prores_ks -profile:v 0 "
        "-pix_fmt yuv422p10le -vendor apl0 -an {out}",
    ]))
    c.append(("proxy_mjpeg_540p.mov", "proxy", "barcode=1;gop=1;proxy_of=h264_2160p30_gop250.mp4", [
        "-i {media}/h264_2160p30_gop250.mp4 -vf scale=960:540:flags=bicubic,format=yuvj420p -c:v mjpeg -q:v 4 "
        "-an {out}",
    ]))

    # Audio at odd sample rates (decode + sample-count check only).
    c.append(("audio_22050_mono_s16.wav", "audio", "rate=22050;channels=1", [
        "-f lavfi -i sine=frequency=330:sample_rate=22050:duration=7 -c:a pcm_s16le {out}"]))
    c.append(("audio_44100_stereo_aac.m4a", "audio", "rate=44100;channels=2", [
        "-f lavfi -i sine=frequency=550:sample_rate=44100:duration=7 -ac 2 -c:a aac -b:a 160k {out}"]))
    c.append(("audio_96000_stereo_flac.flac", "audio", "rate=96000;channels=2", [
        "-f lavfi -i sine=frequency=770:sample_rate=96000:duration=7 -ac 2 -c:a flac {out}"]))

    # A faststart (moov first) copy, used as the source of two broken files.
    c.append(("_faststart_src.mp4", "tmp", "", [
        "-i {media}/h264_1080p23976_gop48.mp4 -c copy -movflags +faststart {out}"]))
    return c


def bitrate(w: int, h: int, mul: float = 1.0) -> str:
    """Camera-like H.264 bitrates: 4K 50 Mb/s, 1080p 16 Mb/s, smaller 8 Mb/s."""
    base = 50 if w >= 3840 else 16 if w >= 1920 else 8
    return f"{int(base * mul)}M"


def broken(media: Path) -> list[tuple[str, str, str]]:
    """Writes the broken files from bytes (deterministic) and returns manifest rows."""
    rows = []
    rng = random.Random(0x53334465)  # "S3De"

    def write(name: str, data: bytes, expect: str):
        (media / name).write_bytes(data)
        rows.append((name, "broken", expect))

    write("broken_zero.mp4", b"", "open=fail")
    write("broken_garbage.mp4", bytes(rng.getrandbits(8) for _ in range(1 << 20)), "open=fail")
    moov_end = (media / "h264_1080p23976_gop48.mp4").read_bytes()
    write("broken_truncated_moov_at_end.mp4", moov_end[: len(moov_end) * 6 // 10], "open=fail")
    fast = (media / "_faststart_src.mp4").read_bytes()
    write("broken_truncated_faststart.mp4", fast[: len(fast) * 6 // 10], "open=ok;tail=missing")
    write("broken_header_only.mp4", fast[:1024], "open=fail")
    mkv = bytearray((media / "h264_1080p25_gop50.mkv").read_bytes())
    lo, hi = len(mkv) // 3, 2 * len(mkv) // 3
    for off in range(lo, hi, 4096):  # one flipped byte every 4 KiB through the middle third
        mkv[off + rng.randrange(4096) if off + 4096 < hi else off] ^= 0xFF
    write("broken_bitflips.mkv", bytes(mkv), "open=ok;errors=expected")
    ts = (media / "h264_1080p25_gop25.ts").read_bytes()
    write("broken_truncated.ts", ts[: len(ts) // 2 + 77], "open=ok;tail=missing")
    return rows


def find_ffmpeg(explicit: str | None) -> str:
    if explicit:
        return explicit
    exe = "ffmpeg.exe" if os.name == "nt" else "ffmpeg"
    if os.environ.get("FFMPEG_DIR"):
        cand = Path(os.environ["FFMPEG_DIR"]) / "bin" / exe
        if cand.exists():
            return str(cand)
    found = shutil.which("ffmpeg")
    if not found:
        sys.exit("gen_media: no ffmpeg (pass --ffmpeg or set FFMPEG_DIR)")
    return found


def run(ffmpeg: str, args: str, out: Path, media: Path) -> None:
    # `-fflags +bitexact` on the output: no muxer version strings or random Matroska UIDs.
    cmd = (f"\"{ffmpeg}\" -hide_banner -loglevel error -nostdin -y "
           + args.format(out=f"-fflags +bitexact \"{out}\"", media=str(media).replace("\\", "/")))
    # shell=True keeps the lavfi graphs readable; every value is ours, nothing comes from input.
    subprocess.run(cmd, shell=True, check=True)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", default=str(HERE / "media"))
    ap.add_argument("--ffmpeg")
    ap.add_argument("--only", help="comma-separated clip names")
    ap.add_argument("--force", action="store_true", help="regenerate existing files")
    ap.add_argument("--list", action="store_true", help="print the clip list and exit")
    a = ap.parse_args()

    media = Path(a.out)
    media.mkdir(parents=True, exist_ok=True)
    ffmpeg = None if a.list else find_ffmpeg(a.ffmpeg)
    only = set(a.only.split(",")) if a.only else None
    rows = []
    for name, kind, expect, steps in clips():
        if a.list:
            print(f"{kind:7} {name}  {expect}")
            continue
        out = media / name
        if (only is None or name in only) and (a.force or not out.exists()):
            print(f"gen {name}", flush=True)
            for step in steps:
                run(ffmpeg, step, out, media)
        if kind != "tmp":
            rows.append((name, kind, expect))
    if a.list:
        return 0
    rows += broken(media)
    for tmp in media.glob("_*"):
        if tmp.name != "_faststart_src.mp4":
            tmp.unlink()
    with open(media / "manifest.tsv", "w", encoding="utf-8", newline="\n") as f:
        f.write("# name\tkind\texpect\tbytes\tsha256\n")
        for name, kind, expect in rows:
            data = (media / name).read_bytes()
            f.write(f"{name}\t{kind}\t{expect}\t{len(data)}\t{hashlib.sha256(data).hexdigest()}\n")
    print(f"wrote {len(rows)} entries to {media / 'manifest.tsv'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
