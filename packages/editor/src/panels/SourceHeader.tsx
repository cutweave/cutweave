// SPDX-License-Identifier: GPL-3.0-or-later
import type { MediaId } from "@cutweave/protocol";
import { formatTimecode } from "@cutweave/timeline";
import { Icon, IconButton, type IconName, Tooltip } from "@cutweave/ui";
import { type DragEvent, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { comboText, strokesOf } from "../keymap/store.ts";
import { type DraggedMedia, dragStore, MEDIA_DRAG_TYPE } from "../media/dragStore.ts";
import { useContextMenu } from "../menus/host.tsx";
import { SOURCE_COMMANDS, type SourceKeyAction } from "../preview/sourceKeys.ts";
import { clearMarks, placeShownSource, setSourceMark } from "../source/actions.ts";
import { markedRange } from "../source/marks.ts";
import { useDocStore } from "../state/docStore.ts";
import { usePlaybackStore } from "../state/playbackStore.ts";
import { mediaName } from "../state/selectors.ts";
import { useViewStore, viewStore } from "../state/viewStore.ts";
import { transportController } from "../transport/useTransport.ts";

/** The keys of `action` in the keymap, spelled for a tooltip. */
const keyOfAction = (action: SourceKeyAction): string[] =>
  strokesOf(SOURCE_COMMANDS[action]).map((combo) => comboText(combo));

export interface SourceAsset {
  readonly id: MediaId;
  readonly name: string;
  readonly kind: "video" | "audio";
  /** Its length, flicks. */
  readonly duration: number;
  /** A picture that also has sound: the two can be placed apart. */
  readonly hasSound: boolean;
}

/** The asset the Source view shows, and what putting it on the timeline needs. */
export function useSourceAsset(): SourceAsset | null {
  const source = usePlaybackStore((s) => s.source);
  const media = useDocStore((s) => (source === null ? undefined : s.project?.media[source.media]));
  return useMemo(
    () =>
      source === null
        ? null
        : {
            id: source.media,
            name: mediaName(media, source.media),
            kind: media?.kind === "audio" ? ("audio" as const) : ("video" as const),
            duration: source.duration,
            hasSound: media !== undefined && media.kind !== "audio" && media.probe.audio != null,
          },
    [source, media],
  );
}

/** Which part of the source a drag carries. */
export type SourcePart = "both" | "video" | "audio";

/**
 * What a drag from the Source view carries to the timeline (WP-2A.14): the marked range (all of
 * the asset without marks); from the picture icon the picture alone, from the sound icon the
 * sound alone. `null` when the marks leave nothing to place.
 */
export function sourceDrag(
  asset: Pick<SourceAsset, "id" | "kind" | "duration">,
  part: SourcePart,
): DraggedMedia | null {
  const marks = viewStore.getState().sourceMarks[asset.id];
  const range = markedRange(marks, asset.duration);
  if (marks !== undefined && range === null) return null;
  return {
    id: asset.id,
    kind: part === "audio" ? "audio" : asset.kind,
    duration: range === null ? asset.duration : range.out - range.in,
    ...(range !== null && { range: { sourceIn: range.in, duration: range.out - range.in } }),
    ...(part === "video" && { link: false as const }),
  };
}

/** Starts the drag of `part` of the source; cancels it when there is nothing to carry. */
export function beginSourceDrag(
  e: DragEvent<HTMLElement>,
  asset: Pick<SourceAsset, "id" | "kind" | "duration"> | null,
  part: SourcePart,
): void {
  const drag = asset === null ? null : sourceDrag(asset, part);
  if (drag === null) {
    e.preventDefault();
    return;
  }
  e.dataTransfer.setData(MEDIA_DRAG_TYPE, drag.id);
  e.dataTransfer.effectAllowed = "copy";
  dragStore.getState().begin(drag);
}

/** A grip that drags one part of the source (its picture or its sound) to the timeline. */
function DragPart({
  part,
  icon,
  label,
  asset,
}: {
  readonly part: SourcePart;
  readonly icon: IconName;
  readonly label: string;
  readonly asset: SourceAsset;
}) {
  return (
    <Tooltip label={label}>
      <span
        className="cw-iconbtn cw-iconbtn--sm cw-source__drag"
        role="img"
        aria-label={label}
        draggable
        data-testid={`source-drag-${part}`}
        onDragStart={(e) => beginSourceDrag(e, asset, part)}
        onDragEnd={() => dragStore.getState().end()}
      >
        <Icon name={icon} size="md" />
      </span>
    </Tooltip>
  );
}

/**
 * The Preview's bar while a source is shown (WP-1M.15, ADR-0022): the "Source" label, the asset's
 * name, its own timecode and length at its own rate, the marks (in, out, clear), Insert and
 * Overwrite of the marked range at the timeline's playhead (WP-2A.14), the grips that drag the
 * picture or the sound alone, and Back to timeline.
 */
export function SourceHeader() {
  const { t } = useTranslation();
  const asset = useSourceAsset();
  const source = usePlaybackStore((s) => s.source);
  const at = usePlaybackStore((s) => s.sourcePlayhead);
  const marked = useViewStore(
    (s) => source !== null && s.sourceMarks[source.media] !== undefined,
  );
  const menuRef = useContextMenu<HTMLElement>("source.view", () => ({}));
  if (source === null || asset === null) return null;
  const timecode = formatTimecode(at, source.fps, true);
  const length = formatTimecode(source.duration, source.fps, true);
  return (
    <section
      ref={menuRef}
      className="cw-preview__bar cw-source__bar"
      aria-label={t("preview.source.region")}
      data-testid="source-header"
    >
      <span className="cw-source__badge label">{t("preview.source.label")}</span>
      <span className="cw-source__name body-strong" title={asset.name}>
        {asset.name}
      </span>
      <span className="cw-source__time timecode">
        <output aria-label={t("preview.source.timecode")} data-testid="source-timecode">
          {timecode}
        </output>
        <span aria-hidden="true">/</span>
        <span data-testid="source-duration">{length}</span>
      </span>
      <Tooltip label={t("preview.source.markIn")} keys={keyOfAction("markIn")}>
        <IconButton
          icon="mark-in"
          size="sm"
          label={t("preview.source.markIn")}
          onClick={() => setSourceMark("in")}
        />
      </Tooltip>
      <Tooltip label={t("preview.source.markOut")} keys={keyOfAction("markOut")}>
        <IconButton
          icon="mark-out"
          size="sm"
          label={t("preview.source.markOut")}
          onClick={() => setSourceMark("out")}
        />
      </Tooltip>
      <Tooltip label={t("preview.source.clearMarks")} keys={keyOfAction("clearMarks")}>
        <IconButton
          icon="x"
          size="sm"
          label={t("preview.source.clearMarks")}
          disabled={!marked}
          onClick={() => clearMarks()}
        />
      </Tooltip>
      <Tooltip label={t("preview.source.insert")} keys={keyOfAction("insert")}>
        <IconButton
          icon="plus"
          size="sm"
          label={t("preview.source.insert")}
          onClick={() => void placeShownSource("insert")}
        />
      </Tooltip>
      <Tooltip label={t("preview.source.overwrite")} keys={keyOfAction("overwrite")}>
        <IconButton
          icon="layers"
          size="sm"
          label={t("preview.source.overwrite")}
          onClick={() => void placeShownSource("overwrite")}
        />
      </Tooltip>
      {asset.hasSound ? (
        <>
          <DragPart part="video" icon="film" label={t("preview.source.dragVideo")} asset={asset} />
          <DragPart
            part="audio"
            icon="audio-waveform"
            label={t("preview.source.dragAudio")}
            asset={asset}
          />
        </>
      ) : null}
      <Tooltip label={t("preview.source.back")} keys={keyOfAction("back")}>
        <IconButton
          icon="arrow-left"
          size="sm"
          label={t("preview.source.back")}
          onClick={() => void transportController().closeSource()}
        />
      </Tooltip>
    </section>
  );
}
