#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Generates the `variable_frame_size` corpus item (testdata/corpus.toml). Needs ffmpeg on PATH.
# Usage: variable_frame_size.sh <output-file>
set -euo pipefail
out="${1:?usage: variable_frame_size.sh <output-file>}"
mkdir -p "$(dirname "$out")"
# Resolution changes mid-stream (720p, 1080p, 480p): H.264 segments with in-band SPS concatenated as MPEG-TS.
# The segments live next to the output, not in /tmp (a native Windows ffmpeg cannot resolve a Git Bash /tmp
# path), and the concat protocol is given bare file names from inside that directory.
outdir="$(cd "$(dirname "$out")" && pwd)"
tmp="$(mktemp -d "$outdir/.vfs.XXXXXX")"; trap 'rm -rf "$tmp"' EXIT
i=0
for size in 1280x720 1920x1080 854x480; do
  ffmpeg -y -hide_banner -f lavfi -i "testsrc2=size=$size:rate=30:duration=2" -c:v libx264 -preset veryfast -crf 28 -pix_fmt yuv420p -f mpegts "$tmp/seg$i.ts"
  i=$((i + 1))
done
(cd "$tmp" && ffmpeg -y -hide_banner -i "concat:seg0.ts|seg1.ts|seg2.ts" -c copy -f mpegts "$outdir/$(basename "$out")")
