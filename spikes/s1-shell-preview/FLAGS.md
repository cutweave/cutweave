# Spike S1: Chromium / Electron flags for the WebGPU preview (WP-0C.02)

Electron 45.4.1 (Chromium 156). The spike sets switches in `desktop/src/main.ts` with
`app.commandLine.appendSwitch`. **Only Windows was run in WP-0C.02.** The Linux and macOS rows
are starting points that WP-0C.04 must confirm on the reference machines; treat them as
unverified.

| Platform | Needed for the preview | Status |
|---|---|---|
| Windows 11 | Nothing: WebGPU (D3D12), `VideoFrame` and `importExternalTexture` work by default. | Verified (this WP) |
| macOS arm64 | Nothing expected: WebGPU runs on Metal by default. | Not run |
| Ubuntu 24.04 X11 | If `navigator.gpu.requestAdapter()` returns `null`: `--enable-unsafe-webgpu` and `--enable-features=Vulkan`. Check `chrome://gpu` (open it in a spike window) for "WebGPU: Hardware accelerated". | Not run |
| Ubuntu 24.04 Wayland | As X11, plus `--ozone-platform=wayland` (or `--ozone-platform-hint=auto`) so Electron does not fall back to XWayland. | Not run |

Switches the spike uses on every platform, for measurement only:

| Switch | Why |
|---|---|
| `--disable-features=CalculateNativeWinOcclusion` | Keeps `requestAnimationFrame` running when the window is covered (Windows). Benchmarks should still run with the window visible. |
| `--force_high_performance_gpu` (opt-in: `--chromium-dgpu`) | Hybrid-GPU laptops: puts Chromium's GPU process on the dGPU, the adapter the engine picks (README finding 5). |

What to record per OS in WP-0C.04: the adapter string the HUD shows (`webgpu ... (vendor /
architecture)`), whether `upload=videoFrame` reports `videoFrame+importExternalTexture`
(no fallback), and the flags that were needed.
