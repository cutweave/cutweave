// SPDX-License-Identifier: GPL-3.0-or-later
import type { SequenceId } from "@cutweave/protocol";
import { IconButton, Menu, type MenuEntry, SegmentedControl, Tooltip } from "@cutweave/ui";
import {
  type DragEvent,
  type KeyboardEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { useEngineStore } from "../engine/engineStore.ts";
import { dragStore } from "../media/dragStore.ts";
import { registerContextSurface } from "../menus/host.tsx";
import { clipLabel } from "../menus/surfaces/facts.ts";
import {
  ASPECT_GUIDES,
  GRID_DIVISIONS,
  guideStore,
  SAFE_AREA_PACK,
  useGuides,
} from "../preview/overlays/guides.ts";
import { buildScene } from "../preview/overlays/scene.ts";
import { TransformOverlay } from "../preview/overlays/TransformOverlay.tsx";
import {
  PreviewCanvas,
  type PreviewPath,
  REDUCED_TEXT,
  usePreviewAspect,
} from "../preview/PreviewCanvas.tsx";
import { sourceKeyAction } from "../preview/sourceKeys.ts";
import { openSequenceSettings, setSequenceAspect } from "../sequences/actions.ts";
import { ASPECT_PRESETS } from "../sequences/presets.ts";
import { clearMarks, placeShownSource, setSourceMark } from "../source/actions.ts";
import { docStore, useDocStore } from "../state/docStore.ts";
import { selectActiveSequenceId } from "../state/selectors.ts";
import { usePreviewFrozen, usePreviewShowsSource } from "../state/sourceViewStore.ts";
import { ASPECTS, type AspectId, useViewStore, viewStore } from "../state/viewStore.ts";
import { transportController } from "../transport/useTransport.ts";
import { EmptyState } from "./EmptyState.tsx";
import { beginSourceDrag, SourceHeader, useSourceAsset } from "./SourceHeader.tsx";
import { SourceScrubBar } from "./SourceScrubBar.tsx";
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

/** The preview's guides menu: what to draw over the picture (WP-2H.06). */
function useGuideItems(): readonly MenuEntry[] {
  const { t } = useTranslation();
  const guides = useGuides((s) => s);
  const set = guideStore.getState().set;
  return [
    {
      id: "thirds",
      label: t("preview.overlay.thirds"),
      checked: guides.thirds,
      keepOpen: true,
      onSelect: () => set({ thirds: !guides.thirds }),
    },
    {
      id: "cross",
      label: t("preview.overlay.cross"),
      checked: guides.cross,
      keepOpen: true,
      onSelect: () => set({ cross: !guides.cross }),
    },
    {
      id: "pixel-ratio",
      label: t("preview.overlay.pixelRatio"),
      checked: guides.pixelRatio,
      keepOpen: true,
      onSelect: () => set({ pixelRatio: !guides.pixelRatio }),
    },
    { kind: "separator", id: "sep-grid" },
    { kind: "label", id: "grid", label: t("preview.overlay.grid") },
    {
      id: "grid-off",
      label: t("preview.overlay.gridOff"),
      checked: guides.grid === null,
      choice: "radio",
      keepOpen: true,
      onSelect: () => set({ grid: null }),
    },
    ...GRID_DIVISIONS.map((n) => ({
      id: `grid-${n}`,
      label: t("preview.overlay.gridN", { n }),
      checked: guides.grid === n,
      choice: "radio" as const,
      keepOpen: true,
      onSelect: () => set({ grid: n }),
    })),
    { kind: "separator", id: "sep-aspect" },
    { kind: "label", id: "aspect", label: t("preview.overlay.aspect") },
    {
      id: "aspect-off",
      label: t("preview.overlay.aspectOff"),
      checked: guides.aspect === null,
      choice: "radio",
      keepOpen: true,
      onSelect: () => set({ aspect: null }),
    },
    ...ASPECT_GUIDES.map((a) => ({
      id: `aspect-${a.id}`,
      label: a.id,
      checked: guides.aspect === a.id,
      choice: "radio" as const,
      keepOpen: true,
      onSelect: () => set({ aspect: a.id }),
    })),
    { kind: "separator", id: "sep-platform" },
    { kind: "label", id: "platform", label: t("preview.overlay.platform") },
    {
      id: "platform-none",
      label: t("preview.overlay.platformNone"),
      checked: guides.platform === null,
      choice: "radio",
      keepOpen: true,
      onSelect: () => set({ platform: null }),
    },
    ...SAFE_AREA_PACK.platforms.map((p) => ({
      id: `platform-${p.id}`,
      label: p.name,
      checked: guides.platform === p.id,
      choice: "radio" as const,
      keepOpen: true,
      onSelect: () => {
        set({ platform: p.id });
        // Choosing a zone to look at turns the safe areas on.
        viewStore.getState().setSafeAreas(true);
      },
    })),
  ];
}

/**
 * The aspect switcher's menu (WP-2B.03): every size preset of the engine, the one the sequence
 * has checked, and Custom size… (Sequence settings, where any width and height can be typed).
 * A preset is one `sequence.set_format`; the clips follow as its `reframe` says (fit).
 */
function useSizeItems(): { readonly sequence: string | null; readonly items: readonly MenuEntry[] } {
  const { t } = useTranslation();
  const sequence = useDocStore((s) => selectActiveSequenceId(s.project));
  // The canvas as one string, so a patch that leaves it alone renders nothing again.
  const size = useDocStore((s) => {
    const format = sequence === null ? undefined : s.project?.sequences[sequence]?.format;
    return format === undefined ? "" : `${format.width}x${format.height}`;
  });
  const items = useMemo<readonly MenuEntry[]>(
    () =>
      sequence === null
        ? []
        : [
            ...ASPECT_PRESETS.map((preset) => ({
              id: `size-${preset.id}`,
              label: t("sequences.presetSize", {
                ratio: preset.ratio,
                width: preset.width,
                height: preset.height,
              }),
              checked: size === `${preset.width}x${preset.height}`,
              choice: "radio" as const,
              onSelect: () => void setSequenceAspect(sequence, preset.id),
            })),
            { kind: "separator" as const, id: "sep-custom" },
            {
              id: "size-custom",
              label: t("preview.sizeCustom"),
              icon: "sliders-horizontal" as const,
              onSelect: () => openSequenceSettings(sequence),
            },
          ],
    [t, sequence, size],
  );
  return { sequence, items };
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
  const cropMode = useGuides((s) => s.cropMode);
  const guideItems = useGuideItems();
  const sizes = useSizeItems();
  const engine = useEngineStore((s) => s.connection !== "none");
  const aspect = usePreviewAspect();
  const [path, setPath] = useState<PreviewPath>({ kind: "pending" });
  const onPath = useCallback((next: PreviewPath) => setPath(next), []);
  const showsSource = usePreviewShowsSource();
  const frozen = usePreviewFrozen();
  const asset = useSourceAsset();

  // The picture's menu (WP-2H.13): the timeline's frame, or the Source view while one plays.
  const stageRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const stage = stageRef.current;
    if (stage === null) return undefined;
    return registerContextSurface(
      stage,
      (query) => {
        if (showsSource) return { surface: "source.view" };
        // On the selected clip's box (or with the keyboard on it): the picture's own menu.
        if (query.source.closest(".cw-xform") !== null) {
          const scene = buildScene(
            docStore.getState().project,
            viewStore.getState().selection,
            viewStore.getState().playhead,
          );
          const ids = scene?.selected.map((l) => l.id) ?? [];
          if (ids.length > 0) {
            return { surface: "preview.item", target: { ids, name: clipLabel(ids[0] ?? "") } };
          }
        }
        return { surface: "preview.canvas" };
      },
      { name: "preview.canvas" },
    );
  }, [showsSource]);

  // The Source view's keys (the keymap's `source` context): Esc goes back to the timeline, I and
  // O mark the source at its playhead, `,` (or Enter) inserts the marked range at the timeline's
  // playhead and `.` overwrites with it. A focused button keeps its Enter.
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!showsSource || e.defaultPrevented) return;
    const action = sourceKeyAction(e);
    if (action === null) return;
    const target = e.target as HTMLElement;
    if (e.key === "Enter" && target.closest("button, [role='slider'], input") !== null) return;
    if (target.closest("input, textarea, [contenteditable='true']") !== null) return;
    e.preventDefault();
    switch (action) {
      case "back":
        void transportController().closeSource();
        return;
      case "insert":
      case "overwrite":
        void placeShownSource(action);
        return;
      case "markIn":
        setSourceMark("in");
        return;
      case "markOut":
        setSourceMark("out");
        return;
      case "clearMarks":
        clearMarks();
        return;
    }
  };

  // Dragging the Source view's picture onto the timeline drops its marked range (the whole asset
  // without marks) like a card.
  const onDragStart = (e: DragEvent<HTMLElement>) =>
    beginSourceDrag(e, showsSource ? asset : null, "both");

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: Esc and Enter of the Source view; the buttons keep their own keys
    <div className="cw-preview" data-source={showsSource} onKeyDown={onKeyDown}>
      {showsSource ? <SourceHeader /> : null}
      <div className="cw-preview__bar" hidden={showsSource}>
        <SegmentedControl<AspectId>
          aria-label={t("preview.aspectLabel")}
          // The sequence's own canvas sets the shape once there is one: the control then shows
          // that shape (none of the presets for a custom one), and a choice gives the sequence
          // that preset's canvas (WP-2B.03). Without a sequence it only shapes the empty frame.
          value={aspect.fromSequence ? aspect.presetId : aspectId}
          onChange={(next) => {
            if (aspect.fromSequence && sizes.sequence !== null) {
              void setSequenceAspect(sizes.sequence as SequenceId, next);
            } else viewStore.getState().setAspect(next);
          }}
          options={ASPECTS.map((a) => ({ value: a.id, label: t(a.labelKey) }))}
        />
        {aspect.fromSequence && sizes.sequence !== null ? (
          <Menu mode="dropdown" items={sizes.items} aria-label={t("preview.sizeMenu")}>
            <Tooltip label={t("preview.sizeMenu")}>
              <IconButton
                icon="chevron-down"
                size="sm"
                label={t("preview.sizeMenu")}
                data-testid="preview-size-menu"
              />
            </Tooltip>
          </Menu>
        ) : null}
        <Tooltip label={t("preview.safeAreas")}>
          <IconButton
            icon="layout-grid"
            size="sm"
            label={t("preview.safeAreas")}
            pressed={safeAreas}
            onClick={() => viewStore.getState().setSafeAreas(!safeAreas)}
          />
        </Tooltip>
        <Menu mode="dropdown" items={guideItems} aria-label={t("preview.overlay.guidesMenu")}>
          <Tooltip label={t("preview.overlay.guides")}>
            <IconButton icon="eye" size="sm" label={t("preview.overlay.guides")} />
          </Tooltip>
        </Menu>
        <Tooltip label={t("preview.overlay.crop")}>
          <IconButton
            icon="crop"
            size="sm"
            label={t("preview.overlay.crop")}
            pressed={cropMode}
            onClick={() => guideStore.getState().set({ cropMode: !cropMode })}
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
          {engine && !showsSource ? <TransformOverlay /> : null}
        </div>
      </div>
      {showsSource ? <SourceScrubBar /> : null}
      <TransportBar />
    </div>
  );
}
