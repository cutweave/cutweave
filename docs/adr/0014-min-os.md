# ADR-0014: Minimum supported operating systems

- **Status:** Accepted (product owner, 2026-11-28)
- **Date:** 2026-11-22
- **Deciders:** Cutweave maintainers (decision pending WP-0J.03)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §21, §24 (open question 4), §20

## Context

The minimum OS set drives build baselines (Linux glibc), CI and smoke-test images, the Electron, wgpu and FFmpeg floors, and what we promise users. Architecture §21 already builds Linux artifacts in an Ubuntu 22.04 container (glibc 2.35) to honor this ADR, and §24 lists the minimum OS as an open question to be settled in Phase 0 together with the pinned Electron and wgpu requirements. The WBS names this file `docs/adr/0014-min-os.md` (WP-0J.03).

## Decision drivers

- The pinned Electron's supported-OS list (the hard constraint) and the wgpu backends available (D3D12/Vulkan, Metal, Vulkan on Mesa).
- FFmpeg hardware acceleration availability per OS version.
- Reach: creators on slightly older machines, versus the cost of testing more OS versions.
- Maintenance and CI cost (self-hosted reference machines, nightly min-OS smoke tests).

## Considered options

1. **Windows 10 22H2+ and 11; macOS 13+; Ubuntu 22.04+ / Fedora 40+ (X11 and Wayland), Mesa >= 23** (current proposal, arch §23).
2. **Windows 11 only** (the fallback inside option 1 if the pinned Electron or wgpu drop Windows 10).
3. **macOS 14+ or Apple-silicon only;** a narrower macOS floor reduces Metal and VideoToolbox variance.
4. **Newer Linux baseline (24.04+)** to match the CI image; excludes many LTS users.

## Decision

Accepted by the product owner on 2026-11-28 as proposed: Windows 10 22H2 and Windows 11 (Windows 10 only while the pinned Electron supports it, else Windows 11 only), macOS 13+, Ubuntu 22.04+ and Fedora 40+ (X11 and Wayland) with Mesa >= 23, built in the glibc 2.35 container. The nightly smoke test on these floors waits for the reference machines (WP-0I.04, #34), and the floor is checked again on each Electron upgrade.

**Decided by: WP-0J.03** (human), with evidence from WP-0A.03 (CI matrix), the Spike S2 audio results (WP-0D.02) and the Electron, wgpu and FFmpeg floors. Criteria: choose the lowest versions that (a) the pinned Electron officially supports, (b) provide a wgpu backend that passes the golden-frame and spike tests, and (c) can be smoke-tested nightly on a real or VM image. Proposed baseline: Windows 10 22H2+ if supported, else Windows 11 only; macOS 13+ (Metal 3, arm64 and Intel as supported); Ubuntu 22.04+ and Fedora 40+ with Mesa >= 23, glibc 2.35 build container. The evidence is recorded in the Accepted ADR. Not decided here.

## Consequences

**Positive** (if the proposal holds)

- A wide reach on LTS Linux and current macOS, with a clear glibc baseline for packaging.

**Negative / costs**

- Older OS versions limit available APIs (GPU features, codecs, audio) and add smoke-test cost.
- Electron's own support policy moves under us; the floor must be revisited on each Electron upgrade (quarterly cadence).
- Windows 10 is past mainstream support, and keeping it adds risk and test burden.

**Follow-ups**

- WP-0J.03 (decision and `docs/m0-go-no-go.md`), nightly min-OS smoke job (arch §21), updates to README and docs.

## Links

- Architecture: [§21](../01-architecture.md), [§24](../01-architecture.md) (open question 4)
- Stack: [02-tech-stack.md](../02-tech-stack.md); WBS: [WP-0J.03](../wbs/P0-foundations.md)
- Integrator resolutions: R24 (Linux builds on a 22.04 baseline)
- Related: ADR-0001, ADR-0003
