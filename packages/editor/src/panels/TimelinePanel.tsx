// SPDX-License-Identifier: GPL-3.0-or-later
import {
  DEFAULT_VIEW_STATE,
  followScrollX,
  TimelineCanvas,
  type ViewState,
} from "@cutweave/timeline";
import { Icon, useOwnerWindow } from "@cutweave/ui";
import { memo, useCallback, useEffect, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { useEngineStore } from "../engine/engineStore.ts";
import { useMediaLoaders } from "../media/mediaWiring.ts";
import { loadersForWindow } from "../media/timelineLoaders.ts";
import { usePrefs } from "../prefs/prefsStore.ts";
import { SequenceTabs } from "../sequences/SequenceTabs.tsx";
import { useDocStore } from "../state/docStore.ts";
import { usePlaybackStore } from "../state/playbackStore.ts";
import { selectInOut, selectTimelineModel } from "../state/selectors.ts";
import { useViewStore, viewStore } from "../state/viewStore.ts";
import { InUseMarks } from "../timeline/InUseMarks.tsx";
import { lanesRelaid, lanesShapeOf } from "../timeline/keyframes.ts";
import { timelineDisplay } from "../timeline/prefs.ts";
import { DropGhost, TrackHeaderControls } from "../timeline/TimelineOverlays.tsx";
import { TimelineToolbar } from "../timeline/TimelineToolbar.tsx";
import { TrackRenameInput } from "../timeline/TrackRenameInput.tsx";
import { useKeyframeLanes } from "../timeline/useKeyframeLanes.ts";
import { type TimelineBinding, useTimelineHost } from "../timeline/useTimelineHost.ts";
import { transportController } from "../transport/useTransport.ts";

/** Lanes shown until the engine supplies tracks (WP-1M.05): one video, one audio. */
const PLACEHOLDER_TRACKS = [
  { kind: "video", n: 1 },
  { kind: "audio", n: 1 },
] as const;

/**
 * The engine's current sequence drawn by the `packages/timeline` renderer from `docStore`
 * selectors (it follows every `doc.patched`), with the interactions of WP-2A.10: pointer input
 * goes through the timeline state machine and comes back as engine commands. Video clips draw
 * filmstrips and audio clips waveforms through the engine's media endpoints (WP-1I.03); the
 * sequence's markers and in/out show in the ruler and over the lanes (WP-2A.09). Memoized, as are
 * the overlays in it: a drag changes the document every frame, and only what moved draws again
 * (issue #241).
 */
const EngineTimeline = memo(function EngineTimeline({
  intents,
}: {
  readonly intents: TimelineBinding;
}) {
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
  // The open keyframe lanes (WP-2C.05): view state, drawn from the document.
  const keyframeLanes = useKeyframeLanes(project);
  // Opening or closing a lane moves the tracks below it: once the canvas has laid them out (its
  // effect runs before this one), what is placed over the headers and clips follows.
  const lanesShape = lanesShapeOf(keyframeLanes);
  useEffect(() => lanesRelaid(), [lanesShape]);
  // The sequence's in and out points shade the timeline (markers come with the model).
  const range = selectInOut(project);
  const playhead = useViewStore((s) => s.playhead);
  const zoom = useViewStore((s) => s.zoom);
  const scroll = useViewStore((s) => s.scroll);
  const selection = useViewStore((s) => s.selection);
  const { snapGuides, marquee } = intents.ui;
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
  // Settings > Timeline: how time, thumbnails and waveforms show and what the wheel does.
  const values = usePrefs((s) => s.values);
  const display = useMemo(() => timelineDisplay(values), [values]);
  // "Follow the playhead while playing": the view turns a page, glides, or stays put.
  const follow = values["timeline.follow"];
  const playing = usePlaybackStore((s) => s.playing && s.source === null);
  const host = intents.host;
  useEffect(() => {
    if (!playing || follow === "off") return;
    const range = host.visibleRange();
    if (range === null) return;
    const next = followScrollX(follow, range.t0, range.t1, playhead);
    if (next !== range.t0) viewStore.getState().setScroll(next, viewStore.getState().scroll.y);
  }, [playing, follow, playhead, host]);
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
          keyframeLanes={keyframeLanes}
          viewState={viewState}
          loaders={loaders}
          display={display}
          pan="host"
          onPointer={intents.onPointer}
          onView={intents.onView}
          onViewChange={onViewChange}
          ownerWindow={ownerWindow}
        />
      </div>
      <TrackHeaderControls host={intents.host} />
      <DropGhost host={intents.host} />
      <TrackRenameInput controller={intents.host} />
      <InUseMarks controller={intents.host} />
    </>
  );
});

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
 * pressed anywhere in the panel reach the timeline through the keymap's `timeline` context: the
 * tools, `S` split, `Delete` and `Shift+Delete`, `N` snapping, `M`, `I` and `O`, `Esc` cancels a
 * drag (WP-2A.10).
 */
export function TimelinePanel() {
  const intents = useTimelineHost();
  const connected = useEngineStore((s) => s.connection !== "none");
  const hasSequence = useDocStore(
    (s) => s.project !== null && Object.keys(s.project.sequences).length > 0,
  );
  const live = connected && hasSequence;
  return (
    // Touching the timeline leaves the Source view: the engine plays the sequence again (WP-1M.15).
    <div
      className="cw-timeline"
      ref={live ? intents.keyTarget : null}
      data-tool={intents.ui.tool}
      data-state={intents.ui.state}
      onPointerDownCapture={() => void transportController().closeSource()}
    >
      {/* One tab per sequence (WP-2B.03); it renders only when the list or the choice changes. */}
      {live ? <SequenceTabs /> : null}
      <TimelineToolbar
        host={intents.host}
        tool={intents.ui.tool}
        linkedSelection={intents.ui.linkedSelection}
        live={live}
      />
      <div className="cw-timeline__body" {...(live ? intents.drop : {})}>
        {live ? <EngineTimeline intents={intents} /> : <SampleLanes />}
      </div>
    </div>
  );
}
