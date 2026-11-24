# SPDX-License-Identifier: GPL-3.0-or-later
# Dot-source before building or running the S3 spike on Windows:  . .\env.ps1 [-FfmpegDir <dir>]
# Sets FFMPEG_DIR (headers + import libs for ffmpeg-sys-next), LIBCLANG_PATH (bindgen) and puts
# the FFmpeg DLLs on PATH (the test and harness executables load avcodec-62.dll etc. at start-up).
param([string]$FfmpegDir)

if (-not $FfmpegDir) { $FfmpegDir = $env:FFMPEG_DIR }
if (-not $FfmpegDir) {
    # Default: the bundle in the main checkout's git-ignored third_party/ffmpeg (see ffmpeg.env there).
    $root = (git rev-parse --path-format=absolute --git-common-dir) -replace '[\\/]\.git$', ''
    $envFile = Join-Path $root 'third_party/ffmpeg/ffmpeg.env'
    if (Test-Path $envFile) {
        $line = Select-String -Path $envFile -Pattern '^FFMPEG_DIR="(.*)"' | Select-Object -First 1
        if ($line) { $FfmpegDir = $line.Matches[0].Groups[1].Value }
    }
}
if (-not $FfmpegDir -or -not (Test-Path (Join-Path $FfmpegDir 'include/libavcodec/avcodec.h'))) {
    throw "No FFmpeg dev bundle found. Run 'just ffmpeg-fetch' or pass -FfmpegDir <dir with include/ lib/ bin/>."
}
$env:FFMPEG_DIR = $FfmpegDir
if (-not $env:LIBCLANG_PATH -and (Test-Path 'C:/Program Files/LLVM/bin/libclang.dll')) {
    $env:LIBCLANG_PATH = 'C:/Program Files/LLVM/bin'
}
$bin = Join-Path $FfmpegDir 'bin'
if (-not ($env:PATH -split ';' | Where-Object { $_ -eq $bin })) { $env:PATH = "$bin;$env:PATH" }
Write-Host "FFMPEG_DIR=$env:FFMPEG_DIR"
Write-Host "LIBCLANG_PATH=$env:LIBCLANG_PATH"
Write-Host "PATH += $bin"
