// SPDX-License-Identifier: GPL-3.0-or-later
// The easing of one keyframe (WP-2C.05): the curve editor of `@cutweave/ui` in a small dialog,
// opened by "Bezier…" in the keyframe's menu and "Easing…" in an Inspector row's. The curve is
// sampled by the engine's own math in the shared wasm module (no engine call), every committed
// change is one `keyframe.set_interp` (one undo step), and the dialog follows the document: an
// undo or an agent's edit shows at once.
import type { Easing, Interp } from "@cutweave/protocol";
import { easing as easingMath } from "@cutweave/protocol/wasm";
import { FLICKS_PER_SECOND } from "@cutweave/timeline";
import {
  Button,
  CurveEditor,
  type CurveEditorLabels,
  type CurveKind,
  type CurvePreset,
  type CurveValue,
  Dialog,
  DialogNote,
} from "@cutweave/ui";
import { useCallback, useMemo } from "react";
import { useTranslation } from "react-i18next";
import type { TKey } from "../i18n/index.ts";
import { PROP_LABELS } from "../inspector/labels.ts";
import { menuServices } from "../menus/defaults.ts";
import { menuDialogStore } from "../menus/dialogs.ts";
import { useDocStore } from "../state/docStore.ts";
import { findClip, setEasing, storedKeys } from "./keyframes.ts";

/** The names of the engine's easing presets. */
const PRESET_LABELS: Readonly<Record<Easing, TKey>> = {
  hold: "easing.presets.hold",
  linear: "easing.presets.linear",
  ease_in: "easing.presets.easeIn",
  ease_out: "easing.presets.easeOut",
  ease_in_out: "easing.presets.easeInOut",
  sine_in: "easing.presets.sineIn",
  sine_out: "easing.presets.sineOut",
  sine_in_out: "easing.presets.sineInOut",
  cubic_in: "easing.presets.cubicIn",
  cubic_out: "easing.presets.cubicOut",
  cubic_in_out: "easing.presets.cubicInOut",
  expo_in: "easing.presets.expoIn",
  expo_out: "easing.presets.expoOut",
  expo_in_out: "easing.presets.expoInOut",
  back_in: "easing.presets.backIn",
  back_out: "easing.presets.backOut",
  back_in_out: "easing.presets.backInOut",
  spring: "easing.presets.spring",
  spring_gentle: "easing.presets.springGentle",
  spring_wobbly: "easing.presets.springWobbly",
  spring_stiff: "easing.presets.springStiff",
  elastic: "easing.presets.elastic",
  bounce: "easing.presets.bounce",
};

/** A flip only holds; every other property takes any interpolation. */
const kindsOf = (prop: string): readonly CurveKind[] | undefined =>
  prop.startsWith("flip.") ? ["hold"] : undefined;

/** The command input for a value: the preset's name when it is one, else its numbers. */
export function easingInput(
  value: CurveValue,
  presets: readonly CurvePreset[],
): { readonly easing: Easing } | { readonly interp: Interp } {
  if (value.kind === "hold" || value.kind === "linear") return { easing: value.kind };
  const named = presets.find((p) => p.id === value.preset && p.value.kind === value.kind);
  if (named !== undefined && JSON.stringify(named.value) === JSON.stringify(value)) {
    return { easing: named.id as Easing };
  }
  const { preset: _preset, ...numbers } = value;
  return { interp: numbers as Interp };
}

export function KeyframeCurveDialog({
  item,
  prop,
  t: keyTime,
}: {
  readonly item: string;
  readonly prop: string;
  /** The key's stored item time, flicks. */
  readonly t: number;
}) {
  const { t } = useTranslation();
  const project = useDocStore((s) => s.project);
  const clip = findClip(project, item);
  const keys = clip === null ? null : storedKeys(clip, prop);
  const index = keys?.findIndex((k) => k.t === keyTime) ?? -1;
  const key = index >= 0 ? keys?.[index] : undefined;
  const next = index >= 0 ? keys?.[index + 1] : undefined;
  // A spring runs in real seconds, so the preview needs the segment's length (one second for
  // the last key, which has no segment yet).
  const span = next === undefined ? FLICKS_PER_SECOND : Math.max(1, next.t - keyTime);

  const presets = useMemo<CurvePreset[]>(
    () =>
      easingMath.presets().map((p) => ({
        id: p.name,
        label: t(PRESET_LABELS[p.name]),
        value: p.interp as CurveValue,
      })),
    [t],
  );
  const labels = useMemo<CurveEditorLabels>(
    () => ({
      kind: t("easing.editor.kind"),
      hold: t("easing.editor.hold"),
      linear: t("easing.editor.linear"),
      bezier: t("easing.editor.bezier"),
      spring: t("easing.editor.spring"),
      preset: t("easing.editor.preset"),
      custom: t("easing.editor.custom"),
      graph: t("easing.editor.graph"),
      time: t("easing.editor.time"),
      value: t("easing.editor.value"),
      handleStart: t("easing.editor.handleStart"),
      handleEnd: t("easing.editor.handleEnd"),
      handleAt: (time, value) => t("easing.editor.handleAt", { time, value }),
      stiffness: t("easing.editor.stiffness"),
      damping: t("easing.editor.damping"),
      mass: t("easing.editor.mass"),
      bounce: t("easing.editor.bounce"),
    }),
    [t],
  );
  const sample = useCallback(
    (value: CurveValue, n: number) => easingMath.sample(value as Interp, span, n),
    [span],
  );
  const close = () => menuDialogStore.getState().close();
  const commit = (value: CurveValue): void => {
    void setEasing(
      (name, args) => menuServices.call(name, args),
      item,
      [{ prop, t: keyTime }],
      easingInput(value, presets),
    ).catch((error: unknown) => menuServices.report(error));
  };
  const label = PROP_LABELS[prop];
  const property = label === undefined ? prop : t(label);
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
      title={t("contextMenu.dialog.keyframeCurveTitle")}
      description={t("contextMenu.dialog.keyframeCurveBody", { property })}
      className="cw-kfcurve"
      footer={
        <Button variant="primary" size="lg" onClick={close}>
          {t("contextMenu.dialog.done")}
        </Button>
      }
    >
      {key === undefined ? (
        <DialogNote>{t("contextMenu.dialog.keyframeCurveGone")}</DialogNote>
      ) : (
        <>
          <CurveEditor
            value={key.interp as CurveValue}
            onCommit={commit}
            presets={presets}
            sample={sample}
            kinds={kindsOf(prop)}
            labels={labels}
          />
          {next === undefined ? (
            <DialogNote>{t("contextMenu.dialog.keyframeCurveLast")}</DialogNote>
          ) : null}
        </>
      )}
    </Dialog>
  );
}
