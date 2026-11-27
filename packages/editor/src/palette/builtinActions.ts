// SPDX-License-Identifier: GPL-3.0-or-later
// The registry commands the editor carries out its own way when the palette runs them (see
// `actions.ts`): the same paths the menus, the toolbar and the keys take, so the palette adds no
// second behaviour for them.

import { exportController } from "../export/useExport.ts";
import { i18n } from "../i18n/index.ts";
import { openArchiveDialog } from "../media/ArchiveDialog.tsx";
import { openCollectDialog } from "../media/CollectDialog.tsx";
import { addShownFrameToBin, pickSlideshow } from "../media/slideshowCommands.ts";
import { projectController } from "../session/useProject.ts";
import { activeTimelineController } from "../timeline/activeController.ts";
import { registerPaletteAction } from "./actions.ts";

let registered = false;

/** Registers the built-in actions once. */
export function registerBuiltinPaletteActions(): void {
  if (registered) return;
  registered = true;
  // The toolbar's Split and the `S` key: the selected clips (or the one under the playhead), at
  // the playhead's frame, linked partners together, one undo step.
  registerPaletteAction("clip.split", () => {
    const controller = activeTimelineController();
    if (controller === null) throw new Error(i18n.t("palette.needTimeline"));
    controller.split();
  });
  registerPaletteAction("edit.undo", async () => void (await projectController().run("undo")));
  registerPaletteAction("edit.redo", async () => void (await projectController().run("redo")));
  // File commands open the app's own dialogs (name, open and save pickers) instead of asking for a path.
  registerPaletteAction("project.new", async () => void (await projectController().run("new")));
  registerPaletteAction("project.open", async () => void (await projectController().run("open")));
  registerPaletteAction("project.save", async () => void (await projectController().run("save")));
  registerPaletteAction(
    "project.save_as",
    async () => void (await projectController().run("saveAs")),
  );
  registerPaletteAction("export.start", () => exportController().open());
  registerPaletteAction("project.collect", () => openCollectDialog());
  // The frame on screen (the Preview's, or the Source view's), as the Preview's menu does; and
  // the picker and the dialog of the bin's "Import as slideshow…" (WP-2F.05d).
  registerPaletteAction("media.import_frame", async () => void (await addShownFrameToBin()));
  registerPaletteAction("media.import_images_as_slideshow", () => pickSlideshow());
  registerPaletteAction("project.archive", () => openArchiveDialog());
}
