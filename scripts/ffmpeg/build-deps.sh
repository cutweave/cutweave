#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Builds the pinned third-party libraries FFmpeg links against, as static position-independent
# archives, into $CW_WORK/deps (WP-0A.06). They end up inside the FFmpeg shared libraries, so the
# bundle has no third-party codec DLLs/dylibs/.so files of its own.
#
# Usage: scripts/ffmpeg/build-deps.sh [component ...]   (default: every component of this OS)
# Environment: see lib.sh. Needs the tools from install-tools.sh.

set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
cw_init

# Build order: headers and libraries that others find through pkg-config come first.
ORDER=(zlib nvcodec amf libvpl x264 x265 svtav1 libvpx dav1d opus lame vidstab zimg soxr
  freetype fribidi harfbuzz libass)

build_zlib() (
  # Windows only (macOS and Linux link the system zlib).
  cd "$(cw_extract zlib)"
  make -f win32/Makefile.gcc -j"$CW_JOBS" libz.a
  make -f win32/Makefile.gcc install prefix="$CW_PREFIX" BINARY_PATH="$CW_PREFIX/bin" \
    INCLUDE_PATH="$CW_PREFIX/include" LIBRARY_PATH="$CW_PREFIX/lib"
)

build_nvcodec() (
  # ffnvcodec headers: NVENC/NVDEC/CUVID are loaded from the NVIDIA driver at runtime.
  make -C "$(cw_extract nvcodec)" PREFIX="$CW_PREFIX" install
)

build_amf() (
  # Header-only; FFmpeg expects them as <AMF/core/...>. The release tarball's layout is not
  # assumed: the include root is the directory that holds core/Version.h.
  local src version
  src="$(cw_extract amf)"
  version="$(find "$src" -type f -path '*/core/Version.h' | head -n 1)"
  [ -n "$version" ] || cw_die "AMF headers: core/Version.h not found in the tarball"
  rm -rf "$CW_PREFIX/include/AMF"
  mkdir -p "$CW_PREFIX/include/AMF"
  cp -R "$(dirname "$(dirname "$version")")"/. "$CW_PREFIX/include/AMF/"
)

build_libvpl() (
  cw_cmake "$(cw_extract libvpl)" "$CW_BLD/libvpl" \
    -DBUILD_TESTS=OFF -DBUILD_EXAMPLES=OFF -DINSTALL_EXAMPLES=OFF
)

build_x264() (
  cd "$(cw_extract x264)"
  local args=(--prefix="$CW_PREFIX" --enable-static --disable-cli --disable-opencl)
  [ "$CW_OS" = windows ] || args+=(--enable-pic)
  ./configure "${args[@]}"
  make -j"$CW_JOBS"
  make install
)

build_x265() (
  # 8-bit library with the 10- and 12-bit encoders linked in (x265's multilib layout), so
  # FFmpeg's libx265 can encode Main, Main10 and Main12.
  local src b common high
  src="$(cw_extract x265)/source"
  b="$CW_BLD/x265"
  common=(-DENABLE_SHARED=OFF -DENABLE_CLI=OFF -DENABLE_LIBNUMA=OFF)
  [ "$CW_OS" = windows ] || common+=(-DENABLE_PIC=ON)
  high=(-DHIGH_BIT_DEPTH=ON -DEXPORT_C_API=OFF)
  cw_cmake_build "$src" "$b/12bit" "${common[@]}" "${high[@]}" -DMAIN12=ON
  cw_cmake_build "$src" "$b/10bit" "${common[@]}" "${high[@]}" -DENABLE_HDR10_PLUS=ON
  cw_cmake_build "$src" "$b/8bit" "${common[@]}" -DLINKED_10BIT=ON -DLINKED_12BIT=ON \
    -DEXTRA_LINK_FLAGS=-L. "-DEXTRA_LIB=x265_main10.a;x265_main12.a"
  cd "$b/8bit"
  mv libx265.a libx265_main.a
  cw_merge_static libx265.a libx265_main.a ../10bit/libx265.a ../12bit/libx265.a
  cmake --install .
  # x265 writes the compiler's implicit link libraries (e.g. -lgcc_s, a DLL import on MinGW)
  # into Libs.private; keep only what a static link really needs.
  local private
  private="$(cw_cxx_libs) -lm"
  [ "$CW_OS" != linux ] || private="$private -lpthread -ldl"
  cw_pc_set "$CW_PREFIX/lib/pkgconfig/x265.pc" Libs.private "$private"
)

build_svtav1() (
  # LTO off: GCC LTO objects inside a static archive do not link into FFmpeg's libraries.
  cw_cmake "$(cw_extract svtav1)" "$CW_BLD/svtav1" \
    -DBUILD_APPS=OFF -DBUILD_TESTING=OFF -DSVT_AV1_LTO=OFF -DNATIVE=OFF
)

build_libvpx() (
  local args
  cd "$(cw_extract libvpx)"
  args=(--prefix="$CW_PREFIX" --libdir="$CW_PREFIX/lib" --enable-static --disable-shared
    --disable-examples --disable-tools --disable-docs --disable-unit-tests
    --enable-vp9-highbitdepth --enable-runtime-cpu-detect)
  case "$CW_OS-$CW_ARCH" in
    linux-x64) args+=(--target=x86_64-linux-gcc --enable-pic --as=nasm) ;;
    windows-x64) args+=(--target=x86_64-win64-gcc --as=nasm) ;;
    # libvpx only knows the Darwin releases that existed when it was tagged.
    macos-arm64) args+=("--force-target=arm64-darwin$(uname -r | cut -d. -f1)-gcc") ;;
    *) cw_die "libvpx: no target for $CW_OS-$CW_ARCH" ;;
  esac
  ./configure "${args[@]}"
  make -j"$CW_JOBS"
  make install
)

build_dav1d() (
  cw_meson "$(cw_extract dav1d)" "$CW_BLD/dav1d" \
    -Denable_tools=false -Denable_tests=false -Denable_examples=false -Denable_docs=false
)

build_opus() (
  local src args=(--disable-doc --disable-extra-programs)
  src="$(cw_extract opus)"
  cw_refresh_config "$src"
  # MinGW's -fstack-protector needs libssp, which would become a DLL dependency.
  [ "$CW_OS" != windows ] || args+=(--disable-stack-protector)
  cw_autotools "$src" "${args[@]}"
)

build_lame() (
  local src
  src="$(cw_extract lame)"
  cw_refresh_config "$src"
  # LAME 3.100 predates C23 and calls a few undeclared functions; build it as C17.
  CFLAGS="-O2 -std=gnu17 -Wno-implicit-function-declaration" cw_autotools "$src" \
    --disable-frontend --disable-decoder --disable-gtktest --disable-analyzer-hooks
)

build_vidstab() (
  cw_cmake "$(cw_extract vidstab)" "$CW_BLD/vidstab" -DUSE_OMP=OFF
)

build_zimg() (
  local src
  src="$(cw_extract zimg)"
  (cd "$src" && ./autogen.sh)
  # zimg.pc's Libs.private names the C++ runtime (libc++ on macOS, not libstdc++).
  STL_LIBS="$(cw_cxx_libs)" cw_autotools "$src" \
    --disable-testapp --disable-example --disable-unit-test
)

build_soxr() (
  cw_cmake "$(cw_extract soxr)" "$CW_BLD/soxr" \
    -DBUILD_TESTS=OFF -DBUILD_EXAMPLES=OFF -DWITH_OPENMP=OFF -DWITH_LSR_BINDINGS=OFF \
    -DWITH_AVFFT=OFF
)

build_freetype() (
  cw_meson "$(cw_extract freetype)" "$CW_BLD/freetype" \
    -Dbrotli=disabled -Dbzip2=disabled -Dharfbuzz=disabled -Dpng=disabled -Dzlib=internal \
    -Dtests=disabled -Dhvf=disabled
)

build_fribidi() (
  cw_meson "$(cw_extract fribidi)" "$CW_BLD/fribidi" -Ddocs=false -Dbin=false -Dtests=false
)

build_harfbuzz() (
  cw_meson "$(cw_extract harfbuzz)" "$CW_BLD/harfbuzz" \
    -Dfreetype=enabled -Dglib=disabled -Dgobject=disabled -Dcairo=disabled -Dchafa=disabled \
    -Dicu=disabled -Dgraphite2=disabled -Dpng=disabled -Dzlib=disabled -Draster=disabled \
    -Dvector=disabled -Dgpu=disabled -Dgpu_demo=disabled -Dsubset=disabled -Dtests=disabled \
    -Dintrospection=disabled -Ddocs=disabled -Dutilities=disabled -Dbenchmark=disabled
)

build_libass() (
  local src args=(--disable-libunibreak --disable-test --disable-compare --disable-profile
    --disable-fuzz)
  src="$(cw_extract libass)"
  cw_refresh_config "$src"
  # System font provider: fontconfig on Linux (system library), CoreText, DirectWrite.
  case "$CW_OS" in
    linux) args+=(--enable-fontconfig --disable-coretext --disable-directwrite) ;;
    macos) args+=(--disable-fontconfig --enable-coretext --disable-directwrite) ;;
    windows) args+=(--disable-fontconfig --disable-coretext --enable-directwrite) ;;
  esac
  cw_autotools "$src" "${args[@]}"
)

main() {
  local wanted=("$@") c built=()
  [ ${#wanted[@]} -gt 0 ] || wanted=("${ORDER[@]}")
  [ "$CW_OS" != windows ] || cw_windows_static_runtime
  for c in "${wanted[@]}"; do
    declare -F "build_$c" >/dev/null || cw_die "unknown component: $c"
    if ! cw_uses "$c"; then
      [ $# -eq 0 ] || cw_warn "$c is not part of the $CW_OS bundle; skipped"
      continue
    fi
    cw_log "$c $(cw_pin VERSION "$c")"
    "build_$c"
    built+=("$c")
  done

  # Static archives only: drop libtool archives and make the pkg-config files relocatable.
  rm -f "$CW_PREFIX"/lib/*.la
  if [ "$CW_OS" = windows ]; then
    # -lgcc_s would import libgcc_s_seh-1.dll; -static-libgcc provides the same code.
    sed -i -e 's/ -lgcc_s\b//g' "$CW_PREFIX"/lib/pkgconfig/*.pc
  fi
  cw_relocate_pc "$CW_PREFIX/lib/pkgconfig" "$CW_PREFIX"

  {
    echo "# Static dependencies built by scripts/ffmpeg/build-deps.sh"
    for c in "${built[@]}"; do echo "$c $(cw_pin VERSION "$c")"; done
  } >>"$CW_PREFIX/.cutweave-deps"
  cw_log "dependencies installed in $CW_PREFIX"
}

main "$@"
