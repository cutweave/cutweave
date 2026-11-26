// SPDX-License-Identifier: GPL-3.0-or-later
// The Audio section of the Inspector (WP-2H.03b): volume and pan from the `item.set_props` schema
// (the properties of `x-ui.group` `audio`, with the keyframe diamonds of the Video sections), and
// the fades (`audio.set_fade`). It shows for clips on audio tracks. The section's reset puts
// volume, pan and fades back (`item.reset_props` group `audio`).
//
// Not here, because the engine has none yet: preserve pitch (WP-2D.02) and normalize.
import type { FadeCurve, Project } from "@cutweave/protocol";
import { FLICKS_PER_SECOND } from "@cutweave/timeline";
import { type JsonSchema, mergeSelection, SchemaForm } from "@cutweave/ui";
import { useCallback, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TKey } from "../i18n/index.ts";
import { useViewStore } from "../state/viewStore.ts";
import type { InspectorEditor } from "./editor.ts";
import type { SelectedClip } from "./fields.ts";
import { useEvaluated } from "./useEvaluated.ts";
import { useVideoSections, VideoSectionView } from "./VideoSections.tsx";
import { audioValue } from "./video.ts";

const CURVES: readonly FadeCurve[] = ["linear", "log", "equal_power", "s_curve"];
const CURVE_LABEL: Readonly<Record<FadeCurve, TKey>> = {
  linear: "inspector.audio.curves.linear",
  log: "inspector.audio.curves.log",
  equal_power: "inspector.audio.curves.equal_power",
  s_curve: "inspector.audio.curves.s_curve",
};

const FADE_NAME: Readonly<Record<"fade_in" | "fade_out" | "fade_curve", TKey>> = {
  fade_in: "inspector.audio.fadeInName",
  fade_out: "inspector.audio.fadeOutName",
  fade_curve: "inspector.audio.fadeShapeName",
};

/** The selected clips that are sound: on an audio track. */
export function audioClips(
  project: Project | null,
  clips: readonly SelectedClip[],
): SelectedClip[] {
  return clips.filter((c) => project?.sequences[c.sequence]?.tracks[c.trackId]?.kind === "audio");
}

type FadeKey = "fade_in" | "fade_out" | "fade_curve";

/** A clip's fades as the form shows them: seconds and the shape. */
function fadeValue(clip: SelectedClip): Record<FadeKey, unknown> {
  const { fade_in, fade_out } = clip.item.audio;
  return {
    fade_in: (fade_in?.duration ?? 0) / FLICKS_PER_SECOND,
    fade_out: (fade_out?.duration ?? 0) / FLICKS_PER_SECOND,
    fade_curve: fade_in?.curve ?? fade_out?.curve ?? "linear",
  };
}

export function AudioSection({
  project,
  clips,
  editor,
  locked,
}: {
  readonly project: Project | null;
  readonly clips: readonly SelectedClip[];
  readonly editor: InspectorEditor;
  readonly locked: boolean;
}) {
  const sound = useMemo(() => audioClips(project, clips), [project, clips]);
  const { props, audio } = useVideoSections();
  const playhead = useViewStore((s) => s.playhead);
  const evaluated = useEvaluated(sound, playhead, "audio");
  const perClip = useMemo(
    () => sound.map((c, i) => audioValue(c.item, evaluated[i])),
    [sound, evaluated],
  );
  const merged = useMemo(() => mergeSelection(perClip), [perClip]);
  const fades = useMemo(() => mergeSelection(sound.map(fadeValue)), [sound]);
  const modified = useMemo(
    () =>
      sound.some((c) => {
        const f = fadeValue(c);
        return f.fade_in !== 0 || f.fade_out !== 0;
      }),
    [sound],
  );
  if (audio === null || sound.length === 0) return null;
  return (
    <VideoSectionView
      section={audio}
      props={props}
      clips={sound}
      editor={editor}
      locked={locked}
      playhead={playhead}
      perClip={perClip}
      value={merged.value}
      mixed={merged.mixed}
      extraModified={modified}
      extra={
        <Fades
          clips={sound}
          editor={editor}
          locked={locked}
          value={fades.value}
          mixed={fades.mixed}
        />
      }
    />
  );
}

function Fades({
  clips,
  editor,
  locked,
  value,
  mixed,
}: {
  readonly clips: readonly SelectedClip[];
  readonly editor: InspectorEditor;
  readonly locked: boolean;
  readonly value: unknown;
  readonly mixed: readonly string[];
}) {
  const { t } = useTranslation();
  // What a drag or a half-typed value shows until it is committed.
  const [draft, setDraft] = useState<Partial<Record<FadeKey, unknown>>>({});
  const longest = Math.min(...clips.map((c) => c.item.duration)) / FLICKS_PER_SECOND;
  const schema = useMemo<JsonSchema>(
    () => ({
      type: "object",
      properties: {
        fade_in: {
          type: "number",
          minimum: 0,
          maximum: longest,
          "x-ui": { label: t("inspector.audio.fadeIn"), unit: "s", step: 0.1, precision: 2 },
        },
        fade_out: {
          type: "number",
          minimum: 0,
          maximum: longest,
          "x-ui": { label: t("inspector.audio.fadeOut"), unit: "s", step: 0.1, precision: 2 },
        },
        fade_curve: {
          type: "string",
          oneOf: CURVES.map((c) => ({ const: c, title: t(CURVE_LABEL[c]) })),
          "x-ui": { label: t("inspector.audio.fadeShape"), widget: "select" },
        },
      },
    }),
    [longest, t],
  );
  const defaults = useMemo(() => ({ fade_in: 0, fade_out: 0, fade_curve: "linear" }), []);
  const shown = { ...(value as Record<string, unknown>), ...draft };
  const onChange = useCallback(
    (path: string, next: unknown, meta: { gesture?: "start" | "update" | "end" | undefined }) => {
      const key = path as FadeKey;
      if (meta.gesture === "start" || meta.gesture === "update") {
        setDraft((d) => ({ ...d, [key]: next }));
        return;
      }
      const settle = (): void => setDraft(({ [key]: _gone, ...rest }) => rest);
      const name = t(FADE_NAME[key]);
      const patch =
        key === "fade_curve"
          ? { curve: next as FadeCurve }
          : { [key === "fade_in" ? "in" : "out"]: Math.max(0, Number(next)) };
      void editor.setFades(clips, patch, name).finally(settle);
    },
    [clips, editor, t],
  );
  return (
    <SchemaForm
      aria-label={t("inspector.audio.fadeIn")}
      schema={schema}
      value={shown}
      defaults={defaults}
      mixed={mixed.filter((p) => !(p in draft))}
      disabled={locked}
      onChange={onChange}
      onReset={(path) => {
        const key = path as FadeKey;
        const patch =
          key === "fade_curve"
            ? { curve: "linear" as const }
            : { [key === "fade_in" ? "in" : "out"]: 0 };
        void editor.setFades(clips, patch, t(FADE_NAME[key]));
      }}
    />
  );
}
