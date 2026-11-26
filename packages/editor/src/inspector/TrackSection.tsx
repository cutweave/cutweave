// SPDX-License-Identifier: GPL-3.0-or-later
// The Track section: the track the selected clips sit on (name, hide, mute, lock, gain). Edits are
// `track.set`. Clips of several tracks show a note instead, because one name cannot cover them.
import type { Project, Track } from "@cutweave/protocol";
import {
  IconButton,
  type IconName,
  InspectorRow,
  NumberScrubber,
  TextField,
  Tooltip,
} from "@cutweave/ui";
import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import type { InspectorEditor } from "./editor.ts";
import type { SelectedClip } from "./fields.ts";
import { TRACK_FLAGS } from "./labels.ts";
import { Section } from "./Section.tsx";

/** The one track every clip sits on, or `null` when they sit on several. */
export function commonTrack(project: Project | null, clips: readonly SelectedClip[]): Track | null {
  const first = clips[0];
  if (project === null || first === undefined) return null;
  if (clips.some((c) => c.trackId !== first.trackId)) return null;
  return project.sequences[first.sequence]?.tracks[first.trackId] ?? null;
}

export function TrackSection({
  project,
  clips,
  editor,
}: {
  readonly project: Project | null;
  readonly clips: readonly SelectedClip[];
  readonly editor: InspectorEditor;
}) {
  const { t } = useTranslation();
  const track = commonTrack(project, clips);
  return (
    <Section id="track" label={t("inspector.track.title")}>
      {track === null ? (
        <p className="cw-inspector__note label">{t("inspector.track.several")}</p>
      ) : (
        <TrackRows key={track.id} track={track} editor={editor} />
      )}
    </Section>
  );
}

function TrackRows({ track, editor }: { readonly track: Track; readonly editor: InspectorEditor }) {
  const { t } = useTranslation();
  const id = useId();
  const [draft, setDraft] = useState<string | null>(null);
  const [gain, setGain] = useState<number | null>(null);
  const commitName = (): void => {
    const next = draft?.trim();
    setDraft(null);
    if (next === undefined || next === "" || next === track.name) return;
    void editor.setTrack(track.id, { name: next }, t("inspector.track.name").toLowerCase());
  };
  const flag = (
    key: "hidden" | "muted" | "locked",
    on: IconName,
    off: IconName,
    visible: boolean,
  ) =>
    visible ? (
      <Tooltip label={t(TRACK_FLAGS[key])}>
        <IconButton
          size="sm"
          icon={track[key] ? on : off}
          label={t(TRACK_FLAGS[key])}
          pressed={track[key]}
          onClick={() =>
            void editor.setTrack(
              track.id,
              { [key]: !track[key] },
              t(TRACK_FLAGS[key]).toLowerCase(),
            )
          }
        />
      </Tooltip>
    ) : null;
  return (
    <div className="cw-inspector__rows" data-testid="track-inspector">
      <InspectorRow label={t("inspector.track.name")} htmlFor={id}>
        <TextField
          id={id}
          value={draft ?? track.name}
          onChange={setDraft}
          onBlur={commitName}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              commitName();
            } else if (event.key === "Escape") {
              setDraft(null);
            }
          }}
        />
      </InspectorRow>
      <InspectorRow label={t("inspector.track.gain")}>
        <NumberScrubber
          aria-label={t("inspector.track.gain")}
          value={gain ?? track.gain}
          min={-144}
          max={48}
          step={0.5}
          precision={1}
          unit="dB"
          onChange={setGain}
          onCommit={(next) => {
            setGain(next);
            void editor
              .setTrack(track.id, { gain: next }, t("inspector.track.gain").toLowerCase())
              .finally(() => setGain(null));
          }}
        />
      </InspectorRow>
      {/* biome-ignore lint/a11y/useSemanticElements: a fieldset would bring its own border and layout */}
      <div className="cw-inspector__flags" role="group" aria-label={t("inspector.track.title")}>
        {flag("hidden", "eye-off", "eye", track.kind === "video")}
        {flag("muted", "volume-x", "volume-2", track.kind === "audio")}
        {flag("locked", "lock", "unlock", true)}
      </div>
    </div>
  );
}
