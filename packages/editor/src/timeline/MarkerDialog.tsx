// SPDX-License-Identifier: GPL-3.0-or-later
// The marker's Edit… (issue #222): its name, its colour (one of the label colours) and its length
// in seconds, where a length makes it a range marker. Opened from the marker's menu and by a
// double-click on its pin. Saving sends one `timeline.update_marker` with what changed.
import type { MarkerId } from "@cutweave/protocol";
import { FLICKS_PER_SECOND } from "@cutweave/timeline";
import { Button, Dialog, TextField } from "@cutweave/ui";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { menuServices } from "../menus/defaults.ts";
import { menuDialogStore } from "../menus/dialogs.ts";
import { useDocStore } from "../state/docStore.ts";
import { selectActiveSequenceId } from "../state/selectors.ts";
import {
  LABEL_COLOR_NAME,
  LABEL_COLORS,
  type LabelColor,
  labelColorOf,
  labelColorValue,
  labelColorVar,
} from "./labelColors.ts";

/** A marker's length as the field shows it: seconds with at most three decimals, or empty. */
export function lengthText(duration: number | null | undefined): string {
  if (duration === null || duration === undefined || duration <= 0) return "";
  return String(Number((duration / FLICKS_PER_SECOND).toFixed(3)));
}

/**
 * The `duration` argument for a typed length: `0` for an empty field (a point marker), seconds
 * as `"1.5s"`, or `null` when the text is not a length.
 */
export function lengthArg(text: string): string | number | null {
  const trimmed = text.trim().replace(",", ".");
  if (trimmed === "") return 0;
  if (!/^\d+(\.\d+)?$/.test(trimmed)) return null;
  const seconds = Number(trimmed);
  return seconds === 0 ? 0 : `${seconds}s`;
}

export interface MarkerEdit {
  readonly label?: string;
  readonly color?: string;
  readonly duration?: string | number;
}

/** What `timeline.update_marker` needs to turn the marker into what the dialog shows. */
export function markerEdit(
  marker: { readonly label: string; readonly duration?: number | null },
  form: { readonly label: string; readonly color: string | null; readonly length: string },
): MarkerEdit | null {
  const length = lengthArg(form.length);
  if (length === null) return null;
  return {
    ...(form.label.trim() !== marker.label && { label: form.label.trim() }),
    ...(form.color !== null && { color: form.color }),
    ...(form.length.trim() !== lengthText(marker.duration) && { duration: length }),
  };
}

export function MarkerDialog({ marker: id }: { readonly marker: string }) {
  const { t } = useTranslation();
  const marker = useDocStore((s) => {
    const sequence = selectActiveSequenceId(s.project);
    return sequence === null
      ? undefined
      : s.project?.sequences[sequence]?.markers?.[id as MarkerId];
  });
  const [label, setLabel] = useState(marker?.label ?? "");
  const [picked, setPicked] = useState<LabelColor | null>(null);
  const [length, setLength] = useState(lengthText(marker?.duration));
  const current = useMemo(() => labelColorOf(marker?.color), [marker?.color]);
  const close = () => menuDialogStore.getState().close();
  // The marker vanished (undo, another connection): nothing to edit.
  useEffect(() => {
    if (marker === undefined) close();
  }, [marker]);
  if (marker === undefined) return null;
  const color = picked ?? current;
  const badLength = lengthArg(length) === null;
  const submit = () => {
    const edit = markerEdit(marker, {
      label,
      color: picked === null || picked === current ? null : labelColorValue(picked),
      length,
    });
    if (edit === null) return;
    close();
    if (Object.keys(edit).length === 0) return;
    void menuServices
      .call("timeline.update_marker", { id, ...edit })
      .catch((error: unknown) => menuServices.report(error));
  };
  return (
    <Dialog
      open
      className="cw-marker-dialog"
      onOpenChange={(open) => {
        if (!open) close();
      }}
      title={t("contextMenu.dialog.editMarkerTitle")}
      description={t("contextMenu.dialog.editMarkerBody")}
      onSubmit={submit}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={close}>
            {t("contextMenu.dialog.cancel")}
          </Button>
          <Button variant="primary" size="lg" disabled={badLength} onClick={submit}>
            {t("contextMenu.dialog.save")}
          </Button>
        </>
      }
    >
      <div className="cw-row">
        <label className="cw-row__label" htmlFor="marker-name">
          {t("contextMenu.dialog.markerName")}
        </label>
        <TextField
          id="marker-name"
          value={label}
          onChange={setLabel}
          autoFocus
          onFocus={(event) => event.currentTarget.select()}
        />
      </div>
      <div className="cw-row">
        <span className="cw-row__label" id="marker-color">
          {t("contextMenu.dialog.markerColor")}
        </span>
        <div role="radiogroup" aria-labelledby="marker-color" className="cw-swatches">
          {LABEL_COLORS.map((name) => (
            <label key={name} className="cw-swatches__item" title={t(LABEL_COLOR_NAME[name])}>
              <input
                type="radio"
                name="marker-color"
                aria-label={t(LABEL_COLOR_NAME[name])}
                checked={color === name}
                onChange={() => setPicked(name)}
              />
              <span
                className="cw-swatches__chip"
                style={{ background: `var(${labelColorVar(name)})` }}
              />
            </label>
          ))}
        </div>
      </div>
      <div className="cw-row">
        <label className="cw-row__label" htmlFor="marker-length">
          {t("contextMenu.dialog.markerLength")}
        </label>
        <TextField
          id="marker-length"
          value={length}
          onChange={setLength}
          inputMode="decimal"
          invalid={badLength}
        />
      </div>
    </Dialog>
  );
}
