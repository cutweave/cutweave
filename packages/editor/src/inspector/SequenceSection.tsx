// SPDX-License-Identifier: GPL-3.0-or-later
// The Sequence section of the Inspector (WP-2H.03b), shown when nothing is selected: the canvas
// size, frame rate and background of the active sequence, each a `sequence.set_format`. A new
// canvas size reframes the video clips as chosen in "Reframe clips" (the engine's `reframe`).
import type { FrameRate, ReframePolicy, SequenceFormat, SequenceId } from "@cutweave/protocol";
import { type JsonSchema, SchemaForm } from "@cutweave/ui";
import { useCallback, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TKey } from "../i18n/index.ts";
import type { InspectorEditor } from "./editor.ts";
import { Section } from "./Section.tsx";

/** The frame rates offered (the edit grid), as `num/den`. */
const RATES: readonly FrameRate[] = [
  { num: 24000, den: 1001 },
  { num: 24, den: 1 },
  { num: 25, den: 1 },
  { num: 30000, den: 1001 },
  { num: 30, den: 1 },
  { num: 50, den: 1 },
  { num: 60000, den: 1001 },
  { num: 60, den: 1 },
];

const REFRAMES: readonly ReframePolicy[] = ["fit", "fill", "keep_scale", "none"];
const REFRAME_LABEL: Readonly<Record<ReframePolicy, TKey>> = {
  fit: "inspector.sequence.reframes.fit",
  fill: "inspector.sequence.reframes.fill",
  keep_scale: "inspector.sequence.reframes.keep_scale",
  none: "inspector.sequence.reframes.none",
};

const rateKey = (r: FrameRate): string => `${r.num}/${r.den}`;

/** `23.976`, `24`, `29.97`: how a person names the rate. */
export function rateLabel(r: FrameRate): string {
  return String(Math.round((r.num / r.den) * 1000) / 1000);
}

type Key = "width" | "height" | "frame_rate" | "background" | "reframe";

export function SequenceSection({
  sequence,
  format,
  editor,
}: {
  readonly sequence: SequenceId;
  readonly format: SequenceFormat;
  readonly editor: InspectorEditor;
}) {
  const { t } = useTranslation();
  const [reframe, setReframe] = useState<ReframePolicy>("fit");
  // A colour picked or a number half typed shows until it is committed.
  const [draft, setDraft] = useState<Partial<Record<Key, unknown>>>({});
  const rates = useMemo(() => {
    const known = RATES.some((r) => rateKey(r) === rateKey(format.frame_rate));
    return known ? RATES : [...RATES, format.frame_rate];
  }, [format.frame_rate]);
  const schema = useMemo<JsonSchema>(
    () => ({
      type: "object",
      properties: {
        width: {
          type: "integer",
          minimum: 1,
          maximum: 16384,
          "x-ui": { label: t("inspector.sequence.width"), unit: "px", step: 1, order: 0 },
        },
        height: {
          type: "integer",
          minimum: 1,
          maximum: 16384,
          "x-ui": { label: t("inspector.sequence.height"), unit: "px", step: 1, order: 1 },
        },
        reframe: {
          type: "string",
          oneOf: REFRAMES.map((r) => ({ const: r, title: t(REFRAME_LABEL[r]) })),
          "x-ui": { label: t("inspector.sequence.reframe"), widget: "select", order: 2 },
        },
        frame_rate: {
          type: "string",
          oneOf: rates.map((r) => ({
            const: rateKey(r),
            title: t("inspector.sequence.rateValue", { fps: rateLabel(r) }),
          })),
          "x-ui": { label: t("inspector.sequence.frameRate"), widget: "select", order: 3 },
        },
        background: {
          type: "string",
          "x-ui": { label: t("inspector.sequence.background"), widget: "color", order: 4 },
        },
      },
    }),
    [rates, t],
  );
  const value = {
    width: format.width,
    height: format.height,
    reframe,
    frame_rate: rateKey(format.frame_rate),
    background: format.background,
    ...draft,
  };
  const onChange = useCallback(
    (path: string, next: unknown, meta: { gesture?: "start" | "update" | "end" | undefined }) => {
      const key = path as Key;
      if (meta.gesture === "start" || meta.gesture === "update") {
        setDraft((d) => ({ ...d, [key]: next }));
        return;
      }
      const settle = (): void => setDraft(({ [key]: _gone, ...rest }) => rest);
      if (key === "reframe") {
        setReframe(next as ReframePolicy);
        return;
      }
      if (key === "width" || key === "height") {
        const size = Math.round(Number(next));
        if (!Number.isFinite(size) || size < 1 || size === format[key]) return settle();
        void editor
          .setSequenceFormat(
            sequence,
            { [key]: size, reframe },
            t(key === "width" ? "inspector.sequence.widthName" : "inspector.sequence.heightName"),
          )
          .finally(settle);
      } else if (key === "frame_rate") {
        const rate = rates.find((r) => rateKey(r) === next);
        if (rate === undefined || rateKey(rate) === rateKey(format.frame_rate)) return settle();
        void editor
          .setSequenceFormat(sequence, { frame_rate: rate }, t("inspector.sequence.frameRateName"))
          .finally(settle);
      } else {
        const color = String(next);
        if (color === format.background) return settle();
        void editor
          .setSequenceFormat(
            sequence,
            { background: color },
            t("inspector.sequence.backgroundName"),
          )
          .finally(settle);
      }
    },
    [editor, sequence, format, rates, reframe, t],
  );
  return (
    <Section id="sequence" label={t("inspector.sequence.title")}>
      <div data-testid="sequence-inspector">
        <SchemaForm
          aria-label={t("inspector.sequence.title")}
          schema={schema}
          value={value}
          onChange={onChange}
        />
      </div>
    </Section>
  );
}
