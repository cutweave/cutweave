#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Generates the `audio_5_1` corpus item (testdata/corpus.toml). Needs ffmpeg on PATH.
# Usage: audio_5_1.sh <output-file>
set -euo pipefail
out="${1:?usage: audio_5_1.sh <output-file>}"
mkdir -p "$(dirname "$out")"
ffmpeg -y -hide_banner -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=5,pan=5.1|FL=c0|FR=c0|FC=c0|LFE=c0|BL=c0|BR=c0" -c:a ac3 -b:a 448k -bitexact "$out"
