# SPDX-License-Identifier: GPL-3.0-or-later
<#
.SYNOPSIS
  Downloads the newest CI-built FFmpeg bundle for this OS (WP-0A.06) into the git-ignored
  third_party/ffmpeg/ and prints the FFMPEG_DIR to use.

.DESCRIPTION
  PowerShell twin of scripts/ffmpeg/fetch.sh (which `just ffmpeg-fetch` runs). Idempotent: an
  up-to-date bundle is not downloaded again. Needs the GitHub CLI (`gh auth login`) and a tar that
  reads .tar.zst (Windows 10 1803+ ships one as tar.exe).

  The bundle is chosen by exact artifact name, cutweave-ffmpeg-<version>-<os>-<arch>.tar.zst,
  with <version> from scripts/ffmpeg/versions.toml; an artifact only exists when that OS's build
  passed its acceptance checks.

.PARAMETER Repo
  Repository whose "FFmpeg bundle" workflow built the bundle
  (default: $env:CUTWEAVE_FFMPEG_REPO or cutweave/cutweave).
.PARAMETER Branch
  Only take bundles built from this branch (default: main).
.PARAMETER Run
  Take the bundle from this workflow run instead of the newest one.
.PARAMETER Force
  Download again even if the same bundle is already unpacked.

.EXAMPLE
  pwsh scripts/ffmpeg/fetch.ps1
#>
[CmdletBinding()]
param(
    [string]$Repo = $(if ($env:CUTWEAVE_FFMPEG_REPO) { $env:CUTWEAVE_FFMPEG_REPO } else { 'cutweave/cutweave' }),
    [string]$Branch = 'main',
    [string]$Run = '',
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Stop-Fetch([string]$Message) {
    Write-Error "ffmpeg-fetch: $Message" -ErrorAction Continue
    exit 1
}

$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$onWindows = ($env:OS -eq 'Windows_NT')
$os = if ($onWindows) { 'windows' } elseif ($IsMacOS) { 'macos' } else { 'linux' }
$arch = switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()) {
    'X64' { 'x64' }
    'Arm64' { 'arm64' }
    default { Stop-Fetch "unsupported CPU: $_" }
}
if (@('windows-x64', 'macos-arm64', 'linux-x64') -notcontains "$os-$arch") {
    Stop-Fetch "no CI bundle is built for $os-$arch (only windows-x64, macos-arm64, linux-x64)"
}
if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
    Stop-Fetch "needs the GitHub CLI: https://cli.github.com (then 'gh auth login')"
}

# version = "..." inside the [ffmpeg] table of versions.toml
$version = $null
$inFfmpeg = $false
foreach ($line in Get-Content (Join-Path $root 'scripts/ffmpeg/versions.toml')) {
    if ($line -match '^\[') { $inFfmpeg = ($line -eq '[ffmpeg]'); continue }
    if ($inFfmpeg -and $line -match '^version\s*=\s*"([^"]+)"') { $version = $Matches[1]; break }
}
if (-not $version) { Stop-Fetch 'cannot read the FFmpeg version from scripts/ffmpeg/versions.toml' }

$name = "cutweave-ffmpeg-$version-$os-$arch"
$artifact = "$name.tar.zst"
$dest = Join-Path $root 'third_party/ffmpeg'
$bundle = Join-Path $dest $name
New-Item -ItemType Directory -Force -Path $dest | Out-Null

if (-not $Run) {
    # Newest unexpired artifact with this exact name from a run on $Branch (filtered here rather
    # than with --jq, whose embedded quotes Windows PowerShell 5.1 would mangle).
    $json = (& gh api -X GET "repos/$Repo/actions/artifacts" -f "name=$artifact" -f per_page=100) | Out-String
    if ($LASTEXITCODE -ne 0) { Stop-Fetch "gh api failed (is 'gh auth login' done?)" }
    $hit = (ConvertFrom-Json $json).artifacts |
        Where-Object { -not $_.expired -and $_.workflow_run.head_branch -eq $Branch } |
        Select-Object -First 1
    if (-not $hit) {
        Stop-Fetch ("no unexpired $artifact from branch '$Branch' in $Repo. Run the 'FFmpeg bundle' " +
            "workflow (Actions tab or 'gh workflow run ffmpeg.yml -R $Repo'), or pick a run with -Run.")
    }
    $Run = "$($hit.workflow_run.id)"
}

$marker = Join-Path $bundle '.cutweave-fetch'
if (-not $Force -and (Test-Path $marker) -and ((Get-Content $marker) -contains "run=$Run")) {
    Write-Host "ffmpeg-fetch: $name from run $Run is already in $bundle"
} else {
    $tmp = Join-Path $dest ('.download.' + [System.IO.Path]::GetRandomFileName())
    New-Item -ItemType Directory -Force -Path (Join-Path $tmp 'x') | Out-Null
    try {
        Write-Host "ffmpeg-fetch: downloading $artifact from $Repo run $Run"
        & gh run download $Run --repo $Repo --name $artifact --dir $tmp
        if ($LASTEXITCODE -ne 0) { Stop-Fetch "gh run download failed for run $Run" }
        $file = Get-ChildItem -Path $tmp -Recurse -File -Filter $artifact | Select-Object -First 1
        if (-not $file) { Stop-Fetch "run $Run has no $artifact" }
        $tar = if ($onWindows) { Join-Path $env:SystemRoot 'System32\tar.exe' } else { 'tar' }
        & $tar -xf $file.FullName -C (Join-Path $tmp 'x')
        if ($LASTEXITCODE -ne 0) { Stop-Fetch "cannot unpack $artifact (needs a tar with zstd support)" }
        $unpacked = Join-Path (Join-Path $tmp 'x') $name
        if (-not (Test-Path $unpacked)) { Stop-Fetch "$artifact does not contain $name/" }
        if (Test-Path $bundle) { Remove-Item -Recurse -Force $bundle }
        Move-Item $unpacked $bundle
        @("run=$Run", "repo=$Repo", "artifact=$artifact",
            "fetched=$((Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ'))") |
            Set-Content -Encoding ascii $marker
    } finally {
        if (Test-Path $tmp) { Remove-Item -Recurse -Force $tmp }
    }
}

$ffmpegDir = (Resolve-Path $bundle).Path
if ($onWindows) { $ffmpegDir = $ffmpegDir -replace '\\', '/' }
@('# Written by scripts/ffmpeg/fetch.ps1; source it or copy the values.',
    "FFMPEG_DIR=`"$ffmpegDir`"",
    "PKG_CONFIG_PATH=`"$ffmpegDir/lib/pkgconfig`"") |
    Set-Content -Encoding ascii (Join-Path $dest 'ffmpeg.env')

$runtime = switch ($os) {
    'windows' { "`$env:PATH = `"$ffmpegDir/bin;`$env:PATH`"" }
    'macos' { "export DYLD_FALLBACK_LIBRARY_PATH=`"$ffmpegDir/lib`"" }
    default { "export LD_LIBRARY_PATH=`"$ffmpegDir/lib`"" }
}
Write-Host ''
Write-Host "FFMPEG_DIR=$ffmpegDir"
Write-Host ''
Write-Host "  PowerShell:  `$env:FFMPEG_DIR = `"$ffmpegDir`""
Write-Host "  bash/zsh:    export FFMPEG_DIR=`"$ffmpegDir`""
Write-Host ''
Write-Host 'Running binaries that link it also needs the shared libraries on the loader path, e.g.'
Write-Host "  $runtime"
Write-Host "Saved to third_party/ffmpeg/ffmpeg.env. Details: $bundle/BUILD_INFO.txt"
