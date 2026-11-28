// SPDX-License-Identifier: GPL-3.0-or-later
// The form over one sequence's format (WP-2H.03b, WP-2B.02, WP-2B.03), shared by the Inspector's
// Sequence section and the Sequence settings dialog: the canvas size, frame rate and background,
// each a `sequence.set_format`, and what shows behind the picture, each a
// `sequence.set_background`. A new canvas size reframes the video clips as chosen in "Reframe
// clips" (the engine's `reframe`). "Fill" is the color alone, a blurred copy of the picture with
// its Blur and Dim, a still image of the project with how it is fitted, or nothing (a
// transparent canvas). The image is chosen only where `images` is given (the dialog): the
// Inspector lists an image fill only while the sequence has one.
import type {
  BackgroundFit,
  BackgroundKind,
  FrameRate,
  MediaId,
  ReframePolicy,
  SequenceFormat,
  SequenceId,
} from "@cutweave/protocol";
import { type JsonSchema, SchemaForm } from "@cutweave/ui";
import { useCallback, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TKey } from "../i18n/index.ts";
import type { InspectorEditor } from "../inspector/editor.ts";

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
  keep_scale: "inspector.sequence.reframes.keepScale",
  none: "inspector.sequence.reframes.none",
};

const FILL_LABEL: Readonly<Record<BackgroundKind, TKey>> = {
  color: "inspector.sequence.fills.color",
  blur: "inspector.sequence.fills.blur",
  image: "inspector.sequence.fills.image",
  transparent: "inspector.sequence.fills.transparent",
};
const FITS: readonly BackgroundFit[] = ["cover", "contain", "stretch"];
const FIT_LABEL: Readonly<Record<BackgroundFit, TKey>> = {
  cover: "inspector.sequence.fits.cover",
  contain: "inspector.sequence.fits.contain",
  stretch: "inspector.sequence.fits.stretch",
};
/** The Blur slider's range: up to a fifth of the canvas height (more only if already set). */
const BLUR_MAX = 0.2;

const rateKey = (r: FrameRate): string => `${r.num}/${r.den}`;

/** `23.976`, `24`, `29.97`: how a person names the rate. */
export function rateLabel(r: FrameRate): string {
  return String(Math.round((r.num / r.den) * 1000) / 1000);
}

/** A still image of the project a background can show. */
export interface BackgroundImage {
  readonly id: MediaId;
  readonly name: string;
}

type Key =
  | "width"
  | "height"
  | "frame_rate"
  | "background"
  | "reframe"
  | "fill"
  | "blur_radius"
  | "blur_dim"
  | "image"
  | "image_fit";

export function SequenceForm({
  sequence,
  format,
  editor,
  images,
  label,
}: {
  readonly sequence: SequenceId;
  readonly format: SequenceFormat;
  readonly editor: InspectorEditor;
  /** The project's still images, where the background image can be chosen (the dialog). */
  readonly images?: readonly BackgroundImage[] | undefined;
  /** Names the form for assistive technology. */
  readonly label: string;
}) {
  const { t } = useTranslation();
  const [reframe, setReframe] = useState<ReframePolicy>("fit");
  // A colour picked or a number half typed shows until it is committed.
  const [draft, setDraft] = useState<Partial<Record<Key, unknown>>>({});
  const rates = useMemo(() => {
    const known = RATES.some((r) => rateKey(r) === rateKey(format.frame_rate));
    return known ? RATES : [...RATES, format.frame_rate];
  }, [format.frame_rate]);
  const fill = format.background_fill ?? null;
  const fillKind: BackgroundKind = fill?.kind ?? "color";
  const blurRadius = fill?.kind === "blur" ? (fill.radius ?? 0) : 0;
  const blurDim = fill?.kind === "blur" ? (fill.dim ?? 0) : 0;
  const blurMax = blurRadius > BLUR_MAX ? 1 : BLUR_MAX;
  const imageId = fill?.kind === "image" ? fill.media : null;
  const imageFit: BackgroundFit = fill?.kind === "image" ? (fill.fit ?? "cover") : "cover";
  // The image the sequence has stays listed even after it left the bin, so the row says so.
  const imageChoices = useMemo(() => {
    if (images === undefined) return null;
    if (imageId === null || images.some((i) => i.id === imageId)) return images;
    return [...images, { id: imageId, name: t("inspector.sequence.imageGone") }];
  }, [images, imageId, t]);
  const canPick = imageChoices !== null && imageChoices.length > 0;
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
        fill: {
          type: "string",
          oneOf: (["color", "blur", "image", "transparent"] as const)
            .filter((k) => k !== "image" || fillKind === "image" || canPick)
            .map((k) => ({ const: k, title: t(FILL_LABEL[k]) })),
          "x-ui": { label: t("inspector.sequence.fill"), widget: "select", order: 5 },
        },
        ...(fillKind === "blur"
          ? {
              blur_radius: {
                type: "number",
                minimum: 0,
                maximum: blurMax,
                "x-ui": {
                  label: t("inspector.sequence.blur"),
                  widget: "slider",
                  step: 0.005,
                  unit: "%",
                  scale: 100,
                  order: 6,
                },
              },
              blur_dim: {
                type: "number",
                minimum: 0,
                maximum: 1,
                "x-ui": {
                  label: t("inspector.sequence.dim"),
                  widget: "slider",
                  step: 0.01,
                  unit: "%",
                  scale: 100,
                  order: 7,
                },
              },
            }
          : {}),
        ...(fillKind === "image" && imageChoices !== null
          ? {
              image: {
                type: "string",
                oneOf: imageChoices.map((i) => ({ const: i.id, title: i.name })),
                "x-ui": { label: t("inspector.sequence.image"), widget: "select", order: 6 },
              },
              image_fit: {
                type: "string",
                oneOf: FITS.map((f) => ({ const: f, title: t(FIT_LABEL[f]) })),
                "x-ui": { label: t("inspector.sequence.fit"), widget: "select", order: 7 },
              },
            }
          : {}),
      },
    }),
    [rates, t, fillKind, blurMax, canPick, imageChoices],
  );
  const value = {
    width: format.width,
    height: format.height,
    reframe,
    frame_rate: rateKey(format.frame_rate),
    background: format.background,
    fill: fillKind,
    blur_radius: blurRadius,
    blur_dim: blurDim,
    image: imageId ?? "",
    image_fit: imageFit,
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
      if (key === "fill") {
        const kind = next as BackgroundKind;
        if (kind === fillKind) return settle();
        // An image fill starts with the first picture of the bin; the Image row changes it.
        const first = imageChoices?.[0]?.id;
        if (kind === "image" && first === undefined) return settle();
        void editor
          .setSequenceBackground(
            sequence,
            kind === "image" && first !== undefined ? { kind, media: first } : { kind },
            t("inspector.sequence.fillName"),
          )
          .finally(settle);
        return;
      }
      if (key === "image" || key === "image_fit") {
        const media = key === "image" ? (next as MediaId) : imageId;
        const fit = key === "image_fit" ? (next as BackgroundFit) : imageFit;
        if (media === null || (media === imageId && fit === imageFit)) return settle();
        void editor
          .setSequenceBackground(
            sequence,
            { kind: "image", media, fit },
            t(key === "image" ? "inspector.sequence.imageName" : "inspector.sequence.fitName"),
          )
          .finally(settle);
        return;
      }
      if (key === "blur_radius" || key === "blur_dim") {
        const amount = Number(next);
        const radius = key === "blur_radius";
        if (!Number.isFinite(amount) || amount === (radius ? blurRadius : blurDim)) {
          return settle();
        }
        void editor
          .setSequenceBackground(
            sequence,
            radius ? { kind: "blur", radius: amount } : { kind: "blur", dim: amount },
            t(radius ? "inspector.sequence.blurName" : "inspector.sequence.dimName"),
          )
          .finally(settle);
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
    [
      editor,
      sequence,
      format,
      rates,
      reframe,
      t,
      fillKind,
      blurRadius,
      blurDim,
      imageChoices,
      imageId,
      imageFit,
    ],
  );
  return <SchemaForm aria-label={label} schema={schema} value={value} onChange={onChange} />;
}
