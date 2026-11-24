# SPDX-License-Identifier: GPL-3.0-or-later
# Source before building or running the S3 spike:  . ./env.sh [FFMPEG_DIR]
# Sets FFMPEG_DIR (headers + libs for ffmpeg-sys-next), LIBCLANG_PATH on Windows (bindgen), and
# the runtime library path (PATH / DYLD_FALLBACK_LIBRARY_PATH / LD_LIBRARY_PATH).

_s3_dir="${1:-${FFMPEG_DIR:-}}"
if [ -z "$_s3_dir" ]; then
  _s3_root="$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null | sed 's#/\.git$##')"
  if [ -f "$_s3_root/third_party/ffmpeg/ffmpeg.env" ]; then
    _s3_dir="$(sed -n 's/^FFMPEG_DIR="\(.*\)"/\1/p' "$_s3_root/third_party/ffmpeg/ffmpeg.env" | head -n 1)"
  fi
fi
if [ -z "$_s3_dir" ] || [ ! -f "$_s3_dir/include/libavcodec/avcodec.h" ]; then
  echo "env.sh: no FFmpeg dev bundle (run 'just ffmpeg-fetch' or pass its directory)" >&2
  return 1 2>/dev/null || exit 1
fi
export FFMPEG_DIR="$_s3_dir"
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*)
    [ -z "${LIBCLANG_PATH:-}" ] && [ -f "/c/Program Files/LLVM/bin/libclang.dll" ] && export LIBCLANG_PATH="C:/Program Files/LLVM/bin"
    export PATH="$(cygpath -u "$FFMPEG_DIR")/bin:$PATH" ;;
  Darwin) export DYLD_FALLBACK_LIBRARY_PATH="$FFMPEG_DIR/lib${DYLD_FALLBACK_LIBRARY_PATH:+:$DYLD_FALLBACK_LIBRARY_PATH}" ;;
  *) export LD_LIBRARY_PATH="$FFMPEG_DIR/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" ;;
esac
echo "FFMPEG_DIR=$FFMPEG_DIR"
unset _s3_dir _s3_root
