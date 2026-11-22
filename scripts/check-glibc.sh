#!/usr/bin/env bash
# Fails if a binary requires a GLIBC symbol version newer than the given maximum.
# Usage: scripts/check-glibc.sh <binary> <max-version, e.g. 2.35>
set -euo pipefail

bin="${1:?usage: check-glibc.sh <binary> <max-version>}"
max="${2:?usage: check-glibc.sh <binary> <max-version>}"

found="$(objdump -T "$bin" | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -n 1)"
if [ -z "$found" ]; then
  echo "no GLIBC symbol versions found in $bin"
  exit 0
fi
echo "max required glibc symbol version: $found (limit $max)"
highest="$(printf '%s\n%s\n' "$found" "$max" | sort -V | tail -n 1)"
if [ "$highest" != "$max" ]; then
  echo "error: $bin requires glibc $found > $max" >&2
  exit 1
fi
