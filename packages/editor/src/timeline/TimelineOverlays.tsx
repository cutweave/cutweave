// SPDX-License-Identifier: GPL-3.0-or-later
// What the host lays over the timeline canvas (WP-2A.10): the track headers' toggles as
// design-system buttons with tooltips (lock, mute, hide; the header's menu has the rest), the
// "Add track" button in the corner above the headers, and the drop ghost of a bin card (where it
// lands and how: insert, overwrite or a new track; marigold, the "here" colour) or the clip an
// Alt-drop replaces.
import type { DropPreview, Rect, TimelineTrack } from "@cutweave/timeline";
import { IconButton, Menu, type MenuEntry, Tooltip } from "@cutweave/ui";
import {
  type ReactElement,
  type RefObject,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import type { TKey } from "../i18n/index.ts";
import { useDocStore } from "../state/docStore.ts";
import { selectTimelineModel } from "../state/selectors.ts";
import { useViewStore } from "../state/viewStore.ts";
import type { TimelineHost } from "./host.ts";

/** The header toggles in the order the canvas draws them (`HEADER_TOGGLES`). */
const TOGGLES = [
  { flag: "lock", field: "locked", icon: "lock", on: "lockTrack", off: "unlockTrack" },
  { flag: "mute", field: "muted", icon: "volume-x", on: "muteTrack", off: "unmuteTrack" },
  { flag: "hide", field: "hidden", icon: "eye-off", on: "hideTrack", off: "showTrack" },
] as const;

/** Length tokens the canvas lays its headers out with, in CSS px. */
interface Metrics {
  readonly space1: number;
  readonly space2: number;
  readonly controlSm: number;
  readonly ruler: number;
}

const ZERO: Metrics = { space1: 0, space2: 0, controlSm: 0, ruler: 0 };

function readMetrics(el: Element): Metrics {
  const win = el.ownerDocument.defaultView;
  if (win === null) return ZERO;
  const cs = win.getComputedStyle(el);
  const px = (name: string): number => Number.parseFloat(cs.getPropertyValue(name)) || 0;
  return {
    space1: px("--space-1"),
    space2: px("--space-2"),
    controlSm: px("--control-sm"),
    ruler: px("--control-md"),
  };
}

/**
 * The density of the window `el` is in (`data-density` on its root, WP-2H.09). The size tokens
 * are other values in another density, and the canvas lays its headers out again: what is
 * placed over them reads the tokens again and follows.
 */
function useDensity(el: RefObject<Element | null>): string {
  const [density, setDensity] = useState("");
  useEffect(() => {
    const root = el.current?.ownerDocument.documentElement;
    const view = root?.ownerDocument.defaultView;
    if (root === undefined || view === null || view === undefined) return;
    const read = (): void => setDensity(root.getAttribute("data-density") ?? "");
    read();
    const observer = new view.MutationObserver(read);
    observer.observe(root, { attributes: true, attributeFilter: ["data-density"] });
    return () => observer.disconnect();
  }, [el]);
  return density;
}

/** Re-renders when what moves the canvas changes: scroll, zoom, the document. */
function useCanvasFollow(): void {
  useViewStore((s) => s.scroll);
  useViewStore((s) => s.zoom);
  useDocStore((s) => s.project);
}

function HeaderToggles({
  host,
  track,
  rect,
  m,
}: {
  readonly host: TimelineHost;
  readonly track: TimelineTrack;
  readonly rect: Rect;
  readonly m: Metrics;
}): ReactElement {
  const { t } = useTranslation();
  // The canvas puts its toggles right of the name, centred in the lane (`geometry.toggleRect`).
  const left = rect.x + rect.w + m.space2;
  const top = rect.y + (rect.h - m.controlSm) / 2 - m.ruler;
  return (
    <div className="cw-timeline__toggles" data-track={track.id} style={{ left, top }}>
      {TOGGLES.map((toggle) => {
        const on = track[toggle.field] === true;
        const label = t(`contextMenu.item.${on ? toggle.off : toggle.on}` as TKey);
        return (
          <Tooltip key={toggle.flag} label={label}>
            <IconButton
              size="sm"
              icon={toggle.icon}
              label={t("timeline.header.toggle", { action: label, name: track.name })}
              pressed={on}
              onClick={() => host.toggleTrack(track.id, toggle.flag)}
            />
          </Tooltip>
        );
      })}
    </div>
  );
}

/** The lock, mute and hide buttons of every track whose header shows. */
export function TrackHeaderControls({ host }: { readonly host: TimelineHost }): ReactElement {
  const { t } = useTranslation();
  useCanvasFollow();
  const project = useDocStore((s) => s.project);
  const model = selectTimelineModel(project);
  const box = useRef<HTMLDivElement>(null);
  const [m, setMetrics] = useState<Metrics>(ZERO);
  const density = useDensity(box);
  // The tokens are read again in another density.
  useLayoutEffect(() => {
    if (box.current !== null) setMetrics(readMetrics(box.current));
  }, [density]);
  const rows: ReactElement[] = [];
  if (m.controlSm > 0) {
    for (const track of model.tracks) {
      const rect = host.trackNameRect(track.id);
      if (rect === null || rect.y + rect.h <= m.ruler) continue;
      rows.push(<HeaderToggles key={track.id} host={host} track={track} rect={rect} m={m} />);
    }
  }
  const addItems: MenuEntry[] = [
    {
      id: "video",
      label: t("timeline.header.addVideo"),
      icon: "film",
      onSelect: () => host.addTrack("video"),
    },
    {
      id: "audio",
      label: t("timeline.header.addAudio"),
      icon: "audio-waveform",
      onSelect: () => host.addTrack("audio"),
    },
  ];
  return (
    <>
      <div className="cw-timeline__corner">
        <Menu mode="dropdown" items={addItems} aria-label={t("timeline.header.addTrack")}>
          <Tooltip label={t("timeline.header.addTrack")}>
            <IconButton size="sm" icon="plus" label={t("timeline.header.addTrack")} />
          </Tooltip>
        </Menu>
      </div>
      <div ref={box} className="cw-timeline__headers">
        {rows}
      </div>
    </>
  );
}

const MODE_LABEL: Readonly<Record<DropPreview["mode"], TKey | null>> = {
  insert: "timeline.drop.insert",
  overwrite: "timeline.drop.overwrite",
  new_track: "timeline.drop.newTrack",
  place: null,
};

/** Where a bin card lands (or the clip an Alt-drop replaces), over the lanes. */
export function DropGhost({ host }: { readonly host: TimelineHost }): ReactElement | null {
  const { t } = useTranslation();
  useCanvasFollow();
  const preview = useStore(host.ui, (s) => s.preview);
  const replace = useStore(host.ui, (s) => s.replaceTarget);
  if (replace !== null) {
    const rect = host.itemRect(replace);
    if (rect === null) return null;
    return (
      <div
        className="cw-timeline__ghost cw-timeline__ghost--replace"
        data-mode="replace"
        style={{ left: rect.x, top: rect.y, width: rect.w, height: rect.h }}
      >
        <span className="cw-timeline__ghost-label caption">{t("timeline.drop.replace")}</span>
      </div>
    );
  }
  if (preview === null) return null;
  const lane = host.trackNameRect(preview.trackId);
  const x0 = host.timeToX(preview.start);
  const x1 = host.timeToX(preview.end);
  if (lane === null || x0 === null || x1 === null) return null;
  const label = MODE_LABEL[preview.mode];
  return (
    <div
      className="cw-timeline__ghost"
      data-mode={preview.mode}
      style={{ left: x0, top: lane.y, width: Math.max(1, x1 - x0), height: lane.h }}
    >
      {label !== null && <span className="cw-timeline__ghost-label caption">{t(label)}</span>}
    </div>
  );
}
