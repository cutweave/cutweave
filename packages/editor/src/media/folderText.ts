// SPDX-License-Identifier: GPL-3.0-or-later
// The words of the media folders (WP-2F.08): what a folder tile counts and what the delete
// dialog says a folder holds. Plain functions of a count, so Vitest holds the wording for 0, 1
// and several, and the panel and the dialog say the same thing.
import type { Translate } from "../timeline/errors.ts";
import type { FolderDeletePlan } from "./folders.ts";

/** "Empty", "1 item", "12 items". */
export function countText(items: number, t: Translate): string {
  if (items === 0) return t("mediaBin.folder.countEmpty");
  return items === 1
    ? t("mediaBin.folder.countOne")
    : t("mediaBin.folder.countMany", { count: String(items) });
}

/** What a screen reader hears for a folder tile: "Interviews, folder, 3 items". */
export function folderAriaLabel(name: string, items: number, t: Translate): string {
  if (items === 0) return t("mediaBin.folder.ariaEmpty", { name });
  return items === 1
    ? t("mediaBin.folder.ariaOne", { name })
    : t("mediaBin.folder.ariaMany", { name, count: String(items) });
}

export interface FolderDeleteText {
  /** The question: Delete "B-roll" and its 30 media? */
  readonly title: string;
  /** Sentences under it: other folders inside, and what happens to media a timeline uses. */
  readonly notes: readonly string[];
}

/**
 * The delete dialog's words for a folder that holds something. The question names what it holds
 * (its media, or its folders when it has no media); a second sentence names the used media, which
 * `Delete folder and media` moves to the parent instead of removing.
 */
export function folderDeleteText(plan: FolderDeletePlan, t: Translate): FolderDeleteText {
  const name = plan.name;
  const notes: string[] = [];
  let title: string;
  if (plan.media > 0) {
    title =
      plan.media === 1
        ? t("mediaBin.folder.deleteTitleMediaOne", { name })
        : t("mediaBin.folder.deleteTitleMediaMany", { name, count: String(plan.media) });
    if (plan.subfolders > 0) {
      notes.push(
        plan.subfolders === 1
          ? t("mediaBin.folder.deleteAlsoFoldersOne")
          : t("mediaBin.folder.deleteAlsoFoldersMany", { count: String(plan.subfolders) }),
      );
    }
  } else {
    title =
      plan.subfolders === 1
        ? t("mediaBin.folder.deleteTitleFoldersOne", { name })
        : t("mediaBin.folder.deleteTitleFoldersMany", { name, count: String(plan.subfolders) });
  }
  if (plan.used > 0) {
    const parent = plan.parentName ?? t("mediaBin.folder.topLevel");
    notes.push(
      plan.used === 1
        ? t("mediaBin.folder.deleteUsedOne", { parent })
        : t("mediaBin.folder.deleteUsedMany", { count: String(plan.used), parent }),
    );
  }
  return { title, notes };
}
