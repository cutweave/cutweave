// SPDX-License-Identifier: GPL-3.0-or-later
// Hover-scrub on a media card (WP-1M.14): moving the pointer across a video card shows the frame
// at that fraction of its length (cut from the filmstrip sprite, so no request per move), with a
// position line and the time; leaving puts the poster back. A sound shows the line over its
// waveform. Pointer only: the Inspector's scrub bar is the keyboard path. The setting
// `media.hover_scrub` turns it off.
//
// The pointer's DOM updates (line, time, canvas) are made directly on the elements, not through
// React state, so a move costs no render.
import { formatTimecode } from "@cutweave/timeline";
import { type PointerEvent, type RefObject, useCallback, useEffect, useMemo, useRef } from "react";
import { usePref } from "../prefs/prefsStore.ts";
import type { MediaCard } from "./cards.ts";
import {
  frameAtFraction,
  frameGrid,
  LatestFrames,
  paintCover,
  pointerFraction,
  timeOfFrame,
} from "./scrub.ts";
import { type Sprite, tileOf, useScrubSource } from "./scrubSource.ts";

/** The waveform picture asked for a sound card (drawn at the card's size, twice for sharpness). */
const WAVEFORM_WIDTH = 320;
const WAVEFORM_HEIGHT = 180;

export interface CardScrub {
  readonly thumbRef: RefObject<HTMLSpanElement | null>;
  readonly canvasRef: RefObject<HTMLCanvasElement | null>;
  readonly lineRef: RefObject<HTMLSpanElement | null>;
  readonly timeRef: RefObject<HTMLSpanElement | null>;
  /** Whether the card scrubs at all (a video or sound, with a length, and the engine's media). */
  readonly enabled: boolean;
  onPointerMove(event: PointerEvent<HTMLElement>): void;
  /** The pointer left, a drag began, or the card went away: back to the poster. */
  stop(): void;
}

export function useCardScrub(card: MediaCard): CardScrub {
  const preference = usePref("media.hover_scrub");
  const source = useScrubSource();
  const thumbRef = useRef<HTMLSpanElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const lineRef = useRef<HTMLSpanElement>(null);
  const timeRef = useRef<HTMLSpanElement>(null);
  const waveform = useRef<AbortController | null>(null);
  const grid = useMemo(() => frameGrid(card.duration, card.rate), [card.duration, card.rate]);
  const enabled =
    preference &&
    source !== null &&
    (card.kind === "video" || card.kind === "audio") &&
    card.duration !== null &&
    card.duration > 0 &&
    !card.broken;
  const id = card.id;
  const kind = card.kind;

  const frames = useMemo(
    () =>
      new LatestFrames<{ sprite: Sprite; t: number }>({
        load: async (t, signal) => {
          const sprite = (await source?.sprite(id)) ?? null;
          if (sprite === null || signal.aborted) return null;
          return { sprite, t };
        },
        paint: (_t, { sprite, t }) => {
          const canvas = canvasRef.current;
          if (canvas === null || thumbRef.current?.dataset.scrubbing !== "true") return;
          try {
            paintCover(canvas, sprite.bitmap, sprite.bitmap, tileOf(sprite, t));
            canvas.hidden = false;
          } catch {
            // The sprite was released meanwhile (evicted, or the engine restarted): keep the poster.
          }
        },
      }),
    [source, id],
  );

  const stop = useCallback(() => {
    frames.cancel();
    waveform.current?.abort();
    waveform.current = null;
    delete thumbRef.current?.dataset.scrubbing;
    if (canvasRef.current !== null) canvasRef.current.hidden = true;
  }, [frames]);

  useEffect(() => stop, [stop]);
  // Turned off (or the engine went away) while the pointer is over a card.
  useEffect(() => {
    if (!enabled) stop();
  }, [enabled, stop]);

  const onPointerMove = useCallback(
    (event: PointerEvent<HTMLElement>) => {
      const thumb = thumbRef.current;
      if (!enabled || thumb === null || source === null || event.pointerType === "touch") return;
      if (event.buttons !== 0) return;
      const box = thumb.getBoundingClientRect();
      const fraction = pointerFraction(event.clientX, box.left, box.width);
      const t = timeOfFrame(frameAtFraction(fraction, grid), grid);
      const first = thumb.dataset.scrubbing !== "true";
      thumb.dataset.scrubbing = "true";
      if (lineRef.current !== null) lineRef.current.style.insetInlineStart = `${fraction * 100}%`;
      if (timeRef.current !== null) timeRef.current.textContent = formatTimecode(t, grid.rate);
      if (kind === "video") {
        frames.request(t);
      } else if (first && kind === "audio") {
        const controller = new AbortController();
        waveform.current = controller;
        void source.waveform(id, WAVEFORM_WIDTH, WAVEFORM_HEIGHT, controller.signal).then(
          (bitmap) => {
            const canvas = canvasRef.current;
            if (bitmap === null || canvas === null || controller.signal.aborted) return;
            try {
              paintCover(canvas, bitmap, bitmap);
              canvas.hidden = false;
            } catch {
              // Not drawable: the tile stays.
            }
          },
          () => undefined,
        );
      }
    },
    [enabled, source, grid, kind, id, frames],
  );

  return { thumbRef, canvasRef, lineRef, timeRef, enabled, onPointerMove, stop };
}
