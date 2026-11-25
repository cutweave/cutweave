// SPDX-License-Identifier: GPL-3.0-or-later
// A picture of the media asset selected in the bin, with a scrub bar (WP-1M.14): click or drag to
// see any frame. The frame comes from `/media/{id}/thumb` at the panel's width; a request still
// running when the bar moves again is aborted, and only the newest answer is drawn. The bar is a
// slider (arrow keys step one frame, Home and End go to the ends, Page Up and Down a second), and
// its value reads as timecode at the asset's rate. A sound shows its waveform instead of a
// picture.
//
// Play and pause (WP-1M.15) play the asset as the engine's source (ADR-0022) with this picture as
// its host: the Preview keeps showing the timeline's frame and the sound comes from the engine.
// While it plays here, the bar follows the source's playhead, the picture is the frame at that
// time (the same `/media/{id}/thumb` frames as scrubbing, so no second frame ring is needed), and
// moving the bar seeks the source. Leaving the asset (selecting something else) ends the source.
import type { MediaAsset } from "@cutweave/protocol";
import { formatTimecode, frameDurationFlicks } from "@cutweave/timeline";
import { IconButton, Tooltip } from "@cutweave/ui";
import { type KeyboardEvent, type PointerEvent, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  fractionOfFrame,
  frameAtFraction,
  frameGrid,
  LatestFrames,
  paintCover,
  pointerFraction,
  timeOfFrame,
} from "../media/scrub.ts";
import { useScrubSource } from "../media/scrubSource.ts";
import { usePlaybackStore } from "../state/playbackStore.ts";
import { mediaName } from "../state/selectors.ts";
import { useSourceViewStore } from "../state/sourceViewStore.ts";
import { transportController } from "../transport/useTransport.ts";

/** Widest picture asked for, px. */
const MAX_PICTURE_WIDTH = 1280;
const MIN_PICTURE_WIDTH = 64;
/** The waveform picture's shape (width : height). */
const WAVEFORM_ASPECT = "16 / 5";

/** The width to ask the engine for: the box at the device's pixel ratio, in steps of 32. */
function pictureWidth(box: HTMLElement | null): number {
  const css = box?.clientWidth ?? 0;
  const px = Math.ceil((css * (globalThis.devicePixelRatio || 1)) / 32) * 32;
  return Math.min(MAX_PICTURE_WIDTH, Math.max(MIN_PICTURE_WIDTH, px));
}

export function MediaPreview({ media }: { readonly media: MediaAsset }) {
  const { t } = useTranslation();
  const source = useScrubSource();
  const pictureRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const barRef = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);
  const [frame, setFrame] = useState(0);
  const id = media.id;
  const sound = media.kind === "audio";
  const grid = useMemo(
    () => frameGrid(media.probe.duration, media.probe.fps),
    [media.probe.duration, media.probe.fps],
  );
  const scrubbable = (media.kind === "video" || sound) && grid.frames > 1;
  // The source plays here: the engine's playhead drives the bar.
  const hosting = usePlaybackStore((s) => s.source?.media === id);
  const hostedHere = useSourceViewStore((s) => s.host === "inspector") && hosting;
  const playing = usePlaybackStore((s) => s.playing) && hostedHere;
  const sourceTime = usePlaybackStore((s) => s.sourcePlayhead);
  const hostedRef = useRef(false);
  hostedRef.current = hostedHere;
  const at = Math.min(frame, grid.frames - 1);
  const time = timeOfFrame(at, grid);
  const timecode = formatTimecode(time, grid.rate);

  useEffect(() => {
    if (!hostedHere) return;
    const step = frameDurationFlicks(grid.rate);
    setFrame(Math.min(grid.frames - 1, Math.max(0, Math.floor(sourceTime / step))));
  }, [hostedHere, sourceTime, grid]);
  // Selecting something else ends a source this picture was showing.
  useEffect(
    () => () => {
      if (hostedRef.current) void transportController().closeSource();
    },
    [],
  );

  const frames = useMemo(
    () =>
      new LatestFrames<ImageBitmap>({
        load: async (when, signal) =>
          (await source?.frame(id, when, pictureWidth(pictureRef.current), signal)) ?? null,
        paint: (_when, bitmap) => {
          const canvas = canvasRef.current;
          const box = pictureRef.current;
          if (canvas !== null && box !== null) {
            box.style.aspectRatio = `${bitmap.width} / ${bitmap.height}`;
            try {
              paintCover(canvas, bitmap, bitmap);
              canvas.dataset.frame = "ready";
            } catch {
              // Not drawable: the last picture stays.
            }
          }
          bitmap.close();
        },
      }),
    [source, id],
  );

  // The picture for the bar's position (a sound has a waveform instead, drawn once below).
  useEffect(() => {
    if (!sound) frames.request(time);
  }, [frames, sound, time]);
  useEffect(() => () => frames.cancel(), [frames]);

  useEffect(() => {
    if (!sound || source === null) return;
    const controller = new AbortController();
    const box = pictureRef.current;
    const width = pictureWidth(box);
    void source.waveform(id, width, Math.round((width * 5) / 16), controller.signal).then(
      (bitmap) => {
        const canvas = canvasRef.current;
        if (bitmap === null || canvas === null || controller.signal.aborted) return;
        try {
          paintCover(canvas, bitmap, bitmap);
          canvas.dataset.frame = "ready";
        } catch {
          // Not drawable: the panel stays plain.
        }
        bitmap.close();
      },
      () => undefined,
    );
    return () => controller.abort();
  }, [sound, source, id]);

  const goTo = (next: number) => {
    const clamped = Math.min(grid.frames - 1, Math.max(0, next));
    setFrame(clamped);
    if (hostedHere) void transportController().seek(timeOfFrame(clamped, grid));
  };
  const togglePlay = () => {
    // From the bar's position, or from the start when it is on the last frame.
    const from = at >= grid.frames - 1 ? 0 : timeOfFrame(at, grid);
    void transportController().toggleSource(id, "inspector", from);
  };
  const moveTo = (clientX: number) => {
    const box = barRef.current?.getBoundingClientRect();
    if (box === undefined) return;
    goTo(frameAtFraction(pointerFraction(clientX, box.left, box.width), grid));
  };
  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    dragging.current = true;
    try {
      event.currentTarget.setPointerCapture?.(event.pointerId);
    } catch {
      // A pointer that is already gone.
    }
    barRef.current?.focus();
    moveTo(event.clientX);
  };
  const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    if (dragging.current) moveTo(event.clientX);
  };
  const onPointerUp = () => {
    dragging.current = false;
  };
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const second = Math.max(1, Math.round(grid.rate.num / grid.rate.den));
    const steps: Record<string, number> = {
      ArrowLeft: -1,
      ArrowDown: -1,
      ArrowRight: 1,
      ArrowUp: 1,
      PageDown: -second,
      PageUp: second,
    };
    let next: number | null = null;
    const step = steps[event.key];
    if (step !== undefined) next = at + step;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = grid.frames - 1;
    if (next === null) return;
    event.preventDefault();
    goTo(next);
  };

  if (source === null || (media.kind !== "video" && media.kind !== "image" && !sound)) return null;
  const place = `${fractionOfFrame(at, grid) * 100}%`;
  const name = mediaName(media, media.id);
  const playLabel = t(playing ? "inspector.media.preview.pause" : "inspector.media.preview.play", {
    name,
  });
  return (
    <div className="cw-media-preview" data-testid="media-preview">
      <div
        className="cw-media-preview__picture"
        ref={pictureRef}
        style={{ aspectRatio: sound ? WAVEFORM_ASPECT : "16 / 9" }}
      >
        <canvas
          className="cw-media-preview__canvas"
          ref={canvasRef}
          role="img"
          aria-label={t("inspector.media.preview.picture", { name: mediaName(media, media.id) })}
          data-testid="media-preview-picture"
        />
        {sound && scrubbable ? (
          <span className="cw-media-preview__line" style={{ insetInlineStart: place }} />
        ) : null}
      </div>
      {scrubbable ? (
        <>
          <div className="cw-media-preview__row">
            <Tooltip label={playLabel}>
              <IconButton
                icon={playing ? "pause" : "play"}
                size="sm"
                label={playLabel}
                data-testid="media-preview-play"
                onClick={togglePlay}
              />
            </Tooltip>
            <div
              className="cw-media-preview__bar"
              ref={barRef}
              role="slider"
              tabIndex={0}
              aria-label={t("inspector.media.preview.scrub")}
              aria-orientation="horizontal"
              aria-valuemin={0}
              aria-valuemax={grid.frames - 1}
              aria-valuenow={at}
              aria-valuetext={timecode}
              data-testid="media-preview-bar"
              onPointerDown={onPointerDown}
              onPointerMove={onPointerMove}
              onPointerUp={onPointerUp}
              onPointerCancel={onPointerUp}
              onKeyDown={onKeyDown}
            >
              <span className="cw-media-preview__track" />
              <span className="cw-media-preview__fill" style={{ inlineSize: place }} />
              <span className="cw-media-preview__knob" style={{ insetInlineStart: place }} />
            </div>
          </div>
          <span className="cw-media-preview__time numeric" data-testid="media-preview-time">
            {timecode}
          </span>
        </>
      ) : null}
    </div>
  );
}
