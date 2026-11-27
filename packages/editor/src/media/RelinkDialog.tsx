// SPDX-License-Identifier: GPL-3.0-or-later
// The relink dialog (WP-2F.03): lists the media whose file is missing and finds them again. It
// opens by itself when a project opens with missing media, and from "Relink…" on a bin card.
// "Search a folder…" sends one `media.relink_folder` (content first, then name); the folder icon
// of a row sends `media.relink` for that asset with the file the person picks. Each is one undo
// step. The OS pickers are the only native part (`window.cutweave.dialog`).
import type { MediaId } from "@cutweave/protocol";
import { Button, Dialog, IconButton, StatusChip } from "@cutweave/ui";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import { engineStore } from "../engine/engineStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { translator } from "../timeline/errors.ts";
import { mediaFilesBridge, pickedPaths } from "./bridge.ts";
import {
  type HealthDeps,
  healthStore,
  relinkFromFolder,
  relinkOne,
  sortMissing,
} from "./health.ts";

/** Opens the relink dialog (the bin's "Relink…" and the palette). */
export function openRelinkDialog(): void {
  healthStore.getState().openDialog();
}

export function RelinkDialog() {
  const { t } = useTranslation();
  const open = useStore(healthStore, (s) => s.dialogOpen);
  const busy = useStore(healthStore, (s) => s.busy);
  const last = useStore(healthStore, (s) => s.last);
  const missingMap = useStore(healthStore, (s) => s.missing);
  if (!open) return null;
  const missing = sortMissing(missingMap);
  const count = missing.length;
  const deps: HealthDeps = { engine: engineStore, toasts: toastStore, t: translator(t) };
  const bridge = mediaFilesBridge();
  const close = () => healthStore.getState().closeDialog();
  const desktopOnly = () =>
    toastStore.getState().push({ tone: "info", title: t("mediaBin.import.desktopOnly") });

  const searchFolder = async () => {
    if (bridge.pickMediaFolders === undefined) return desktopOnly();
    const folders = pickedPaths(await bridge.pickMediaFolders().catch(() => null));
    for (const folder of folders) {
      if (healthStore.getState().missing.size === 0) break;
      await relinkFromFolder(deps, folder);
    }
  };

  const locate = async (media: MediaId) => {
    if (bridge.pickMediaFiles === undefined) return desktopOnly();
    const path = pickedPaths(await bridge.pickMediaFiles().catch(() => null))[0];
    if (path !== undefined) await relinkOne(deps, media, path);
  };

  const title =
    count === 0
      ? t("relink.title.none")
      : count === 1
        ? t("relink.title.one")
        : t("relink.title.many", { count: String(count) });
  const result =
    last === null
      ? null
      : last.relinked === 0
        ? t("relink.result.none")
        : last.relinked === 1
          ? t("relink.result.one")
          : t("relink.result.many", { count: String(last.relinked) });

  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next) close();
      }}
      title={title}
      description={count === 0 ? t("relink.allFound") : t("relink.hint")}
      className="cw-relink"
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={close}>
            {count === 0 ? t("relink.done") : t("relink.later")}
          </Button>
          {count === 0 ? null : (
            <Button
              variant="primary"
              size="lg"
              icon="search"
              disabled={busy}
              onClick={() => void searchFolder()}
            >
              {busy ? t("relink.searching") : t("relink.searchFolder")}
            </Button>
          )}
        </>
      }
    >
      {result === null ? null : (
        <p className="body cw-relink__result" role="status">
          {result}
          {last?.truncated === true ? ` ${t("relink.result.truncated")}` : ""}
        </p>
      )}
      {count === 0 ? null : (
        <ul className="cw-relink__list" aria-label={t("relink.listLabel")}>
          {missing.map((m) => (
            <li key={m.media_id} className="cw-relink__row" data-media-id={m.media_id}>
              <span className="cw-relink__text">
                <span className="body-strong cw-relink__name">{m.name}</span>
                <span className="caption cw-relink__path">{m.path ?? ""}</span>
              </span>
              <StatusChip tone={m.status === "missing" ? "danger" : "warning"}>
                {m.status === "changed"
                  ? t("mediaBin.status.changed")
                  : m.status === "incomplete"
                    ? t("mediaBin.status.incomplete", { count: m.missing_frames ?? 0 })
                    : t("mediaBin.status.offline")}
              </StatusChip>
              <IconButton
                icon="folder-open"
                size="sm"
                label={t("relink.locateFile", { name: m.name })}
                disabled={busy}
                onClick={() => void locate(m.media_id)}
              />
            </li>
          ))}
        </ul>
      )}
    </Dialog>
  );
}
