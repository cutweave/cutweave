// SPDX-License-Identifier: GPL-3.0-or-later
// The Source view on the reduced (JPEG) preview path (WP-1M.15). The engine's frame ring is what
// shows a playing source, and this path has no ring, so it draws still frames of the asset
// instead: `/media/{id}/thumb` at the source's playhead, latest wins, at most one request per
// animation frame. The sound still plays; the picture is a few frames a second, which the
// reduced-preview banner already says.
import { useEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { LatestFrames } from "../media/scrub.ts";
import { useScrubSource } from "../media/scrubSource.ts";
import { usePlaybackStore } from "../state/playbackStore.ts";

/** Widest still asked for, px. */
const MAX_WIDTH = 1280;

export function SourceStills() {
  const { t } = useTranslation();
  const scrub = useScrubSource();
  const media = usePlaybackStore((s) => s.source?.media ?? null);
  const at = usePlaybackStore((s) => s.sourcePlayhead);
  const canvasRef = useRef<HTMLCanvasElement>(null);

  const frames = useMemo(
    () =>
      new LatestFrames<ImageBitmap>({
        load: async (when, signal) => {
          if (media === null) return null;
          const width = Math.min(MAX_WIDTH, canvasRef.current?.clientWidth || MAX_WIDTH);
          return (await scrub?.frame(media, when, width, signal)) ?? null;
        },
        paint: (_when, bitmap) => {
          const canvas = canvasRef.current;
          const ctx = canvas?.getContext("2d") ?? null;
          if (canvas !== null && ctx !== null) {
            if (canvas.width !== bitmap.width) canvas.width = bitmap.width;
            if (canvas.height !== bitmap.height) canvas.height = bitmap.height;
            ctx.drawImage(bitmap, 0, 0);
            canvas.dataset.frame = "ready";
          }
          bitmap.close();
        },
      }),
    [scrub, media],
  );
  useEffect(() => {
    frames.request(at);
  }, [frames, at]);
  useEffect(() => () => frames.cancel(), [frames]);

  return (
    <canvas
      ref={canvasRef}
      className="cw-preview__stills"
      role="img"
      aria-label={t("preview.source.stills")}
      data-testid="source-stills"
    />
  );
}
