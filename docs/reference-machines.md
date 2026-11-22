# Reference machines

> Work package: WP-0I.02 (draft for owner decision). Machine ids below are the values of the `machine`
> field in benchmark result JSON ([`bench/result.schema.json`](../bench/result.schema.json)) and of
> `machine_scope` in [`bench/budgets.toml`](../bench/budgets.toml). The machine-readable registry is
> [`bench/machines.toml`](../bench/machines.toml); keep both in sync.
> Contract: [01-architecture.md](01-architecture.md) §19 (budgets), §20 (golden tolerances),
> [ADR-014](adr/0014-min-os.md) (minimum OS). Last updated 2026-11-22.

## 1. Status at a glance

| Id | Status | Hardware | Owner | Due |
|---|---|---|---|---|
| `ref-win-dgpu` | **Owned** (verified) | Maintainer laptop, RTX 3080 Laptop | product owner | done |
| `ref-win-igpu` | **Interim, owned**, deviates from plan | Same laptop, adapter forced to AMD Radeon (Vega 8) | product owner | Intel Xe machine recommended, see §3 |
| `ref-mac-m1` | **TBD** (not purchased) | Apple Silicon M1, 16 GB | TBD owner=product owner | due=2026-12-06 |
| `ref-linux-x11` | **TBD** (not purchased) | One Linux mini PC, Xorg session | TBD owner=product owner | due=2026-12-06 |
| `ref-linux-wayland` | **TBD** (same box as `ref-linux-x11`) | Same mini PC, Wayland session | TBD owner=product owner | due=2026-12-06 |
| `min-win10`, `min-mac13`, `min-ubuntu2204` | VMs, see §4 | hosted on the machines above | product owner | with host |

**Acceptance gap (must be read before signing off WP-0I.02).** The WP allows at most one machine with a
"TBD" in any field. Three of the five ids (`ref-mac-m1`, `ref-linux-x11`, `ref-linux-wayland`) have no
hardware today. Treating the two Linux ids as one physical box still leaves two TBD machines, so the
criterion "at most 1 TBD machine" is **not met** until the Mac or the Linux box is bought and filled in.
Hardware order is day-0 item D0.5, and nothing in this document places an order. The order date field
in §6 is empty on purpose.

## 2. Fixed decisions

- **Intel macOS is not a reference platform.** macOS references are arm64 only (roadmap). Intel Macs
  may run the app on a best-effort basis but are never measured and no budget applies to them.
- **One machine, several ids is allowed** when the hardware really is the same (`ref-win-dgpu` and
  `ref-win-igpu` on the laptop, `ref-linux-x11` and `ref-linux-wayland` on the mini PC). Results always
  carry the id of the configuration actually tested, never the box name.
- **Dual-sessioned Linux, not VM passthrough.** X11 and Wayland are two login sessions of the same
  Ubuntu 24.04 install (GNOME: "Ubuntu on Xorg" vs "Ubuntu"). One session is live at a time, so the two
  runner labels share one concurrency group and never run jobs at once. GPU passthrough VMs add a
  hypervisor between wgpu and the driver and would invalidate frame-time and A/V numbers.
- **Measured, not emulated, GPUs.** WARP and lavapipe are for CI goldens (§20), never for the budgets
  in §7 that name an iGPU or HW encode.

## 3. Machine definitions

Fields: CPU, GPU, RAM, storage, OS build, driver versions, display refresh, audio device and loopback
fixture, owner. A value written "record at first run" is read from the machine by `runner-health.yml`
(WP-0I.04) or by the owner and committed here; no value is guessed.

### 3.1 `ref-win-dgpu` (owned)

| Field | Value |
|---|---|
| CPU | AMD Ryzen 9 5900HX, 8 cores / 16 threads |
| GPU | NVIDIA GeForce RTX 3080 Laptop GPU (discrete); AMD Radeon Graphics iGPU present, must not be the measured adapter for this id |
| RAM | 31 GB usable |
| Storage | NVMe SSD, 1.9 TB data volume (repo, corpus and result dirs live here) |
| OS build | Windows 11 Pro 10.0.26200 |
| Driver versions | NVIDIA 32.0.16.1692 (Windows driver version as reported in the S1 spike run); AMD: record at first run. Pin and re-record on every update |
| Display refresh | Record at first run (laptop panel). Presenting 60 fps content requires the active mode to be a multiple of 60 Hz or VRR; record the mode used |
| Audio device | WASAPI shared mode. Informal soak used a SteelSeries Arctis Nova Pro Wireless headset (48 kHz, F32, stereo). A wireless link hides latency, so it is **not valid for A/V offset**. Needed: a wired output |
| Loopback fixture | **Missing, to buy** (see §5): wired output into a USB audio interface line-in, plus a 240 fps phone camera or photodiode for flash-to-click offset |
| Owner | product owner (maintainer laptop) |
| Runner label | `ref-win-dgpu` (WP-0I.04). A laptop doubles as the maintainer's workstation: runner jobs only run in a dedicated window, see §6 |
| Selecting the adapter | wgpu `HighPerformance` picks the RTX. Chromium WebGPU picked the AMD iGPU in the S1 spike; the spike's `--chromium-dgpu` flag adds `force_high_performance_gpu`. Record which adapter each process used in the result |

### 3.2 `ref-win-igpu` (interim, owned: AMD iGPU on the same laptop)

**Deviation from the plan.** WP-0I.02 asks for a Win11 machine with an **Intel Xe/Arc iGPU and 16 GB
RAM**. The only owned hardware has an AMD Radeon Vega 8 iGPU (GCN 5) in the 5900HX. This interim entry
runs the same laptop with the adapter forced to the Radeon (engine: `--adapter` / `WGPU_ADAPTER_NAME`
substring `AMD`; Chromium: no `force_high_performance_gpu`, which is its default here). Consequences,
all recorded so results are not over-claimed:

- The Vega 8 is an older, slower iGPU than Intel Xe-LP/Arc. The S4 informal run measured p50 6.5 ms and
  p95 8.1 to 8.5 ms for the 1080p demo scene, borderline against the < 8 ms GPU-time bar. A pass here
  is weaker evidence than a pass on Intel Xe, and a fail is likely to be a fail on Xe too.
- RAM is 31 GB, not 16 GB. Memory-pressure behaviour (idle-memory budget, cache eviction) is **not**
  representative of a 16 GB machine. Cap the process with a job object (Windows) or run the memory
  budget on `ref-mac-m1` (16 GB) instead.
- The 5900HX shares a package TDP with the dGPU. With the dGPU idle the iGPU is not limited by it, but
  results must record that the dGPU was idle (not under load from another process).
- Intel-specific driver and DX12/Vulkan behaviour (the most common Windows iGPU on user machines) is
  **untested** by this entry.

**Recommendation: also acquire an Intel Xe/Arc Windows machine.** Cheapest route: a mini PC or
thin laptop with a Core Ultra 5/7 (Arc iGPU) or 12th/13th-gen Core (Iris Xe), 16 GB RAM, about
350 to 650 USD, 3 to 10 days lead time. It would replace the AMD interim (`ref-win-igpu` moves to it,
the AMD run stays in the registry as `ref-win-igpu-amd` for comparison). If the Linux box is bought with an Intel Arc iGPU (§3.5), Xe is at least covered on Mesa,
but that does not cover Intel's Windows drivers. Priority: after Mac and Linux (they close the TBD
gap), before M0 sign-off of S4.

All other fields match §3.1 except: GPU = AMD Radeon Graphics (Vega 8, 8 CU), driver version record at
first run, runner label `ref-win-igpu`. Both Windows ids share the one physical audio path.

### 3.3 `ref-mac-m1` (TBD owner=product owner, due=2026-12-06)

Recommended spec (concrete; to be bought):

| Field | Value |
|---|---|
| CPU / GPU | Apple M1 (8-core CPU, 8-core GPU), unified memory |
| RAM | 16 GB (plan allows 8 to 16; 16 GB is the honest "creator laptop" case. An 8 GB unit is useful later as a second low-memory data point) |
| Storage | 256 GB minimum, 512 GB preferred (corpus plus caches) |
| OS build | macOS 15 or newer arm64 (record the exact build, e.g. via `sw_vers`); pin and re-record on updates. macOS 13 is covered by the VM in §4 |
| Driver versions | Not applicable; record macOS build, Metal GPU family, Xcode CLT version |
| Display refresh | Built-in 60 Hz or external at 60 Hz; for a headless Mac mini add an HDMI dummy plug (about 10 USD) so a Metal display exists |
| Audio device | CoreAudio built-in output; wired headphone-out or a USB interface for loopback |
| Loopback fixture | Same as §5 (wired, no Bluetooth) |
| Owner | TBD owner=product owner, due=2026-12-06 |
| Runner label | `ref-mac-m1` |

Purchase recommendation (price and lead time are estimates to verify at order time):

- **Preferred:** a refurbished or used Apple M1 Mac mini 16 GB or M1 MacBook Air 16 GB, about 400 to 650 USD.
  Lead time 3 to 10 days (Apple Refurbished stock varies; used market is faster).
- **Fallback if M1 is not obtainable in the window:** a new base Mac mini with a newer M-series chip,
  about 600 USD, 2 to 7 days. This is a **deviation** (the plan and the iGPU budget name M1): record it,
  and treat the newer chip as faster than the budget's M1 target, so a pass is weaker evidence.
- A mini avoids a laptop's battery and lid. Keep it on AC with Low Power mode off.

### 3.4 `ref-linux-x11` and `ref-linux-wayland` (TBD owner=product owner, due=2026-12-06)

One physical box, two sessions (§2).

| Field | Value |
|---|---|
| CPU | Intel Core Ultra 7 155H (6P+8E+2LP) **or** AMD Ryzen 7 7840HS class; 8+ cores either way |
| GPU | Integrated: Intel Arc (Xe-LPG) **or** AMD Radeon 780M (RDNA3). Intel is preferred because it also covers the "Intel Xe" gap on Mesa (ANV/Vulkan, VA-API iHD); choose AMD only if the box is not Intel-capable at the price |
| RAM | 32 GB (dual-channel; iGPU performance depends on it). 16 GB acceptable, 32 GB preferred as the box also hosts the Ubuntu 22.04 VM |
| Storage | 1 TB NVMe |
| OS build | Ubuntu 24.04 LTS (record `lsb_release` and kernel); GNOME; sessions "Ubuntu on Xorg" and "Ubuntu" (Wayland); default session set by `/etc/gdm3/custom.conf` or the login picker, one active at a time. Log the active `XDG_SESSION_TYPE` in each result |
| Driver versions | Mesa 24.x from Ubuntu 24.04 (record `vulkaninfo`, `glxinfo`, `vainfo` versions); kernel i915/xe or amdgpu. Do not mix in a PPA without recording it; ADR-014 requires Mesa >= 23 |
| Display refresh | 60 Hz on a real HDMI/DP monitor or a dummy plug (about 10 USD) so both sessions get a real output; record mode and scale factor |
| Audio device | PipeWire with the PulseAudio compatibility layer (default on 24.04); built-in analog out or USB interface |
| Loopback fixture | Same as §5 |
| Owner | TBD owner=product owner, due=2026-12-06 |
| Runner labels | `ref-linux-x11`, `ref-linux-wayland` (one runner service, label switches with the active session via the setup script in WP-0I.04, or two services with a shared concurrency group) |

Purchase recommendation: a Core Ultra 7 155H mini PC (for example from the Minisforum, GEEKOM, Beelink
or ASUS NUC class), 32 GB, 1 TB, about 600 to 850 USD, lead time 2 to 10 days from Amazon or the
vendor store (imports can take longer; prefer in-stock regional stock). An AMD 7840HS equivalent is
about 450 to 700 USD. Check that Ubuntu 24.04 boots and both sessions work before buying (vendor
Linux reports and the `linux-hardware.org` probe database are a quick filter).

## 4. Minimum-OS machines (ADR-014)

Used only by the nightly `min-os` smoke job (WP-0I.04): fetched FFmpeg bundle probe, `engine --version`,
headless render of one synthetic frame **on a software adapter**, Electron launch smoke. VMs are
acceptable because no GPU or timing measurement is taken. They are not reference machines for any
budget.

| Label | OS | Recommended host | Notes |
|---|---|---|---|
| `min-win10` | Windows 10 22H2 | Hyper-V (or VirtualBox) VM on `ref-win-dgpu` (owned, Win11 Pro has Hyper-V). 4 vCPU, 8 GB, 80 GB disk | Needs a Windows 10 licence/ISO decision (owner). Windows 10 has left mainstream support; if the pinned Electron drops it, ADR-014 falls back to Windows 11 only |
| `min-mac13` | macOS 13 arm64 | Apple Virtualization framework VM on `ref-mac-m1` (a macOS 13 guest needs a host on macOS 13 or newer). Intel macOS 13 is not tested (§2) | Licensing allows macOS guests only on Apple hardware. Software Metal renderer only |
| `min-ubuntu2204` | Ubuntu 22.04 | VM (KVM/QEMU) or container on the Linux box; the Linux build container is already 22.04 (glibc 2.35) per arch §21 | Mesa software (llvmpipe/lavapipe) |

Until the Mac and the Linux box exist, only `min-win10` can be run (on the owned laptop). Windows and
Linux min-OS checks may meanwhile run on a CI hosted runner or a local VM on any developer machine
and be flagged "dev-machine".

## 5. Audio loopback and A/V measurement fixture

Needed by WP-0D.02 and the `av_sync_offset_ms` budget. One kit per machine that measures audio
(minimum: one for Windows and one per Mac/Linux box, or move one kit between machines).

- Wired output: the machine's 3.5 mm or USB DAC output. **No Bluetooth or 2.4 GHz wireless headsets for
  offsets**; link buffering is invisible to software (S2 finding).
- Loopback: output into a USB audio interface line-in (Behringer UCA202 class about 30 USD, or a
  Focusrite Scarlett Solo class about 130 USD) with a 3.5 mm or RCA cable, recorded together with the
  display flash.
- Display flash capture: a phone at 240 fps (slow-motion mode) pointed at the screen with the speaker or
  line signal audible, or a photodiode on the screen into the second input channel. The spike's
  `--scene sync` window emits the loud click and white flash. Accepted measurement error is +-1 frame at
  60 fps (about 17 ms), so the < 40 ms budget leaves usable margin.
- Procedure is written into `docs/spikes/S2-report.md` (WP-0D.02); this document only fixes the kit.
- Estimated total per kit: 40 to 160 USD plus cables; the phone is assumed to be owned by the product
  owner.

## 6. Provisioning facts and procurement

| Item | Value |
|---|---|
| Order owner | product owner (D0.5 `hardware-owner`) |
| Order placed | **Not yet** (fill in date, vendor and order number per line when placed) |
| Lead times | see §3 and the list below (estimates) |
| Runner registration | WP-0I.04: self-hosted GitHub Actions, repo-scoped, only `workflow_dispatch`, nightly and `main`; no fork PRs; non-privileged user, ephemeral working dir |
| Laptop caveat | `ref-win-dgpu` and `ref-win-igpu` run on the maintainer's daily laptop. Runner jobs must not run concurrently with other GPU or CPU load: schedule them for a fixed window, require AC and the performance power mode, and stop the runner service outside it |

Purchase list (rough, estimates to verify at order time; the product owner decides):

| Item | For | Approx. USD | Lead time |
|---|---|---|---|
| M1 Mac mini/Air 16 GB, refurbished/used | `ref-mac-m1`, `min-mac13` | 400 to 650 | 3 to 10 days |
| Core Ultra 7 155H mini PC, 32 GB / 1 TB | `ref-linux-x11`, `ref-linux-wayland`, `min-ubuntu2204` | 600 to 850 | 2 to 10 days |
| Intel Xe/Arc Windows mini PC or thin laptop, 16 GB (recommended) | proper `ref-win-igpu` | 350 to 650 | 3 to 10 days |
| 2 x USB audio interface + cables | loopback fixtures | 60 to 260 | 2 to 5 days |
| 2 x HDMI dummy plugs | headless Mac and Linux displays | 20 | 2 to 5 days |
| Windows 10 22H2 ISO/licence (if none) | `min-win10` | 0 to 150 | immediate |
| **Total, required (Mac + Linux + fixtures)** | | **about 1,100 to 1,800** | **up to about 10 days** |
| **Total with the Intel Windows machine** | | **about 1,450 to 2,450** | |

## 7. Measurement rules

### 7.1 Result tagging

Every result JSON follows [`bench/result.schema.json`](../bench/result.schema.json):

| Field | Value |
|---|---|
| `machine` | One id from this document (`ref-win-dgpu`, `ref-win-igpu`, `ref-mac-m1`, `ref-linux-x11`, `ref-linux-wayland`), set through `CUTWEAVE_BENCH_MACHINE`. Anything else is `local` (developer run, never evidence for a budget, never in a spike report as "reference") |
| `commit` | Git SHA measured; dirty trees are not valid |
| `date` | UTC `YYYY-MM-DD` |
| `toolchain.rustc` | `rustc --version` |
| `toolchain.ffmpeg` | Bundled FFmpeg version and build id (or null if unused) |
| `toolchain.wgpu` | Pinned wgpu version (30.0.2 in the spikes) |
| `toolchain.electron` | Pinned Electron (45 in S1) with the Chromium version |

Extra facts that the schema has no slot for go in the spike report next to the numbers (and, once
WP-0I.03 allows, in a sidecar `machine.json`): adapter name and backend actually used (wgpu and
Chromium separately), driver version, OS build, display mode and refresh, audio device, session type
(`XDG_SESSION_TYPE`), power plan, ambient room temperature note if throttling was seen, and whether
the run was `dev-machine` (a non-reference box) per the WBS fallback. A result missing its `machine`
or toolchain fields is rejected by `cargo xtask bench-check`.

### 7.2 Power and thermal conditions

All measured runs, with no exceptions for "quick checks" that feed a report:

1. **AC power** connected for the whole run; battery runs are invalid.
2. **Performance mode:** Windows power mode "Best performance" (and the OEM performance profile), macOS
   Low Power mode off and no battery-health throttling, Linux `power-profiles-daemon` = `performance`
   (or the CPU governor `performance`). Record the mode in the result.
3. **Clean machine state:** no other GPU or CPU heavy work, no Windows Update/Spotlight indexing
   running, display not sleeping, notifications off, one monitor at the recorded refresh, no screen
   recorder or remote-desktop session. Windows: close game overlays. Laptop lid open with the cooling
   unobstructed.
4. **Warm-up:** 5 minutes of the same workload before sampling, so caches, JIT and clocks settle.
5. **Sustained load, not bursts.** The S4 spike found that the RTX 3080 laptop GPU clocks down under
   sustained export (about 7.3 fps for 300 PNG frames, versus a 0.7 ms GPU pass on a cold burst). Any
   GPU, encode or export measurement (`export_1080p_h264_realtime_x`, `playback_1080p60_dropped_frames`,
   S4 timings, S1 10-minute runs) therefore must: run at least 5 minutes of sampled load after the
   warm-up, record GPU clock and temperature at the start and end (`nvidia-smi -q -d CLOCK,TEMPERATURE`,
   `powermetrics` on macOS, `intel_gpu_top` / `radeontop` on Linux), and report the **steady-state**
   value, not the first sample. If clocks fall during the run, report both the cold and steady values
   and judge the budget on steady-state. Locking clocks (`nvidia-smi -lgc`) is allowed only as an
   explicitly flagged secondary run and needs admin rights.
6. **Repeat:** at least 3 runs, report the median, keep the raw JSON (`samples` field is the run count
   or sample count).
7. A reboot before a formal session is recommended; record uptime if not.

### 7.3 Reproducibility checklist

- [ ] Machine id chosen from this document; deviations (e.g. AMD stand-in for Intel Xe) named in the report
- [ ] `git rev-parse HEAD` recorded, tree clean, `just setup` done with the pinned toolchains
- [ ] FFmpeg bundle version and the corpus manifest hash recorded
- [ ] Driver, OS build and session type recorded; auto-updates (Windows Update, GPU drivers, apt, macOS) paused for the session
- [ ] Correct adapter confirmed from `s4 adapters` / the engine log (not the software adapter, not the wrong GPU)
- [ ] AC, performance mode, 5-minute warm-up, sustained load window (§7.2)
- [ ] 3 repetitions; raw JSON committed (spike `results/` or the report folder)
- [ ] Audio runs: wired output, loopback fixture connected, sample rate and buffer size logged
- [ ] `cargo xtask bench-check` run against `bench/budgets.toml` for the metrics that have a budget
- [ ] Results from a non-reference machine labelled `dev-machine` and scheduled for a re-run on reference hardware before M0 sign-off

## 8. Spike to machine mapping

"Owned today" says whether it can be run right now. Dev-machine runs are labelled per §7.1 and re-run
on reference hardware before M0 sign-off.

| Spike (WP) | What is measured | Must pass on | Owned today | Notes |
|---|---|---|---|---|
| S1 preview + transport (0C.04) | 60 fps, dropped frames/min < 2, scrub p95 < 80 ms, transport CPU, `writeTexture` vs `importExternalTexture`, transport (a) vs (b), z-order overlays | `ref-win-dgpu`, `ref-win-igpu`, `ref-mac-m1`, `ref-linux-x11`, `ref-linux-wayland` (4 OS rows: Win11, macOS, X11, Wayland) | Windows only | The spike's shared memory is implemented for Windows only; Linux/macOS need the POSIX ring first. Run Chromium on the iGPU and dGPU separately (informal run showed Chromium picked the AMD iGPU while wgpu picked the RTX) |
| S1 timeline stress (0C.03/0C.04) | 5,000-item pan/zoom >= 55 fps, hit-test p95 < 2 ms | all five ids | Windows only | Informal numbers came from headless Chromium on this laptop; formal runs use the real Electron window at recorded refresh and DPR 1 and 2 (HiDPI was not measured) |
| S2 audio clock (0D.02) | 30-min drift (frames/30 min), underruns, latency, A/V offset (loopback) | `ref-win-dgpu` (WASAPI), `ref-mac-m1` (CoreAudio), `ref-linux-x11` or `ref-linux-wayland` (PipeWire; run on the active session, both if time allows) | Windows only, wired fixture still needed | One machine per audio stack is enough since audio does not depend on the GPU; the informal Windows run used a wireless headset and cannot count for offset |
| S3 decode/scrub (0E.03) | 500/500 frame-exact, 4K long-GOP scrub p95 < 150 ms, proxy p95 < 33 ms, SW and HW decode, zero-copy status | `ref-win-dgpu` (NVDEC, the "NVIDIA-equipped Windows machine"), `ref-win-igpu`, `ref-mac-m1` (VideoToolbox), `ref-linux-x11` and `ref-linux-wayland` (VA-API where it differs) | Windows (NVDEC + AMD) only | Windows iGPU HW decode on AMD VCN differs from Intel Quick Sync; flag if Intel hardware is later added |
| S4 compositor (0F.03) | GPU time < 8 ms (1080p60 scene) on an iGPU, PSNR > 45 dB vs reference, DeltaE < 2, 4K text crispness, 1000 glyphs < 1 ms; WARP/lavapipe vs real-GPU tolerances | iGPU criterion: `ref-win-igpu`, `ref-mac-m1`, and the Linux box (Intel Arc or AMD iGPU). Quality criteria: every id plus WARP (and lavapipe on Linux) | Windows: RTX 3080 + AMD iGPU + WARP | Informal: AMD iGPU p50 6.5 ms / p95 8.1 to 8.5 ms (borderline). Use sustained-load measurement (§7.2) |
| S4b scene scripting (0G.02) | transpile < 50 ms, scene eval < 1 ms per frame (200 nodes), determinism hashes equal, sandbox suite | every reference machine (CPU-bound; one run per OS is the hard minimum, per-id as the WBS says) | Windows only | Determinism is hashed across three separate processes per machine |
| S5 command bus / agent run (0H.03) | agent runs 3/3, rmcp pin | any one machine (headless, no GPU or audio need); record id as `local` or the machine used | Yes | Needs Claude Code access, not special hardware |

## 9. Budget to machine mapping (arch §19)

`machine_scope` in [`bench/budgets.toml`](../bench/budgets.toml) defines which results are checked
against a row. "Human-run" means no self-hosted runner exists yet (the runners are WP-0I.04): the owner
runs the measurement by hand on the named machine and attaches JSON to the spike report.

| Budget id | Budget | Machine scope in `budgets.toml` | Reference machines to measure on | Until self-hosted runners exist |
|---|---|---|---|---|
| `app_start_ms` | < 2.5 s | any | all five | human-run on each owned machine; hosted CI gives informational numbers only |
| `project_open_1h_ms` | < 2 s | any | all five | human-run; needs a P1+ corpus project |
| `command_roundtrip_p95_ms` | p95 < 16 ms | any | `ref-win-dgpu`, `ref-mac-m1`, `ref-linux-x11` | human-run |
| `scrub_proxy_p95_ms` | p95 < 80 ms | any | all five (S1/S3) | human-run |
| `scrub_4k_longgop_p95_ms` | p95 < 150 ms | any | all five (S3) | human-run |
| `playback_1080p60_dropped_frames` | 0 over 60 s (3 video layers, 2 effects, captions) on an iGPU | `ref-win-igpu`, `ref-mac-m1` | `ref-win-igpu` (Intel Xe target, AMD interim), `ref-mac-m1`; Linux box as extra data | **human-run only**; runner results informational (sustained load, §7.2) |
| `timeline_panzoom_5000_fps` | >= 55 fps | any | all five | human-run |
| `av_sync_offset_ms` | < 40 ms, drift < 1 frame/30 min | any | `ref-win-dgpu`, `ref-mac-m1`, `ref-linux-*` (audio stack) | **human-run only** (needs the loopback fixture and a camera) |
| `export_1080p_h264_realtime_x` | >= 2x realtime, HW encode | any | `ref-win-dgpu` (NVENC), `ref-mac-m1` (VideoToolbox), Linux box (VA-API), `ref-win-igpu` (AMD VCN interim) | human-run; sustained load mandatory (S4 saw dGPU clock-down) |
| `idle_memory_1h_1080p_mb` | < 1.5 GB | any | `ref-mac-m1` (16 GB, the representative size) and the Linux box; the 31 GB Windows laptop is not representative of a 16 GB machine | human-run |

## 10. Maintenance

- Change a machine (driver, OS update, new hardware): update this file and `bench/machines.toml` in one
  PR and re-run the baseline before comparing new numbers against stored baselines.
- When a TBD machine arrives, fill every field with concrete values, set its `status` to `owned` in
  `bench/machines.toml`, and record the order date and vendor in §6.
- Adding an id requires adding it to `docs/reference-machines.md`, `bench/machines.toml`, the runner
  labels in WP-0I.04 and, if a budget scopes to it, `bench/budgets.toml`.
