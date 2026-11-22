#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Generates the `hevc_10bit_sdr` corpus item (testdata/corpus.toml). Needs ffmpeg on PATH.
# Usage: hevc_10bit_sdr.sh <output-file>
set -euo pipefail
out="${1:?usage: hevc_10bit_sdr.sh <output-file>}"
mkdir -p "$(dirname "$out")"
ffmpeg -y -hide_banner -f lavfi -i "testsrc2=size=1920x1080:rate=30:duration=5" -c:v libx265 -preset veryfast -crf 26 -pix_fmt yuv420p10le \
  -x265-params "colorprim=bt709:transfer=bt709:colormatrix=bt709:log-level=error" -tag:v hvc1 -an -bitexact "$out"
