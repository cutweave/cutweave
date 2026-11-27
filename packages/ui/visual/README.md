<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# UI visual regression, axe and fonts (WP-0K.05)

`just ui-snapshots` builds Storybook (`storybook-static/`) and runs Playwright against it:

| Spec              | What                                                                                                                                                                                                                                                                           |
| ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `visual.spec.ts`  | Every story, dark and light, 1280x800 viewport, full-page Chromium screenshot with software rendering. Compared to `__screenshots__/linux-chromium-sw/<theme>/<story-id>.png` (Git LFS) with SSIM >= 0.995 and PSNR >= 40 dB (docs/01-architecture.md section 20). Linux only. |
| `a11y.spec.ts`    | `@axe-core/playwright` on every story in both themes; any violation fails.                                                                                                                                                                                                     |
| `density.spec.ts` | The editor shell and the mixed-tracks timeline in both densities and themes at 100% and 125% (a 1024x640 viewport at device scale 1.25), as `<story-id>@<density>-<scale>.png`; axe on every story in Comfortable (WP-2H.09).                                                  |
| `fonts.spec.ts`   | Network blocked; `document.fonts.check()` is true for Instrument Sans, Bricolage Grotesque and JetBrains Mono.                                                                                                                                                                 |
| `compare.spec.ts` | Unit tests of the SSIM/PSNR comparator.                                                                                                                                                                                                                                        |

Stories come from `src/**/*.stories.tsx`; new ones are picked up automatically.

## Changing a story's look on purpose

Apply the `ui-snapshots-update` label to the PR. CI rewrites the baselines, uploads them as the
`ui-baselines` artifact and fails until they are committed: download the artifact, copy
`linux-chromium-sw/` into `packages/ui/__screenshots__/`, commit, push. The next run is green.
Baselines must come from Linux CI; `just ui-snapshots --update` refuses to run elsewhere.
On a failure CI uploads `ui-visual-failures` (actual and diff images).

## Bootstrap (first run, no baselines committed yet)

While `__screenshots__/` holds no PNG, the visual spec writes every baseline instead of failing,
and CI warns. Open the first `ui` run, download `ui-baselines`, commit its contents under
`packages/ui/__screenshots__/` (Git LFS tracks it via `.gitattributes`). From then on a missing
or changed baseline fails the job.
