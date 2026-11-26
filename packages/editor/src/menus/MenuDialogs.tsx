// SPDX-License-Identifier: GPL-3.0-or-later
// The questions context-menu choices ask (WP-2H.13, `dialogs.ts`): rename a clip, a custom speed
// (WP-2D.01), confirm deleting
// a track with clips on it, confirm removing a media asset clips use. Each sends exactly one
// command when the person agrees, through the same call every menu choice uses.
import type { FolderId, MediaId } from "@cutweave/protocol";
import { Button, Dialog, Icon, Switch, TextField } from "@cutweave/ui";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { engineStore } from "../engine/engineStore.ts";
import type { TKey } from "../i18n/index.ts";
import { deleteFolder, moveTiles } from "../media/binActions.ts";
import { selectMediaCards, selectMediaUsage } from "../media/cards.ts";
import { canDrop, planFolderDelete, selectFolders } from "../media/folders.ts";
import { folderDeleteText } from "../media/folderText.ts";
import { removeMedia } from "../media/remove.ts";
import { requestFolderDelete } from "../media/requestDelete.ts";
import { useDocStore } from "../state/docStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { ATTRIBUTE_GROUPS, type AttributeGroup } from "../timeline/clipboard.ts";
import { translator } from "../timeline/errors.ts";
import { menuServices } from "./defaults.ts";
import { menuDialogStore, useMenuDialog } from "./dialogs.ts";

function RenameClip({ item, name }: { readonly item: string; readonly name: string }) {
  const { t } = useTranslation();
  const [value, setValue] = useState(name);
  useEffect(() => setValue(name), [name]);
  const close = () => menuDialogStore.getState().close();
  const submit = () => {
    close();
    if (value.trim() === name) return;
    void menuServices
      .call("item.set_props", { item, props: { name: value.trim() } })
      .catch((error: unknown) => menuServices.report(error));
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
      title={t("contextMenu.dialog.renameClipTitle")}
      onSubmit={submit}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={close}>
            {t("contextMenu.dialog.cancel")}
          </Button>
          <Button variant="primary" size="lg" onClick={submit}>
            {t("contextMenu.dialog.rename")}
          </Button>
        </>
      }
    >
      <div className="cw-row">
        <TextField
          aria-label={t("contextMenu.dialog.clipName")}
          value={value}
          onChange={setValue}
          autoFocus
          onFocus={(event) => event.currentTarget.select()}
        />
      </div>
    </Dialog>
  );
}

/** Speed ▸ Custom… (WP-2D.01): one `clip.set_speed` for the clips; the engine checks the text. */
function CustomSpeed({
  items,
  speed,
}: {
  readonly items: readonly string[];
  readonly speed: string;
}) {
  const { t } = useTranslation();
  const [value, setValue] = useState(speed);
  useEffect(() => setValue(speed), [speed]);
  const close = () => menuDialogStore.getState().close();
  const submit = () => {
    const text = value.trim();
    close();
    if (text === "" || text === speed) return;
    void menuServices
      .call("clip.set_speed", { items, speed: text })
      .catch((error: unknown) => menuServices.report(error));
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
      title={t("contextMenu.dialog.customSpeedTitle")}
      description={t("contextMenu.dialog.customSpeedBody")}
      onSubmit={submit}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={close}>
            {t("contextMenu.dialog.cancel")}
          </Button>
          <Button variant="primary" size="lg" onClick={submit}>
            {t("contextMenu.dialog.setSpeed")}
          </Button>
        </>
      }
    >
      <div className="cw-row">
        <TextField
          aria-label={t("contextMenu.dialog.speed")}
          value={value}
          onChange={setValue}
          autoFocus
          onFocus={(event) => event.currentTarget.select()}
        />
      </div>
    </Dialog>
  );
}

function DeleteTrack({
  track,
  name,
  clips,
}: {
  readonly track: string;
  readonly name: string;
  readonly clips: number;
}) {
  const { t } = useTranslation();
  const close = () => menuDialogStore.getState().close();
  const confirm = () => {
    close();
    void menuServices.call("track.remove", { track }).catch((error: unknown) => {
      menuServices.report(error);
    });
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
      title={t("contextMenu.dialog.deleteTrackTitle", { name })}
      description={t("contextMenu.dialog.deleteTrackBody", { count: clips })}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={close}>
            {t("contextMenu.dialog.cancel")}
          </Button>
          <Button variant="danger" size="lg" onClick={confirm}>
            {t("contextMenu.dialog.deleteTrack")}
          </Button>
        </>
      }
    >
      <p className="body">{t("contextMenu.dialog.deleteTrackNote")}</p>
    </Dialog>
  );
}

function RemoveMedia({
  id,
  name,
  uses,
}: {
  readonly id: string;
  readonly name: string;
  readonly uses: number;
}) {
  const { t } = useTranslation();
  const close = () => menuDialogStore.getState().close();
  const confirm = () => {
    close();
    void removeMedia(
      { engine: engineStore, toasts: toastStore, t: translator(t) },
      id as Parameters<typeof removeMedia>[1],
      name,
    );
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
      title={t("mediaBin.remove.confirmTitle", { name })}
      description={t("mediaBin.remove.confirmBody", { count: String(uses) })}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={close}>
            {t("mediaBin.remove.cancel")}
          </Button>
          <Button variant="danger" size="lg" onClick={confirm}>
            {t("mediaBin.remove.confirm")}
          </Button>
        </>
      }
    >
      <p className="body">{t("mediaBin.remove.confirmNote")}</p>
    </Dialog>
  );
}

/**
 * Deleting a folder that holds something (WP-2F.08): says what it holds and what happens to the
 * media a timeline uses, and offers Delete folder and media, Keep media, Cancel. Undo restores it.
 */
function DeleteFolder({
  folder,
  rest,
}: {
  readonly folder: string;
  readonly rest: readonly string[];
}) {
  const { t } = useTranslation();
  const project = useDocStore((s) => s.project);
  const plan = useMemo(() => {
    const cards = selectMediaCards(project);
    return planFolderDelete(project, cards, selectMediaUsage(project), folder as FolderId);
  }, [project, folder]);
  const close = () => menuDialogStore.getState().close();
  // The folder vanished (undo, another connection): nothing to ask.
  useEffect(() => {
    if (plan === null) close();
  }, [plan]);
  if (plan === null) return null;
  const text = folderDeleteText(plan, translator(t));
  const choose = (contents: "remove" | "move_up") => {
    close();
    void deleteFolder(
      { engine: engineStore, toasts: toastStore, t: translator(t) },
      plan.folder,
      plan.name,
      contents,
    ).then(() => {
      const next = rest[0];
      if (next !== undefined) void requestFolderDelete(next as FolderId, rest.slice(1));
    });
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
      title={text.title}
      description={text.notes.join(" ")}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={close}>
            {t("mediaBin.folder.cancel")}
          </Button>
          <Button variant="secondary" size="lg" onClick={() => choose("move_up")}>
            {t("mediaBin.folder.deleteKeep")}
          </Button>
          <Button variant="danger" size="lg" onClick={() => choose("remove")}>
            {t("mediaBin.folder.deleteAll")}
          </Button>
        </>
      }
    >
      <p className="body">{t("mediaBin.folder.deleteNote")}</p>
    </Dialog>
  );
}

/** "Move to…": a folder picker for the chosen assets and folders; the move is one `media.move`. */
function MoveTo({
  media,
  folders,
}: {
  readonly media: readonly string[];
  readonly folders: readonly string[];
}) {
  const { t } = useTranslation();
  const project = useDocStore((s) => s.project);
  const [target, setTarget] = useState<FolderId | null | undefined>(undefined);
  const payload = { media, folders };
  // Every folder the engine would accept, in tree order with its depth for the indent.
  const options = useMemo(() => {
    const all = selectFolders(project);
    const byParent = new Map<string | null, typeof all>();
    for (const f of all) byParent.set(f.parent, [...(byParent.get(f.parent) ?? []), f]);
    const out: { id: FolderId | null; label: string; depth: number; ok: boolean }[] = [
      {
        id: null,
        label: t("mediaBin.folder.topLevel"),
        depth: 0,
        ok: canDrop(project, payload, null),
      },
    ];
    const walk = (parent: string | null, depth: number) => {
      const kids = [...(byParent.get(parent) ?? [])].sort((a, b) =>
        a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
      );
      for (const f of kids) {
        out.push({ id: f.id, label: f.name, depth, ok: canDrop(project, payload, f.id) });
        walk(f.id, depth + 1);
      }
    };
    walk(null, 1);
    return out;
    // `payload` is rebuilt every render from props that only change with the dialog.
  }, [project, media, folders, t]);
  const close = () => menuDialogStore.getState().close();
  const submit = () => {
    if (target === undefined) return;
    close();
    void moveTiles(
      { engine: engineStore, toasts: toastStore, t: translator(t) },
      { media: media as MediaId[], folders: folders as FolderId[] },
      target,
    );
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
      title={t("mediaBin.folder.moveTitle")}
      description={t("mediaBin.folder.moveHint")}
      onSubmit={submit}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={close}>
            {t("mediaBin.folder.cancel")}
          </Button>
          <Button variant="primary" size="lg" disabled={target === undefined} onClick={submit}>
            {t("mediaBin.folder.moveConfirm")}
          </Button>
        </>
      }
    >
      <div
        role="radiogroup"
        aria-label={t("mediaBin.folder.moveListLabel")}
        className="cw-movelist"
      >
        {options.map((option) => (
          <label
            key={option.id ?? "top"}
            className="cw-movelist__row"
            data-disabled={option.ok ? undefined : "true"}
            style={{
              paddingInlineStart: `calc(var(--space-2) + ${option.depth} * var(--space-4))`,
            }}
          >
            <input
              type="radio"
              name="move-target"
              disabled={!option.ok}
              checked={target === option.id}
              onChange={() => setTarget(option.id)}
            />
            <Icon name="folder" size="md" />
            <span className="body">{option.label}</span>
          </label>
        ))}
      </div>
    </Dialog>
  );
}

const ATTRIBUTE_LABEL: Readonly<Record<AttributeGroup, TKey>> = {
  transform: "contextMenu.dialog.attribute.transform",
  crop: "contextMenu.dialog.attribute.crop",
  flip: "contextMenu.dialog.attribute.flip",
  compositing: "contextMenu.dialog.attribute.compositing",
  audio: "contextMenu.dialog.attribute.audio",
};

function PasteAttributes({ from, to }: { readonly from: string; readonly to: readonly string[] }) {
  const { t } = useTranslation();
  const [chosen, setChosen] = useState<ReadonlySet<AttributeGroup>>(new Set(ATTRIBUTE_GROUPS));
  const close = () => menuDialogStore.getState().close();
  const submit = () => {
    if (chosen.size === 0) return;
    close();
    void menuServices
      .call("clip.paste_attributes", {
        from,
        to: to.filter((id) => id !== from),
        which: ATTRIBUTE_GROUPS.filter((group) => chosen.has(group)),
      })
      .catch((error: unknown) => menuServices.report(error));
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
      title={t("contextMenu.dialog.pasteAttributesTitle")}
      description={t("contextMenu.dialog.pasteAttributesBody")}
      onSubmit={submit}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={close}>
            {t("contextMenu.dialog.cancel")}
          </Button>
          <Button variant="primary" size="lg" disabled={chosen.size === 0} onClick={submit}>
            {t("contextMenu.dialog.paste")}
          </Button>
        </>
      }
    >
      {ATTRIBUTE_GROUPS.map((group) => {
        const id = `paste-attribute-${group}`;
        return (
          <div className="cw-row" key={group}>
            <label className="body" htmlFor={id}>
              {t(ATTRIBUTE_LABEL[group])}
            </label>
            <Switch
              id={id}
              checked={chosen.has(group)}
              onCheckedChange={(on) => {
                const next = new Set(chosen);
                if (on) next.add(group);
                else next.delete(group);
                setChosen(next);
              }}
            />
          </div>
        );
      })}
    </Dialog>
  );
}

/** Mounted once in the shell, next to the menu host. */
export function MenuDialogs() {
  const dialog = useMenuDialog();
  if (dialog === null) return null;
  switch (dialog.kind) {
    case "rename-clip":
      return <RenameClip key={dialog.item} item={dialog.item} name={dialog.name} />;
    case "custom-speed":
      return <CustomSpeed key={dialog.items.join()} items={dialog.items} speed={dialog.speed} />;
    case "delete-track":
      return (
        <DeleteTrack
          key={dialog.track}
          track={dialog.track}
          name={dialog.name}
          clips={dialog.clips}
        />
      );
    case "remove-media":
      return <RemoveMedia key={dialog.id} id={dialog.id} name={dialog.name} uses={dialog.uses} />;
    case "delete-folder":
      return <DeleteFolder key={dialog.folder} folder={dialog.folder} rest={dialog.rest} />;
    case "move-to":
      return <MoveTo media={dialog.media} folders={dialog.folders} />;
    case "paste-attributes":
      return <PasteAttributes key={dialog.from} from={dialog.from} to={dialog.to} />;
  }
}
