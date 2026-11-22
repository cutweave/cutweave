#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Downloads the newest CI-built FFmpeg bundle for this OS (WP-0A.06) into the git-ignored
# third_party/ffmpeg/ and prints the FFMPEG_DIR to use. Idempotent: an up-to-date bundle is not
# downloaded again. Needs the GitHub CLI (`gh auth login`), tar and zstd (or a libarchive tar).
#
# Usage: scripts/ffmpeg/fetch.sh [--repo OWNER/NAME] [--branch BRANCH] [--run RUN_ID] [--force]
#   --repo    repository whose "FFmpeg bundle" workflow built the bundle
#             (default: $CUTWEAVE_FFMPEG_REPO or cutweave/cutweave)
#   --branch  only take bundles built from this branch (default: main)
#   --run     take the bundle from this workflow run instead of the newest one
#   --force   download again even if the same bundle is already unpacked
#
# The bundle is chosen by exact artifact name, cutweave-ffmpeg-<version>-<os>-<arch>.tar.zst,
# with <version> from scripts/ffmpeg/versions.toml; an artifact only exists when that OS's build
# passed its acceptance checks.

set -euo pipefail

die() {
  printf 'ffmpeg-fetch: %s\n' "$*" >&2
  exit 1
}

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
repo="${CUTWEAVE_FFMPEG_REPO:-cutweave/cutweave}"
branch="main"
run_id=""
force=0
while [ $# -gt 0 ]; do
  case "$1" in
    --repo) repo="${2:?--repo needs OWNER/NAME}"; shift 2 ;;
    --branch) branch="${2:?--branch needs a name}"; shift 2 ;;
    --run) run_id="${2:?--run needs a run id}"; shift 2 ;;
    --force) force=1; shift ;;
    -h | --help) sed -n '3,18p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown argument: $1 (see --help)" ;;
  esac
done

case "$(uname -s)" in
  Linux) os=linux ;;
  Darwin) os=macos ;;
  MINGW* | MSYS* | CYGWIN*) os=windows ;;
  *) die "unsupported OS: $(uname -s)" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x64 ;;
  arm64 | aarch64) arch=arm64 ;;
  *) die "unsupported CPU: $(uname -m)" ;;
esac
case "$os-$arch" in
  windows-x64 | macos-arm64 | linux-x64) ;;
  *) die "no CI bundle is built for $os-$arch (only windows-x64, macos-arm64, linux-x64)" ;;
esac

command -v gh >/dev/null 2>&1 || die "needs the GitHub CLI: https://cli.github.com (then 'gh auth login')"
version="$(sed -n '/^\[ffmpeg\]/,/^\[/ s/^version *= *"\([^"]*\)".*/\1/p' \
  "$root/scripts/ffmpeg/versions.toml" | head -n 1)"
[ -n "$version" ] || die "cannot read the FFmpeg version from scripts/ffmpeg/versions.toml"
name="cutweave-ffmpeg-$version-$os-$arch"
artifact="$name.tar.zst"
dest="$root/third_party/ffmpeg"
bundle="$dest/$name"
mkdir -p "$dest"

if [ -z "$run_id" ]; then
  run_id="$(gh api -X GET "repos/$repo/actions/artifacts" -f name="$artifact" -f per_page=100 \
    --jq "[.artifacts[] | select(.expired == false and .workflow_run.head_branch == \"$branch\")][0].workflow_run.id // empty")"
  [ -n "$run_id" ] || die "no unexpired $artifact from branch '$branch' in $repo.
Run the 'FFmpeg bundle' workflow (Actions tab or 'gh workflow run ffmpeg.yml -R $repo'),
or pick a run with --run. To build it yourself, see scripts/ffmpeg/README.md."
fi

marker="$bundle/.cutweave-fetch"
if [ "$force" = 0 ] && [ -f "$marker" ] && grep -qx "run=$run_id" "$marker"; then
  echo "ffmpeg-fetch: $name from run $run_id is already in $bundle"
else
  tmp="$(mktemp -d "$dest/.download.XXXXXX")"
  trap 'rm -rf "$tmp"' EXIT
  echo "ffmpeg-fetch: downloading $artifact from $repo run $run_id"
  gh run download "$run_id" --repo "$repo" --name "$artifact" --dir "$tmp"
  file="$(find "$tmp" -type f -name "$artifact" | head -n 1)"
  [ -n "$file" ] || die "run $run_id has no $artifact"
  mkdir -p "$tmp/x"
  if command -v zstd >/dev/null 2>&1; then
    zstd -dc "$file" | tar -xf - -C "$tmp/x"
  elif [ "$os" = windows ]; then
    # Windows' own bsdtar reads .tar.zst; Git Bash ships GNU tar without zstd.
    "$(cygpath -u "${SYSTEMROOT:-C:\\Windows}")/System32/tar.exe" -xf "$(cygpath -w "$file")" \
      -C "$(cygpath -w "$tmp/x")"
  else
    tar -xf "$file" -C "$tmp/x" || die "cannot unpack .tar.zst: install zstd"
  fi
  [ -d "$tmp/x/$name" ] || die "$artifact does not contain $name/"
  rm -rf "$bundle"
  mv "$tmp/x/$name" "$bundle"
  {
    echo "run=$run_id"
    echo "repo=$repo"
    echo "artifact=$artifact"
    echo "fetched=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  } >"$marker"
fi

ffmpeg_dir="$bundle"
[ "$os" != windows ] || ffmpeg_dir="$(cygpath -m "$bundle")"
case "$os" in
  windows) runtime="PATH=\"$ffmpeg_dir/bin;\$PATH\"" ;;
  macos) runtime="DYLD_FALLBACK_LIBRARY_PATH=\"$ffmpeg_dir/lib\"" ;;
  linux) runtime="LD_LIBRARY_PATH=\"$ffmpeg_dir/lib\"" ;;
esac
{
  echo "# Written by scripts/ffmpeg/fetch.sh; source it or copy the values."
  echo "FFMPEG_DIR=\"$ffmpeg_dir\""
  echo "PKG_CONFIG_PATH=\"$ffmpeg_dir/lib/pkgconfig\""
} >"$dest/ffmpeg.env"

cat <<EOF

FFMPEG_DIR=$ffmpeg_dir

  bash/zsh:    export FFMPEG_DIR="$ffmpeg_dir"
  PowerShell:  \$env:FFMPEG_DIR = "$ffmpeg_dir"

Running binaries that link it also needs the shared libraries on the loader path, e.g.
  $runtime
Saved to third_party/ffmpeg/ffmpeg.env. Details: $bundle/BUILD_INFO.txt
EOF
