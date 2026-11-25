// SPDX-License-Identifier: GPL-3.0-or-later
import { IconButton, SegmentedControl, Tooltip } from "@cutweave/ui";
import {
  type DragEvent,
  type KeyboardEvent,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { useEngineStore } from "../engine/engineStore.ts";
import { dragStore, MEDIA_DRAG_TYPE } from "../media/dragStore.ts";
import { registerContextSurface } from "../menus/host.tsx";
import {
  PreviewCanvas,
  type PreviewPath,
  REDUCED_TEXT,
  usePreviewAspect,
} from "../preview/PreviewCanvas.tsx";
import { sourceKeyAction } from "../preview/sourceKeys.ts";
import { usePreviewFrozen, usePreviewShowsSource } from "../state/sourceViewStore.ts";
import { ASPECTS, type AspectId, useViewStore, viewStore } from "../state/viewStore.ts";
import { transportController } from "../transport/useTransport.ts";
import { EmptyState } from "./EmptyState.tsx";
import { SourceHeader, useInsertSource, useSourceAsset } from "./SourceHeader.tsx";
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
  const showsSource = usePreviewShowsSource();
  const frozen = usePreviewFrozen();
  const asset = useSourceAsset();
  const insertSource = useInsertSource();

  // The picture's menu (WP-2H.13): the timeline's frame, or the Source view while one plays.
  const stageRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const stage = stageRef.current;
    if (stage === null) return undefined;
    return registerContextSurface(
      stage,
      () => ({ surface: showsSource ? "source.view" : "preview.canvas" }),
      { name: "preview.canvas" },
    );
  }, [showsSource]);

  // Esc goes back to the timeline and Enter places the clip (a focused button keeps its Enter).
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!showsSource || e.defaultPrevented) return;
    const action = sourceKeyAction(e);
    if (action === null) return;
    const target = e.target as HTMLElement;
    if (action === "insert" && target.closest("button, [role='slider'], input") !== null) return;
    e.preventDefault();
    if (action === "back") void transportController().closeSource();
    else insertSource();
  };

  // Dragging the Source view's picture onto the timeline drops the whole asset like a card.
  const onDragStart = (e: DragEvent<HTMLElement>) => {
    if (!showsSource || asset === null) {
      e.preventDefault();
      return;
    }
    e.dataTransfer.setData(MEDIA_DRAG_TYPE, asset.id);
    e.dataTransfer.effectAllowed = "copy";
    dragStore.getState().begin({ id: asset.id, kind: asset.kind, duration: asset.duration });
  };

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: Esc and Enter of the Source view; the buttons keep their own keys
    <div className="cw-preview" data-source={showsSource} onKeyDown={onKeyDown}>
      {showsSource ? <SourceHeader /> : null}
      <div className="cw-preview__bar" hidden={showsSource}>
        <SegmentedControl<AspectId>
          aria-label={t("preview.aspectLabel")}
          // The sequence's own canvas sets the shape once there is one: the control then shows
          // that shape (none of the presets for a custom one) and is off.
          value={aspect.fromSequence ? aspect.presetId : aspectId}
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
      <div
        ref={stageRef}
        className="cw-preview__stage"
        {...(showsSource && {
          role: "group",
          tabIndex: 0,
          "aria-label": t("preview.source.frameLabel", { name: asset?.name ?? "" }),
        })}
      >
        {/* biome-ignore lint/a11y/noStaticElementInteractions: the Source view's picture is a drag source for the timeline; the keyboard places it with Enter */}
        <div
          className="cw-preview__frame"
          style={aspect.style}
          draggable={showsSource}
          onDragStart={onDragStart}
          onDragEnd={() => dragStore.getState().end()}
        >
          {engine ? (
            <PreviewCanvas onPath={onPath} source={showsSource} frozen={frozen} />
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
