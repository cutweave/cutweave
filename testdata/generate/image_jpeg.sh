#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Generates the `image_jpeg` corpus item (testdata/corpus.toml). Needs ffmpeg on PATH.
# Usage: image_jpeg.sh <output-file>
set -euo pipefail
out="${1:?usage: image_jpeg.sh <output-file>}"
mkdir -p "$(dirname "$out")"
ffmpeg -y -hide_banner -f lavfi -i "smptebars=size=1920x1080:rate=1" -frames:v 1 -q:v 3 -bitexact "$out"
