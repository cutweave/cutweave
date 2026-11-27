// SPDX-License-Identifier: GPL-3.0-or-later
// The in-place track name input (issue 149): a design-system text field laid over the name in the
// track header while `host.rename` is open. Enter or blur commits through `track.set`, Esc
// cancels; an empty name is refused with its reason shown beside the field. It keeps the keyboard:
// key events stop here, so the timeline's keys (S, J/K/L, Delete) never fire while typing.
import { TextField } from "@cutweave/ui";
import { type KeyboardEvent, memo, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import { useDocStore } from "../state/docStore.ts";
import { useViewStore } from "../state/viewStore.ts";
import type { TimelineHost } from "./host.ts";

function RenameField({
  controller,
  initial,
  reason,
  rect,
}: {
  readonly controller: TimelineHost;
  readonly initial: string;
  readonly reason: string | null;
  readonly rect: { readonly x: number; readonly y: number; readonly w: number; readonly h: number };
}) {
  const { t } = useTranslation();
  const [value, setValue] = useState(initial);
  const input = useRef<HTMLInputElement>(null);
  /** Set once Enter or Esc ended the edit, so the blur that follows the unmount does not repeat it. */
  const ended = useRef(false);

  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);

  // A refused name puts the old one back and selects it, ready to type again.
  useEffect(() => {
    if (reason === null) return;
    setValue(initial);
    input.current?.select();
  }, [reason, initial]);

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>): void => {
    e.stopPropagation(); // typing belongs to the field
    if (e.key === "Enter") {
      e.preventDefault();
      ended.current = true;
      void controller.commitRename(value, "enter").then((outcome) => {
        if (outcome === "refused") ended.current = false; // the input stays open
      });
    } else if (e.key === "Escape") {
      e.preventDefault();
      ended.current = true;
      controller.cancelRename();
    }
  };

  return (
    <div
      className="cw-timeline__rename"
      style={{ left: rect.x, top: rect.y, width: rect.w, height: rect.h }}
    >
      <TextField
        ref={input}
        className="cw-timeline__rename-field"
        aria-label={t("timeline.rename.label")}
        value={value}
        invalid={reason !== null}
        title={reason ?? undefined}
        onChange={setValue}
        onKeyDown={onKeyDown}
        onBlur={() => {
          if (ended.current) return;
          ended.current = true;
          void controller.commitRename(value, "blur");
        }}
      />
      {reason !== null && (
        <span className="cw-timeline__rename-reason caption" role="alert">
          {reason}
        </span>
      )}
    </div>
  );
}

/** Renders nothing until a rename is open and the track's name is on screen. */
export const TrackRenameInput = memo(function TrackRenameInput({
  controller,
}: {
  readonly controller: TimelineHost;
}) {
  const { trackId, reason } = useStore(controller.rename);
  // No rename open: nothing to follow (a drag on the canvas changes the document every frame).
  if (trackId === null) return null;
  return <OpenRename controller={controller} trackId={trackId} reason={reason} />;
});

function OpenRename({
  controller,
  trackId,
  reason,
}: {
  readonly controller: TimelineHost;
  readonly trackId: string;
  readonly reason: string | null;
}) {
  // Follow the lane when the timeline scrolls or the project changes under the input.
  useViewStore((s) => s.scroll);
  useDocStore((s) => s.project);
  const target = controller.renameTarget();
  const rect = controller.trackNameRect(trackId);
  if (target === null || rect === null) return null;
  return (
    <RenameField
      key={target.trackId}
      controller={controller}
      initial={target.name}
      reason={reason}
      rect={rect}
    />
  );
}
