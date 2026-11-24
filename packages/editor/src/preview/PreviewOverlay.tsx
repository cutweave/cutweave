// SPDX-License-Identifier: GPL-3.0-or-later
// The overlay layer over the preview canvas (WP-1M.04, arch §10 step 4): DOM in the same page and
// the same box as the canvas, so it never has z-order trouble and lines up at every size. v1 is
// display only: safe-area guides (toggled from the preview bar), the selected clip's outline and
// a timecode readout. Transform handles and the crop box come later.
import { formatTimecode } from "@cutweave/timeline";
import { type CSSProperties, useMemo } from "react";
import { useDocStore } from "../state/docStore.ts";
import { selectActiveSequenceId } from "../state/selectors.ts";
import { useViewStore } from "../state/viewStore.ts";
import { SAFE_AREAS, selectionOutline } from "./overlay.ts";

const pct = (v: number): string => `${(v * 100).toFixed(4)}%`;

export function PreviewOverlay() {
  const project = useDocStore((s) => s.project);
  const selection = useViewStore((s) => s.selection);
  const safeAreas = useViewStore((s) => s.safeAreas);
  const playhead = useViewStore((s) => s.playhead);
  const outline = useMemo(() => selectionOutline(project, selection), [project, selection]);
  const fps = useMemo(() => {
    const id = selectActiveSequenceId(project);
    return id === null ? undefined : project?.sequences[id]?.format.frame_rate;
  }, [project]);

  const outlineStyle: CSSProperties | undefined =
    outline === null
      ? undefined
      : {
          left: pct(outline.left),
          top: pct(outline.top),
          width: pct(outline.width),
          height: pct(outline.height),
          transformOrigin: `${pct(outline.originX)} ${pct(outline.originY)}`,
          transform: outline.rotation === 0 ? undefined : `rotate(${outline.rotation}deg)`,
        };

  return (
    // Display only: the canvas's accessible name and the transport's timecode carry the meaning.
    <div className="cw-preview__overlay" aria-hidden="true">
      {safeAreas
        ? SAFE_AREAS.map((area) => (
            <div
              key={area.id}
              className="cw-preview__safe"
              data-area={area.id}
              style={{ inset: `${pct(area.inset)} ${pct(area.inset)}` }}
            />
          ))
        : null}
      {outlineStyle === undefined ? null : (
        <div className="cw-preview__selection" style={outlineStyle} />
      )}
      {fps === undefined ? null : (
        <span className="cw-preview__readout timecode">{formatTimecode(playhead, fps, true)}</span>
      )}
    </div>
  );
}
