# Cutweave — Phase 7 Work Breakdown: Hardening & 1.0

> Status: draft 2026-11-22. Source: [04-roadmap.md](../04-roadmap.md) Phase 7, [01-architecture.md](../01-architecture.md) §18, §19, §20, §21; research/03 §8.
> ID scheme `WP-7<epic>.<nn>`. Size S ≤ 1 agent-day, M = 2–4. Executor default `sonnet`; `opus` for perf/concurrency; `human` for legal, signing-key custody, security review, accessibility sign-off and launch decisions.
> Dependencies on Phase 6 are by WP id; on earlier phases by epic code. Phase 7 is **checklist-style**: each WP ends in numbers that CI or a named reviewer can verify.

## Phase goal

Make Cutweave shippable as 1.0: signed installers on every platform with safe auto-update, measurable reliability, accessible and localized UI chrome, every §19 performance budget green in CI, complete documentation, legal/compliance obligations met, and a launch kit.

## Exit criteria

The roadmap defines **eight 1.0 gates** (04-roadmap.md, Phase 7 exit); the gates below mirror them one-to-one, each mapped to WPs in the traceability section. **All gates must be green to tag 1.0.**

- **G1 Packaging & update:** signed/notarized installers on every target (Windows NSIS + MSI Authenticode, macOS DMG signed + notarized + stapled, Linux AppImage/deb/rpm, Flatpak accepted on Flathub) **verified on clean VMs**, plus **auto-update verified across two consecutive releases** on stable/beta/nightly channels with staged rollout.
- **G2 Performance:** **all architecture §19 budgets green for 7 consecutive nightly runs** on reference machines; memory profile of long projects clean.
- **G3 Reliability:** crash-free session rate **≥ 99.5% over the final 14 days** in the opted-in beta (**n ≥ 2,000 sessions**), with **no open P0/P1 bugs**. **Fallback gate** if opted-in n < 2,000: **≥ 99.5% over ≥ 500 sessions + a 72 h soak matrix with zero crashes + zero data-loss bugs**. Fuzz campaign and 100 forced crashes (no data loss) are supporting evidence.
- **G4 Accessibility & i18n:** **WCAG 2.2 AA** for UI chrome (zero serious/critical automated violations, external audit with no open AA failures) and English + **≥ 6 languages** (es, pt-BR, fr, de, ja, zh-Hans) shipped; RTL check passed (ar).
- **G5 Docs complete:** user guide, "coming from CapCut", plugin SDK reference, MCP/agent guide, auto-generated command reference, contributor + governance docs published; code samples tested.
- **G6 Legal/security checklist:** third-party notices generated and shipped; FFmpeg source offer published per release; codec-patent posture reviewed; model licenses displayed; security reviews of local API and plugin sandbox closed with no open High/Critical findings.
- **G7 Agent Kit & MCP:** Agent Kit evals **≥ 80% task success on two different agent clients** (6H.12, re-run on the RC) **+ MCP conformance** green for both spec revisions, 2025-11-25 and 2026-07-28 (6J.03, re-run on the RC).
- **G8 Go/no-go:** product-owner go/no-go held against this checklist (website, sample projects, template gallery and community channels live).

## Epics (one-line goals)

| Epic | Goal |
|---|---|
| **7A** Packaging, signing & updates | Reproducible, signed, notarized installers for every format and a safe staged auto-update pipeline. |
| **7B** Reliability | Crash-report analysis, crash-free metric and triage/hardening (collection ships in P3, WP-3I.05), fuzz campaign, soak matrix and forced-crash campaign. |
| **7C** Accessibility & i18n | WCAG 2.2 AA chrome (incl. an accessible timeline), external audit, en + 6 languages, RTL. |
| **7D** Performance sign-off | A single §19 budget dashboard and burn-down of any budget that is not green. |
| **7E** Documentation | Docs site, user and CapCut guides, SDK/MCP references, contributor and governance docs. |
| **7F** Legal, compliance & security | Notices, FFmpeg source offer, codec posture, model licenses/privacy audit, two security reviews, hardening. |
| **7G** Launch | Website, samples, gallery, community, RC process and the 1.0 decision. |

## Critical path

1. **Credentials → signing → installers → matrix → RC:** 7A.07 (human: certificates/accounts; **start in P5/P6**, lead time weeks) → 7A.02/7A.03 → 7A.01 → 7A.06 → 7A.08 → 7G.04.
2. **Flathub review (external, weeks):** 7A.05 submission must start in week 1; it gates G1.
3. **Beta crash-free window:** crash collection already ships with the Public Alpha (WP-3I.05); 7B.01 (dashboard/analysis) must be live by week 1 so ≥ 14 days of opted-in beta sessions exist before the go/no-go (7G.04).
4. **Perf burn-down:** 7D.01 (week 1) → 7D.02/7D.03/7D.04 → 7 consecutive green nights before RC.
5. **A11y:** 7C.01 → 7C.02 → 7C.03 (external audit scheduled in advance; remediation window) → green before RC.
6. **Security reviews** 7F.05/7F.06 need a frozen API/plugin surface; schedule for week 2–4, remediation until week 6.

---

# Epic 7A — Packaging, signing & updates

### WP-7A.01 — Release build pipeline, reproducibility & SBOM
- **Area:** .github/workflows/release.yml, apps/desktop, tools/release/
- **Depends on:** E1L, WP-6G.14 (kit bundled)
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - One tag-driven workflow building all targets (win x64, mac arm64 [+ x64 per Notes #4], linux x64 [+ arm64 if decided]) with pinned toolchains, `--locked` Cargo, frozen pnpm lockfile, vendored/pinned FFmpeg build (7F.02), engine binary + FFmpeg libs inside the bundle (§21).
  - electron-builder config for NSIS, MSI, DMG, AppImage, deb, rpm; versioning from tag; release notes from conventional commits.
  - CycloneDX SBOM per artifact (Rust + JS + bundled FFmpeg), SHA-256 checksums file, provenance attestation (SLSA/GitHub attestations); signed in later WPs.
  - Reproducibility check: two builds from the same tag produce identical engine binary hash (document and list any known non-determinism, e.g. installer timestamps).
- **Interfaces:** workflow `release`; artifacts `Cutweave-<ver>-<os>-<arch>.<ext>`, `SHA256SUMS`, `*.cdx.json`.
- **Acceptance criteria:** tagging `v1.0.0-rc.N` on a fork produces all artifacts for all targets in < 90 min; engine binary hash identical across two builds; SBOM lists 100% of resolved crates/packages (compare to lockfiles).
- **Tests:** workflow dry-run on a test tag; hash-compare job; SBOM completeness script.
- **Out of scope:** signing (7A.02–7A.04), updater (7A.06).

### WP-7A.02 — Windows: NSIS + MSI, Authenticode signing
- **Area:** apps/desktop/build/windows, .github/workflows
- **Depends on:** WP-7A.01, WP-7A.07
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - NSIS (per-user default, per-machine option) and MSI (enterprise, silent install flags); file association `.cutweave`, Start-menu entry, uninstaller removes app but **not** user projects/caches (prompt for caches).
  - Sign every PE we ship (app exe, engine, DLLs, installers, uninstaller) via the signing service chosen in 7A.07 (Azure Trusted Signing / SignPath OSS / EV cert) with RFC 3161 timestamp; fail CI if any unsigned PE is found.
  - SmartScreen reputation plan and WARP/VC++ runtime prerequisites handled; Windows min version per the P0 decision.
- **Interfaces:** `Cutweave-<ver>-win-x64.exe|.msi`.
- **Acceptance criteria:** `signtool verify /pa /all` passes for every PE inside the installer; installer runs silently (`/S`, `msiexec /qn`) and starts the app on a clean Windows 11 VM; uninstall leaves no program files; no AV false-positive on VirusTotal ≥ 3 engines (record).
- **Tests:** CI signature-scan job; VM install/uninstall test (7A.08).
- **Out of scope:** Microsoft Store/winget (post-1.0 nice-to-have).

### WP-7A.03 — macOS: DMG signed, notarized & stapled
- **Area:** apps/desktop/build/mac, .github/workflows
- **Depends on:** WP-7A.01, WP-7A.07
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Hardened runtime with minimal entitlements (e.g. `allow-jit` only where wasmtime/QuickJS/Chromium require it, `device.audio-input` for voiceover, `network.client`, user-selected files); sign nested binaries inside-out (engine, FFmpeg dylibs, helpers, native addon) then app, DMG; notarize with `notarytool`, staple ticket.
  - Target arch per Notes #4 (arm64 required; universal or separate x64 decided); DMG layout with Applications link.
  - Gatekeeper assessment automated.
- **Interfaces:** `Cutweave-<ver>-mac-arm64.dmg` (+ x64 if decided).
- **Acceptance criteria:** `spctl -a -vv` = accepted (notarized Developer ID); `codesign --verify --deep --strict` passes; `stapler validate` passes; app launches on a clean macOS 14 and 15 VM without any Gatekeeper dialog beyond the first-open prompt; mic permission prompt appears for voiceover.
- **Tests:** CI verification job; VM launch test (7A.08).
- **Out of scope:** Mac App Store.

### WP-7A.04 — Linux packages: AppImage, deb, rpm
- **Area:** apps/desktop/build/linux
- **Depends on:** WP-7A.01
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - AppImage (with update info for the updater), `.deb` (Ubuntu 22.04+/24.04, Debian 12) and `.rpm` (Fedora current-1, current), desktop file, MIME type for `.cutweave`, icons, AppStream metainfo; declare runtime deps (Vulkan/ALSA/PulseAudio-PipeWire, GTK libs); GPG-sign deb/rpm and publish a checksum + detached signature for the AppImage.
  - Wayland and X11 launch flags verified; HW-accel capability detection documented (VA-API/NVDEC availability).
- **Interfaces:** `.AppImage`, `.deb`, `.rpm`, `*.asc`.
- **Acceptance criteria:** installs and launches on Ubuntu 24.04 (X11 + Wayland), Debian 12, Fedora current; `lintian`/`rpmlint` no errors; signatures verify with the published key.
- **Tests:** container/VM install tests (7A.08).
- **Out of scope:** Flatpak (7A.05), Snap.

### WP-7A.05 — Flatpak & Flathub submission
- **Area:** packaging/flatpak/ (org.cutweave.Cutweave manifest), Flathub PR
- **Depends on:** WP-7A.01, WP-7F.02
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Flatpak manifest building from source with offline dependency generators (flatpak-cargo-generator, flatpak-node-generator) as Flathub requires; runtime `org.freedesktop.Platform` + `org.freedesktop.Platform.ffmpeg-full` extension for codecs per §21; Electron base app; permissions: `--socket=wayland/x11`, `--device=dri`, `--socket=pulseaudio`, `--filesystem` limited (portal file chooser), network.
  - AppStream metadata (screenshots, release notes, OARS, license), Flathub CI passing, **submit the Flathub PR in week 1** and drive review to acceptance.
  - Decide and document FFmpeg source inside Flatpak vs the codecs extension (GPL/x264 handling; Notes #9).
- **Interfaces:** Flathub app id `org.cutweave.Cutweave`; manifest repository.
- **Acceptance criteria:** `flatpak-builder` succeeds offline; `flatpak run` launches, imports, plays and exports an H.264 MP4 on Ubuntu 24.04 (X11 and Wayland); Flathub review passes and app is published (or reviewer-approved pending state recorded with exact blockers).
- **Tests:** Flatpak CI build; smoke e2e inside sandbox (open sample project, export).
- **Out of scope:** Snap, distro-maintained packages.

### WP-7A.06 — Auto-update: stable/beta/nightly channels & staged rollout
- **Area:** apps/desktop (electron-updater), tools/release/feed
- **Depends on:** WP-7A.02, WP-7A.03, WP-7A.04
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - `electron-updater` feeds per channel (stable, beta, nightly) hosted as static files (GitHub Releases + Pages mirror); update **signature verification** (platform code-signing check + published Ed25519 manifest signature, fail-closed); differential/blockmap updates where supported.
  - **Staged rollout:** feed percentage (e.g. 5% → 25% → 100% over 72 h) using a stable hashed client bucket (no identifiers sent); a **halt/rollback switch** (publish previous version, `minimumVersion` guard); channel switching UI; "skip this version"; updates never interrupt an export/active job (deferred until idle).
  - Respects global "disable update checks" (zero requests when off); linux .deb/.rpm/Flatpak/AppImage behaviors documented (package managers handle theirs; AppImage self-updates).
  - The Updates page of Settings (WP-1M.11's frame): channel (stable), download automatically (on), check now, skipped versions; the "Check for updates" switch sits on the Privacy page (catalog `docs/reference/settings.md`).
- **Interfaces:** `latest*.yml` feeds + signature files; settings → Updates.
- **Acceptance criteria:** N-1 → N update succeeds on win/mac/AppImage in the matrix; tampered feed or artifact is rejected; staged rollout buckets verified at 5/25/100% over a simulated 10k-client population (±2%); pulling a release stops further rollout within one feed refresh; update during an active export is deferred.
- **Tests:** local feed-server integration tests; tamper tests; bucket distribution unit test.
- **Out of scope:** delta compression beyond blockmaps.

### WP-7A.07 — Signing credentials, accounts & custody (human)
- **Area:** process, repo secrets
- **Depends on:** — (**start in Phase 5/6; lead time weeks**)
- **Size:** M
- **Executor:** human
- **Scope:**
  - **Windows:** obtain a code-signing path viable for an OSS project without a company (candidates: SignPath Foundation OSS program, Azure Trusted Signing if eligible, OV/EV certificate); **Apple:** Developer Program enrollment + Developer ID Application cert + notary API key; **Linux:** GPG release key; **Update feed:** Ed25519 key; **Flathub:** account + app-id ownership verification.
  - Custody: hardware-backed or cloud-HSM where possible; secrets only in protected CI environments with required reviewers; ≥ 2 maintainers hold recovery material; documented rotation/revocation/compromise runbook; one rehearsal (sign a test build, rotate).
  - Coordinate with 6C.03 (plugin index keys): separate keys, shared process.
- **Interfaces:** `docs/security/signing-runbook.md`; CI environments `release-signing`.
- **Acceptance criteria:** a test-signed build for each OS is produced from CI with no human-held secrets on dev machines; runbook rehearsal log committed; credentials expiry dates tracked (calendar issue).
- **Tests:** rehearsal log.
- **Out of scope:** EV hardware token distribution to contributors.

### WP-7A.08 — Install/upgrade/uninstall matrix & clean-machine smoke tests
- **Area:** tools/release/install-tests, .github/workflows
- **Depends on:** WP-7A.02, WP-7A.03, WP-7A.04, WP-7A.05, WP-7A.06
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Automated or scripted VM/container matrix: Windows 11 (NSIS + MSI), macOS 14 + 15 arm64, Ubuntu 24.04 X11 + Wayland (deb, AppImage), Fedora (rpm), Flatpak; for each: install → first launch → open sample project → play 10 s → export MP4 → check engine + FFmpeg versions → upgrade from N-1 → uninstall → verify nothing left except user data.
  - Verification of signatures/notarization on the **downloaded** artifact (not the build tree).
  - Matrix results attached to the RC GitHub release.
- **Interfaces:** CI/manual job `install-matrix`; report `docs/release/install-matrix.md`.
- **Acceptance criteria:** 100% of matrix cells pass on the RC; **auto-update verified across two consecutive releases** (N-2 → N-1 → N on stable and beta channels, signatures verified by the updater) on each OS; any manual cell (macOS VM, GPU-dependent) has a recorded human run with date and build hash.
- **Tests:** is the test suite.
- **Out of scope:** performance testing (7D).

---

# Epic 7B — Reliability

### WP-7B.01 — Crash analysis, crash-free-session metric & hardening
- **Area:** tools/crash-intake/ (dashboard + triage), crates/cutweave-engine, apps/desktop
- **Depends on:** WP-3I.05 (crash reporting collection, shipped with the Public Alpha), E1L (supervision), WP-6B.04 (error UX pattern)
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Crash **collection** (opt-in, self-hosted, privacy-preserving) moved earlier to **WP-3I.05**; this WP does not re-implement it. It consumes those reports and clean-exit session counts.
  - Analysis: symbolication/grouping pipeline tuning (dedupe by stack signature), weekly triage report, P0/P1 labelling rules, top-N crash signatures burn-down with owners.
  - **Crash-free metric:** daily aggregate of opt-in sessions (clean-exit flag from 3I.05); dashboard computes the crash-free session rate with a Wilson 95% interval over the final-14-day window, shows n, and evaluates **both** G3 variants: primary (≥ 99.5%, n ≥ 2,000) and fallback (n < 2,000: ≥ 99.5% over ≥ 500 sessions + 72 h soak with zero crashes + zero data-loss bugs, evidence from 7B.03).
  - Hardening: add GPU-device-lost and hang detection (watchdog) coverage where 3I.05 left gaps, recovery-UI improvements for the top crash classes, regression tests for every fixed signature.
- **Interfaces:** dashboard query; crash schema `schema/crash-report.json` (owned by 3I.05).
- **Acceptance criteria:** dashboard reproduces the crash-free rate and CI from a seeded synthetic dataset (unit-tested, both G3 variants); every P0/P1 signature has an owner or fix; forced engine panic, renderer crash, GPU loss and hang each group under the correct signature; no PII appears in the analysed dataset (scrub audit on 50 sampled reports).
- **Tests:** dashboard unit tests on synthetic sessions, signature-grouping tests, scrub audit.
- **Out of scope:** the collection client/endpoint (3I.05), automated issue filing.

### WP-7B.02 — Fuzzing campaign & triage gate
- **Area:** fuzz/, .github/workflows/fuzz.yml
- **Depends on:** WP-6A.08, WP-6E.03
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Ensure `cargo-fuzz` targets with seed corpora for: project load + every migration, command JSON (schema-aware), OTIO/EDL/FCPXML importers, SRT/VTT/ASS parsers, plugin manifest + pack validators + importer `probe` + host imports, template loader, scene-graph decoder, MCP JSON-RPC input, WS RPC frames, effect.toml/WGSL front-end, media probe wrappers (FFmpeg with a sanitizer build where feasible).
  - Nightly fuzz on CI/dedicated runner with corpus persistence; apply to OSS-Fuzz (optional) ; ASAN/UBSAN builds.
  - **Campaign gate:** ≥ 24 CPU-hours per target (≥ 100 CPU-hours for project-load and importers) over the final two weeks with **zero open crashes/hangs/OOMs**; each found bug gets a regression test in the corpus.
- **Interfaces:** `fuzz/targets/*`, `fuzz/corpus/*`, report `docs/release/fuzz-report.md`.
- **Acceptance criteria:** ≥ 14 targets live; gate numbers met and report committed; all findings fixed with regression tests (list in report).
- **Tests:** nightly fuzz job; corpus replay in PR CI (fast).
- **Out of scope:** fuzzing third-party FFmpeg itself.

### WP-7B.03 — Soak-test matrix & forced-crash campaign
- **Area:** tools/soak/, .github/workflows/soak.yml
- **Depends on:** E1C (recovery), E2J (benchmarks), WP-7B.01
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - **Soak scenarios** (scripted via MCP/RPC + UI e2e): (1) 4-hour randomized editing session (proptest-driven commands, undo/redo, save/reload) on a 1 h/400-item project; (2) 30-min A/V playback drift nightly (existing) extended to 2 h weekly; (3) 8 h idle with project open; (4) 50-job export queue incl. cancels; (5) plugin load/unload/crash loops ×1000; (6) AI mock-provider jobs ×500 with cancel/resume; (7) repeated open/close of 100 projects.
  - Each scenario reports RSS/VRAM/handle/thread growth, dropped frames, errors; thresholds: RSS growth < 10% over 4 h after warm-up, 0 handle leaks, 0 unhandled errors, A/V drift < 1 frame/30 min, 0 underruns at 256 samples.
  - **Forced-crash campaign:** 100 forced kills (SIGKILL/TerminateProcess, power-loss simulation via fs-sync fault injection) at randomized points (mid-command, mid-save, mid-export, mid-autosave) → recovery UI → compare doc to journal truth: **0 lost committed commands, 0 corrupted projects** (extends the 20-crash Phase 2 check).
  - Matrix: 3 OS × reference machines; weekly full run, nightly subset.
- **Interfaces:** `cutweave debug kill-at <point>`; soak reports in CI artifacts.
- **Acceptance criteria:** all scenarios green on 3 consecutive weekly runs before RC; 100/100 crash recoveries lossless; regression issues filed for any threshold breach.
- **Tests:** is the test suite.
- **Out of scope:** hardware-specific GPU driver bugs (tracked as known issues).

---

# Epic 7C — Accessibility & i18n

**Scope definition (WCAG 2.2 AA "UI chrome"):** all app UI except user media content in the preview, rendered video, and user-authored captions/text. Screens in scope are enumerated in `docs/a11y/scope.md` (7C.01): welcome, project dialogs, media bin, inspector, timeline (see 7C.02), transport, dialogs, command palette, settings, plugin manager, template browser, AI Studio, copilot panel, export/publish.

### WP-7C.01 — Automated a11y checks, keyboard access, focus & screen-reader labels
- **Area:** packages/editor, packages/ui, e2e/
- **Depends on:** E2H, WP-6B.04, WP-6C.05, WP-6D.04, WP-6I.07
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Scope doc + checklist mapped to WCAG 2.2 AA success criteria (incl. new 2.4.11 focus not obscured, 2.5.7 dragging alternatives, 2.5.8 target size ≥ 24 px, 3.2.6 consistent help, 3.3.7 redundant entry, 3.3.8 accessible authentication N/A).
  - axe-core in Playwright across every in-scope screen/state (light/dark/high-contrast); CI blocks on serious/critical; `eslint-plugin-jsx-a11y`.
  - Remediate: tab order, visible focus, focus traps/returns in dialogs, dockview panel keyboard nav, command palette semantics, form labels/errors, `aria-live` regions for jobs/toasts/copilot streaming, names/roles on all icon buttons, keyboard-only operation of every non-timeline feature.
  - Keyboard-only e2e: import → edit via palette/shortcuts → export without a mouse.
  - **Context menu (product owner request, 2026-11-25; framework WP-2H.12, plan in [09-context-menus.md](../research/09-context-menus.md)):** a test opens every registered surface menu (from the surface tables) with the pointer and with Shift+F10 or the Menu key and runs axe; a table row without a keyboard path fails the gate.
- **Interfaces:** `docs/a11y/scope.md`, CI job `a11y`.
- **Acceptance criteria:** axe: 0 serious/critical on all scoped screens in 3 themes; keyboard-only e2e passes; every interactive element has an accessible name (script check).
- **Tests:** Playwright+axe matrix; keyboard e2e; lint.
- **Out of scope:** the Canvas timeline (7C.02).

### WP-7C.02 — Accessible timeline & visual-criteria remediation
- **Area:** packages/timeline, packages/ui
- **Depends on:** WP-7C.01, E2A
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - **Timeline (Canvas2D) alternative:** a parallel accessible representation (virtualized ARIA tree/grid "Timeline outline": tracks → items with name, start, duration, selected state) kept in sync with selection; full keyboard editing (move/trim/split/delete/snap by arrow keys with step sizes, announce results via live region); satisfies 2.5.7 (every drag has a non-drag alternative: nudge/command/inspector fields).
  - Visual criteria: contrast ≥ 4.5:1 text / 3:1 UI components in all themes (token audit script), high-contrast theme, `prefers-reduced-motion` and `prefers-contrast`, 200% zoom/reflow of dialogs, 24 px minimum targets, no color-only meaning (e.g. track colors, error states), caption-style preview not relied upon for UI.
  - Performance guard: accessible tree must not regress timeline budget (≥ 55 fps at 5k items — virtualized to visible region).
  - The high-contrast theme is a third option of Settings > Appearance > Theme (WP-1M.11).
- **Interfaces:** `TimelineA11yTree` component; token contrast report.
- **Acceptance criteria:** a user can perform select/move/trim/split/delete with keyboard only and hear/read results in NVDA, VoiceOver and Orca; token contrast script reports 0 failures; timeline benchmark unchanged (±5%).
- **Tests:** Playwright keyboard e2e on timeline; contrast script in CI; benchmark guard.
- **Out of scope:** making rendered video content accessible.

### WP-7C.03 — External accessibility audit & sign-off (human)
- **Area:** process
- **Depends on:** WP-7C.02
- **Size:** M
- **Executor:** human
- **Scope:** engage an accessibility auditor (or qualified contributor with AT expertise) to test in-scope screens with NVDA (Win), VoiceOver (mac), Orca (Linux), keyboard-only and zoom/high-contrast; produce a report mapped to WCAG 2.2 AA; triage findings into fix WPs (tracked here, fixed by sonnet agents); re-test; publish an Accessibility Conformance Report (VPAT-style) with known limitations (Canvas preview).
- **Interfaces:** `docs/a11y/audit-report.md`, `docs/a11y/acr.md`.
- **Acceptance criteria:** **no open AA-level failures** in scope; any exceptions explicitly listed with rationale and approved; sign-off recorded by named reviewer.
- **Tests:** retest evidence attached.
- **Out of scope:** AAA conformance.

### WP-7C.04 — i18n infrastructure, RTL support & pseudo-localization
- **Area:** packages/editor, packages/ui, crates/cutweave-api
- **Depends on:** E2H (i18n scaffold), WP-7C.01
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - ICU MessageFormat (plurals, select, gender), string extraction + CI (fail on hard-coded UI strings, missing keys, unused keys, unbalanced placeholders), pseudo-locale (accents + 40% expansion + RTL pseudo) used in visual regression to catch truncation.
  - Locale-aware date/time/number/duration formatting; engine error **codes** → localized messages in UI (engine never ships display strings the UI cannot translate); locale switch without restart; CJK/Arabic/Hebrew/Devanagari font fallback in chrome.
  - **RTL:** logical CSS properties throughout, mirrored icons list, `dir` handling in dialogs/panels; **timeline time axis stays LTR** (documented decision; Notes #8), transport controls not mirrored; bidi text in inspector/text fields; captions/text engine bidi verified via 3A (shaping tests with Arabic/Hebrew samples).
  - Settings > General gains "Language" (default: the OS language when Cutweave has it, else English), applied without a restart, main's menus included (catalog `docs/reference/settings.md`).
- **Interfaces:** `locales/<lang>/*.json` (ICU), `cutweave i18n check`; docs for translators.
- **Acceptance criteria:** pseudo-locale and RTL-pseudo e2e screenshots show no clipped/overlapping text in the 12 core screens (visual diff review recorded); string-lint passes; Arabic text renders shaped (no disconnected letters) in the Text tool preview and export golden.
- **Tests:** string lint, visual regression in pseudo/RTL locales, text-engine bidi golden tests.
- **Out of scope:** translations (7C.05).

### WP-7C.05 — Language packs: es, pt-BR, fr, de, ja, zh-Hans, ar
- **Area:** locales/
- **Depends on:** WP-7C.04
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Machine-first translation (LLM with glossary: video-editing terms consistent with CapCut/Premiere-localized equivalents, "do not translate" list: Cutweave, MCP, WGSL, FFmpeg…) of ~all UI strings (+ templates names, error messages, plugin manager) for **6 shipped languages + Arabic** (RTL check pack).
  - Glossary per language; length-budgeted strings; plural forms per CLDR.
  - Ships as `core` locale packs (L0); community contribution workflow documented (Weblate/Crowdin decision in Notes #8).
  - Human review step is tracked in 7C.06.
- **Interfaces:** `locales/{es,pt-BR,fr,de,ja,zh-Hans,ar}/`; coverage report.
- **Acceptance criteria:** 100% key coverage per language; lint passes (placeholders, plurals); screenshot pass of 12 core screens per language with no overflow (visual regression); app starts and switches language live.
- **Tests:** i18n lint, visual regression per locale.
- **Out of scope:** localizing docs (post-1.0) and AI prompts.

### WP-7C.06 — Native-speaker review & RTL sign-off (human)
- **Area:** process
- **Depends on:** WP-7C.05
- **Size:** S
- **Executor:** human
- **Scope:** one native/fluent reviewer per language spot-checks ≥ 150 strings each (timeline/inspector/export/errors prioritised) and rejects mistranslations; an Arabic-reading reviewer signs off RTL layout across the 12 core screens.
- **Interfaces:** review log `docs/i18n/review-<lang>.md`.
- **Acceptance criteria:** each language: ≥ 95% of sampled strings approved, rest fixed; RTL checklist signed.
- **Tests:** n/a.
- **Out of scope:** full proofreading.

---

# Epic 7D — Performance sign-off (architecture §19)

### WP-7D.01 — §19 budget dashboard & CI gates
- **Area:** crates/*/benches, tools/perf/, .github/workflows/perf.yml
- **Depends on:** E2J, E1O
- **Size:** M
- **Executor:** opus
- **Scope:**
  - Map each of the 10 §19 rows to a named benchmark id, scenario fixture (generated projects: empty, 1 h/400 items, 5k items, 4K long-GOP, 1080p60 3-layer + 2 effects + animated captions), reference machine (P0 definitions incl. iGPU M1 / Intel Xe) and threshold = the budget; nightly job writes JSON; static dashboard (green/red per budget, 30-day trend); **PR gate** on regressions > 5% vs baseline for criterion benches; release gate = all rows green.
  - Runner strategy: self-hosted reference runners for GPU/HW-encode rows (hosted runners only for software-adapter checks); noise control (CPU governor, repeat N=5, median/p95 rules per row).
  - Rows (copied): app start → editable project < 2.5 s; open 1 h/400-item project < 2 s; command RTT p95 < 16 ms; scrub p95 < 80 ms (proxy/intra) and < 150 ms (4K long-GOP no proxy); playback 1080p60 (3 video layers + 2 effects + animated captions, iGPU) 0 dropped frames over 60 s; timeline pan/zoom 5,000 items ≥ 55 fps; A/V offset < 40 ms and drift < 1 frame/30 min; export 1080p H.264 (HW) ≥ 2× realtime; idle memory 1 h 1080p project < 1.5 GB.
- **Interfaces:** `tools/perf/budgets.toml` (single source of truth), dashboard artifact, CI job `perf-gate`.
- **Acceptance criteria:** all 10 rows measured automatically on all reference machines; each row shows current value vs budget; seeded 20% regression flips the relevant row red and fails the PR gate.
- **Tests:** self-test of the gate with a synthetic regression.
- **Out of scope:** fixing red rows (7D.02–7D.04).

### WP-7D.02 — Startup, project-open & memory budgets burn-down
- **Area:** crates/cutweave-engine, crates/cutweave-cache, apps/desktop
- **Depends on:** WP-7D.01
- **Size:** M
- **Executor:** opus
- **Scope:** profile (tracing, samply/Instruments/WPR) and fix until green: **app start → editable empty project < 2.5 s**, **open 1-hour/400-item project < 2 s to interactive** (lazy thumbnail/waveform loading, snapshot build cost, sqlite index warm-up), **idle memory < 1.5 GB** with a 1 h 1080p project (frame-cache sizing policy, thumbnail pyramid caps, no leaks per 7B.03); heap profiling (dhat/heaptrack) report; cold vs warm start distinction documented (budget applies to warm start on reference machine; cold start reported).
- **Interfaces:** cache size policy settings; report `docs/perf/startup-memory.md`.
- **Acceptance criteria:** the three rows green on all reference machines for 7 consecutive nightly runs; heap report shows no unbounded growth over a 1 h idle + edit loop.
- **Tests:** benchmarks from 7D.01; soak scenario (3) from 7B.03.
- **Out of scope:** preview/playback paths (7D.03).

### WP-7D.03 — Playback, scrub & A/V sync budgets burn-down
- **Area:** crates/cutweave-playback, cutweave-render, cutweave-media, cutweave-audio
- **Depends on:** WP-7D.01
- **Size:** M
- **Executor:** opus
- **Scope:** fix until green: scrub p95 < 80 ms (proxy/intra) and < 150 ms (4K long-GOP no proxy); 1080p60 playback with 3 video layers + 2 effects + animated captions on iGPU (M1/Intel Xe) with **0 dropped frames over 60 s**; A/V offset < 40 ms and drift < 1 frame/30 min (nightly 30-min and weekly 2-h checks). Typical levers: quality-ladder policy, decode-ahead tuning, GPU buffer pooling, shader pass merging, HW decode defaults, audio buffer sizing. Also verify third-party/first-party effect GPU budgets (4A) still hold under the combined load.
- **Interfaces:** quality-ladder defaults; report `docs/perf/playback.md`.
- **Acceptance criteria:** rows green on all reference machines (Win11, macOS arm64, Ubuntu 24.04) for 7 consecutive nightly runs; no regression in golden frames (vs the stored per-adapter reference: SSIM ≥ 0.995 AND PSNR ≥ 40 dB; preview vs pre-encode export on the same adapter bit-identical).
- **Tests:** playback/scrub/sync benchmarks and the nightly 30-min sync soak.
- **Out of scope:** new features; HDR; zero-copy interop beyond what Spike 3 delivered.

### WP-7D.04 — Timeline, command RTT & export-speed budgets burn-down
- **Area:** packages/timeline, crates/cutweave-commands, crates/cutweave-api, crates/cutweave-media
- **Depends on:** WP-7D.01
- **Size:** M
- **Executor:** sonnet
- **Scope:** fix until green: timeline pan/zoom with 5,000 items ≥ 55 fps (with accessible tree from 7C.02 and proposal layer from 6I.08 active); command round-trip UI→engine→patch→UI p95 < 16 ms (including when a plugin handler or the copilot is active); export 1080p H.264 simple timeline with HW encode ≥ 2× realtime on each OS (NVENC/QSV/AMF, VideoToolbox, VA-API) with the software fallback measured and documented.
- **Interfaces:** report `docs/perf/timeline-export.md`.
- **Acceptance criteria:** rows green for 7 consecutive nightly runs; HW-encode availability matrix documented (which GPUs verified).
- **Tests:** benchmarks from 7D.01.
- **Out of scope:** export features.

---

# Epic 7E — Documentation

### WP-7E.01 — Docs site infrastructure
- **Area:** website/docs/ (or docs-site/), .github/workflows/docs.yml
- **Depends on:** WP-6C.08 (content format), E1N
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Static docs site (Astro Starlight or Docusaurus — pick one, ADR note) with versioned docs (1.0, next), full-text search (Pagefind), dark/light, i18n-ready structure (English only at 1.0), "edit this page", sidebar from folder structure, WCAG-conformant theme (checked by 7C tooling), generated pages mount points (command reference, schemas, WIT/typedoc).
  - Deploy on tag to GitHub Pages (or equivalent) under the project domain; preview deploys on PRs; sitemap/robots; no third-party trackers.
  - Link-check, spell-check (cspell with project dictionary), code-block extraction harness reused from 6C.08.
- **Interfaces:** `docs-site/`; CI `docs-build`, `docs-check`.
- **Acceptance criteria:** site builds in < 5 min; search returns results for 10 sample queries; 0 broken internal links; Lighthouse accessibility ≥ 95 on 5 sample pages; PR previews work.
- **Tests:** CI link/spell/lighthouse jobs.
- **Out of scope:** writing content (7E.02–7E.04).

### WP-7E.02 — User guide & "Coming from CapCut" guide
- **Area:** docs-site/content/guide/
- **Depends on:** WP-7E.01, E2H, E3E, E4F, E5F, WP-6D.04, WP-6F.04, WP-6I.07
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - **User guide:** install, first project, import, timeline editing, keyframes/speed, audio, text & captions & transcript editing, effects/transitions/color/masks, AI Studio (keys, cost, provenance), templates, copilot, export & publish, projects/backup/recovery, shortcuts (printable cheat-sheet), troubleshooting, privacy statement (what leaves the machine), accessibility features.
  - **Coming from CapCut:** feature-by-feature mapping table (from `03-feature-parity.md`), shortcut preset switch, workflow translations (auto captions, templates, effects, keyframes, export), "what's different/what's not in 1.0" honest list, importing media/assets, tips.
  - Screenshots auto-generated by a Playwright script against sample projects (re-runnable, theme-consistent), alt text on all images.
- **Interfaces:** markdown pages; `tools/docs/screenshots.ts`.
- **Acceptance criteria:** every top-level feature in the roadmap has a page; ≥ 40 pages; screenshot job reproducible; two non-developer testers complete "first captioned short" using only the guide (recorded).
- **Tests:** link/spell checks; screenshot regeneration diff.
- **Out of scope:** video tutorials.

### WP-7E.03 — Plugin SDK reference, MCP/agent guide & command reference publication
- **Area:** docs-site/content/{plugins,agents,reference}/
- **Depends on:** WP-7E.01, WP-6C.08, WP-6G.15, WP-6J.02, E1N
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - **Plugin SDK reference:** publish 6C.08 content; generated references from WIT, JSON Schemas (manifest, `effect.toml`, `composition.toml`, template, preset), `@cutweave/sdk` typedoc, `cutweave-plugin-sdk` rustdoc; permission reference; stability/versioning policy; licensing/plugin exception page.
  - **MCP/agent guide:** concepts (headless engine, attach vs headless), per-client setup (links to 6G.15 guides), tool catalog (core + generic), profiles, prompts and skills reference (generated from `agent-kit/`), safety model (confirm tokens, budgets, untrusted content), recipes ("make 3 shorts", "caption pass"), copilot reference, eval harness for contributors.
  - **Auto-generated command reference** from `schema/commands.json` (all namespaces, schemas, examples, MCP exposure/annotations, since-version), regenerated and diff-checked in CI so docs cannot drift from the registry.
- **Interfaces:** generated pages under `reference/`; CI `docs-generated-diff`.
- **Acceptance criteria:** 100% of registered commands appear in the reference with schema and example (count check against registry); doctest of all code samples passes; generated pages are identical to a fresh generation (drift check).
- **Tests:** drift check, doctest, link check.
- **Out of scope:** translated docs.

### WP-7E.04 — Contributor docs
- **Area:** docs-site/content/contributing/, CONTRIBUTING.md
- **Depends on:** WP-7E.01
- **Size:** S
- **Executor:** sonnet
- **Scope:** architecture tour (from `01-architecture.md`), repo layout, build/test per OS, running the engine/UI/MCP locally, adding a command (`#[command]` → generated artifacts), adding an effect/template/skill/eval task, benchmark and golden-frame workflow, DCO sign-off, PR/WP process (human + agent), release process overview, security reporting pointer; verified by following it on a clean machine per OS in CI where feasible (build instructions job).
- **Interfaces:** pages + updated `CONTRIBUTING.md`.
- **Acceptance criteria:** "clone → build → run engine → run tests" instructions execute successfully in a clean-container CI job (Linux) and were followed manually once on Windows and macOS (recorded).
- **Tests:** CI doc-test job for Linux.
- **Out of scope:** governance (7E.05).

### WP-7E.05 — Governance, RFC process & security policy (human)
- **Area:** GOVERNANCE.md, docs/rfcs/, SECURITY.md
- **Depends on:** WP-6C.03, WP-7E.04
- **Size:** S
- **Executor:** human
- **Scope:** write and ratify: maintainers list and roles, decision-making (lazy consensus + tie-break), RFC process (template, lifecycle, ADR relationship), release authority, trademark/name policy, code of conduct enforcement contacts, security policy and coordinated disclosure (contacts, SLAs, advisory workflow, supported versions), succession/bus-factor plan (≥ 2 people with signing and merge rights).
- **Interfaces:** documents + `docs/rfcs/0000-template.md`.
- **Acceptance criteria:** documents merged; ≥ 2 maintainers acknowledge in PR; first RFC (or dry-run) exercised.
- **Tests:** n/a.
- **Out of scope:** foundation/legal entity decisions.

### WP-7E.06 — Docs QA & completeness gate
- **Area:** .github/workflows/docs.yml, tools/docs/
- **Depends on:** WP-7E.02, WP-7E.03, WP-7E.04
- **Size:** S
- **Executor:** sonnet
- **Scope:** CI gate aggregating: link check (internal + external, allow-list), spell-check, code-sample doctests, generated-reference drift check, screenshot freshness (flag pages whose screenshots are older than 2 releases), accessibility of the site (axe on 10 pages), required-pages checklist (every roadmap-listed doc exists), version-banner correctness for 1.0.
- **Interfaces:** CI job `docs-gate` (required for release).
- **Acceptance criteria:** gate green; required-pages checklist 100%.
- **Tests:** seeded failures (broken link, drifted reference) turn it red.
- **Out of scope:** content creation.

---

# Epic 7F — Legal, compliance & security

### WP-7F.01 — Third-party notices generation & license audit
- **Area:** tools/notices/, apps/desktop (About → Licenses), .github/workflows
- **Depends on:** WP-6D.06 (asset licenses), E3G, E4D
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Build-time generation: `cargo-about` (Rust), JS license collector (pnpm/electron-builder deps), bundled FFmpeg + codec libs (from the FFmpeg build manifest), bundled fonts, LUTs, sticker/music/SFX packs, templates, caption styles, effect packs and the Agent Kit — each with license text and attribution (CC-BY credits included).
  - Outputs: `THIRD_PARTY_NOTICES.txt` + HTML in the installer and **About → Licenses** screen (searchable); machine-readable `licenses.json` in the release.
  - CI: `cargo-deny` + JS equivalent fail on unknown/forbidden licenses (GPL-incompatible, non-redistributable, "nonfree"), and on any bundled asset without an SPDX entry; SPDX header + DCO + LICENSE-file audit: `LICENSE` (GPL-3.0), plugin exception, MIT/Apache headers on `wit/`, `packages/plugin-sdk`, `packages/compose-runtime`.
- **Interfaces:** notices files in every installer; `licenses.json`.
- **Acceptance criteria:** installer contains the notices file and the About screen renders ≥ 99% entries of the SBOM (script compares to SBOM, 7A.01); CI fails on a seeded GPL-incompatible dependency and on an unlicensed asset; license-header audit script passes.
- **Tests:** CI checks; unit tests for collector; e2e About screen.
- **Out of scope:** legal opinion on specific licenses (7F.03).

### WP-7F.02 — FFmpeg build, source offer & GPL compliance
- **Area:** tools/ffmpeg-build/, .github/workflows, docs/legal/
- **Depends on:** E1D
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Pinned, reproducible FFmpeg build scripts for all OSes (GPL build with x264/x265/etc. as allowed by §21; no `--enable-nonfree`; HW APIs per OS), producing shared libs bundled in the app; versions and configure flags recorded.
  - **Per-release source offer:** publish alongside each release: exact FFmpeg source tarball (or git commit + verified mirror), all patches, build scripts, full `configure` line, third-party codec library sources/versions; a **written offer** (≥ 3 years) in the installer, About screen and docs; `ffmpeg -version`/`-buildconf` output embedded and compared with the published configure flags by CI.
  - Flatpak variant documented (uses codecs extension; Notes #9).
  - Source mirror is retained for ≥ 3 years (release assets, not only upstream).
- **Interfaces:** `docs/legal/ffmpeg-source-offer.md`, release assets `ffmpeg-source-<ver>.tar.xz`, `ffmpeg-build-info.json`.
- **Acceptance criteria:** CI verifies that the bundled library build-flags (read from the shipped binaries) equal `ffmpeg-build-info.json` and that the source tarball hash is listed in the release; rebuilding from the published source + scripts reproduces the library hash (or documented diffs); `--enable-nonfree` absent (scripted check).
- **Tests:** release-time compliance job (`ffmpeg-compliance`).
- **Out of scope:** FFmpeg upstream patches; codec patent licensing (7F.03).

### WP-7F.03 — Codec patent posture review (human/legal)
- **Area:** docs/legal/
- **Depends on:** WP-7F.02
- **Size:** M
- **Executor:** human
- **Scope:** counsel review of distributing H.264/HEVC/AAC-capable software and bundled encoders/decoders in target jurisdictions; decide and document the posture: prefer OS/HW codecs where available, default export presets (consider AV1/VP9/H.264-HW first; HEVC off by default or HW-only), whether x264/x265 ship in the base build or a separate "codec pack" (research/03 §8.4 suggests an optional pack; architecture §21 allows GPL build — **needs decision**, Notes #9), Flatpak codec extension handling, user-facing disclosure text, region notes; no patent-grant claims anywhere.
- **Interfaces:** `docs/legal/codec-posture.md`; in-app Export dialog disclosure text; installer/EULA-free notice text.
- **Acceptance criteria:** written counsel (or maintainer-with-counsel) sign-off recorded; build config matches the decision (CI check on encoder list); disclosure text shipped.
- **Tests:** CI check of `ffmpeg -encoders` vs the decision.
- **Out of scope:** obtaining patent licenses.

### WP-7F.04 — Model-license display & privacy/egress audit
- **Area:** crates/cutweave-ai, apps/desktop, docs/privacy/
- **Depends on:** E5I, E5B, WP-7B.01
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - **Model licenses:** every on-demand downloadable local model (Whisper, ONNX matting/denoise/scene/beat…) shows license, source, size, restrictions (non-commercial?) **before** download and in Settings → AI → Models; hash verification on download; weights never bundled silently (§21). Cloud catalog entries show provider terms link and data-retention note where known.
  - **Privacy/egress audit:** automated e2e with a recording proxy and no keys configured, telemetry/crash opt-in off: **only allowed outbound requests** are (a) update check (if enabled) and (b) plugin index fetch (if enabled), both disable-able; with them disabled → zero requests. Keys never appear in logs/crash reports/MCP output/plugin memory (secret-scan test). Provenance sidecars contain no machine-identifying data.
  - Privacy statement page (docs) generated from the allowlist of destinations.
- **Interfaces:** `docs/privacy/data-flows.md`; CI job `egress-audit`.
- **Acceptance criteria:** license dialog shown for 100% of downloadable models (list check vs `models.toml`); egress test passes in 3 configurations (default, all-off, AI-keyed mock); statement matches allowlist exactly (generated).
- **Tests:** e2e egress audit; secret-scan test; unit tests for hash verification.
- **Out of scope:** legal review of provider ToS.

### WP-7F.05 — Security review: local API (human-led)
- **Area:** docs/security/review-local-api.md
- **Depends on:** WP-6J.04, E1I, WP-6B.01
- **Size:** M
- **Executor:** human
- **Scope:** human security reviewer (or external firm), assisted by an opus-generated pre-review checklist and automated tooling, tests the engine's local surfaces: token auth entropy/rotation and run-file permissions (`~/.cutweave/run/<pid>.json`), localhost binding and Host/Origin checks (DNS rebinding, CSWSH on `/rpc`), MCP HTTP auth (401/403), CORS, path allowlist (symlinks, `..`, UNC, 8.3 names, junctions), media endpoints (SSRF-like, traversal), confirm-token forgery/replay, `cutweave_execute` abuse (resource exhaustion), job/export file writes, SHM ring permissions, Windows loopback/firewall prompt behaviour, update channel trust. Report severity-rated findings; fixes tracked as follow-up WPs; re-test.
- **Interfaces:** review report + fix list.
- **Acceptance criteria:** **0 open High/Critical** findings; Medium findings fixed or accepted with written rationale; re-test evidence attached.
- **Tests:** regression tests added for every fixed finding.
- **Out of scope:** the plugin sandbox (7F.06).

### WP-7F.06 — Security review: plugin sandbox & supply chain (human-led)
- **Area:** docs/security/review-plugins.md
- **Depends on:** WP-6A.08, WP-6B.03, WP-6C.04, WP-6F.01
- **Size:** M
- **Executor:** human
- **Scope:** review wasmtime host configuration and version (known CVEs, p2/p3 features enabled), permission enforcement bypass attempts (network allowlist redirect/DNS tricks, fs escapes, secret exfiltration through logs/errors/timing), command-bus scope escapes and recursion, L3 iframe sandbox escapes (CSP, protocol handler traversal, postMessage spoofing), plugin-supplied-text injection into MCP/copilot, GPU shader DoS handling, index/signing chain (rollback, key rotation, revoked keys), OAuth token isolation (6F.01), update path. Includes a time-boxed red-team exercise with the abuse corpus extended by the reviewer.
- **Interfaces:** review report + fix list.
- **Acceptance criteria:** **0 open High/Critical**; accepted-risk register for residual GPU DoS documented honestly in the plugin docs; re-test evidence.
- **Tests:** regression tests/corpus additions for each finding.
- **Out of scope:** third-party plugin audits.

### WP-7F.07 — Electron hardening verification & supply-chain gates
- **Area:** apps/desktop, .github/workflows/security.yml
- **Depends on:** WP-7A.01, E1L
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Verify §18 hardening with automated checks: `contextIsolation`, `sandbox: true`, no `nodeIntegration`, strict CSP (connect only to engine origin), validated IPC (schema per channel), Electron **fuses** set (disable `runAsNode`, `nodeCliInspect`, enable ASAR integrity/only-load-from-asar), `webSecurity` on, navigation/new-window denial, permission request handler default-deny (mic only on user action), no remote content; run Electronegativity (or equivalent) and fix findings.
  - Supply chain: `cargo-deny` (advisories, bans, licenses), `cargo-audit`, `pnpm audit` (high+ blocking, with reviewed ignore list), lockfile integrity, GitHub Actions pinned by SHA, least-privilege tokens, SBOM vulnerability scan (grype/osv) in the release workflow, provenance attestations verified; Dependabot/Renovate policy for security updates with SLA.
- **Interfaces:** CI job `security-gates`; `docs/security/hardening-checklist.md`.
- **Acceptance criteria:** automated checks assert every setting (fail on deliberate regression); release SBOM scan shows 0 known High/Critical vulnerabilities (or documented, time-boxed ignores); all workflow actions pinned.
- **Tests:** security-gate CI with seeded regressions.
- **Out of scope:** pentest (7F.05/7F.06).

---

# Epic 7G — Launch

### WP-7G.01 — Website, download page & template gallery
- **Area:** website/
- **Depends on:** WP-7E.01, WP-7A.08, WP-6D.12
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - Landing page (what/why, screenshots/short demo videos from sample projects, feature grid vs CapCut honest comparison, "your files, your keys, no account"), **download page** (OS detection, all formats, checksums, signature verification instructions, Flathub link, channel selector, system requirements), docs link, changelog/release notes, privacy page (from 7F.04), accessibility statement, FAQ, security contact.
  - **Template gallery page** generated from the template packs' previews + metadata (static, filterable by category/aspect; license/credits per template).
  - Static, privacy-respecting (no trackers; optional self-hosted privacy-friendly analytics decision in Notes), WCAG AA, Lighthouse ≥ 90 perf/≥ 95 a11y.
- **Interfaces:** `website/`; CI deploy on release tag.
- **Acceptance criteria:** download links resolve to the signed release artifacts and display correct SHA-256 (script check on RC); gallery lists all ≥ 30 templates with working previews; Lighthouse thresholds met.
- **Tests:** link/checksum check job; Lighthouse CI; Playwright smoke.
- **Out of scope:** blog, newsletter.

### WP-7G.02 — Sample projects & welcome experience
- **Area:** samples/, packages/editor
- **Depends on:** WP-6D.12, E3G, WP-6G.15
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - ≥ 5 sample projects with CC0/permissively licensed media (and SPDX credits): (1) 9:16 captioned short (talking head), (2) 10-min-style long-form with chapters and B-roll (short excerpt), (3) template-driven promo, (4) audio-polish/podcast, (5) an agent-ready project with `AGENTS.md` and notes for MCP/copilot demos; each opens from the Welcome screen ("Open sample") with media included or lazily downloaded (hash verified, size shown).
  - Used by docs screenshots, website demos and install-matrix smoke tests (7A.08).
  - First-run welcome: workspace choice (short-form/long-form), shortcut preset (NLE/CapCut), update/crash/telemetry choices explained, optional guided tour (skippable, accessible).
  - The welcome choices write the same preferences as Settings (WP-1M.11's store) and can be changed there later.
- **Interfaces:** `samples/<id>/`, Welcome screen.
- **Acceptance criteria:** all samples open, play and export on 3 OSes in CI (golden frames); total download ≤ 150 MB (lazy); every media file has license metadata (notices job passes); welcome flow keyboard accessible.
- **Tests:** CI sample-export job; Playwright e2e of first-run.
- **Out of scope:** video tutorials.

### WP-7G.03 — Community channels & issue/PR templates (human)
- **Area:** .github/, docs/community/
- **Depends on:** WP-7E.05
- **Size:** S
- **Executor:** human
- **Scope:** choose and open channels (GitHub Discussions + one chat platform, e.g. Discord or Matrix), moderation team and rules (CoC), pinned "start here" posts, issue forms (bug with crash-report attach, feature request, plugin submission, template submission), PR template with DCO reminder, labels/triage rotation, support expectations ("best effort"), roadmap/milestone board public, plugin-author and template-author channels.
- **Interfaces:** `.github/ISSUE_TEMPLATE/*`, `docs/community/*`.
- **Acceptance criteria:** channels live with ≥ 2 moderators; templates tested by filing one of each; triage rotation scheduled for launch month.
- **Tests:** n/a.
- **Out of scope:** paid support.

### WP-7G.04 — Release candidates, bug bash & 1.0 go/no-go (human)
- **Area:** docs/release/
- **Depends on:** WP-7A.08, WP-7B.02, WP-7B.03, WP-7C.03, WP-7C.06, WP-7D.02, WP-7D.03, WP-7D.04, WP-7E.06, WP-7F.01, WP-7F.02, WP-7F.03, WP-7F.04, WP-7F.05, WP-7F.06, WP-7F.07, WP-7G.01, WP-7G.02, WP-7G.03, WP-7B.01
- **Size:** M
- **Executor:** human
- **Scope:**
  - Feature freeze, RC builds (`rc.1…`) through beta channel; structured **bug bash** (maintainers + volunteers across 3 OSes; checklist of core flows, plugin install, templates, AI Studio mock/live, copilot, MCP clients, upgrade from last beta, project migration from earlier alphas/betas) with severity triage; burn-down of P0/P1 (fix WPs created as needed — a contingency budget of ~10 agent-days is reserved).
  - **Go/no-go meeting** (product-owner decision) against the checklist: G1–G8 each marked with evidence link (CI run, report, sign-off), crash-free rate, n and its confidence interval (G3 primary or fallback variant stated explicitly), the Agent Kit eval ≥ 80% result on two clients and MCP conformance report (G7), perf dashboard (7 green nights), a11y report, security reports, legal sign-offs, docs gate, install matrix, signing validity; explicit list of known issues; rollback plan (halt staged rollout, previous version pin).
  - Write release notes; tag `v1.0.0`; publish to stable channel with staged rollout; announce.
- **Interfaces:** `docs/release/go-no-go-1.0.md` (checklist with evidence), release notes.
- **Acceptance criteria:** every gate has evidence and an owner sign-off; no open P0/P1; decision recorded (go / no-go + date); 1.0 tagged and staged rollout started.
- **Tests:** n/a (decision gate).
- **Out of scope:** post-1.0 roadmap.

---

# Traceability

## Roadmap Phase 7 scope bullets → WPs

| Roadmap scope bullet | WPs |
|---|---|
| Signed and notarized installers for all formats | 7A.01, 7A.02 (NSIS/MSI), 7A.03 (DMG), 7A.04 (AppImage/deb/rpm), 7A.07 (credentials), 7A.08 (verification) |
| Flatpak on Flathub | 7A.05 |
| Staged auto-update channels (stable/beta/nightly) | 7A.06 |
| Crash-free session rate ≥ 99.5% (opt-in reports) | 3I.05 (collection, P3), 7B.01 (analysis/hardening) (+ 7B.02, 7B.03 as drivers; 7G.04 verifies) |
| Fuzzing campaign | 7B.02 |
| Soak-test matrix | 7B.03 |
| Accessibility audit (WCAG 2.2 AA for UI chrome) | 7C.01, 7C.02, 7C.03 |
| i18n (en plus ≥ 6 languages, RTL check) | 7C.04, 7C.05, 7C.06 |
| Every architecture §19 budget green in CI | 7D.01, 7D.02, 7D.03, 7D.04 |
| Memory profiling on long projects | 7D.02 (+ 7B.03 scenarios 1, 3, 7) |
| User guide and "coming from CapCut" guide | 7E.02 |
| Plugin SDK reference, MCP/agent guide, auto-generated command reference | 7E.03 |
| Contributor docs and governance doc (maintainers, RFC process) | 7E.04, 7E.05 |
| Third-party notices | 7F.01 |
| FFmpeg source offer | 7F.02 |
| Codec patent posture review | 7F.03 |
| Model-license display | 7F.04 |
| Security review of local API and plugin sandbox | 7F.05, 7F.06 (+ 7F.07 Electron hardening) |
| Website, sample projects, template gallery, community channels | 7G.01, 7G.02, 7G.03 (gallery in 7G.01) |

## Gates (derived exit criteria) → WPs

| Gate | Measured by | WPs |
|---|---|---|
| G1 Signed/notarized installers verified on clean VMs + auto-update across two releases | install matrix + signature verification + two-release update test | 7A.01–7A.08 |
| G2 §19 budgets green, 7 consecutive nights | perf dashboard | 7D.01–7D.04 |
| G3 Crash-free ≥ 99.5% (opted-in beta, n ≥ 2,000, final 14 days), no open P0/P1; fallback n < 2,000: ≥ 500 sessions + 72 h soak, 0 crashes, 0 data-loss bugs | crash dashboard (3I.05 data) + soak/fuzz reports | 3I.05, 7B.01, 7B.02, 7B.03 |
| G4 WCAG 2.2 AA chrome + ≥ 6 languages (RTL check) | axe CI + external audit + locale coverage | 7C.01–7C.06 |
| G5 Docs complete | docs gate | 7E.01–7E.06 |
| G6 Legal/security checklist | notices, source offer, reviews | 7F.01–7F.07 |
| G7 Agent Kit evals ≥ 80% on two clients + MCP conformance | 6H.12 and 6J.03 re-run on the RC | 6H.12, 6J.03 (P6), verified in 7G.04 |
| G8 Product-owner go/no-go | go/no-go record | 7G.01–7G.04 |

---

# Notes for integrator

1. **RESOLVED — the roadmap now has 8 1.0 gates**; P7's G1–G8 mirror them one-to-one (see Exit criteria).
2. **Crash-free ≥ 99.5% is statistically hard under "no telemetry by default".** Only opt-in sessions can be counted; with a small beta, the confidence interval can exceed ±0.5 pt (≈ 2,000+ sessions needed for ±0.4 pt at 99.5%). Session counting also is *new telemetry* beyond crash reports (§18). *RESOLVED — (a) metric: ≥ 99.5% over the final 14 days of the opted-in beta with n ≥ 2,000 sessions and no open P0/P1; (b) the opt-in aggregate session counter ships with the P3 crash reporting (3I.05); (c) fallback if opted-in n < 2,000: ≥ 99.5% over ≥ 500 sessions + 72 h soak matrix with zero crashes + zero data-loss bugs.* Collection ships with the Public Alpha (3I.05), so the window exists long before P7; 7B.01 now covers analysis/hardening.
3. **Lead times that are not engineering:** code-signing eligibility (an OSS project with no legal entity may not qualify for Azure Trusted Signing; SignPath Foundation or an OV/EV certificate are alternatives), Apple Developer enrollment, Flathub review, accessibility auditor booking, security-review firm booking, counsel for codec review. *Start 7A.07, 7A.05, 7C.03, 7F.03, 7F.05/06 scheduling in Phase 5–6; otherwise the 7-week phase is not achievable.*
4. **Target architectures are unspecified.** Roadmap/architecture say "universal or arm64" for macOS and no Linux arm64/Windows arm64 position. The CI matrix (§20) lists only Windows x64, macOS arm64, Ubuntu x64. *Proposed: Windows x64, macOS arm64 (+ x64 only if the P0 minimum-OS/Intel decision says so), Linux x64; others post-1.0.* Also minimum OS versions are still open (§24 Q4).
5. **macOS JIT and notarization.** wasmtime (Cranelift JIT) and QuickJS/Chromium need `allow-jit`-class entitlements under hardened runtime; verify in P6 (6A.04 should run signed on macOS in CI) to avoid a late surprise in 7A.03.
6. **Flathub policy risk.** Flathub expects building from source with offline dependencies and discourages bundling a GPL FFmpeg with extra codecs; Electron + Rust + native addon is a heavy manifest. The roadmap's "Flatpak on Flathub" is therefore *uncertain by external review*. *Fallback: self-hosted Flatpak repo (`.flatpakrepo`) published by the project if Flathub review is not complete by RC; treat Flathub acceptance as "submitted + approved or fallback live" for G1.*
7. **WCAG 2.2 AA vs a Canvas2D timeline.** A canvas is opaque to assistive tech and drag-only interactions violate 2.5.7. 7C.02 adds a parallel ARIA outline and full keyboard editing; this is real feature work and a risk to the 55 fps budget. *Confirm "UI chrome" scope = everything except media content/preview/rendered captions, and that the accessible timeline outline counts as the conformant alternative.*
8. **i18n decisions needed:** the six shipped languages (proposed es, pt-BR, fr, de, ja, zh-Hans) + ar for the RTL check; time axis stays LTR in RTL locales; translation platform (Weblate/Crowdin vs PRs); LLM-first translation acceptable if native reviewers sign off (7C.06).
9. **FFmpeg build/license contradiction.** Architecture §21 permits a GPL build with x264/x265; research/03 §8.4 recommends an LGPL base build + optional codec pack. Flatpak uses the freedesktop codec extension. 7F.03 (human) must pick one; 7A.01/7A.05/7F.02 build accordingly. *Suggested: ship GPL build on Windows/Linux packages (consistent with GPL-3.0 app), HW/OS codecs preferred in presets, HEVC encode off by default, Flatpak via codecs extension.*
10. **Perf gates need dedicated hardware.** Rows for iGPU playback, HW export and scrub cannot run on GitHub-hosted runners. *Budget for self-hosted reference machines (Win11 NVIDIA/Intel, M1 Mac, Ubuntu Intel Xe) or 7D.01 cannot produce authoritative numbers.* 7D.02–7D.04 are contingency WPs: if budgets are already green they close in ≤ 1 day each.
11. **Security reviews need a frozen surface.** 7F.05/7F.06 depend on 6J/6B/6A being stable; findings feed fixes (reserve ~10 agent-days contingency, included in 7G.04 scope). External firm vs trusted maintainer is a budget decision.
12. **Update-feed hosting and staged rollout** with static hosting is approximate (client-side buckets); no server-side kill switch except publishing a new feed. *Accept for 1.0, document in 7A.06.*
13. **Plugin docs split with P6:** see P6 Notes #17 (6C.08 writes, 7E.03 publishes).
14. **Totals vs guidance:** Phase 7 decomposes into ~38 WPs vs the 25–35 guidance because each OS target and each review is its own gate; merge 7A.04+7A.05 or 7E.04+7E.05 if fewer tickets are preferred.
