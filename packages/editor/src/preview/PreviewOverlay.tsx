// SPDX-License-Identifier: GPL-3.0-or-later
// The overlay layer over the preview canvas (WP-1M.04, arch §10 step 4): DOM in the same page and
// the same box as the canvas, so it never has z-order trouble and lines up at every size. It is
// the guides (WP-2H.06): safe areas (the generic action and title zones, plus the zone of a
// platform's own UI), the rule-of-thirds and custom grids, the center cross, an aspect-ratio
// frame and the zoom readout, and a timecode readout. The clip's transform handles are
// `TransformOverlay`, which may draw outside the frame; this layer is clipped to it.
import { formatTimecode } from "@cutweave/timeline";
import { useOwnerWindow } from "@cutweave/ui";
import { type CSSProperties, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { observeSize } from "../shell/windows.ts";
import { useDocStore } from "../state/docStore.ts";
import { selectActiveSequenceId } from "../state/selectors.ts";
import { useViewStore } from "../state/viewStore.ts";
import {
  ASPECT_GUIDES,
  aspectFrame,
  type Inset,
  platformSafeArea,
  SAFE_AREA_PACK,
  useGuides,
} from "./overlays/guides.ts";

const pct = (v: number): string => `${(v * 100).toFixed(4)}%`;

const insetStyle = (i: Inset): CSSProperties => ({
  top: pct(i.top),
  right: pct(i.right),
  bottom: pct(i.bottom),
  left: pct(i.left),
});

/** The tick positions of an `n`-division grid, without the two edges: `1/n .. (n-1)/n`. */
const divisions = (n: number): number[] => Array.from({ length: n - 1 }, (_, i) => (i + 1) / n);

export function PreviewOverlay() {
  const { t } = useTranslation();
  const safeAreas = useViewStore((s) => s.safeAreas);
  const playhead = useViewStore((s) => s.playhead);
  const { platform, thirds, cross, grid, aspect, pixelRatio } = useGuides((s) => s);
  // The format, not the document: a clip dragged on the timeline changes it on every update (#244).
  const format = useDocStore((s) => {
    const id = selectActiveSequenceId(s.project);
    return id === null ? undefined : s.project?.sequences[id]?.format;
  });
  const rootRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  // Observed from its own window (a detached Preview's), again when it moves.
  const ownerWindow = useOwnerWindow();
  useEffect(() => {
    const el = rootRef.current;
    if (el === null || !pixelRatio || ownerWindow === null) return undefined;
    const read = () => setWidth(el.getBoundingClientRect().width);
    read();
    return observeSize(el, read);
  }, [pixelRatio, ownerWindow]);

  const fps = format?.frame_rate;
  const canvasRatio =
    format !== undefined && format.height > 0 ? format.width / format.height : 16 / 9;
  const platformZone = safeAreas ? platformSafeArea(platform) : undefined;
  const aspectGuide = ASPECT_GUIDES.find((a) => a.id === aspect);
  const zoom = format !== undefined && format.width > 0 && width > 0 ? width / format.width : 0;
  // The density of the display the Preview's window is on.
  const dpr = ownerWindow?.devicePixelRatio || 1;

  return (
    // Display only: the canvas's accessible name and the transport's timecode carry the meaning.
    <div ref={rootRef} className="cw-preview__overlay" aria-hidden="true">
      {safeAreas
        ? SAFE_AREA_PACK.generic.map((area) => (
            <div
              key={area.id}
              className="cw-preview__safe"
              data-area={area.id}
              style={{ inset: `${pct(area.inset)} ${pct(area.inset)}` }}
            />
          ))
        : null}
      {platformZone === undefined ? null : (
        <div
          className="cw-preview__safe cw-preview__safe--platform"
          data-area={platformZone.id}
          style={insetStyle(platformZone.insets)}
        />
      )}
      {thirds
        ? [1 / 3, 2 / 3].flatMap((f) => [
            <div
              key={`tx${f}`}
              className="cw-preview__line"
              data-axis="x"
              style={{ left: pct(f) }}
            />,
            <div
              key={`ty${f}`}
              className="cw-preview__line"
              data-axis="y"
              style={{ top: pct(f) }}
            />,
          ])
        : null}
      {grid === null
        ? null
        : divisions(grid).flatMap((f) => [
            <div
              key={`gx${f}`}
              className="cw-preview__line"
              data-axis="x"
              data-grid=""
              style={{ left: pct(f) }}
            />,
            <div
              key={`gy${f}`}
              className="cw-preview__line"
              data-axis="y"
              data-grid=""
              style={{ top: pct(f) }}
            />,
          ])}
      {cross ? (
        <>
          <div className="cw-preview__line" data-axis="x" data-cross="" style={{ left: "50%" }} />
          <div className="cw-preview__line" data-axis="y" data-cross="" style={{ top: "50%" }} />
        </>
      ) : null}
      {aspectGuide === undefined ? null : (
        <div
          className="cw-preview__safe cw-preview__safe--aspect"
          data-aspect={aspectGuide.id}
          style={insetStyle(aspectFrame(canvasRatio, aspectGuide.ratio))}
        />
      )}
      {pixelRatio && zoom > 0 ? (
        <span className="cw-preview__readout cw-preview__readout--zoom caption numeric">
          {t("preview.overlay.pixelReadout", {
            zoom: Math.round(zoom * 100),
            ratio: (1 / (zoom * dpr)).toFixed(2),
          })}
        </span>
      ) : null}
      {fps === undefined ? null : (
        <span className="cw-preview__readout timecode">{formatTimecode(playhead, fps, true)}</span>
      )}
    </div>
  );
}
