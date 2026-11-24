# SPDX-License-Identifier: GPL-3.0-or-later
<#
.SYNOPSIS
  Dot-source to build and run against the FFmpeg dev bundle (docs/dev/ffmpeg.md):
  . scripts/ffmpeg/env.ps1 [-BundleDir <dir>]

.DESCRIPTION
  PowerShell twin of scripts/ffmpeg/env.sh. Sets FFMPEG_DIR and PKG_CONFIG_PATH
  (ffmpeg-sys-next's build script), LIBCLANG_PATH when LLVM is in its default place (bindgen), and
  puts the bundle's bin/ (the FFmpeg DLLs) on PATH: test executables die before main without it
  (STATUS_DLL_NOT_FOUND).

  The bundle is, in order: -BundleDir; $env:FFMPEG_DIR; FFMPEG_DIR from
  third_party/ffmpeg/ffmpeg.env of this checkout; the same file in the main checkout (git worktrees
  share it). Each candidate must contain include/libavcodec/avcodec.h.
#>
param([string]$BundleDir)

function Test-CwBundle([string]$Dir) {
    return $Dir -and (Test-Path (Join-Path $Dir 'include/libavcodec/avcodec.h'))
}

function Read-CwEnvFile([string]$Root) {
    if (-not $Root) { return $null }
    $file = Join-Path $Root 'third_party/ffmpeg/ffmpeg.env'
    if (-not (Test-Path $file)) { return $null }
    $line = Select-String -Path $file -Pattern '^FFMPEG_DIR="(.*)"' | Select-Object -First 1
    if ($line) { return $line.Matches[0].Groups[1].Value }
    return $null
}

$cwDir = $null
foreach ($candidate in @($BundleDir, $env:FFMPEG_DIR)) {
    if (Test-CwBundle $candidate) { $cwDir = $candidate; break }
}
if (-not $cwDir) {
    $here = (git rev-parse --show-toplevel 2>$null)
    $main = ((git rev-parse --path-format=absolute --git-common-dir 2>$null) -replace '[\\/]\.git$', '')
    foreach ($root in @($here, $main)) {
        $candidate = Read-CwEnvFile $root
        if (Test-CwBundle $candidate) { $cwDir = $candidate; break }
    }
}
if (-not $cwDir) {
    throw ("No FFmpeg dev bundle found (FFMPEG_DIR unset or without include/, no " +
        "third_party/ffmpeg/ffmpeg.env). Run 'just ffmpeg-fetch' or pass -BundleDir; see docs/dev/ffmpeg.md.")
}

$env:FFMPEG_DIR = $cwDir
$env:PKG_CONFIG_PATH = if ($env:PKG_CONFIG_PATH) { "$cwDir/lib/pkgconfig;$env:PKG_CONFIG_PATH" } else { "$cwDir/lib/pkgconfig" }
if ($env:OS -eq 'Windows_NT') {
    if (-not $env:LIBCLANG_PATH -and (Test-Path 'C:/Program Files/LLVM/bin/libclang.dll')) {
        $env:LIBCLANG_PATH = 'C:/Program Files/LLVM/bin'
    }
    $bin = Join-Path $cwDir 'bin'
    if (-not ($env:PATH -split ';' | Where-Object { $_ -eq $bin })) { $env:PATH = "$bin;$env:PATH" }
} elseif ($IsMacOS) {
    $env:DYLD_FALLBACK_LIBRARY_PATH = "$cwDir/lib" + $(if ($env:DYLD_FALLBACK_LIBRARY_PATH) { ":$env:DYLD_FALLBACK_LIBRARY_PATH" } else { '' })
} else {
    $env:LD_LIBRARY_PATH = "$cwDir/lib" + $(if ($env:LD_LIBRARY_PATH) { ":$env:LD_LIBRARY_PATH" } else { '' })
}
Write-Host "FFMPEG_DIR=$env:FFMPEG_DIR"
