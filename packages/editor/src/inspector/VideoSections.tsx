// SPDX-License-Identifier: GPL-3.0-or-later
// The Video sections of the Inspector (WP-2H.03a): one form per part of the `item.set_props`
// schema (transform, crop, compositing, flip, and any property a newer schema adds), with the
// keyframe diamonds, row and section resets, and several clips edited at once.
import type { PropId } from "@cutweave/protocol";
import {
  deepEqual,
  type FieldKeyframe,
  getPath,
  humanize,
  type JsonSchema,
  mergeSelection,
  SchemaForm,
  type UiHints,
} from "@cutweave/ui";
import { useCallback, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import { ContextSurface } from "../menus/host.tsx";
import type { RowData } from "../menus/surfaces/inspector.ts";
import { commandCatalog } from "../palette/catalog.ts";
import { useViewStore } from "../state/viewStore.ts";
import { transportController } from "../transport/useTransport.ts";
import type { InspectorEditor, PropEdit } from "./editor.ts";
import type { SelectedClip } from "./fields.ts";
import { GROUP_LABELS, PROP_LABELS, PROP_NAMES } from "./labels.ts";
import { Section } from "./Section.tsx";
import { useEvaluated } from "./useEvaluated.ts";
import {
  ANIMATABLE,
  fullPath,
  keyFacts,
  keyframeablePaths,
  propsPatch,
  propsSchema,
  sectionDefaults,
  type VideoSection,
  videoSections,
  visualValue,
} from "./video.ts";

/** The Video sections of the schema, which the command catalog carries. */
export function useVideoSections(): {
  readonly sections: readonly VideoSection[];
  readonly props: JsonSchema | null;
} {
  const commands = useStore(commandCatalog, (s) => s.commands);
  return useMemo(() => {
    const input = commands.find((c) => c.name === "item.set_props")?.inputSchema as
      | JsonSchema
      | undefined;
    return input === undefined
      ? { sections: [], props: null }
      : { sections: videoSections(input), props: propsSchema(input) };
  }, [commands]);
}

/** What a row copies: the shown value, as numbers (or on/off). */
function copyText(value: unknown): string {
  if (typeof value === "object" && value !== null) {
    return Object.values(value)
      .map((v) => String(v))
      .join(", ");
  }
  return value === undefined ? "" : String(value);
}

export function VideoSections({
  clips,
  editor,
  locked,
}: {
  readonly clips: readonly SelectedClip[];
  readonly editor: InspectorEditor;
  readonly locked: boolean;
}) {
  const { sections, props } = useVideoSections();
  const playhead = useViewStore((s) => s.playhead);
  const evaluated = useEvaluated(clips, playhead);
  const perClip = useMemo(
    () => clips.map((c, i) => visualValue(c.item, evaluated[i])),
    [clips, evaluated],
  );
  const merged = useMemo(() => mergeSelection(perClip), [perClip]);
  return (
    <>
      {sections.map((section) => (
        <VideoSectionView
          key={section.id}
          section={section}
          props={props}
          clips={clips}
          editor={editor}
          locked={locked}
          playhead={playhead}
          perClip={perClip}
          value={merged.value}
          mixed={merged.mixed}
        />
      ))}
    </>
  );
}

function VideoSectionView({
  section,
  props,
  clips,
  editor,
  locked,
  playhead,
  perClip,
  value,
  mixed,
}: {
  readonly section: VideoSection;
  readonly props: JsonSchema | null;
  readonly clips: readonly SelectedClip[];
  readonly editor: InspectorEditor;
  readonly locked: boolean;
  readonly playhead: number;
  readonly perClip: readonly Record<string, unknown>[];
  readonly value: unknown;
  readonly mixed: readonly string[];
}) {
  const { t } = useTranslation();
  const groupKey = GROUP_LABELS[section.id];
  const label = groupKey === undefined ? humanize(section.id) : t(groupKey);
  const propName = useCallback(
    (full: string): string => {
      const key = PROP_NAMES[full];
      return key === undefined ? humanize(full.split(".").at(-1) ?? full).toLowerCase() : t(key);
    },
    [t],
  );

  const local = useMemo(
    () => (section.prefix === "" ? value : getPath(value, section.prefix)),
    [section.prefix, value],
  );
  const localMixed = useMemo(
    () =>
      section.prefix === ""
        ? [...mixed]
        : mixed
            .filter((p) => p === section.prefix || p.startsWith(`${section.prefix}.`))
            .map((p) => (p === section.prefix ? "" : p.slice(section.prefix.length + 1))),
    [section.prefix, mixed],
  );
  const defaults = useMemo(() => sectionDefaults(section), [section]);
  const hints = useMemo(() => {
    const out: Record<string, UiHints> = {};
    for (const path of new Set([
      ...Object.keys(defaults),
      ...Object.keys(section.schema.properties ?? {}),
    ])) {
      const key = PROP_LABELS[fullPath(section, path)];
      if (key !== undefined) out[path] = { label: t(key) };
    }
    return out;
  }, [defaults, section, t]);

  const edit = useCallback(
    (path: string, next: unknown): PropEdit => {
      const full = fullPath(section, path);
      return {
        clips,
        path: full,
        value: next,
        props: propsPatch(props, full, next),
        at: playhead,
        name: propName(full),
        current: perClip.map((v) => getPath(v, full)),
      };
    },
    [section, clips, props, playhead, propName, perClip],
  );

  const onChange = useCallback(
    (path: string, next: unknown, meta: { gesture?: "start" | "update" | "end" | undefined }) => {
      const e = edit(path, next);
      if (meta.gesture === "start" || meta.gesture === "update") editor.liveProp(e);
      else if (meta.gesture === "end") void editor.commitProp(e);
      else void editor.applyProp(e);
    },
    [edit, editor],
  );

  const onReset = useCallback(
    (path: string) => {
      const full = fullPath(section, path);
      if (ANIMATABLE.has(full)) {
        void editor.resetProps(clips, { props: [full as PropId] }, propName(full));
      } else {
        // Not animatable (the blend mode): it is set back like any value.
        const def = defaults[path];
        if (def !== undefined) void editor.applyProp({ ...edit(path, def) });
      }
    },
    [section, clips, editor, propName, defaults, edit],
  );

  const keyframes = useMemo(() => {
    const out: Record<string, FieldKeyframe> = {};
    for (const path of keyframeablePaths(section)) {
      const full = fullPath(section, path);
      const facts = keyFacts(clips, full, playhead);
      const name = propName(full);
      const seek = (to: number | null) => () => {
        if (to !== null) void transportController().seek(to);
      };
      out[path] = {
        state: facts.state,
        disabled: locked || (facts.state !== "on" && !facts.canAdd),
        onToggle: () => {
          void editor.toggleKey(clips, full as PropId, playhead, facts.state === "on", name);
        },
        ...(facts.state === "off"
          ? {}
          : { onPrev: seek(facts.previous), onNext: seek(facts.next) }),
      };
    }
    return out;
  }, [section, clips, playhead, locked, editor, propName]);

  const modified = useMemo(
    () =>
      Object.entries(defaults).some(([path, def]) => {
        const v = getPath(local, path);
        return v !== undefined && !deepEqual(v, def);
      }) || localMixed.length > 0,
    [defaults, local, localMixed],
  );

  const target = useCallback(
    (query: { readonly source: Element }) => {
      const row = query.source.closest<HTMLElement>("[data-path]");
      const path = row?.dataset.path;
      if (path === undefined) return {};
      const full = fullPath(section, path);
      const data: RowData = {
        text: copyText(getPath(local, path)),
        ...(ANIMATABLE.has(full) && { prop: full as PropId }),
      };
      return { name: propName(full), ids: clips.map((c) => c.item.id), data };
    },
    [section, local, clips, propName],
  );

  return (
    <Section
      id={section.id}
      label={label}
      ids={clips.map((c) => c.item.id)}
      {...(section.resetGroup !== undefined && { group: section.resetGroup })}
      modified={modified}
      {...(section.resetGroup !== undefined &&
        !locked && {
          onReset: () => {
            if (section.resetGroup !== undefined) {
              void editor.resetProps(clips, { group: section.resetGroup }, label);
            }
          },
        })}
    >
      <ContextSurface surface="inspector.row" className="cw-context-surface" target={target}>
        <SchemaForm
          aria-label={label}
          schema={section.schema}
          value={local}
          hints={hints}
          defaults={defaults}
          mixed={localMixed}
          keyframes={keyframes}
          disabled={locked}
          onChange={onChange}
          onReset={onReset}
        />
      </ContextSurface>
    </Section>
  );
}
