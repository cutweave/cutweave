#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Acceptance checks for an FFmpeg bundle (WP-0A.06): layout, pinned version, configure line,
# -buildconf, encoders, decoders, filters, hwaccels and dynamic linkage (glibc <= 2.35 and only
# system libraries on Linux; only system frameworks on macOS; only bundled or Windows system DLLs
# on Windows). Runs the bundle's own ffmpeg, so it must run on the bundle's OS.
#
# Usage: scripts/ffmpeg/verify.sh <bundle.tar.zst | bundle-dir> [windows|macos|linux]

set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

[ $# -ge 1 ] || cw_die "usage: verify.sh <bundle.tar.zst | bundle-dir> [windows|macos|linux]"
CW_OS="${2:-$(cw_host_os)}"

dir="$1"
if [ -f "$1" ]; then
  # Check the archive as a consumer gets it (symlinks, rpaths and all), not the staging tree.
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  cw_untar "$1" "$tmp"
  dir="$(find "$tmp" -mindepth 1 -maxdepth 1 -type d | head -n 1)"
fi
[ -d "$dir" ] || cw_die "not a bundle: $1"

failures=0
ok() { printf '  ok    %s\n' "$*"; }
fail() {
  printf '  FAIL  %s\n' "$*"
  failures=$((failures + 1))
}
# expect <what> <list> <item...>: every item must be a line of <list>.
expect() {
  local what="$1" list="$2" item missing=()
  shift 2
  for item in "$@"; do
    grep -qxF -- "$item" <<<"$list" || missing+=("$item")
  done
  if [ ${#missing[@]} -eq 0 ]; then ok "$what: $*"; else fail "$what missing: ${missing[*]}"; fi
}

exe=""
[ "$CW_OS" != windows ] || exe=".exe"
ff="$dir/bin/ffmpeg$exe"
[ -f "$dir/source/pins.env" ] || cw_die "$dir/source/pins.env missing"
# shellcheck source=/dev/null
. "$dir/source/pins.env"

echo "verifying $(basename "$dir") ($CW_OS)"

# --- Layout ------------------------------------------------------------------------------------
files=(bin/ffmpeg$exe bin/ffprobe$exe include/libavcodec/avcodec.h include/libavformat/avformat.h
  include/libavutil/avutil.h lib/pkgconfig/libavformat.pc BUILD_INFO.txt configure.txt
  source/SOURCES.txt source/README.txt source/versions.toml licenses/ffmpeg/COPYING.GPLv3)
for lib in avcodec avdevice avfilter avformat avutil swresample swscale; do
  case "$CW_OS" in
    windows) files+=("lib/$lib.lib" "lib/lib$lib.dll.a") ;;
    macos) files+=("lib/lib$lib.dylib") ;;
    linux) files+=("lib/lib$lib.so") ;;
  esac
done
missing=()
for f in "${files[@]}"; do [ -e "$dir/$f" ] || missing+=("$f"); done
if [ ${#missing[@]} -eq 0 ]; then ok "layout (${#files[@]} files)"; else fail "missing files: ${missing[*]}"; fi
if [ "$CW_OS" = windows ]; then
  ls "$dir"/bin/avformat-*.dll >/dev/null 2>&1 && ok "FFmpeg DLLs in bin/" || fail "no avformat-*.dll in bin/"
fi

# --- Version and configure line ----------------------------------------------------------------
if ! version="$("$ff" -version 2>&1 | tr -d '\r')"; then
  printf '%s\n' "$version" >&2
  cw_die "the bundle's ffmpeg does not run (missing shared library or DLL?)"
fi
if [[ "$(head -n 1 <<<"$version")" == "ffmpeg version $CW_FFMPEG_VERSION "* ]]; then
  ok "ffmpeg -version reports $CW_FFMPEG_VERSION"
else
  fail "ffmpeg -version: $(head -n 1 <<<"$version") (expected $CW_FFMPEG_VERSION)"
fi
grep -q -- "--enable-gpl" <<<"$version" && ok "-version shows --enable-gpl" || fail "-version lacks --enable-gpl"
"$dir/bin/ffprobe$exe" -version >/dev/null && ok "ffprobe runs" || fail "ffprobe does not run"

required_flags=(--enable-shared --enable-gpl --enable-version3 --enable-libx264 --enable-libx265
  --enable-libsvtav1 --enable-libvpx --enable-libopus --enable-libmp3lame --enable-libvidstab
  --enable-libzimg --enable-libsoxr --enable-libass --enable-libdav1d)
case "$CW_OS" in
  windows) required_flags+=(--enable-d3d11va --enable-d3d12va --enable-dxva2 --enable-mediafoundation
    --enable-ffnvcodec --enable-nvdec --enable-nvenc --enable-cuvid --enable-amf --enable-libvpl) ;;
  macos) required_flags+=(--enable-videotoolbox --enable-audiotoolbox) ;;
  linux) required_flags+=(--enable-vaapi --enable-vdpau --enable-libdrm --enable-ffnvcodec
    --enable-nvdec --enable-nvenc --enable-cuvid --enable-libvpl) ;;
esac
expect "configure.txt" "$(tr ' ' '\n' <"$dir/configure.txt")" "${required_flags[@]}"
buildconf="$("$ff" -hide_banner -buildconf | tr -d '\r' | awk '{print $1}')"
expect "ffmpeg -buildconf" "$buildconf" "${required_flags[@]}"

# --- Encoders, decoders, filters, hwaccels ----------------------------------------------------
# Column 1 is the capability flags (e.g. V....D), column 2 the name.
encoders="$("$ff" -hide_banner -encoders | tr -d '\r' | awk 'NF >= 2 { print $2 }')"
required_encoders=(libx264 libx265 libsvtav1 libvpx libvpx-vp9 libopus libmp3lame prores_ks dnxhd gif)
case "$CW_OS" in
  windows) required_encoders+=(h264_nvenc hevc_nvenc av1_nvenc h264_amf hevc_amf av1_amf h264_qsv
    hevc_qsv av1_qsv h264_mf hevc_mf) ;;
  macos) required_encoders+=(h264_videotoolbox hevc_videotoolbox prores_videotoolbox) ;;
  linux) required_encoders+=(h264_vaapi hevc_vaapi h264_nvenc hevc_nvenc av1_nvenc h264_qsv hevc_qsv
    av1_qsv) ;;
esac
expect "ffmpeg -encoders" "$encoders" "${required_encoders[@]}"

decoders="$("$ff" -hide_banner -decoders | tr -d '\r' | awk 'NF >= 2 { print $2 }')"
required_decoders=(libdav1d h264 hevc prores)
[ "$CW_OS" = macos ] || required_decoders+=(h264_cuvid hevc_cuvid)
expect "ffmpeg -decoders" "$decoders" "${required_decoders[@]}"

filters="$("$ff" -hide_banner -filters | tr -d '\r' | awk 'NF >= 3 { print $2 }')"
expect "ffmpeg -filters" "$filters" vidstabdetect vidstabtransform zscale subtitles ass drawtext

hwaccels="$("$ff" -hide_banner -hwaccels | tr -d '\r' | sed '1d' | awk 'NF { print $1 }')"
case "$CW_OS" in
  windows) expect "ffmpeg -hwaccels" "$hwaccels" cuda dxva2 d3d11va d3d12va qsv ;;
  macos) expect "ffmpeg -hwaccels" "$hwaccels" videotoolbox ;;
  linux) expect "ffmpeg -hwaccels" "$hwaccels" cuda vaapi vdpau qsv drm ;;
esac

# --- Dynamic linkage ---------------------------------------------------------------------------
case "$CW_OS" in
  linux)
    allowed='^(lib(c|m|mvec|dl|rt|pthread|stdc\+\+|gcc_s|z|va|va-drm|vdpau|X11|drm|fontconfig|freetype|expat|uuid)\.so\.[0-9]+|ld-linux-x86-64\.so\.2|lib(avcodec|avdevice|avfilter|avformat|avutil|swresample|swscale|postproc)\.so\.[0-9]+)$'
    bad=()
    glibc_ok=1
    for f in "$dir"/bin/* "$dir"/lib/*.so.*; do
      [ -L "$f" ] && continue
      while read -r needed; do
        [[ $needed =~ $allowed ]] || bad+=("$(basename "$f"): $needed")
      done < <(readelf -d "$f" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p')
      bash "$CW_ROOT/scripts/check-glibc.sh" "$f" 2.35 >/dev/null || glibc_ok=0
    done
    if [ ${#bad[@]} -eq 0 ]; then ok "NEEDED: only system interface libraries"; else fail "unexpected NEEDED: ${bad[*]}"; fi
    if [ "$glibc_ok" = 1 ]; then ok "glibc symbol versions <= 2.35"; else fail "a binary needs glibc > 2.35"; fi
    ;;
  macos)
    bad=()
    for f in "$dir"/bin/ffmpeg "$dir"/bin/ffprobe "$dir"/lib/*.dylib; do
      [ -L "$f" ] && continue
      while read -r dep; do
        [[ $dep =~ ^(/usr/lib/|/System/Library/|@rpath/lib(avcodec|avdevice|avfilter|avformat|avutil|swresample|swscale|postproc)\.) ]] ||
          bad+=("$(basename "$f"): $dep")
      done < <(otool -L "$f" | tail -n +2 | awk '{print $1}')
    done
    if [ ${#bad[@]} -eq 0 ]; then ok "load commands: system frameworks and @rpath FFmpeg only"; else fail "unexpected dylibs: ${bad[*]}"; fi
    ;;
  windows)
    bad=()
    sys32="$(cygpath -u "${SYSTEMROOT:-C:\\Windows}")/System32"
    for f in "$dir"/bin/*.exe "$dir"/bin/*.dll; do
      while read -r dll; do
        [ -n "$dll" ] || continue
        [ -f "$dir/bin/$dll" ] || [ -f "$sys32/$dll" ] || [[ ${dll,,} == api-ms-win-* ]] ||
          [[ ${dll,,} == ext-ms-* ]] ||
          bad+=("$(basename "$f"): $dll")
      done < <(objdump -p "$f" | tr -d '\r' | sed -n 's/^[[:space:]]*DLL Name: //p')
    done
    if [ ${#bad[@]} -eq 0 ]; then ok "DLL imports: bundled or Windows system DLLs only"; else fail "unresolvable DLL imports: ${bad[*]}"; fi
    ;;
esac

if [ "$failures" -ne 0 ]; then
  echo "verify: $failures check(s) failed"
  exit 1
fi
echo "verify: all checks passed"
