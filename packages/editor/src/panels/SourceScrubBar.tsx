// SPDX-License-Identifier: GPL-3.0-or-later
// The Source view's scrub bar (WP-2A.14): where the source's playhead is in the clip, and the
// marked range (in to out) that Insert and Overwrite place. Click or drag to seek; it is a slider
// (the arrows step a frame, Page Up and Down a second, Home and End go to the ends).
import type { MediaId } from "@cutweave/protocol";
import { formatTimecode, frameDurationFlicks } from "@cutweave/timeline";
import { type KeyboardEvent, type PointerEvent, useRef } from "react";
import { useTranslation } from "react-i18next";
import { pointerFraction } from "../media/scrub.ts";
import { markedRange } from "../source/marks.ts";
import { usePlaybackStore } from "../state/playbackStore.ts";
import { useViewStore } from "../state/viewStore.ts";
import { transportController } from "../transport/useTransport.ts";

const percent = (part: number, whole: number): string =>
  `${whole <= 0 ? 0 : Math.min(100, Math.max(0, (part / whole) * 100))}%`;

export function SourceScrubBar() {
  const { t } = useTranslation();
  const source = usePlaybackStore((s) => s.source);
  const at = usePlaybackStore((s) => s.sourcePlayhead);
  const marks = useViewStore((s) =>
    source === null ? undefined : s.sourceMarks[source.media as MediaId],
  );
  const barRef = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);
  if (source === null) return null;
  const length = source.duration;
  const frame = frameDurationFlicks(source.fps);
  const last = Math.max(0, length - frame);
  const range = markedRange(marks, length);
  const code = (time: number) => formatTimecode(time, source.fps, true);

  const seek = (time: number) => {
    // On the source's own frames.
    const snapped = Math.round(Math.min(last, Math.max(0, time)) / frame) * frame;
    void transportController().seek(snapped);
  };
  const moveTo = (clientX: number) => {
    const box = barRef.current?.getBoundingClientRect();
    if (box !== undefined) seek(pointerFraction(clientX, box.left, box.width) * length);
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
    const second = Math.max(1, Math.round(source.fps.num / source.fps.den));
    const steps: Record<string, number> = {
      ArrowLeft: -1,
      ArrowDown: -1,
      ArrowRight: 1,
      ArrowUp: 1,
      PageDown: -second,
      PageUp: second,
    };
    const step = steps[event.key];
    let next: number | null = null;
    if (step !== undefined) next = at + step * frame;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = last;
    if (next === null) return;
    event.preventDefault();
    event.stopPropagation();
    seek(next);
  };

  const valueText =
    range === null
      ? code(at)
      : t("preview.source.scrubValue", { time: code(at), in: code(range.in), out: code(range.out) });
  return (
    <div className="cw-source__scrub" data-testid="source-scrub">
      <div
        className="cw-media-preview__bar cw-source__scrub-bar"
        ref={barRef}
        role="slider"
        tabIndex={0}
        aria-label={t("preview.source.scrub")}
        aria-orientation="horizontal"
        aria-valuemin={0}
        aria-valuemax={Math.round(last / frame)}
        aria-valuenow={Math.round(Math.min(last, at) / frame)}
        aria-valuetext={valueText}
        data-testid="source-scrub-bar"
        data-marked={range !== null}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        onKeyDown={onKeyDown}
      >
        <span className="cw-media-preview__track" />
        {range === null ? null : (
          <span
            className="cw-source__range"
            data-testid="source-range"
            data-in={marks?.in ?? ""}
            data-out={marks?.out ?? ""}
            style={{
              insetInlineStart: percent(range.in, length),
              inlineSize: percent(range.out - range.in, length),
            }}
          />
        )}
        <span
          className="cw-media-preview__knob"
          style={{ insetInlineStart: percent(at, length) }}
        />
      </div>
      {range === null ? null : (
        <span className="cw-source__marks numeric" data-testid="source-marks">
          {t("preview.source.marksValue", {
            in: code(range.in),
            out: code(range.out),
            length: code(range.out - range.in),
          })}
        </span>
      )}
    </div>
  );
}
