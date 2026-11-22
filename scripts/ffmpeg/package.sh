#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Turns the FFmpeg install in $CW_WORK/stage into the relocatable bundle
# $CW_WORK/dist/cutweave-ffmpeg-<ver>-<os>-<arch>.tar.zst (WP-0A.06):
#
#   bin/        ffmpeg, ffprobe (+ the FFmpeg DLLs on Windows)
#   lib/        shared libraries (Linux/macOS) or import libraries: avcodec.lib (MSVC),
#               libavcodec.dll.a (MinGW); lib/pkgconfig/*.pc relative to the bundle
#   include/    FFmpeg headers
#   licenses/   license texts of FFmpeg and every component linked into it
#   source/     SOURCES.txt (origin + pin of every component), versions.toml, the build scripts
#               and where to get the complete source archive
#   configure.txt, BUILD_INFO.txt
#
# Environment: see lib.sh; CW_SOURCE_ARCHIVE (optional) is the source .tar.zst published next to
# the bundle, recorded by name and sha256.

set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
cw_init

exe=""
[ "$CW_OS" != windows ] || exe=".exe"
[ -x "$CW_STAGE/bin/ffmpeg$exe" ] || cw_die "no FFmpeg in $CW_STAGE; run build-ffmpeg.sh first"

B="$CW_DIST/$CW_BUNDLE"
rm -rf "$B" "$B.tar.zst" "$B.tar.zst.sha256"
mkdir -p "$B"
cp -RP "$CW_STAGE/bin" "$CW_STAGE/include" "$CW_STAGE/lib" "$B/"
cp "$CW_WORK/configure.txt" "$B/configure.txt"

# --- Per-OS layout fixes -----------------------------------------------------------------------

windows_imports() { objdump -p "$1" | tr -d '\r' | sed -n 's/^[[:space:]]*DLL Name: //p'; }

windows_is_system_dll() {
  local lower="${1,,}"
  case "$lower" in api-ms-win-* | ext-ms-*) return 0 ;; esac
  [ -f "$(cygpath -u "${SYSTEMROOT:-C:\\Windows}")/System32/$1" ]
}

# Copies any MinGW runtime DLL the binaries still import (there should be none, see
# cw_windows_static_runtime) and records it; fails on an import that is not resolvable.
windows_bundle_runtime() {
  local bin="$1" changed=1 f dll mingw="${MINGW_PREFIX:-/ucrt64}"
  while [ "$changed" = 1 ]; do
    changed=0
    for f in "$bin"/*.exe "$bin"/*.dll; do
      for dll in $(windows_imports "$f"); do
        [ ! -f "$bin/$dll" ] || continue
        ! windows_is_system_dll "$dll" || continue
        [ -f "$mingw/bin/$dll" ] || cw_die "$(basename "$f") imports $dll, which is neither bundled nor a Windows system DLL"
        cw_warn "bundling MinGW runtime $dll (imported by $(basename "$f"))"
        cp "$mingw/bin/$dll" "$bin/"
        echo "$dll" >>"$B/licenses/mingw-runtime.txt"
        changed=1
      done
    done
  done
}

case "$CW_OS" in
  windows)
    # FFmpeg installs the MSVC import libraries next to the DLLs; Rust's FFMPEG_DIR wants lib/.
    mv "$B"/bin/*.lib "$B/lib/"
    ;;
  linux)
    for f in "$B"/bin/*; do
      [ -L "$f" ] || patchelf --set-rpath '$ORIGIN/../lib' "$f"
    done
    for f in "$B"/lib/*.so.*; do
      [ -L "$f" ] || patchelf --set-rpath '$ORIGIN' "$f"
    done
    ;;
esac

# pkg-config files: relative to the bundle, without the CI dependency prefix.
cw_relocate_pc "$B/lib/pkgconfig" "$CW_STAGE"
for f in "$B"/lib/pkgconfig/*.pc; do
  sed -i.bak -e "s|-[IL]$CW_PREFIX[^ ]*||g" "$f"
  rm -f "$f.bak"
done

# --- Licenses ----------------------------------------------------------------------------------

mkdir -p "$B/licenses"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
for c in $CW_COMPONENTS; do
  cw_uses "$c" || continue
  mkdir -p "$tmp/$c" "$B/licenses/$c"
  cw_untar "$CW_SOURCES/$(cw_pin ARCHIVE "$c")" "$tmp/$c"
  (
    cd "$tmp/$c"
    find . -maxdepth 4 -type f \( -iname 'COPYING*' -o -iname 'LICENSE*' -o -iname 'LICENCE*' \
      -o -iname 'COPYRIGHT*' -o -iname 'PATENTS*' -o -iname 'NOTICE*' -o -iname 'FTL.TXT' \
      -o -iname 'GPLv2.TXT' \) | while read -r f; do
      # Keep the path inside the project (minus its top-level directory) to avoid collisions.
      rel="${f#./}"
      [ "$(cw_pin STRIP "$c")" = 0 ] || rel="${rel#*/}"
      mkdir -p "$B/licenses/$c/$(dirname "$rel")"
      cp "$f" "$B/licenses/$c/$rel"
    done
  )
  if [ -z "$(ls -A "$B/licenses/$c")" ]; then
    # Header-only packages (nv-codec-headers, AMF) carry the license in each file's header.
    header="$(find "$tmp/$c" -type f -name '*.h' | sort | head -n 1)"
    [ -n "$header" ] || cw_die "no license text found for $c"
    {
      echo "$c $(cw_pin VERSION "$c"): $(cw_pin LICENSE "$c")"
      echo "Origin: $(cw_pin ORIGIN "$c")"
      echo "The license is stated in the header of every source file, e.g. ${header#"$tmp/$c/"}:"
      echo
      sed -n '1,/\*\//p' "$header" | head -n 60
    } >"$B/licenses/$c/LICENSE-NOTICE.txt"
  fi
  rm -rf "${tmp:?}/$c"
done
[ "$CW_OS" != windows ] || windows_bundle_runtime "$B/bin"
if [ -f "$B/licenses/mingw-runtime.txt" ]; then
  {
    echo
    echo "The DLLs above are part of the MinGW-w64 GCC toolchain: libgcc/libstdc++ are"
    echo "GPL-3.0-or-later WITH GCC-exception-3.1, winpthreads is MIT/BSD licensed."
  } >>"$B/licenses/mingw-runtime.txt"
fi

# --- Corresponding source pointers -------------------------------------------------------------

mkdir -p "$B/source"
cp "$CW_SOURCES/SOURCES.txt" "$CW_SOURCES/versions.toml" "$CW_SOURCES/pins.env" "$B/source/"
cp -R "$CW_SOURCES/scripts" "$B/source/scripts"
run_url=""
if [ -n "${GITHUB_RUN_ID:-}" ]; then
  run_url="${GITHUB_SERVER_URL:-https://github.com}/${GITHUB_REPOSITORY:-}/actions/runs/$GITHUB_RUN_ID"
fi
archive_line="cutweave-ffmpeg-$CW_VERSION-source.tar.zst"
if [ -n "${CW_SOURCE_ARCHIVE:-}" ] && [ -f "$CW_SOURCE_ARCHIVE" ]; then
  archive_line="$(basename "$CW_SOURCE_ARCHIVE") (sha256 $(cw_sha256 "$CW_SOURCE_ARCHIVE"))"
fi
{
  echo "Corresponding source of $CW_BUNDLE"
  echo
  echo "This bundle contains FFmpeg built as GPL-3.0-or-later together with the libraries listed"
  echo "in SOURCES.txt. The complete corresponding source is the archive"
  echo
  echo "  $archive_line"
  echo
  echo "published together with this bundle${run_url:+ by $run_url}. It holds every upstream"
  echo "source archive (pinned by sha256 or commit in versions.toml) and the build scripts, which"
  echo "are also included here under scripts/. Rebuild instructions: scripts/ffmpeg/README.md."
  echo "Anyone redistributing this bundle must make that archive available as well."
} >"$B/source/README.txt"

# --- BUILD_INFO.txt ----------------------------------------------------------------------------

ff="$B/bin/ffmpeg$exe"
first_line() { "$@" 2>&1 | tr -d '\r' | head -n 1 || true; }
dump() {
  echo
  echo "== ffmpeg $* =="
  "$ff" -hide_banner "$@" 2>&1 | tr -d '\r'
}
{
  echo "Cutweave FFmpeg shared bundle"
  echo
  echo "bundle:          $CW_BUNDLE"
  echo "ffmpeg:          $CW_VERSION"
  echo "origin:          $(cw_pin ORIGIN ffmpeg)"
  echo "license:         GPL-3.0-or-later (licenses/, source/SOURCES.txt)"
  echo "built:           $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  [ -z "$run_url" ] || echo "workflow run:    $run_url"
  [ -z "${GITHUB_SHA:-}" ] || echo "repository:      ${GITHUB_REPOSITORY:-} @ $GITHUB_SHA"
  echo "source archive:  $archive_line"
  echo
  echo "== configure line =="
  cat "$B/configure.txt"
  echo
  echo "== components linked into the bundle =="
  for c in $CW_COMPONENTS; do
    cw_uses "$c" || continue
    printf '%-10s %-24s %s\n' "$c" "$(cw_pin VERSION "$c")" "$(cw_pin LICENSE "$c")"
    printf '%-10s %s\n' "" "$(cw_pin ORIGIN "$c")"
  done
  if [ -f "$B/licenses/mingw-runtime.txt" ]; then
    echo
    echo "== bundled MinGW runtime DLLs =="
    cat "$B/licenses/mingw-runtime.txt"
  fi
  echo
  echo "== toolchain =="
  echo "host:            $(uname -a)"
  case "$CW_OS" in
    linux) echo "os:              $(. /etc/os-release && echo "$PRETTY_NAME"), $(first_line ldd --version)" ;;
    macos) echo "os:              macOS $(sw_vers -productVersion), SDK $(xcrun --show-sdk-version), MACOSX_DEPLOYMENT_TARGET=$MACOSX_DEPLOYMENT_TARGET" ;;
    windows) echo "os:              MSYS2 ${MSYSTEM:-}, $(cmd.exe //c ver 2>/dev/null | tr -d '\r' | grep -m 1 . || true)" ;;
  esac
  echo "cc:              $(first_line "${CC:-cc}" --version)"
  echo "nasm:            $(first_line nasm -v)"
  echo "cmake:           $(first_line cmake --version)"
  echo "meson:           $(first_line meson --version)"
  echo "pkg-config:      $(first_line pkg-config --version)"
  if [ "$CW_OS" = windows ]; then
    implib=dlltool
    grep -q 'lib\.exe' "$CW_SRC/ffmpeg/ffbuild/config.mak" 2>/dev/null && implib="MSVC lib.exe"
    echo "import libs:     $implib"
  fi
  echo
  echo "== ffmpeg -version =="
  "$ff" -version 2>&1 | tr -d '\r'
  dump -buildconf
  dump -hwaccels
  dump -encoders
  dump -filters
} >"$B/BUILD_INFO.txt"

# --- Archive -----------------------------------------------------------------------------------

cw_log "packing $CW_BUNDLE.tar.zst"
(cd "$CW_DIST" && COPYFILE_DISABLE=1 tar -cf - "$CW_BUNDLE") | zstd -q -19 -T0 -f -o "$B.tar.zst"
sum="$(cw_sha256 "$B.tar.zst")"
echo "$sum  $CW_BUNDLE.tar.zst" >"$B.tar.zst.sha256"
size="$(du -h "$B.tar.zst" | cut -f1)"
cw_log "$B.tar.zst ($size, sha256 $sum)"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "### $CW_BUNDLE"
    echo
    echo "| file | size | sha256 |"
    echo "|---|---|---|"
    echo "| \`$CW_BUNDLE.tar.zst\` | $size | \`$sum\` |"
    echo
    echo "Configure line: \`$(cat "$B/configure.txt")\`"
  } >>"$GITHUB_STEP_SUMMARY"
fi
