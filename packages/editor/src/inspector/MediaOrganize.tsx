// SPDX-License-Identifier: GPL-3.0-or-later
// The rating, tags and note of the media asset selected in the bin (WP-2F.06b, ADR-0036), in the
// Inspector's media info. Five stars set the rating (the current star again clears it), the tag
// editor adds and removes tags, and the note is a plain multi-line field committed when the focus
// leaves it or on Ctrl+Enter (Escape puts the stored note back). Each change is one command and
// one undo step: `media.set_rating`, `media.add_tags`, `media.remove_tags`, `media.set_note`.
import type { MediaAsset } from "@cutweave/protocol";
import { InspectorRow, RatingStars, TextArea } from "@cutweave/ui";
import { type KeyboardEvent, useEffect, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { useEngineReady } from "../engine/engineStore.ts";
import { binDeps } from "../media/requestDelete.ts";
import { TagEditor } from "../media/TagEditor.tsx";
import { setNote, setRating } from "../media/tagActions.ts";
import { charCount, NOTE_COUNT_FROM, notePlan, noteTooLong } from "../media/tags.ts";

function NoteField({ media }: { readonly media: MediaAsset }) {
  const { t } = useTranslation();
  const ready = useEngineReady();
  const id = useId();
  const hintId = `${id}-hint`;
  const stored = media.note ?? null;
  // What is typed and not yet committed; `null`: the field shows the stored note.
  const [draft, setDraft] = useState<string | null>(null);
  // Another asset, or the note changed under the field (undo, an agent): show what is stored.
  // biome-ignore lint/correctness/useExhaustiveDependencies: the stored note is the trigger
  useEffect(() => setDraft(null), [media.id, stored]);
  const text = draft ?? stored ?? "";
  const count = charCount(text.trim());
  const tooLong = noteTooLong(text);

  const commit = () => {
    if (draft === null || tooLong) return;
    const plan = notePlan(draft, stored);
    if (!plan.change) {
      setDraft(null);
      return;
    }
    // The draft stays on screen until the document's patch replaces it (or the engine refuses).
    void setNote(binDeps(), media.id, plan.note).then((ok) => {
      if (ok) setDraft((current) => (current === draft ? null : current));
    });
  };
  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
      event.preventDefault();
      commit();
    } else if (event.key === "Escape" && draft !== null) {
      event.preventDefault();
      event.stopPropagation();
      setDraft(null);
    }
  };
  const hint = tooLong
    ? t("mediaBin.note.tooLong")
    : count >= NOTE_COUNT_FROM
      ? t("mediaBin.note.count", { count })
      : null;
  return (
    <div className="cw-media-note">
      <label className="cw-row__label label" htmlFor={id}>
        {t("mediaBin.note.label")}
      </label>
      <TextArea
        id={id}
        value={text}
        placeholder={t("mediaBin.note.placeholder")}
        disabled={!ready}
        invalid={tooLong}
        aria-describedby={hint === null ? undefined : hintId}
        data-testid="media-note"
        onChange={setDraft}
        onBlur={commit}
        onKeyDown={onKeyDown}
      />
      {hint === null ? null : (
        <span
          id={hintId}
          className="cw-media-note__hint caption numeric"
          data-invalid={tooLong ? "true" : undefined}
          role="status"
        >
          {hint}
        </span>
      )}
    </div>
  );
}

export function MediaOrganize({ media }: { readonly media: MediaAsset }) {
  const { t } = useTranslation();
  const ready = useEngineReady();
  return (
    <div className="cw-inspector__rows" data-testid="media-organize">
      <InspectorRow label={t("mediaBin.rating.label")}>
        <RatingStars
          value={media.rating ?? null}
          aria-label={t("mediaBin.rating.label")}
          starLabel={(count) =>
            count === 1 ? t("mediaBin.rating.starOne") : t("mediaBin.rating.starMany", { count })
          }
          disabled={!ready}
          onChange={(rating) => void setRating(binDeps(), [media.id], rating)}
        />
      </InspectorRow>
      <div className="cw-media-tags">
        <span className="cw-row__label label">{t("mediaBin.tags.label")}</span>
        <TagEditor key={media.id} media={[media.id]} />
      </div>
      <NoteField key={media.id} media={media} />
    </div>
  );
}
