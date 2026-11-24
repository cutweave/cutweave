# SPDX-License-Identifier: GPL-3.0-or-later
#
# Source to build and run against the FFmpeg dev bundle (docs/dev/ffmpeg.md):
#
#   . scripts/ffmpeg/env.sh [BUNDLE_DIR]
#
# Exports FFMPEG_DIR and PKG_CONFIG_PATH (ffmpeg-sys-next's build script), LIBCLANG_PATH on
# Windows when LLVM is in its default place (bindgen), and the run-time library path every
# binary that links FFmpeg needs (PATH on Windows, DYLD_FALLBACK_LIBRARY_PATH on macOS,
# LD_LIBRARY_PATH on Linux). Test executables die before main without it.
#
# The bundle is, in order: BUNDLE_DIR; $FFMPEG_DIR; FFMPEG_DIR from third_party/ffmpeg/ffmpeg.env
# of this checkout; the same file in the main checkout (git worktrees share it). Each candidate
# must contain include/libavcodec/avcodec.h. `just ffmpeg-fetch` writes ffmpeg.env.

_cw_has() { [ -n "$1" ] && [ -f "$1/include/libavcodec/avcodec.h" ]; }
_cw_from_env_file() {
  if [ -f "$1" ]; then sed -n 's/^FFMPEG_DIR="\(.*\)"/\1/p' "$1" | head -n 1; fi
}

_cw_dir=""
for _cw_try in "${1:-}" "${FFMPEG_DIR:-}"; do
  if _cw_has "$_cw_try"; then _cw_dir="$_cw_try"; break; fi
done
if [ -z "$_cw_dir" ]; then
  # `|| true`: safe to source from `set -eu` shells (just recipes) outside a git checkout.
  _cw_here="$(git rev-parse --show-toplevel 2>/dev/null || true)"
  _cw_main="$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null | sed 's#/\.git$##' || true)"
  for _cw_root in "$_cw_here" "$_cw_main"; do
    [ -n "$_cw_root" ] || continue
    _cw_try="$(_cw_from_env_file "$_cw_root/third_party/ffmpeg/ffmpeg.env")"
    if _cw_has "$_cw_try"; then _cw_dir="$_cw_try"; break; fi
  done
fi
if [ -z "$_cw_dir" ]; then
  echo "env.sh: no FFmpeg dev bundle found (FFMPEG_DIR unset or without include/, no third_party/ffmpeg/ffmpeg.env)." >&2
  echo "env.sh: run 'just ffmpeg-fetch' or pass the bundle directory; see docs/dev/ffmpeg.md" >&2
  unset -f _cw_has _cw_from_env_file
  unset _cw_dir _cw_try _cw_here _cw_main _cw_root
  return 1 2>/dev/null || exit 1
fi

export FFMPEG_DIR="$_cw_dir"
export PKG_CONFIG_PATH="$FFMPEG_DIR/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*)
    if [ -z "${LIBCLANG_PATH:-}" ] && [ -f "/c/Program Files/LLVM/bin/libclang.dll" ]; then
      export LIBCLANG_PATH="C:/Program Files/LLVM/bin"
    fi
    export PATH="$(cygpath -u "$FFMPEG_DIR")/bin:$PATH" ;;
  Darwin)
    export DYLD_FALLBACK_LIBRARY_PATH="$FFMPEG_DIR/lib${DYLD_FALLBACK_LIBRARY_PATH:+:$DYLD_FALLBACK_LIBRARY_PATH}" ;;
  *)
    export LD_LIBRARY_PATH="$FFMPEG_DIR/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" ;;
esac
echo "FFMPEG_DIR=$FFMPEG_DIR" >&2
unset -f _cw_has _cw_from_env_file
unset _cw_dir _cw_try _cw_here _cw_main _cw_root
