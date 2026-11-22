#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Generates the `audio_22050` corpus item (testdata/corpus.toml). Needs ffmpeg on PATH.
# Usage: audio_22050.sh <output-file>
set -euo pipefail
out="${1:?usage: audio_22050.sh <output-file>}"
mkdir -p "$(dirname "$out")"
ffmpeg -y -hide_banner -f lavfi -i "sine=frequency=1000:sample_rate=22050:duration=5" -c:a pcm_s16le -bitexact "$out"
