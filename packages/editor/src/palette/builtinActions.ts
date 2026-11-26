// SPDX-License-Identifier: GPL-3.0-or-later
// The registry commands the editor carries out its own way when the palette runs them (see
// `actions.ts`): the same paths the menus, the toolbar and the keys take, so the palette adds no
// second behaviour for them.
import { exportController } from "../export/useExport.ts";
import { i18n } from "../i18n/index.ts";
import { projectController } from "../session/useProject.ts";
import { activeTimelineHost } from "../timeline/activeHost.ts";
import { registerPaletteAction } from "./actions.ts";

let registered = false;

/** Registers the built-in actions once. */
export function registerBuiltinPaletteActions(): void {
  if (registered) return;
  registered = true;
  // The toolbar's Split and the `S` key: the selected clips (or the one under the playhead), at
  // the playhead's frame, linked partners together, one undo step.
  registerPaletteAction("clip.split", () => {
    const host = activeTimelineHost();
    if (host === null) throw new Error(i18n.t("palette.needTimeline"));
    host.split();
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
}
