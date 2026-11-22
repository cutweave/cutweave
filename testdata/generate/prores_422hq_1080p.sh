#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Generates the `prores_422hq_1080p` corpus item (testdata/corpus.toml). Needs ffmpeg on PATH.
# Usage: prores_422hq_1080p.sh <output-file>
set -euo pipefail
out="${1:?usage: prores_422hq_1080p.sh <output-file>}"
mkdir -p "$(dirname "$out")"
ffmpeg -y -hide_banner -f lavfi -i "testsrc2=size=1920x1080:rate=24:duration=3" -f lavfi -i "sine=frequency=523:sample_rate=48000:duration=3" \
  -c:v prores_ks -profile:v 3 -pix_fmt yuv422p10le -c:a pcm_s16le -shortest -bitexact "$out"
