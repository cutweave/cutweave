// SPDX-License-Identifier: GPL-3.0-or-later
import { formatTimecode } from "@cutweave/timeline";
import { IconButton, Tooltip } from "@cutweave/ui";
import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { engineStore } from "../engine/engineStore.ts";
import { comboText, strokesOf } from "../keymap/store.ts";
import { insertAtPlayhead } from "../media/insert.ts";
import { SOURCE_COMMANDS } from "../preview/sourceKeys.ts";
import { docStore, useDocStore } from "../state/docStore.ts";
import { usePlaybackStore } from "../state/playbackStore.ts";
import { mediaName } from "../state/selectors.ts";
import { toastStore } from "../state/toastStore.ts";
import { viewStore } from "../state/viewStore.ts";
import { translator } from "../timeline/errors.ts";
import { transportController } from "../transport/useTransport.ts";

/** The keys of `action` in the keymap, spelled for a tooltip. */
const keyOfAction = (action: "back" | "insert"): string[] =>
  strokesOf(SOURCE_COMMANDS[action]).map((combo) => comboText(combo));

/** The asset the Source view shows, and what putting it on the timeline needs. */
export function useSourceAsset() {
  const source = usePlaybackStore((s) => s.source);
  const media = useDocStore((s) => (source === null ? undefined : s.project?.media[source.media]));
  return useMemo(
    () =>
      source === null
        ? null
        : {
            id: source.media,
            name: mediaName(media, source.media),
            kind: media?.kind === "audio" ? ("audio" as const) : ("video" as const),
            duration: source.duration,
          },
    [source, media],
  );
}

/** Puts the whole source asset on the timeline at the playhead, as the bin's Enter does. */
export function useInsertSource(): () => void {
  const { t } = useTranslation();
  const asset = useSourceAsset();
  const translate = translator(t);
  return () => {
    if (asset === null) return;
    void insertAtPlayhead(
      {
        doc: docStore,
        view: viewStore,
        engine: engineStore,
        toasts: toastStore,
        t: (key, vars) => translate(key, vars),
      },
      { id: asset.id, kind: asset.kind },
    );
  };
}

/**
 * The Preview's bar while a source is shown (WP-1M.15, ADR-0022): the "Source" label, the asset's
 * name, its own timecode and length at its own rate, Insert (the whole asset at the timeline's
 * playhead) and Back to timeline. Marks and placing part of a clip are WP-2A.14.
 */
export function SourceHeader() {
  const { t } = useTranslation();
  const asset = useSourceAsset();
  const source = usePlaybackStore((s) => s.source);
  const at = usePlaybackStore((s) => s.sourcePlayhead);
  const insert = useInsertSource();
  if (source === null || asset === null) return null;
  const timecode = formatTimecode(at, source.fps, true);
  const length = formatTimecode(source.duration, source.fps, true);
  return (
    <section
      className="cw-preview__bar cw-source__bar"
      aria-label={t("preview.source.region")}
      data-testid="source-header"
    >
      <span className="cw-source__badge label">{t("preview.source.label")}</span>
      <span className="cw-source__name body-strong" title={asset.name}>
        {asset.name}
      </span>
      <span className="cw-source__time timecode">
        <output aria-label={t("preview.source.timecode")} data-testid="source-timecode">
          {timecode}
        </output>
        <span aria-hidden="true">/</span>
        <span data-testid="source-duration">{length}</span>
      </span>
      <Tooltip label={t("preview.source.insert")} keys={keyOfAction("insert")}>
        <IconButton icon="plus" size="sm" label={t("preview.source.insert")} onClick={insert} />
      </Tooltip>
      <Tooltip label={t("preview.source.back")} keys={keyOfAction("back")}>
        <IconButton
          icon="arrow-left"
          size="sm"
          label={t("preview.source.back")}
          onClick={() => void transportController().closeSource()}
        />
      </Tooltip>
    </section>
  );
}
