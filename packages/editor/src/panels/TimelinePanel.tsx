// SPDX-License-Identifier: GPL-3.0-or-later
import { DEFAULT_VIEW_STATE, TimelineCanvas, type ViewState } from "@cutweave/timeline";
import { Icon, IconButton, Toolbar, ToolbarSeparator, Tooltip, useOwnerWindow } from "@cutweave/ui";
import { useCallback, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { useEngineStore } from "../engine/engineStore.ts";
import { useMediaLoaders } from "../media/mediaWiring.ts";
import { loadersForWindow } from "../media/timelineLoaders.ts";
import { useDocStore } from "../state/docStore.ts";
import { selectInOut, selectTimelineModel } from "../state/selectors.ts";
import { useViewStore, viewStore } from "../state/viewStore.ts";
import { InUseMarks } from "../timeline/InUseMarks.tsx";
import { TrackRenameInput } from "../timeline/TrackRenameInput.tsx";
import { type TimelineIntents, useTimelineIntents } from "../timeline/useTimelineIntents.ts";
import { transportController } from "../transport/useTransport.ts";

/** Lanes shown until the engine supplies tracks (WP-1M.05): one video, one audio. */
const PLACEHOLDER_TRACKS = [
  { kind: "video", n: 1 },
  { kind: "audio", n: 1 },
] as const;

/**
 * The engine's current sequence drawn by the `packages/timeline` renderer from `docStore`
 * selectors (it follows every `doc.patched`), with the interactions of WP-1M.06: pointer input
 * goes through the timeline state machine and comes back as engine commands. Video clips draw
 * filmstrips and audio clips waveforms through the engine's media endpoints (WP-1I.03); the
 * sequence's markers and in/out show in the ruler and over the lanes (WP-2A.09).
 */
function EngineTimeline({ intents }: { readonly intents: TimelineIntents }) {
  const { t } = useTranslation();
  const project = useDocStore((s) => s.project);
  const mediaLoaders = useMediaLoaders();
  // Detached into its own window (or docked back): the canvas observes and draws there.
  const ownerWindow = useOwnerWindow();
  // Filmstrip frames are sized for the density of the window the canvas is in (#191).
  const loaders = useMemo(
    () => loadersForWindow(mediaLoaders, () => ownerWindow?.devicePixelRatio ?? 1),
    [mediaLoaders, ownerWindow],
  );
  const model = useMemo(() => selectTimelineModel(project), [project]);
  // The sequence's in and out points shade the timeline (markers come with the model).
  const range = selectInOut(project);
  const playhead = useViewStore((s) => s.playhead);
  const zoom = useViewStore((s) => s.zoom);
  const scroll = useViewStore((s) => s.scroll);
  const selection = useViewStore((s) => s.selection);
  const { snapGuides, marquee } = intents.overlay;
  const viewState = useMemo<Partial<ViewState>>(
    () => ({
      pxPerSec: zoom ?? DEFAULT_VIEW_STATE.pxPerSec,
      scrollX: scroll.x,
      scrollY: scroll.y,
      playhead,
      selection: new Set(selection.filter((ref) => ref.kind === "item").map((ref) => ref.id)),
      snapGuides,
      marquee,
      range,
    }),
    [zoom, scroll, playhead, selection, snapGuides, marquee, range],
  );
  // Zoom and scroll the user caused (wheel, drag) are view state: `viewSync` mirrors them.
  const onViewChange = useCallback((v: ViewState) => {
    const store = viewStore.getState();
    store.setZoom(v.pxPerSec);
    store.setScroll(v.scrollX, v.scrollY);
  }, []);
  return (
    <>
      {/* `display: contents`: no box of its own, it only marks the canvas for the menu host. */}
      <div ref={intents.canvasSurface} style={{ display: "contents" }}>
        <TimelineCanvas
          className="cw-timeline__canvas"
          aria-label={t("timeline.canvasLabel")}
          model={model}
          viewState={viewState}
          loaders={loaders}
          pan="host"
          onPointer={intents.onPointer}
          onView={intents.onView}
          onViewChange={onViewChange}
          ownerWindow={ownerWindow}
        />
      </div>
      <TrackRenameInput controller={intents.controller} />
      <InUseMarks controller={intents.controller} />
    </>
  );
}

/** The sample track area shown while no engine supplies a sequence. */
function SampleLanes() {
  const { t } = useTranslation();
  return (
    <>
      <ul className="cw-timeline__lanes" aria-label={t("timeline.lanesLabel")}>
        {PLACEHOLDER_TRACKS.map(({ kind, n }) => (
          <li key={`${kind}-${n}`} className="cw-timeline__lane" data-kind={kind}>
            <span className="cw-timeline__head label">
              <Icon name={kind === "video" ? "film" : "audio-waveform"} size="md" />
              {kind === "video" ? t("timeline.trackVideo", { n }) : t("timeline.trackAudio", { n })}
            </span>
          </li>
        ))}
      </ul>
      <div className="cw-timeline__playhead" aria-hidden="true" />
      <p className="cw-timeline__hint body">
        <span className="body-strong">{t("timeline.emptyTitle")}</span>
        {t("timeline.emptyBody")}
      </p>
    </>
  );
}

/**
 * Bottom dock panel: timeline tools and the track area. Connected to an engine with a sequence
 * it shows that sequence and edits it; without an engine it keeps the empty sample lanes. Keys
 * pressed anywhere in the panel reach the timeline: `S` split at playhead, `Delete`/`Backspace`
 * delete the selection, `N` snapping, `Esc` cancels a drag (WP-1M.06).
 */
export function TimelinePanel() {
  const { t } = useTranslation();
  const intents = useTimelineIntents();
  const snapping = useViewStore((s) => s.snapping);
  const connected = useEngineStore((s) => s.connection !== "none");
  const hasSequence = useDocStore(
    (s) => s.project !== null && Object.keys(s.project.sequences).length > 0,
  );
  const live = connected && hasSequence;
  const { controller } = intents;
  return (
    // Touching the timeline leaves the Source view: the engine plays the sequence again (WP-1M.15).
    <div
      className="cw-timeline"
      ref={live ? intents.keyTarget : null}
      onPointerDownCapture={() => void transportController().closeSource()}
    >
      <Toolbar aria-label={t("timeline.toolbarLabel")} className="cw-timeline__tools">
        <Tooltip label={t("timeline.select")}>
          <IconButton icon="mouse-pointer-2" label={t("timeline.select")} pressed disabled />
        </Tooltip>
        <Tooltip label={t("timeline.razor")}>
          <IconButton icon="razor" label={t("timeline.razor")} pressed={false} disabled />
        </Tooltip>
        <Tooltip label={t("timeline.ripple")}>
          <IconButton icon="ripple" label={t("timeline.ripple")} pressed={false} disabled />
        </Tooltip>
        <ToolbarSeparator />
        <Tooltip label={t("timeline.split")} keys={["S"]}>
          <IconButton
            icon="scissors"
            label={t("timeline.split")}
            disabled={!live}
            onClick={() => controller.split()}
          />
        </Tooltip>
        <ToolbarSeparator />
        <Tooltip label={t("timeline.snapping")} keys={["N"]}>
          <IconButton
            icon="magnet"
            label={t("timeline.snapping")}
            pressed={snapping}
            disabled={!live}
            onClick={() => controller.toggleSnapping()}
          />
        </Tooltip>
        <ToolbarSeparator />
        <Tooltip label={t("timeline.zoomOut")}>
          <IconButton icon="zoom-out" label={t("timeline.zoomOut")} disabled />
        </Tooltip>
        <Tooltip label={t("timeline.zoomIn")}>
          <IconButton icon="zoom-in" label={t("timeline.zoomIn")} disabled />
        </Tooltip>
      </Toolbar>
      <div className="cw-timeline__body" {...(live ? intents.drop : {})}>
        {live ? <EngineTimeline intents={intents} /> : <SampleLanes />}
      </div>
    </div>
  );
}
