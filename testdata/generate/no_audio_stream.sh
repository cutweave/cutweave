#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Generates the `no_audio_stream` corpus item (testdata/corpus.toml). Needs ffmpeg on PATH.
# Usage: no_audio_stream.sh <output-file>
set -euo pipefail
out="${1:?usage: no_audio_stream.sh <output-file>}"
mkdir -p "$(dirname "$out")"
ffmpeg -y -hide_banner -f lavfi -i "testsrc2=size=1280x720:rate=30:duration=4" -an -c:v libx264 -preset veryfast -crf 28 -pix_fmt yuv420p -bitexact "$out"
