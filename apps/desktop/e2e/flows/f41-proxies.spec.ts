// SPDX-License-Identifier: GPL-3.0-or-later
// F41 Proxies for 4K media (docs/user-flows.md; WP-2F.01): import the corpus 4K long-GOP clip
// (an 8 s GOP, far over the scrub budget on its own). The engine indexes it and makes a proxy
// in the background (a 960×540 ProRes Proxy in the cache). Place it and drag the playhead
// along the ruler like a person scrubbing: the preview's picture follows the pointer while it
// moves (scrub frames come from the proxy) and settles on the frame under the playhead when
// the pointer stops (the full-quality frame from the original).
//
// Needs an engine built with FFmpeg (`just e2e-flows` builds it when the bundle is set up) and
// the fetched corpus (`just corpus`); without either the flow skips and says why. The proxy's
// state is read back through the engine (`media.proxy_status`, `job.list`): no screen shows it
// before WP-2F.03.
import { createHash } from "node:crypto";
import type { Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import { copyCorpusClips, corpusDir } from "./export-helpers.ts";
import {
  createProject,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  launchFlowApp,
  layout,
  mediaCard,
  mediaCards,
  pickFiles,
  rulerY,
  sec,
  snapshot,
  timecode,
  timelineGeometry,
  workspace,
  xAt,
} from "./helpers.ts";

const CLIP = "h264_4k30_longgop.mp4";

interface ProxyView {
  readonly status: string;
  readonly profile: string | null;
  readonly size: [number, number] | null;
  readonly wanted: boolean;
}

/** A hash of the preview's upper half (the corpus test pattern moves there). */
async function previewHash(page: Page): Promise<string> {
  const box = await page.locator("canvas.cw-preview__canvas").boundingBox();
  if (box === null) throw new Error("no preview canvas");
  const png = await page.screenshot({
    clip: {
      x: Math.round(box.x + 4),
      y: Math.round(box.y + 4),
      width: Math.round(box.width - 8),
      height: Math.round(box.height / 2 - 4),
    },
    animations: "allow",
    caret: "initial",
  });
  return createHash("sha1").update(png).digest("hex");
}

/** Resolves once `read` gives the same answer twice in a row. */
async function settled<T>(read: () => Promise<T>): Promise<T> {
  let last: T | undefined;
  let value: T | undefined;
  await expect
    .poll(
      async () => {
        value = await read();
        const same = value === last;
        last = value;
        return same;
      },
      { intervals: [150, 250, 400] },
    )
    .toBe(true);
  return value as T;
}

test.describe("F41 proxies for 4K media", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("a 4K long-GOP clip gets a proxy and scrubbing the ruler follows the pointer", async () => {
    test.setTimeout(300_000);
    const info = test.info();
    const corpus = corpusDir();
    test.skip(corpus === null, "the corpus is not fetched (just corpus)");
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work, undefined, { realMedia: true });
    try {
      const caps = await editorCall<{ available?: boolean }>(page, "media.capabilities");
      test.skip(
        caps.available !== true,
        "this engine has no media backend; build it with --features cutweave-engine/ffmpeg",
      );
      await createProject(app, page, "Proxies", work.dir);

      // Import the 4K clip with the bin's Import button.
      const files = copyCorpusClips(corpus ?? "", work.media, [CLIP]);
      await pickFiles(app, files);
      await page.getByRole("button", { name: "Import", exact: true }).click();
      await expect(mediaCards(page)).toHaveCount(1);
      await expect(mediaCard(page, CLIP)).toContainText("3840×2160");

      // The engine makes its proxy in the background: a proxy job, then a ready proxy.
      const media = await editorCall<{ media: { id: string }[] }>(page, "media.list");
      const id = media.media[0]?.id;
      if (id === undefined) throw new Error("the clip is not in the bin");
      await expect
        .poll(
          async () => {
            const out = await editorCall<{ proxies: ProxyView[] }>(page, "media.proxy_status", {
              media: [id],
            });
            return out.proxies[0]?.status;
          },
          { timeout: 180_000, message: "the clip's proxy" },
        )
        .toBe("ready");
      const status = await editorCall<{ proxies: ProxyView[] }>(page, "media.proxy_status", {
        media: [id],
      });
      expect(status.proxies[0]?.wanted).toBe(true);
      expect(status.proxies[0]?.profile).toBe("prores_proxy-960");
      expect(status.proxies[0]?.size).toEqual([960, 540]);
      const { jobs } = await editorCall<{ jobs: { label: string; state: string }[] }>(
        page,
        "job.list",
        { media_id: id, kind: "proxy" },
      );
      expect(jobs.map((j) => `${j.label}:${j.state}`)).toEqual(["proxy:succeeded"]);
      const cache = await editorCall<{ kinds: { kind: string; entries: number }[] }>(
        page,
        "media.cache_status",
      );
      expect(cache.kinds.find((k) => k.kind === "proxies")?.entries).toBeGreaterThanOrEqual(1);

      // Place it on V1 and wait for the preview to stream frames.
      await dropCard(page, CLIP, "V1", 0);
      await expect.poll(async () => (await layout(page))[0]?.clips.length ?? 0).toBe(1);
      await editorCaughtUp(page);
      const surface = page.locator(".cw-preview__surface");
      await expect(surface).toHaveAttribute("data-path", /^(gpu|jpeg)$/, { timeout: 20_000 });
      const g = await timelineGeometry(page);
      await page.mouse.click(xAt(g, sec(1)), rulerY(g));
      await expect(timecode(page)).toHaveText("00:00:01:00");
      const atOne = await settled(() => previewHash(page));

      // Scrub: press on the ruler and drag to 6 s in small steps, looking at the preview on
      // the way. The picture follows the pointer while it moves.
      await page.mouse.move(xAt(g, sec(1)), rulerY(g));
      await page.mouse.down();
      const seen = new Set<string>([atOne]);
      for (let s = 1.5; s <= 6; s += 0.5) {
        await page.mouse.move(xAt(g, sec(s)), rulerY(g), { steps: 3 });
        await expect.poll(() => previewHash(page), { timeout: 10_000 }).not.toBe(atOne);
        seen.add(await previewHash(page));
      }
      await page.mouse.up();
      expect(seen.size, "pictures seen while scrubbing").toBeGreaterThanOrEqual(4);
      await snapshot(page, info, "scrubbed");

      // Stopped at 6 s: the picture settles (the full-quality frame), and clicking the ruler
      // there again shows the same frame.
      await expect(timecode(page)).toHaveText("00:00:06:00");
      const atSix = await settled(() => previewHash(page));
      expect(atSix).not.toBe(atOne);
      await page.mouse.click(xAt(g, sec(1)), rulerY(g));
      await expect(timecode(page)).toHaveText("00:00:01:00");
      expect(await settled(() => previewHash(page))).toBe(atOne);
      await page.mouse.click(xAt(g, sec(6)), rulerY(g));
      await expect(timecode(page)).toHaveText("00:00:06:00");
      expect(await settled(() => previewHash(page))).toBe(atSix);
    } finally {
      await close();
      work.remove();
    }
  });
});
