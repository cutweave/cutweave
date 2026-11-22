#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Generates the `h264_4k30_longgop` corpus item (testdata/corpus.toml). Needs ffmpeg on PATH.
# Usage: h264_4k30_longgop.sh <output-file>
set -euo pipefail
out="${1:?usage: h264_4k30_longgop.sh <output-file>}"
mkdir -p "$(dirname "$out")"
ffmpeg -y -hide_banner -f lavfi -i "testsrc2=size=3840x2160:rate=30:duration=8" -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=8" \
  -c:v libx264 -preset veryfast -crf 28 -g 250 -bf 3 -pix_fmt yuv420p -c:a aac -b:a 128k -shortest -bitexact -movflags +faststart "$out"
