#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The integrator's gate for a checkout: every `check-affected` step against <base>, continuing
# past failures (its GPU steps take the GPU lock themselves), then the FFmpeg suites, built
# outside the lock and run inside it. Run it through the lock wrapper, which passes the lock file
# along and does not lock the whole gate:
#
#   python scripts/gpu-lock.py -- bash scripts/gate-integrate.sh <base> [checkout dir]
#
# The last line is `GATE FAILED: none` or `GATE FAILED: [step] ...`.
set -u
if [ $# -lt 1 ] || [ -z "$1" ]; then
  echo "usage: bash scripts/gate-integrate.sh <base> [checkout dir]" >&2
  exit 2
fi
cd "${2:-$(git rev-parse --show-toplevel)}"
LOCKPY="$(pwd)/scripts/gpu-lock.py"
FAILED=""
python scripts/check-affected.py --base "$1" --keep-going || FAILED="$FAILED [check-affected]"
echo "==> ffmpeg suites"
FF="cargo nextest run -p cutweave-media -p cutweave-engine -p cutweave-cache -p cutweave-api --features cutweave-engine/ffmpeg,cutweave-cache/ffmpeg"
if ( . scripts/ffmpeg/env.sh && $FF --no-run ); then
  python "$LOCKPY" -- bash -c ". scripts/ffmpeg/env.sh && $FF" || FAILED="$FAILED [ffmpeg suites]"
else
  FAILED="$FAILED [ffmpeg build]"
fi
echo "GATE FAILED:${FAILED:-none}"
