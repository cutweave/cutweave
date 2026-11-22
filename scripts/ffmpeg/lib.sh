# SPDX-License-Identifier: GPL-3.0-or-later
# shellcheck shell=bash
#
# Shared helpers for the FFmpeg bundle scripts (WP-0A.06). Sourced by build-deps.sh,
# build-ffmpeg.sh, package.sh, verify.sh and install-tools.sh; not meant to be executed.
#
# Environment (all optional):
#   CW_OS       windows | macos | linux               (default: the host)
#   CW_ARCH     x64 | arm64                           (default: the host)
#   CW_SOURCES  source tree written by `sources.py fetch`
#               (default: third_party/ffmpeg/cutweave-ffmpeg-<version>-source)
#   CW_WORK     scratch root (default: third_party/ffmpeg/build) holding
#               src/ (unpacked sources), build/, deps/ (static libraries), stage/ (FFmpeg
#               install) and dist/ (the bundle)
#   CW_JOBS     parallel build jobs (default: CPU count)

CW_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CW_SCRIPTS="$CW_ROOT/scripts/ffmpeg"

cw_log() { printf '\n==> %s\n' "$*" >&2; }
cw_warn() { printf 'warning: %s\n' "$*" >&2; }
cw_die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

cw_host_os() {
  case "$(uname -s)" in
    Linux) echo linux ;;
    Darwin) echo macos ;;
    MINGW* | MSYS* | CYGWIN*) echo windows ;;
    *) cw_die "unsupported OS: $(uname -s)" ;;
  esac
}

cw_host_arch() {
  case "$(uname -m)" in
    x86_64 | amd64) echo x64 ;;
    arm64 | aarch64) echo arm64 ;;
    *) cw_die "unsupported CPU: $(uname -m)" ;;
  esac
}

cw_nproc() { nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 2; }

# The FFmpeg version pinned in versions.toml (first `version = "..."` of the [ffmpeg] table).
cw_pinned_version() {
  sed -n '/^\[ffmpeg\]/,/^\[/ s/^version *= *"\([^"]*\)".*/\1/p' "$CW_SCRIPTS/versions.toml" |
    head -n 1
}

# Absolute path in the form native tools understand (C:/x on Windows, where MSYS2 shells see /c/x).
cw_native_path() {
  if [ "${CW_OS:-}" = windows ]; then cygpath -m "$1"; else printf '%s\n' "$1"; fi
}

cw_mkdir_abs() {
  mkdir -p "$1"
  cw_native_path "$(cd "$1" && pwd)"
}

# Indirect read of a pins.env variable, e.g. `cw_pin ARCHIVE x264`.
cw_pin() {
  local var="CW_$1_$2"
  printf '%s\n' "${!var:-}"
}

# True if component $1 is pinned and part of this OS's bundle.
cw_uses() {
  local oses
  oses=" $(cw_pin OS "$1") "
  [ -n "$(cw_pin ARCHIVE "$1")" ] && [[ $oses == *" $CW_OS "* ]]
}

cw_init() {
  CW_OS="${CW_OS:-$(cw_host_os)}"
  CW_ARCH="${CW_ARCH:-$(cw_host_arch)}"
  CW_VERSION="$(cw_pinned_version)"
  [ -n "$CW_VERSION" ] || cw_die "cannot read the FFmpeg version from $CW_SCRIPTS/versions.toml"
  CW_SOURCES="${CW_SOURCES:-$CW_ROOT/third_party/ffmpeg/cutweave-ffmpeg-$CW_VERSION-source}"
  [ -f "$CW_SOURCES/pins.env" ] ||
    cw_die "no pins.env in $CW_SOURCES: run 'python3 scripts/ffmpeg/sources.py fetch' or unpack the source archive (scripts/ffmpeg/README.md)"
  CW_SOURCES="$(cw_mkdir_abs "$CW_SOURCES")"
  # shellcheck source=/dev/null
  . "$CW_SOURCES/pins.env"
  [ "$CW_FFMPEG_VERSION" = "$CW_VERSION" ] ||
    cw_die "source tree is for FFmpeg $CW_FFMPEG_VERSION but versions.toml pins $CW_VERSION"

  CW_WORK="$(cw_mkdir_abs "${CW_WORK:-$CW_ROOT/third_party/ffmpeg/build}")"
  CW_SRC="$CW_WORK/src"
  CW_BLD="$CW_WORK/build"
  CW_PREFIX="$CW_WORK/deps"
  CW_STAGE="$CW_WORK/stage"
  CW_DIST="$CW_WORK/dist"
  CW_JOBS="${CW_JOBS:-$(cw_nproc)}"
  CW_BUNDLE="cutweave-ffmpeg-$CW_VERSION-$CW_OS-$CW_ARCH"
  mkdir -p "$CW_SRC" "$CW_BLD" "$CW_PREFIX/include" "$CW_PREFIX/lib/pkgconfig"
  export CW_OS CW_ARCH CW_VERSION CW_SOURCES CW_WORK CW_PREFIX CW_STAGE CW_DIST CW_JOBS
  cw_env
}

# Compiler and pkg-config environment shared by every dependency build and by FFmpeg.
cw_env() {
  # Only our prefix (plus the Linux system interface libraries) may leak into the build.
  unset CPATH C_INCLUDE_PATH CPLUS_INCLUDE_PATH OBJC_INCLUDE_PATH LIBRARY_PATH PKG_CONFIG_PATH
  PKG_CONFIG_LIBDIR="$CW_PREFIX/lib/pkgconfig"
  CPPFLAGS="-I$CW_PREFIX/include"
  LDFLAGS="-L$CW_PREFIX/lib"
  case "$CW_OS" in
    linux)
      local multiarch
      multiarch="$(gcc -print-multiarch 2>/dev/null || true)"
      PKG_CONFIG_LIBDIR="$PKG_CONFIG_LIBDIR:/usr/lib/${multiarch:+$multiarch/}pkgconfig:/usr/lib/pkgconfig:/usr/share/pkgconfig"
      ;;
    macos)
      export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.0}"
      export CC=clang CXX=clang++
      # Homebrew's GNU libtool is installed as glibtoolize (zimg's autogen.sh needs it).
      if command -v glibtoolize >/dev/null 2>&1; then export LIBTOOLIZE=glibtoolize; fi
      ;;
    windows)
      export CC=gcc CXX=g++
      # Link the GCC runtime statically into everything (see cw_windows_static_runtime).
      LDFLAGS="$LDFLAGS -static-libgcc -static-libstdc++"
      ;;
  esac
  export PKG_CONFIG_LIBDIR CPPFLAGS LDFLAGS
}

# C++ runtime that static C++ dependencies (x265, zimg, libvpl, SVT-AV1) need at link time.
cw_cxx_libs() {
  case "$CW_OS" in
    linux) echo "-lstdc++" ;;
    macos) echo "-lc++" ;;
    windows) echo "-lstdc++ -lwinpthread" ;;
  esac
}

# Windows: ld searches the -L directories before the toolchain's, and in each directory it takes
# the first of libX.dll.a / libX.a. Copying the static GCC/winpthreads archives into our prefix
# makes -lstdc++ / -lpthread / -lwinpthread resolve statically, so the FFmpeg DLLs do not import
# libstdc++-6.dll or libwinpthread-1.dll.
cw_windows_static_runtime() {
  local lib path
  for lib in libstdc++.a libwinpthread.a; do
    path="$("$CC" -print-file-name="$lib")"
    [ -f "$path" ] || cw_die "static $lib not found in the MinGW toolchain"
    cp "$path" "$CW_PREFIX/lib/$lib"
  done
  cp "$CW_PREFIX/lib/libwinpthread.a" "$CW_PREFIX/lib/libpthread.a"
}

# Unpacks a pinned component into $CW_SRC/<name> and prints that directory.
cw_extract() {
  local name="$1" archive strip dest
  archive="$(cw_pin ARCHIVE "$name")"
  [ -n "$archive" ] || cw_die "$name is not in $CW_SOURCES/pins.env"
  strip="$(cw_pin STRIP "$name")"
  dest="$CW_SRC/$name"
  rm -rf "$dest"
  mkdir -p "$dest"
  cw_untar "$CW_SOURCES/$archive" "$dest" --strip-components="${strip:-1}"
  printf '%s\n' "$dest"
}

# tar -xf ARCHIVE -C DIR [tar options]. GNU tar reads "C:/x" as host "C", so MSYS2 gets
# POSIX paths.
cw_untar() {
  local archive="$1" dir="$2"
  shift 2
  if [ "${CW_OS:-}" = windows ]; then
    archive="$(cygpath -u "$archive")"
    dir="$(cygpath -u "$dir")"
  fi
  case "$archive" in
    *.zst) zstd -dc "$archive" | tar -xf - -C "$dir" "$@" ;;
    *) tar -xf "$archive" -C "$dir" "$@" ;;
  esac
}

# Replaces outdated config.guess/config.sub in release tarballs (lame's date from 2017).
cw_refresh_config() {
  local auxdir f
  auxdir="$(automake --print-libdir 2>/dev/null)" || return 0
  [ -f "$auxdir/config.guess" ] || return 0
  find "$1" \( -name config.guess -o -name config.sub \) -type f | while read -r f; do
    cp "$auxdir/$(basename "$f")" "$f"
  done
}

cw_cmake_build() {
  local src="$1" bld="$2"
  shift 2
  local args=(
    -G Ninja
    -DCMAKE_BUILD_TYPE=Release
    "-DCMAKE_INSTALL_PREFIX=$CW_PREFIX"
    -DCMAKE_INSTALL_LIBDIR=lib
    "-DCMAKE_PREFIX_PATH=$CW_PREFIX"
    -DBUILD_SHARED_LIBS=OFF
    -DCMAKE_POSITION_INDEPENDENT_CODE=ON
    # CMake 4 rejects projects that still declare cmake_minimum_required < 3.5 (soxr).
    -DCMAKE_POLICY_VERSION_MINIMUM=3.5
  )
  [ "$CW_OS" != macos ] || args+=("-DCMAKE_IGNORE_PREFIX_PATH=/opt/homebrew;/usr/local")
  rm -rf "$bld"
  cmake -S "$src" -B "$bld" "${args[@]}" "$@"
  cmake --build "$bld" --parallel "$CW_JOBS"
}

cw_cmake() {
  cw_cmake_build "$@"
  cmake --install "$2"
}

cw_meson() {
  local src="$1" bld="$2"
  shift 2
  rm -rf "$bld"
  meson setup "$bld" "$src" --prefix="$CW_PREFIX" --libdir=lib --buildtype=release \
    --default-library=static -Db_staticpic=true --wrap-mode=nodownload "$@"
  meson compile -C "$bld" -j "$CW_JOBS"
  meson install -C "$bld"
}

# In-tree autotools build (changes directory; call it from a subshell). make-based builds stay
# in-tree on purpose: with C:/x paths, an out-of-tree VPATH would split at the drive colon.
cw_autotools() {
  local src="$1"
  shift
  local args=(--prefix="$CW_PREFIX" --libdir="$CW_PREFIX/lib" --enable-static --disable-shared)
  [ "$CW_OS" != linux ] || args+=(--with-pic)
  cd "$src"
  ./configure "${args[@]}" "$@"
  make -j"$CW_JOBS"
  make install
}

# Merges static archives (x265's 8/10/12-bit builds) into one.
cw_merge_static() {
  local out="$1"
  shift
  if [ "$CW_OS" = macos ]; then
    /usr/bin/libtool -static -o "$out" "$@"
  else
    {
      echo "CREATE $out"
      for a in "$@"; do echo "ADDLIB $a"; done
      echo "SAVE"
      echo "END"
    } | ar -M
    ranlib "$out"
  fi
}

# Sets (or adds) one field of a pkg-config file.
cw_pc_set() {
  local file="$1" key="$2" value="$3"
  if grep -q "^$key:" "$file"; then
    sed -i.bak "s|^$key:.*|$key: $value|" "$file"
    rm -f "$file.bak"
  else
    printf '%s: %s\n' "$key" "$value" >>"$file"
  fi
}

# Rewrites absolute paths below $2 in the .pc files of $1 to ${pcfiledir}-relative ones, so a
# prefix restored from the CI cache (or unpacked elsewhere) still resolves.
cw_relocate_pc() {
  local pcdir="$1" prefix="$2" f alt=""
  [ "$CW_OS" != windows ] || alt="$(cygpath -u "$prefix")"
  for f in "$pcdir"/*.pc; do
    [ -f "$f" ] || continue
    sed -i.bak -e "s|$prefix|\${pcfiledir}/../..|g" "$f"
    [ -z "$alt" ] || sed -i.bak -e "s|$alt|\${pcfiledir}/../..|g" "$f"
    rm -f "$f.bak"
  done
}

cw_sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# Puts MSVC's lib.exe on PATH (appended, so MSYS2's own tools keep precedence). FFmpeg's MinGW
# build then emits MSVC-native import libraries (avcodec.lib, ...) for MSVC Rust targets.
cw_msvc_lib_on_path() {
  command -v lib.exe >/dev/null 2>&1 && return 0
  local vswhere="/c/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe" found
  [ -x "$vswhere" ] || return 1
  found="$("$vswhere" -latest -products '*' \
    -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 \
    -find 'VC/Tools/MSVC/**/bin/Hostx64/x64/lib.exe' | tr -d '\r' | head -n 1)"
  [ -n "$found" ] || return 1
  PATH="$PATH:$(dirname "$(cygpath -u "$found")")"
  export PATH
}
