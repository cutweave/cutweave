// SPDX-License-Identifier: GPL-3.0-or-later
import { IconButton, SegmentedControl, Tooltip } from "@cutweave/ui";
import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { useEngineStore } from "../engine/engineStore.ts";
import {
  PreviewCanvas,
  type PreviewPath,
  REDUCED_TEXT,
  usePreviewAspect,
} from "../preview/PreviewCanvas.tsx";
import { ASPECTS, type AspectId, useViewStore, viewStore } from "../state/viewStore.ts";
import { EmptyState } from "./EmptyState.tsx";
import { TransportBar } from "./TransportBar.tsx";

/** Says the preview runs on still frames, and why (WP-1M.04 fallback). */
function ReducedBanner({ path }: { readonly path: PreviewPath }) {
  const { t } = useTranslation();
  if (path.kind !== "jpeg") return null;
  return (
    <div className="cw-banner cw-preview__banner" role="status">
      <span className="cw-banner__text">
        <span className="body-strong">{t("preview.reducedTitle")}</span>
        <span className="body">{t(REDUCED_TEXT[path.reason])}</span>
      </span>
    </div>
  );
}

/**
 * Centre dock panel: the preview bar (aspect, safe areas), the frame at the playhead on
 * `surface-preview` with its overlay, and the transport bar (WP-1M.04). Without an engine it
 * keeps the empty frame.
 */
export function PreviewPanel() {
  const { t } = useTranslation();
  const aspectId = useViewStore((s) => s.aspect);
  const safeAreas = useViewStore((s) => s.safeAreas);
  const engine = useEngineStore((s) => s.connection !== "none");
  const aspect = usePreviewAspect();
  const [path, setPath] = useState<PreviewPath>({ kind: "pending" });
  const onPath = useCallback((next: PreviewPath) => setPath(next), []);
  return (
    <div className="cw-preview">
      <div className="cw-preview__bar">
        <SegmentedControl<AspectId>
          aria-label={t("preview.aspectLabel")}
          value={aspectId}
          // The sequence's own canvas sets the shape once there is one.
          disabled={aspect.fromSequence}
          onChange={(next) => viewStore.getState().setAspect(next)}
          options={ASPECTS.map((a) => ({ value: a.id, label: t(a.labelKey) }))}
        />
        <Tooltip label={t("preview.safeAreas")}>
          <IconButton
            icon="layout-grid"
            size="sm"
            label={t("preview.safeAreas")}
            pressed={safeAreas}
            onClick={() => viewStore.getState().setSafeAreas(!safeAreas)}
          />
        </Tooltip>
      </div>
      {engine ? <ReducedBanner path={path} /> : null}
      <div className="cw-preview__stage">
        <div className="cw-preview__frame" style={aspect.style}>
          {engine ? (
            <PreviewCanvas onPath={onPath} />
          ) : (
            <EmptyState
              icon="file-video"
              title={t("preview.emptyTitle")}
              body={t("preview.emptyBody")}
            />
          )}
        </div>
      </div>
      <TransportBar />
    </div>
  );
}
