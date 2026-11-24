# SPDX-License-Identifier: GPL-3.0-or-later
# CPU / GPU sampler for the S3 harness (WP-0E.02). `s3 bench --monitor` and `s3 gpu --monitor`
# start it; it can also run by hand next to any process:
#   powershell -NoProfile -ExecutionPolicy Bypass -File monitor.ps1 -ProcessId <pid> -Out mon.csv
# Once per interval it writes one CSV row: the machine's total CPU %, and the Windows "GPU Engine"
# utilisation (the numbers Task Manager shows) for the watched process and for all processes.
# Engines are classed as decode (NVIDIA "VideoDecode", AMD "Video Decode N" / "Video Codec N"),
# 3d, copy and compute; per class the busiest engine counts. It stops when the process exits.
param(
    [Parameter(Mandatory = $true)][int]$ProcessId,
    [Parameter(Mandatory = $true)][string]$Out,
    [int]$IntervalSeconds = 1
)
$ErrorActionPreference = 'SilentlyContinue'
'unix_ms,cpu_total,proc_decode,proc_3d,proc_copy,proc_compute,all_decode,all_3d,all_copy,all_compute' |
    Set-Content -Encoding ascii -Path $Out

function Get-EngineClass([string]$name) {
    if ($name -match 'engtype_video ?(decode|codec)') { return 'decode' }
    if ($name -match 'engtype_3d') { return '3d' }
    if ($name -match 'engtype_copy') { return 'copy' }
    if ($name -match 'engtype_compute') { return 'compute' }
    return $null
}

$classes = @('decode', '3d', 'copy', 'compute')
while (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue) {
    $s = Get-Counter -Counter '\Processor(_Total)\% Processor Time', '\GPU Engine(*)\Utilization Percentage' `
        -SampleInterval $IntervalSeconds -MaxSamples 1
    if (-not $s) { continue }
    $ms = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    $cpu = 0.0
    $proc = @{}; $all = @{}
    foreach ($c in $classes) { $proc[$c] = @{}; $all[$c] = @{} }
    foreach ($x in $s.CounterSamples) {
        $n = $x.InstanceName
        if ($x.Path -match 'processor\(_total\)') { $cpu = $x.CookedValue; continue }
        $cls = Get-EngineClass $n
        if (-not $cls) { continue }
        # Engine identity without the pid: luid + engine index.
        $eng = ($n -replace '^pid_\d+_', '') -replace '_engtype_.*$', ''
        $all[$cls][$eng] = [double]$all[$cls][$eng] + $x.CookedValue
        if ($n -match "^pid_$ProcessId`_") { $proc[$cls][$eng] = [double]$proc[$cls][$eng] + $x.CookedValue }
    }
    $row = @($ms, ('{0:F1}' -f $cpu))
    foreach ($c in $classes) { $row += ('{0:F1}' -f (($proc[$c].Values | Measure-Object -Maximum).Maximum + 0)) }
    foreach ($c in $classes) { $row += ('{0:F1}' -f (($all[$c].Values | Measure-Object -Maximum).Maximum + 0)) }
    ($row -join ',') | Add-Content -Encoding ascii -Path $Out
}
