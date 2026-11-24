#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Generates the `vfr_phone_rotated90` corpus item (testdata/corpus.toml). Needs ffmpeg on PATH.
# Usage: vfr_phone_rotated90.sh <output-file>
set -euo pipefail
out="${1:?usage: vfr_phone_rotated90.sh <output-file>}"
mkdir -p "$(dirname "$out")"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
# Pass 1: variable frame rate. Keep every second frame of a 60 fps source plus every seventh, so the
# gaps between presentation timestamps alternate (16.7, 33.3 and 50 ms) and `-fps_mode vfr` keeps them.
# Deterministic: no random().
ffmpeg -y -hide_banner -f lavfi -i "testsrc2=size=1080x1920:rate=60:duration=10" -f lavfi -i "sine=frequency=261:sample_rate=48000:duration=10" \
  -vf "select='not(mod(n,2))+not(mod(n,7))'" -fps_mode vfr \
  -c:v libx264 -preset veryfast -crf 26 -pix_fmt yuv420p -c:a aac -shortest "$tmp/vfr.mp4"
# Pass 2: the display matrix. `-display_rotation` is an input option (it must precede `-i`), so stream-copy
# the first pass through it. 90 is counter-clockwise, which ffprobe reports as rotation -90 (270 clockwise).
ffmpeg -y -hide_banner -display_rotation 90 -i "$tmp/vfr.mp4" -c copy "$out"
