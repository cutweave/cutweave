// SPDX-License-Identifier: GPL-3.0-or-later
// The tag editor (WP-2F.06b, ADR-0036): the tags of one or several assets as chips, and a field
// that adds more with suggestions from the project's own tags. With several assets a tag all of
// them have is solid; one only some have is dashed, says how many, and can be added to the rest.
// Nothing is staged: each add is one `media.add_tags` and each remove one `media.remove_tags` for
// all the assets, so each is one undo step. The media info (Inspector) shows it for the selected
// asset; "Add tag…" on a card opens it in a dialog (`TagsDialog`).
import type { MediaId } from "@cutweave/protocol";
import { Button, Dialog, TagInput, type TagInputTag } from "@cutweave/ui";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useEngineReady } from "../engine/engineStore.ts";
import type { TKey } from "../i18n/index.ts";
import { menuDialogStore } from "../menus/dialogs.ts";
import { useDocStore } from "../state/docStore.ts";
import { selectMediaCards } from "./cards.ts";
import { selectedTiles } from "./folderUi.ts";
import { binDeps } from "./requestDelete.ts";
import { addTags, removeTags } from "./tagActions.ts";
import {
  MAX_TAGS_PER_ASSET,
  MAX_TAGS_PER_CALL,
  parseTagEntry,
  projectTags,
  selectionTags,
  suggestTags,
  type TagProblem,
  tagKey,
  tagProblem,
} from "./tags.ts";

const PROBLEM: Readonly<Record<Exclude<TagProblem, "empty" | "comma">, TKey>> = {
  tooLong: "mediaBin.tags.tooLong",
  control: "mediaBin.tags.control",
};

export interface TagEditorProps {
  /** The assets whose tags are edited (ones that no longer exist are skipped). */
  readonly media: readonly string[];
  readonly autoFocus?: boolean;
}

export function TagEditor({ media, autoFocus }: TagEditorProps) {
  const { t } = useTranslation();
  const project = useDocStore((s) => s.project);
  const ready = useEngineReady();
  const [text, setText] = useState("");
  const [error, setError] = useState<string | undefined>(undefined);

  const all = useMemo(() => selectMediaCards(project), [project]);
  const cards = useMemo(() => {
    const wanted = new Set<string>(media);
    return all.filter((card) => wanted.has(card.id));
  }, [all, media]);
  const ids = useMemo(() => cards.map((card) => card.id as MediaId), [cards]);
  const known = useMemo(() => projectTags(all), [all]);
  const tags = useMemo(() => selectionTags(cards), [cards]);
  const onAll = useMemo(() => tags.filter((tag) => tag.all).map((tag) => tag.tag), [tags]);
  const suggestions = useMemo(() => suggestTags(known, text, onAll), [known, text, onAll]);
  const total = cards.length;

  const chips: TagInputTag[] = tags.map((tag) =>
    tag.all
      ? { tag: tag.tag }
      : {
          tag: tag.tag,
          partial: true,
          detail: t("mediaBin.tags.someOf", { count: tag.count, total }),
        },
  );

  const add = (entry: string) => {
    const typed = parseTagEntry(entry);
    for (const tag of typed) {
      const problem = tagProblem(tag);
      if (problem === "tooLong" || problem === "control") {
        setError(t(PROBLEM[problem]));
        return;
      }
    }
    // A tag every asset has already is not a step to undo.
    const have = new Set(onAll.map(tagKey));
    const fresh = typed.filter((tag) => !have.has(tagKey(tag)));
    const full = cards.some((card) => {
      const own = new Set(card.tags.map(tagKey));
      return own.size + fresh.filter((tag) => !own.has(tagKey(tag))).length > MAX_TAGS_PER_ASSET;
    });
    if (full) {
      setError(t("mediaBin.tags.tooMany"));
      return;
    }
    setText("");
    setError(undefined);
    if (fresh.length === 0 || ids.length === 0) return;
    void (async () => {
      // The engine takes 16 tags a call; a longer paste goes in as many steps as it needs.
      for (let i = 0; i < fresh.length; i += MAX_TAGS_PER_CALL) {
        if (!(await addTags(binDeps(), ids, fresh.slice(i, i + MAX_TAGS_PER_CALL)))) return;
      }
    })();
  };

  return (
    <TagInput
      tags={chips}
      value={text}
      onValueChange={(next) => {
        setText(next);
        setError(undefined);
      }}
      onAdd={add}
      onRemove={(tag) => void removeTags(binDeps(), ids, [tag])}
      onPromote={(tag) => void addTags(binDeps(), ids, [tag])}
      suggestions={suggestions}
      label={t("mediaBin.tags.field")}
      tagsLabel={t("mediaBin.tags.label")}
      suggestionsLabel={t("mediaBin.tags.suggestions")}
      removeLabel={(tag) => t("mediaBin.tags.remove", { tag })}
      promoteLabel={(tag) => t("mediaBin.tags.addToAll", { tag, count: total })}
      placeholder={t("mediaBin.tags.field")}
      error={error}
      disabled={!ready || total === 0}
      autoFocus={autoFocus}
    />
  );
}

/** "Add tag…" on a card: the tag editor for the cards the menu was opened on. */
export function TagsDialog({ media }: { readonly media: readonly string[] }) {
  const { t } = useTranslation();
  const project = useDocStore((s) => s.project);
  const cards = useMemo(() => {
    const wanted = new Set<string>(media);
    return selectMediaCards(project).filter((card) => wanted.has(card.id));
  }, [project, media]);
  const close = () => menuDialogStore.getState().close();
  const one = cards.length === 1 ? cards[0] : undefined;
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
      className="cw-dialog--tags"
      title={
        one !== undefined
          ? t("mediaBin.tags.titleOne", { name: one.name })
          : t("mediaBin.tags.titleMany", { count: cards.length })
      }
      description={t(one !== undefined ? "mediaBin.tags.bodyOne" : "mediaBin.tags.bodyMany")}
      footer={
        <Button variant="primary" size="lg" onClick={close}>
          {t("mediaBin.tags.done")}
        </Button>
      }
    >
      <TagEditor media={media} autoFocus />
    </Dialog>
  );
}

/**
 * Opens the tag editor for `media`, or for the cards selected in the bin when none are given (the
 * command palette). Nothing selected: nothing opens.
 */
export function openTagEditor(media?: readonly string[]): void {
  const chosen = media !== undefined && media.length > 0 ? media : selectedTiles().media;
  if (chosen.length === 0) return;
  menuDialogStore.getState().open({ kind: "tags", media: [...chosen] });
}
