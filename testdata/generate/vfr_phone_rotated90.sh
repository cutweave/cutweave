#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Generates the `vfr_phone_rotated90` corpus item (testdata/corpus.toml). Needs ffmpeg on PATH.
# Usage: vfr_phone_rotated90.sh <output-file>
set -euo pipefail
out="${1:?usage: vfr_phone_rotated90.sh <output-file>}"
mkdir -p "$(dirname "$out")"
# Variable frame rate (timestamps jittered, 24-35 fps average) and a 90 degree display-matrix rotation, like phone footage.
ffmpeg -y -hide_banner -f lavfi -i "testsrc2=size=1080x1920:rate=60:duration=10" -f lavfi -i "sine=frequency=261:sample_rate=48000:duration=10" \
  -vf "select='not(mod(n,2))+lt(random(1),0.3)',setpts=N/FRAME_RATE/TB+sin(N)*0.004/TB" -fps_mode vfr \
  -c:v libx264 -preset veryfast -crf 26 -pix_fmt yuv420p -c:a aac -shortest -display_rotation:v:0 90 "$out"
