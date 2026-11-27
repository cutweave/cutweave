// SPDX-License-Identifier: GPL-3.0-or-later
// The questions the collection actions ask (WP-2F.06c, ADR-0036): a name for a new collection
// (manual, with the chosen clips, or smart, with the bin's search and filters), which collection
// the chosen clips go onto (the palette's "Add to collection…"), and a yes before a collection
// that lists something is deleted. Each sends one `media.*` command when the person agrees;
// deleting never removes media, and the dialog says so.
import type { CollectionId, MediaId } from "@cutweave/protocol";
import { Button, Dialog, DialogNote, Icon, TextField } from "@cutweave/ui";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TKey } from "../i18n/index.ts";
import { menuDialogStore } from "../menus/dialogs.ts";
import { useDocStore } from "../state/docStore.ts";
import { selectMediaCards, selectMediaUsage } from "./cards.ts";
import { createCollection, deleteCollection } from "./collectionActions.ts";
import { addMediaToCollection, openNewCollectionDialog } from "./collectionCommands.ts";
import {
  type CollectionNameProblem,
  collectionNameProblem,
  freeCollectionName,
  notYetOn,
  selectCollections,
} from "./collections.ts";
import type { MediaFilter } from "./find.ts";
import { binDeps } from "./requestDelete.ts";

const close = () => menuDialogStore.getState().close();

const NAME_PROBLEM: Readonly<Record<Exclude<CollectionNameProblem, "empty">, TKey>> = {
  taken: "mediaBin.collections.taken",
  tooLong: "mediaBin.collections.tooLong",
  control: "mediaBin.collections.control",
};

/**
 * "New collection…" and "Save as smart collection…": a name, then one
 * `media.create_collection`. With `filter` the collection is smart; else it is manual and
 * `media` are its first members.
 */
export function NewCollectionDialog({
  media,
  filter,
}: {
  readonly media: readonly string[];
  readonly filter: MediaFilter | null;
}) {
  const { t } = useTranslation();
  const project = useDocStore((s) => s.project);
  const smart = filter !== null;
  // The name it opens with: free, and for a saved search the words searched for.
  const [name, setName] = useState(() =>
    freeCollectionName(
      project,
      (smart ? (filter.text?.trim() ?? filter.tags?.[0]) : undefined) ||
        t("mediaBin.collections.defaultName"),
    ),
  );
  const problem = collectionNameProblem(project, name);
  const submit = () => {
    if (problem !== null) return;
    close();
    const deps = binDeps();
    const known = project?.media ?? {};
    void createCollection(
      deps,
      name,
      smart
        ? { type: "smart", filter }
        : { type: "manual", media: media.filter((id) => id in known) as MediaId[] },
    ).then((id) => {
      if (id === null || !smart) return;
      deps.toasts.getState().push({
        tone: "success",
        title: deps.t("mediaBin.collections.saved", { name: name.trim() }),
      });
    });
  };
  const body = smart
    ? t("mediaBin.collections.smartBody")
    : media.length === 0
      ? t("mediaBin.collections.newBody")
      : media.length === 1
        ? t("mediaBin.collections.newBodyOne")
        : t("mediaBin.collections.newBodyMany", { count: media.length });
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
      title={t(smart ? "mediaBin.collections.smartTitle" : "mediaBin.collections.newTitle")}
      description={body}
      onSubmit={submit}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={close}>
            {t("mediaBin.collections.cancel")}
          </Button>
          <Button variant="primary" size="lg" disabled={problem !== null} onClick={submit}>
            {t(smart ? "mediaBin.collections.save" : "mediaBin.collections.create")}
          </Button>
        </>
      }
    >
      <div className="cw-row">
        <TextField
          aria-label={t("mediaBin.collections.nameLabel")}
          value={name}
          onChange={setName}
          autoFocus
          onFocus={(event) => event.currentTarget.select()}
        />
      </div>
      {problem === null || problem === "empty" ? null : (
        <DialogNote icon="info">{t(NAME_PROBLEM[problem])}</DialogNote>
      )}
    </Dialog>
  );
}

/**
 * "Add to collection…" from the command palette: the manual collections as a list, and "New
 * collection…". The card's menu has the same choices as a submenu.
 */
export function AddToCollectionDialog({ media }: { readonly media: readonly string[] }) {
  const { t } = useTranslation();
  const project = useDocStore((s) => s.project);
  const [target, setTarget] = useState<CollectionId | null>(null);
  const options = useMemo(
    () =>
      selectCollections(project, selectMediaCards(project), selectMediaUsage(project))
        .filter((entry) => entry.kind === "manual")
        .map((entry) => ({ ...entry, ok: notYetOn(project, entry.id, media).length > 0 })),
    [project, media],
  );
  const submit = () => {
    if (target === null) return;
    close();
    void addMediaToCollection(target, media);
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
      title={
        media.length === 1
          ? t("mediaBin.collections.addTitleOne")
          : t("mediaBin.collections.addTitleMany", { count: media.length })
      }
      description={t("mediaBin.collections.addBody")}
      onSubmit={submit}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={close}>
            {t("mediaBin.collections.cancel")}
          </Button>
          <Button variant="secondary" size="lg" onClick={() => openNewCollectionDialog(media)}>
            {t("mediaBin.collections.addNew")}
          </Button>
          <Button variant="primary" size="lg" disabled={target === null} onClick={submit}>
            {t("mediaBin.collections.addConfirm")}
          </Button>
        </>
      }
    >
      {options.length === 0 ? (
        <p className="body">{t("mediaBin.collections.addNone")}</p>
      ) : (
        <div
          role="radiogroup"
          aria-label={t("mediaBin.collections.addListLabel")}
          className="cw-movelist"
        >
          {options.map((option) => (
            <label
              key={option.id}
              className="cw-movelist__row cw-movelist__row--flat"
              data-disabled={option.ok ? undefined : "true"}
              title={option.ok ? undefined : t("mediaBin.collections.reasonAlreadyIn")}
            >
              <input
                type="radio"
                name="collection-target"
                disabled={!option.ok}
                checked={target === option.id}
                onChange={() => setTarget(option.id)}
              />
              <Icon name="layers" size="md" />
              <span className="body">{option.name}</span>
            </label>
          ))}
        </div>
      )}
    </Dialog>
  );
}

/** Deleting a collection that lists something: a yes, and the note that the media stay. */
export function DeleteCollectionDialog({ collection }: { readonly collection: string }) {
  const { t } = useTranslation();
  const project = useDocStore((s) => s.project);
  const entry = useMemo(
    () =>
      selectCollections(project, selectMediaCards(project), selectMediaUsage(project)).find(
        (c) => c.id === collection,
      ),
    [project, collection],
  );
  // The collection vanished (undo, another connection): nothing to ask.
  useEffect(() => {
    if (entry === undefined) close();
  }, [entry]);
  if (entry === undefined) return null;
  const confirm = () => {
    close();
    void deleteCollection(binDeps(), entry.id, entry.name);
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
      title={t("mediaBin.collections.deleteTitle", { name: entry.name })}
      description={
        entry.count === 1
          ? t("mediaBin.collections.deleteBodyOne")
          : t("mediaBin.collections.deleteBodyMany", { count: entry.count })
      }
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={close}>
            {t("mediaBin.collections.cancel")}
          </Button>
          <Button variant="danger" size="lg" onClick={confirm}>
            {t("mediaBin.collections.deleteConfirm")}
          </Button>
        </>
      }
    >
      <p className="body">{t("mediaBin.collections.deleteNote")}</p>
    </Dialog>
  );
}
