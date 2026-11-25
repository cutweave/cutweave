// SPDX-License-Identifier: GPL-3.0-or-later
import type { MediaAsset } from "@cutweave/protocol";
import { formatTimecode } from "@cutweave/timeline";
import { InspectorRow, InspectorSection, NumberScrubber, Slider, TextField } from "@cutweave/ui";
import { type ReactNode, useEffect, useId, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { engineStore } from "../engine/engineStore.ts";
import type { TKey } from "../i18n/index.ts";
import { selectSelectedClips, selectSelectedMedia } from "../inspector/clips.ts";
import { InspectorEditor } from "../inspector/editor.ts";
import {
  FIELDS,
  type FieldDef,
  type NumberField,
  readField,
  SECTIONS,
  type SelectedClip,
  type TextField as TextFieldDef,
  type TimeField,
} from "../inspector/fields.ts";
import { MediaPreview } from "../inspector/MediaPreview.tsx";
import { type InspectorSectionId, useSection } from "../inspector/sections.ts";
import { ContextSurface } from "../menus/host.tsx";
import { useDocStore } from "../state/docStore.ts";
import { selectSequenceSummary } from "../state/selectors.ts";
import { toastStore } from "../state/toastStore.ts";
import { useViewStore } from "../state/viewStore.ts";
import { parseTimeInput, type TimeInputError } from "../time/timeInput.ts";
import { translator } from "../timeline/errors.ts";
import { EmptyState } from "./EmptyState.tsx";

const TIME_ERROR: Record<TimeInputError, TKey> = {
  syntax: "inspector.time.invalid",
  needs_rate: "inspector.time.needsRate",
  range: "inspector.time.range",
  invalid: "inspector.time.frames",
};

const MEDIA_KIND: Record<MediaAsset["kind"], TKey> = {
  video: "inspector.media.kinds.video",
  audio: "inspector.media.kinds.audio",
  image: "inspector.media.kinds.image",
  sequence_ref: "inspector.media.kinds.sequence",
};

/** How long a committed value stays on screen waiting for the document's patch. */
const COMMIT_SETTLE_MS = 500;

/** One editor per inspector panel; an open drag rolls back when the panel goes away. */
function useInspectorEditor(): InspectorEditor {
  const { t } = useTranslation();
  const latest = useRef(translator(t));
  latest.current = translator(t);
  const editor = useMemo(
    () =>
      new InspectorEditor({
        engine: engineStore,
        toasts: toastStore,
        t: (key, vars) => latest.current(key, vars),
      }),
    [],
  );
  useEffect(
    () => () => {
      void editor.dispose();
    },
    [editor],
  );
  return editor;
}

interface RowProps<F extends FieldDef> {
  readonly field: F;
  readonly clips: readonly SelectedClip[];
  readonly editor: InspectorEditor;
  readonly disabled: boolean;
}

/** A name field: commits on Enter or blur, Escape puts the old text back. */
function NameRow({ field, clips, editor, disabled }: RowProps<TextFieldDef>) {
  const { t } = useTranslation();
  const id = useId();
  const current = readField(clips, (item) => field.read(item));
  const [draft, setDraft] = useState<string | null>(null);
  const commit = (): void => {
    const next = draft;
    setDraft(null);
    if (next === null || (!current.mixed && next === current.value)) return;
    if (current.mixed && next === "") return;
    void editor.apply(field, clips, next);
  };
  return (
    <InspectorRow label={t(field.label)} htmlFor={id}>
      <TextField
        id={id}
        value={draft ?? (current.mixed ? "" : current.value)}
        placeholder={current.mixed ? t("inspector.mixed") : undefined}
        disabled={disabled}
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
  );
}

/** Start or duration as a timecode (or seconds, or `f:N`); one clip at a time. */
function TimeRow({ field, clips, editor, disabled }: RowProps<TimeField>) {
  const { t } = useTranslation();
  const id = useId();
  const only = clips.length === 1 ? clips[0] : undefined;
  const [draft, setDraft] = useState<string | null>(null);
  const [error, setError] = useState<TimeInputError | null>(null);
  const flicks = only === undefined ? null : field.read(only.item);
  const shown = only === undefined || flicks === null ? "" : formatTimecode(flicks, only.fps);
  const commit = (): void => {
    if (only === undefined || draft === null) return;
    const parsed = parseTimeInput(draft, only.fps);
    if (!parsed.ok) {
      setError(parsed.error);
      return;
    }
    setError(null);
    setDraft(null);
    if (parsed.flicks !== flicks) void editor.apply(field, clips, parsed.flicks);
  };
  return (
    <>
      <InspectorRow label={t(field.label)} htmlFor={id}>
        <TextField
          id={id}
          numeric
          value={draft ?? shown}
          placeholder={only === undefined ? t("inspector.mixed") : undefined}
          disabled={disabled || only === undefined}
          invalid={error !== null}
          aria-describedby={error === null ? undefined : `${id}-error`}
          onChange={(value) => {
            setDraft(value);
            setError(null);
          }}
          onBlur={() => {
            if (error === null) commit();
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              commit();
            } else if (event.key === "Escape") {
              setDraft(null);
              setError(null);
            }
          }}
        />
      </InspectorRow>
      {error === null ? null : (
        <p id={`${id}-error`} className="cw-inspector__invalid label" role="alert">
          {t(TIME_ERROR[error])}
        </p>
      )}
    </>
  );
}

/** A slider or scrubber: a drag is one gesture, release commits one undo entry. */
function NumberRow({ field, clips, editor, disabled }: RowProps<NumberField>) {
  const { t } = useTranslation();
  const current = readField(clips, (item) => field.read(item));
  // What the control shows while the engine catches up: the dragged value, until the document's
  // patch shows it (or, for a refused edit, a moment after the commit answered).
  const [draft, setDraft] = useState<number | null>(null);
  const [committed, setCommitted] = useState(false);
  const settle = useRef<ReturnType<typeof setTimeout> | null>(null);
  const stopSettling = (): void => {
    if (settle.current !== null) clearTimeout(settle.current);
    settle.current = null;
  };
  useEffect(() => stopSettling, []);
  useEffect(() => {
    if (committed && draft !== null && !current.mixed && current.value === draft) {
      stopSettling();
      setDraft(null);
      setCommitted(false);
    }
  }, [committed, draft, current.mixed, current.value]);
  const value = draft ?? current.value;
  const mixed = current.mixed && draft === null;
  // What the control shows right now. A ref, because the slider calls these handlers from its own
  // effects, with whatever closure it kept: the echo it sends must see the latest value.
  const shown = useRef({ value, drafted: draft !== null, mixed: current.mixed });
  shown.current = { value, drafted: draft !== null, mixed: current.mixed };
  const onChange = (next: number): void => {
    // A controlled slider echoes the value it shows whenever its prop changes: not an edit.
    const now = shown.current;
    if (next === now.value && (now.drafted || !now.mixed)) return;
    shown.current = { value: next, drafted: true, mixed: now.mixed };
    stopSettling();
    setCommitted(false);
    setDraft(next);
    editor.live(field, clips, next);
  };
  const onCommit = (next: number): void => {
    // The slider reports a key press's commit before its change: show the value now, so the
    // change that follows is recognised as an echo and does not start another edit.
    shown.current = { value: next, drafted: true, mixed: shown.current.mixed };
    stopSettling();
    setDraft(next);
    void editor.commit(field, clips, next).finally(() => {
      setCommitted(true);
      settle.current = setTimeout(() => {
        setDraft(null);
        setCommitted(false);
      }, COMMIT_SETTLE_MS);
    });
  };
  if (field.control === "slider") {
    return (
      <Slider
        label={t(field.label)}
        value={value}
        min={field.min}
        max={field.max}
        step={field.step}
        disabled={disabled}
        format={(v) => (mixed ? t("inspector.mixed") : `${v} ${field.unit ?? ""}`.trim())}
        onChange={onChange}
        onCommit={onCommit}
      />
    );
  }
  return (
    <InspectorRow label={t(field.label)}>
      <NumberScrubber
        aria-label={t(field.label)}
        value={value}
        mixed={mixed}
        min={field.min}
        max={field.max}
        step={field.step}
        {...(field.unit !== undefined && { unit: field.unit })}
        {...(field.precision !== undefined && { precision: field.precision })}
        disabled={disabled}
        onChange={onChange}
        onCommit={onCommit}
      />
    </InspectorRow>
  );
}

/** What "Copy value" copies for a row: the text, the timecode or the number with its unit. */
function rowText(field: FieldDef, clips: readonly SelectedClip[]): string {
  const only = clips.length === 1 ? clips[0] : undefined;
  if (field.control === "time") {
    const flicks = only === undefined ? null : field.read(only.item);
    return only === undefined || flicks === null ? "" : formatTimecode(flicks, only.fps);
  }
  const current = readField(clips, field.read as (item: SelectedClip["item"]) => string | number);
  if (current.mixed) return "";
  const unit = "unit" in field && field.unit !== undefined ? ` ${field.unit}` : "";
  return `${current.value}${unit}`;
}

function FieldRow({ field, ...rest }: RowProps<FieldDef>) {
  const { t } = useTranslation();
  const row =
    field.control === "text" ? (
      <NameRow field={field} {...rest} />
    ) : field.control === "time" ? (
      <TimeRow field={field} {...rest} />
    ) : (
      <NumberRow field={field} {...rest} />
    );
  // Right-click on the label (a field has the text menu): Copy value.
  return (
    <ContextSurface
      surface="inspector.row"
      className="cw-context-surface"
      target={() => ({ name: t(field.label), data: { text: rowText(field, rest.clips) } })}
    >
      {row}
    </ContextSurface>
  );
}

/** A collapsible section whose header menu collapses or expands them all (WP-2H.13). */
function Section({
  id,
  label,
  children,
}: {
  readonly id: InspectorSectionId;
  readonly label: string;
  readonly children: ReactNode;
}) {
  const [expanded, setExpanded] = useSection(id);
  return (
    <ContextSurface
      surface="inspector.section"
      className="cw-context-surface"
      target={() => ({ name: label })}
    >
      <InspectorSection label={label} expanded={expanded} onExpandedChange={setExpanded}>
        {children}
      </InspectorSection>
    </ContextSurface>
  );
}

/** The inspector of one or more selected clips. */
function ClipInspector({
  clips,
  editor,
}: {
  readonly clips: readonly SelectedClip[];
  readonly editor: InspectorEditor;
}) {
  const { t } = useTranslation();
  const locked = clips.some((c) => c.locked);
  return (
    <div className="cw-inspector" data-testid="clip-inspector">
      {clips.length > 1 ? (
        <p className="cw-inspector__note body">
          {t("inspector.multiple", { count: clips.length })}
        </p>
      ) : null}
      {locked ? <p className="cw-inspector__note body">{t("timeline.reason.locked")}</p> : null}
      {SECTIONS.map((section) => {
        const fields = FIELDS.filter((f) => f.section === section.id);
        return (
          <Section key={section.id} id={section.id} label={t(section.label)}>
            <div className="cw-inspector__rows">
              {fields.map((field) => (
                <FieldRow
                  key={field.id}
                  field={field}
                  clips={clips}
                  editor={editor}
                  disabled={locked}
                />
              ))}
              {section.id === "timing" && clips.length > 1 ? (
                <p className="cw-inspector__note label">{t("inspector.multipleTime")}</p>
              ) : null}
            </div>
          </Section>
        );
      })}
    </div>
  );
}

function fileName(path: string | null): string | null {
  return path === null ? null : (path.split(/[\\/]/).filter(Boolean).at(-1) ?? path);
}

/** Probe facts of a media asset selected in the bin. */
function MediaInfo({ media }: { readonly media: MediaAsset }) {
  const { t } = useTranslation();
  const video = media.probe.streams.find((s) => s.kind === "video");
  const fps = media.probe.fps;
  const timeBase = fps ?? { num: 30, den: 1 };
  const rate = fps === null ? null : Math.round((fps.num / fps.den) * 1000) / 1000;
  const audio = media.probe.audio;
  const stub = media.probe.stub === true;
  const name = fileName(media.path_abs ?? media.path_rel);
  return (
    <div className="cw-inspector" data-testid="media-inspector">
      <Section id="media" label={t("inspector.media.title")}>
        <ContextSurface
          surface="inspector.picture"
          className="cw-context-surface"
          target={() => ({ ids: [media.id] })}
        >
          <MediaPreview key={media.id} media={media} />
        </ContextSurface>
        <div className="cw-inspector__rows">
          {name === null ? null : (
            <ContextSurface
              surface="inspector.path"
              className="cw-context-surface"
              target={() => ({ ids: [media.id] })}
            >
              <InspectorRow label={t("inspector.media.path")}>
                <span className="numeric">{name}</span>
              </InspectorRow>
            </ContextSurface>
          )}
          <InspectorRow label={t("inspector.media.kind")}>
            <span className="body">{t(MEDIA_KIND[media.kind])}</span>
          </InspectorRow>
          {media.probe.duration === null ? null : (
            <InspectorRow label={t("inspector.media.duration")}>
              <span className="numeric">{formatTimecode(media.probe.duration, timeBase)}</span>
            </InspectorRow>
          )}
          {video?.width == null || video.height == null ? null : (
            <InspectorRow label={t("inspector.media.resolution")}>
              <span className="numeric">{`${video.width} × ${video.height}`}</span>
            </InspectorRow>
          )}
          {rate === null ? null : (
            <InspectorRow label={t("inspector.media.frameRate")}>
              <span className="numeric">{`${rate} fps`}</span>
            </InspectorRow>
          )}
          {video?.codec == null ? null : (
            <InspectorRow label={t("inspector.media.video")}>
              <span className="numeric">{video.codec}</span>
            </InspectorRow>
          )}
          {audio === null ? null : (
            <InspectorRow label={t("inspector.media.audioStream")}>
              <span className="numeric">
                {t("inspector.media.audioValue", {
                  rate: String(audio.rate),
                  channels: String(audio.channels),
                })}
              </span>
            </InspectorRow>
          )}
          {stub ? (
            <p className="cw-inspector__note label">{t("inspector.media.placeholder")}</p>
          ) : null}
        </div>
      </Section>
    </div>
  );
}

/** Nothing selected: the empty state and the project's settings. */
function ProjectInspector() {
  const { t } = useTranslation();
  const project = useDocStore((s) => s.project);
  const summary = useMemo(() => selectSequenceSummary(project), [project]);
  const rate =
    summary === null
      ? null
      : Math.round((summary.frameRate.num / summary.frameRate.den) * 1000) / 1000;
  return (
    <div className="cw-inspector">
      <EmptyState
        icon="sliders-horizontal"
        title={t("inspector.emptyTitle")}
        body={t("inspector.emptyBody")}
      />
      {summary === null ? null : (
        <Section id="project" label={t("inspector.project")}>
          <InspectorRow label={t("inspector.resolution")}>
            <span className="numeric" data-testid="project-resolution">
              {t("inspector.resolutionValue", { width: summary.width, height: summary.height })}
            </span>
          </InspectorRow>
          <InspectorRow label={t("inspector.frameRate")}>
            <span className="numeric" data-testid="project-frame-rate">
              {t("inspector.frameRateValue", { fps: rate })}
            </span>
          </InspectorRow>
          <InspectorRow label={t("inspector.duration")}>
            <span className="numeric" data-testid="project-duration">
              {formatTimecode(summary.duration, summary.frameRate)}
            </span>
          </InspectorRow>
        </Section>
      )}
    </div>
  );
}

/**
 * Right dock panel (WP-1M.07): the selected clip's properties, a media asset's probe info, or the
 * project when nothing is selected. Every edit is an engine command: sliders and scrubbers send
 * `item.set_props` in a gesture, start and duration send `item.move` and `clip.trim`.
 */
export function InspectorPanel() {
  const selection = useViewStore((s) => s.selection);
  const project = useDocStore((s) => s.project);
  const editor = useInspectorEditor();
  const clips = useMemo(() => selectSelectedClips(project, selection), [project, selection]);
  const media = useMemo(() => selectSelectedMedia(project, selection), [project, selection]);
  if (clips.length > 0) return <ClipInspector clips={clips} editor={editor} />;
  if (media !== null) return <MediaInfo media={media} />;
  return <ProjectInspector />;
}
