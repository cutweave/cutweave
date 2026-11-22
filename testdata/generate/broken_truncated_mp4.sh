#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Generates the `broken_truncated_mp4` corpus item (testdata/corpus.toml). Needs ffmpeg on PATH.
# Usage: broken_truncated_mp4.sh <output-file>
set -euo pipefail
out="${1:?usage: broken_truncated_mp4.sh <output-file>}"
mkdir -p "$(dirname "$out")"
# Valid MP4 cut at 60% of its size: the moov atom is at the end (no faststart), so it is unreadable without recovery.
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
ffmpeg -y -hide_banner -f lavfi -i "testsrc2=size=1280x720:rate=30:duration=6" -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=6" \
  -c:v libx264 -preset veryfast -crf 28 -pix_fmt yuv420p -c:a aac -shortest -bitexact "$tmp/full.mp4"
size="$(wc -c < "$tmp/full.mp4")"
head -c "$((size * 6 / 10))" "$tmp/full.mp4" > "$out"
