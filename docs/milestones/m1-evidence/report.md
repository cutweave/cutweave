# M1 exit gate

- Commit: `1744a14a607ac6aa2f1e53b40929c009eabbc8f6 (with uncommitted changes)`
- Machine: Windows-11-10.0.26200-SP0; AMD64 Family 25 Model 80 Stepping 0, AuthenticAMD
- GPUs: NVIDIA GeForce RTX 3080 Laptop GPU, AMD Radeon(TM) Graphics
- FFmpeg: ffmpeg version n8.1.3-39-g660b5aa7db-20261123 Copyright (c) 2000-2026 the FFmpeg developers
- Node: v24.19.0; rustc 1.100.0 (0ef1bc386 2026-11-09)
- Crash trials per kind: 20

| Criterion | Result |
|---|---|
| 1. UI: import 3 clips, arrange, trim, split, export a 1080p MP4 that plays | **pass** |
| 2. MCP headless: a scripted client does the same and exports an identical-timeline file | **pass** |
| 3. Kill the engine mid-edit: the UI recovers with no lost commands | **pass** |
| 4. CI green (Windows, local gates, R34); golden frames pass; parity (CLI = MCP = RPC) passes | **pass (4a not run: --full)** |

| Criterion | Step | Result | Time | Detail |
|---|---|---|---|---|
| 0 | build the engine with FFmpeg | pass | 98 s | exit 0 |
| 0 | build the CLI with FFmpeg | pass | 31 s | exit 0 |
| 0 | build the preview addon | pass | 1 s | exit 0 |
| 0 | build the app (main, preload) | pass | 0 s | exit 0 |
| 0 | build the app (renderer) | pass | 1 s | exit 0 |
| 0 | Electron binary | pass | 0 s | exit 0 |
| 1 | UI cut and export (e2e/m1/m1-exit) | pass | 17 s | exit 0 |
| 1 | the UI export decodes, has sound, matches the render | pass | 8 s | decode errors 0, max volume -14.8 dB, render vs export: f30 48.9 dB (RGB 37.5), f150 48.9 dB (RGB 37.3), f255 47.7 dB (RGB 35.3) |
| 1 | user flow F14 export (pointer drags) | pass | 15 s | exit 0 |
| 2 | MCP conformance and the M1 scripted agent | pass | 38 s | exit 0 |
| 2 | UI and agent: same timeline, equivalent exports | pass | 1 s | timeline equal (7 items), streams equal, durations {'ui': 12.0, 'mcp': 12.0}, 36 frames hashed: decoded frames identical; the two files are byte-identical |
| 3 | crash trials (20 engine kills, 20 app kills) | pass | 74 s | exit 0 |
| 3 | user flows F11 and F12 | pass | 9 s | exit 0 |
| 4b | golden frames (cargo xtask golden) | pass | 46 s | exit 0 |
| 4c | parity CLI = MCP = RPC | pass | 79 s | exit 0 |
