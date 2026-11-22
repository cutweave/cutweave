#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Generates the `image_png` corpus item (testdata/corpus.toml). Needs ffmpeg on PATH.
# Usage: image_png.sh <output-file>
set -euo pipefail
out="${1:?usage: image_png.sh <output-file>}"
mkdir -p "$(dirname "$out")"
ffmpeg -y -hide_banner -f lavfi -i "testsrc2=size=1920x1080:rate=1" -frames:v 1 -bitexact "$out"
