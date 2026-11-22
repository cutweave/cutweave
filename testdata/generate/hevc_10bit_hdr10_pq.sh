#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Generates the `hevc_10bit_hdr10_pq` corpus item (testdata/corpus.toml). Needs ffmpeg on PATH.
# Usage: hevc_10bit_hdr10_pq.sh <output-file>
set -euo pipefail
out="${1:?usage: hevc_10bit_hdr10_pq.sh <output-file>}"
mkdir -p "$(dirname "$out")"
ffmpeg -y -hide_banner -f lavfi -i "testsrc2=size=1920x1080:rate=30:duration=5" -c:v libx265 -preset veryfast -crf 24 -pix_fmt yuv420p10le \
  -x265-params "hdr10=1:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=1000,400:log-level=error" \
  -tag:v hvc1 -an -bitexact "$out"
