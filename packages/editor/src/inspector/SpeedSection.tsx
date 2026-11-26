// SPDX-License-Identifier: GPL-3.0-or-later
// The Speed section of the Inspector (WP-2H.03b): the clip's speed as text the engine parses
// (`2x`, `50%`, `1/3`), Reverse, what a change keeps (the clip's length or its media), whether
// keyframes stretch, the section's reset (Reset speed) and Freeze frame. Every edit is `clip.set_speed`, `clip.reverse`,
// `clip.reset_speed` or `clip.freeze_frame`. It shows for clips that play moving pictures (not for
// stills, freeze frames or sound): the engine refuses those with `Unsupported`.
//
// Not here, because the engine has none yet: preserve pitch (WP-2D.02).
import type { Project, Speed } from "@cutweave/protocol";
import {
  IconButton,
  InspectorRow,
  SegmentedControl,
  Switch,
  TextField,
  Tooltip,
} from "@cutweave/ui";
import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { useViewStore } from "../state/viewStore.ts";
import type { InspectorEditor, SpeedOptions } from "./editor.ts";
import type { SelectedClip } from "./fields.ts";
import { Section } from "./Section.tsx";

/** A clip that plays moving pictures: video media on a video track, not a freeze frame. */
export function playsMovingPicture(project: Project | null, clip: SelectedClip): boolean {
  const track = project?.sequences[clip.sequence]?.tracks[clip.trackId];
  const media = project?.media[clip.item.media as keyof typeof project.media];
  return track?.kind === "video" && media?.kind === "video" && (clip.item.freeze ?? null) === null;
}

const speedOf = (clip: SelectedClip): Speed => clip.item.speed ?? { num: 1, den: 1 };

/** The speed as text the engine takes back: `2x`, `0.5x`, or `1/3` when it has no short decimal. */
export function formatSpeed({ num, den }: Speed): string {
  if (den === 1) return `${num}x`;
  const decimal = num / den;
  const short = Number(decimal.toFixed(4));
  return Math.abs(short * den - num) < 1e-9 ? `${short}x` : `${num}/${den}`;
}

const isNormal = (clip: SelectedClip): boolean => {
  const s = speedOf(clip);
  return s.num === s.den && clip.item.reverse !== true;
};

export function SpeedSection({
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
  const { t } = useTranslation();
  const id = useId();
  const playhead = useViewStore((s) => s.playhead);
  const [keep, setKeep] = useState<SpeedOptions["keep"]>("in_point");
  const [scaleKeys, setScaleKeys] = useState(false);
  const [ripple, setRipple] = useState(false);
  const [draft, setDraft] = useState<string | null>(null);
  if (clips.length === 0 || !clips.every((c) => playsMovingPicture(project, c))) return null;

  const options: SpeedOptions = { keep, scaleKeys, ripple };
  const first = clips[0];
  const texts = new Set(clips.map((c) => formatSpeed(speedOf(c))));
  const mixed = texts.size > 1;
  const shown = mixed || first === undefined ? "" : formatSpeed(speedOf(first));
  const reversed = clips.every((c) => c.item.reverse === true);
  const name = t("inspector.speed.rate").toLowerCase();
  const only = clips.length === 1 ? first : undefined;
  const inside =
    only !== undefined &&
    playhead >= only.item.start &&
    playhead < only.item.start + only.item.duration;

  const commit = (): void => {
    const next = draft?.trim();
    setDraft(null);
    if (next === undefined || next === "" || (!mixed && next === shown)) return;
    void editor.setSpeed(clips, next, options, name);
  };
  const freezeReason: string | null =
    only === undefined
      ? t("inspector.speed.freezeNeedsOne")
      : inside
        ? null
        : t("inspector.speed.freezeNeedsPlayhead");
  const reset = (): void => void editor.resetSpeed(clips, options, name);

  return (
    <Section
      id="speed"
      label={t("inspector.speed.title")}
      ids={clips.map((c) => c.item.id)}
      modified={!clips.every(isNormal)}
      {...(!locked && { onReset: reset })}
    >
      <div className="cw-inspector__rows" data-testid="speed-inspector">
        <InspectorRow label={t("inspector.speed.rate")} htmlFor={id}>
          <TextField
            id={id}
            numeric
            value={draft ?? shown}
            placeholder={mixed ? t("inspector.mixed") : t("inspector.speed.placeholder")}
            disabled={locked}
            onChange={setDraft}
            onBlur={commit}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                commit();
              } else if (event.key === "Escape") {
                setDraft(null);
              }
            }}
          />
        </InspectorRow>
        <InspectorRow label={t("inspector.speed.reverse")}>
          <Switch
            aria-label={t("inspector.speed.reverse")}
            checked={reversed}
            disabled={locked}
            onCheckedChange={(on) =>
              void editor.setReverse(clips, on, t("inspector.speed.reverse"))
            }
          />
        </InspectorRow>
        <InspectorRow label={t("inspector.speed.length")}>
          <SegmentedControl<SpeedOptions["keep"]>
            aria-label={t("inspector.speed.length")}
            value={keep}
            disabled={locked}
            options={[
              { value: "in_point", label: t("inspector.speed.lengthChanges") },
              { value: "duration", label: t("inspector.speed.lengthStays") },
            ]}
            onChange={setKeep}
          />
        </InspectorRow>
        <InspectorRow label={t("inspector.speed.scaleKeys")}>
          <Switch
            aria-label={t("inspector.speed.scaleKeys")}
            checked={scaleKeys}
            disabled={locked}
            onCheckedChange={setScaleKeys}
          />
        </InspectorRow>
        <InspectorRow label={t("inspector.speed.ripple")}>
          <Switch
            aria-label={t("inspector.speed.ripple")}
            checked={ripple}
            disabled={locked}
            onCheckedChange={setRipple}
          />
        </InspectorRow>
        {/* biome-ignore lint/a11y/useSemanticElements: a fieldset would bring its own border and layout */}
        <div className="cw-inspector__flags" role="group" aria-label={t("inspector.speed.actions")}>
          <Tooltip label={freezeReason ?? t("inspector.speed.freeze")}>
            <IconButton
              size="sm"
              icon="image"
              label={t("inspector.speed.freeze")}
              disabled={locked || freezeReason !== null}
              onClick={() => {
                if (only !== undefined) {
                  void editor.freezeFrame(only, playhead, t("inspector.speed.freeze"));
                }
              }}
            />
          </Tooltip>
        </div>
      </div>
    </Section>
  );
}
