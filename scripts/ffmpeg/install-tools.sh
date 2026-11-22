#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Installs the build tools for the FFmpeg bundle (WP-0A.06) on a CI runner or a clean machine.
# Tool versions are not pinned (except meson on Linux, see versions.toml [tools]); the exact
# versions used end up in each bundle's BUILD_INFO.txt.
#
# Usage: scripts/ffmpeg/install-tools.sh [linux|macos|windows]
#   linux:   as root inside ubuntu:22.04 (the glibc 2.35 baseline, docs/01-architecture.md §21)
#   macos:   Homebrew
#   windows: MSYS2, from a UCRT64 login shell

set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

os="${1:-$(cw_host_os)}"
toml="$CW_SCRIPTS/versions.toml"

case "$os" in
  linux)
    meson_version="$(sed -n 's/^meson_pip *= *"\([^"]*\)".*/\1/p' "$toml")"
    system_apt="$(sed -n '/^\[system\.linux\]/,/^\[/ s/^apt *= *\[\(.*\)\].*/\1/p' "$toml" | tr -d '",')"
    [ -n "$meson_version" ] && [ -n "$system_apt" ] || cw_die "cannot read [tools]/[system.linux] from $toml"
    export DEBIAN_FRONTEND=noninteractive
    apt-get update
    # shellcheck disable=SC2086 # word splitting of the package list is intended
    apt-get install -y --no-install-recommends \
      build-essential ca-certificates curl xz-utils bzip2 zstd file patchelf binutils \
      pkg-config nasm cmake ninja-build autoconf automake libtool perl python3 python3-pip \
      $system_apt
    python3 -m pip install --no-cache-dir "meson==$meson_version"
    ;;
  macos)
    export HOMEBREW_NO_AUTO_UPDATE=1 HOMEBREW_NO_INSTALL_CLEANUP=1 HOMEBREW_NO_ENV_HINTS=1
    # command -> formula; only what is missing, so preinstalled runner tools are left alone.
    missing=()
    for pair in meson:meson ninja:ninja pkg-config:pkgconf autoconf:autoconf automake:automake \
      glibtoolize:libtool cmake:cmake zstd:zstd; do
      command -v "${pair%%:*}" >/dev/null 2>&1 || missing+=("${pair#*:}")
    done
    [ ${#missing[@]} -eq 0 ] || brew install "${missing[@]}"
    ;;
  windows)
    [ -n "${MSYSTEM:-}" ] || cw_die "run this from an MSYS2 UCRT64 shell"
    pkg="${MINGW_PACKAGE_PREFIX:-mingw-w64-ucrt-x86_64}"
    pacman -Sy --noconfirm --needed \
      make diffutils autoconf automake libtool perl patch tar xz zstd \
      "$pkg-gcc" "$pkg-pkgconf" "$pkg-nasm" "$pkg-cmake" "$pkg-ninja" "$pkg-meson" "$pkg-python"
    ;;
  *) cw_die "usage: install-tools.sh [linux|macos|windows]" ;;
esac

# Toolchain fingerprint for the CI dependency cache key: a compiler update (rolling MSYS2,
# runner image Xcode) must not reuse static libraries built by the previous compiler.
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  if [ "$os" = macos ]; then cc=clang; else cc=gcc; fi
  id="$({ "$cc" --version | head -n 1; cmake --version | head -n 1; } | tr -d '\r' |
    { sha256sum 2>/dev/null || shasum -a 256; } | cut -c 1-16)"
  echo "toolchain=$id" >>"$GITHUB_OUTPUT"
fi
