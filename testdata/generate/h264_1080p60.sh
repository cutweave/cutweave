#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Generates the `h264_1080p60` corpus item (testdata/corpus.toml). Needs ffmpeg on PATH.
# Usage: h264_1080p60.sh <output-file>
set -euo pipefail
out="${1:?usage: h264_1080p60.sh <output-file>}"
mkdir -p "$(dirname "$out")"
ffmpeg -y -hide_banner -f lavfi -i "testsrc2=size=1920x1080:rate=60:duration=8" -f lavfi -i "sine=frequency=330:sample_rate=48000:duration=8" \
  -c:v libx264 -preset veryfast -crf 26 -g 120 -pix_fmt yuv420p -c:a aac -b:a 128k -shortest -bitexact "$out"
