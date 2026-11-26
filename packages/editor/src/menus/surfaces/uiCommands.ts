// SPDX-License-Identifier: GPL-3.0-or-later
// The editor-side actions the surface tables name (WP-2H.13): layout, view state, the clipboard
// and the file manager. None of them changes the project document; every document change in a
// menu is a catalog command (`clip.split`, `track.set`...) named by the row itself, so the CLI and
// MCP have the same call. A row that touches the OS is `host: true` and listed in
// `host-actions-exempt.json`.
import type { FolderId, MediaId, PreviewQuality, SelectionRef } from "@cutweave/protocol";
import { engineStore } from "../../engine/engineStore.ts";
import { exportController } from "../../export/useExport.ts";
import { i18n } from "../../i18n/index.ts";
import { sectionsStore } from "../../inspector/sections.ts";
import { applyLayout, placeAssets, swapLayout } from "../../layout/actions.ts";
import { newFolder, selectAllInBin } from "../../media/binCommands.ts";
import { binStore } from "../../media/binStore.ts";
import { mediaFilesBridge, pickedPaths } from "../../media/bridge.ts";
import { currentOpenFolder, folderUiStore, openFolder } from "../../media/folderUi.ts";
import { importMedia } from "../../media/import.ts";
import { layoutStore, type MediaLayout } from "../../media/layoutStore.ts";
import { openRelinkDialog } from "../../media/RelinkDialog.tsx";
import { requestFolderDelete, requestMediaDelete } from "../../media/requestDelete.ts";
import { requestMediaRemoval } from "../../media/requestRemoval.ts";
import { addShownFrameToBin, pickSlideshow } from "../../media/slideshowCommands.ts";
import { preferences } from "../../prefs/prefsStore.ts";
import { desktopFromWindow } from "../../session/desktop.ts";
import { projectController } from "../../session/useProject.ts";
import { openSettings } from "../../settings/settingsStore.ts";
import { toastStore } from "../../state/toastStore.ts";
import { viewStore } from "../../state/viewStore.ts";
import { activeTimelineController } from "../../timeline/activeController.ts";
import {
  attributesSource,
  clipboardStore,
  copyClips,
  cutClips,
  pasteClips,
} from "../../timeline/clipboard.ts";
import { translator } from "../../timeline/errors.ts";
import {
  canReplaceWith,
  pickedClip,
  pickedMedia,
  replaceClip,
} from "../../timeline/replacePick.ts";
import { transportController } from "../../transport/useTransport.ts";
import { runWorkspaceCommand } from "../../workspace/commands.ts";
import { floatPanel, movePanel, toggleMaximizePanel } from "../../workspace/dockHost.ts";
import type { UiCommand } from "../commands.ts";
import { menuDialogStore } from "../dialogs.ts";
import { clipFact, clipFacts, trackFact } from "./facts.ts";

const tr = () => translator(i18n.t.bind(i18n));

function arg<T>(args: unknown): T {
  return (args ?? {}) as T;
}

/** Puts `text` on the clipboard. The renderer's own async clipboard: the page has the focus. */
export async function copyText(text: string): Promise<void> {
  await navigator.clipboard.writeText(text);
}

/** The native picker (files, or folders: WP-2F.09), then one `media.import` for what was chosen. */
async function pickAndImport(what: "files" | "folders" = "files"): Promise<void> {
  const bridge = mediaFilesBridge();
  const pick = what === "files" ? bridge.pickMediaFiles : bridge.pickMediaFolders;
  if (pick === undefined) {
    toastStore.getState().push({ tone: "info", title: i18n.t("mediaBin.import.desktopOnly") });
    return;
  }
  const paths = pickedPaths(await pick().catch(() => null));
  if (paths.length === 0) return;
  await importMedia(
    { engine: engineStore, toasts: toastStore, t: tr() },
    paths,
    currentOpenFolder(),
    { keepStructure: preferences.get("media.keep_structure") },
  );
}

const selectIds = (kind: SelectionRef["kind"], ids: readonly string[]): void => {
  viewStore.getState().setSelection(ids.map((id) => ({ kind, id }) as SelectionRef));
};

export const UI_COMMANDS: readonly UiCommand[] = [
  // ---- dock ---------------------------------------------------------------------------------
  {
    id: "workspace.close_panel",
    label: "panel.close",
    icon: "x",
    run: (args) => void runWorkspaceCommand("workspace.close_panel", arg(args)),
  },
  {
    id: "workspace.toggle_maximize",
    label: "contextMenu.item.maximize",
    icon: "maximize-2",
    run: (args) => toggleMaximizePanel(arg<{ panel: string }>(args).panel),
  },
  {
    id: "workspace.float_panel",
    label: "contextMenu.item.float",
    run: (args) => floatPanel(arg<{ panel: string }>(args).panel),
  },
  // A panel in a window of its own and back (WP-2H.14). The palette lists them with the
  // workspace commands (for the active panel), so they are hidden here.
  {
    id: "workspace.popout_panel",
    label: "contextMenu.item.moveToWindow",
    hidden: true,
    run: (args) => void runWorkspaceCommand("workspace.popout_panel", arg(args)),
  },
  {
    id: "workspace.dock_panel",
    label: "contextMenu.item.moveToMain",
    hidden: true,
    run: (args) => void runWorkspaceCommand("workspace.dock_panel", arg(args)),
  },
  {
    id: "workspace.move_panel",
    label: "contextMenu.item.moveTo",
    run: (args) => {
      const { panel, side } = arg<{ panel: string; side: "left" | "right" | "below" }>(args);
      movePanel(panel, side);
    },
  },
  {
    id: "workspace.open_panel",
    label: "contextMenu.item.openPanel",
    run: (args) => void runWorkspaceCommand("workspace.open_panel", arg(args)),
  },
  {
    id: "workspace.reopen_panels",
    label: "contextMenu.item.reopenAll",
    run: () => void runWorkspaceCommand("workspace.reopen_panels"),
  },
  {
    id: "workspace.reset_layout",
    label: "workspace.reset",
    icon: "refresh-cw",
    run: () => void runWorkspaceCommand("workspace.reset_layout"),
  },
  {
    id: "workspace.reset_all",
    label: "workspace.resetAll",
    run: () => void runWorkspaceCommand("workspace.reset_all"),
  },

  // ---- the host: the clipboard and the file manager -----------------------------------------
  {
    id: "host.copy_text",
    label: "contextMenu.item.copy",
    icon: "copy",
    host: true,
    run: async (args) => copyText(arg<{ text: string }>(args).text),
  },
  {
    id: "host.reveal_path",
    label: "contextMenu.item.revealFile",
    icon: "folder-open",
    host: true,
    run: async (args) => {
      await desktopFromWindow()?.shell?.showItemInFolder(arg<{ path: string }>(args).path);
    },
  },
  {
    id: "host.open_logs_folder",
    label: "contextMenu.item.openLogs",
    host: true,
    run: async () => {
      await desktopFromWindow()?.shell?.openAppFolder?.("logs");
    },
  },

  // ---- the media bin ------------------------------------------------------------------------
  {
    id: "bin.import",
    label: "contextMenu.item.import",
    icon: "upload",
    host: true,
    run: () => pickAndImport("files"),
  },
  {
    id: "bin.import_folder",
    label: "contextMenu.item.importFolder",
    icon: "folder-open",
    host: true,
    run: () => pickAndImport("folders"),
  },
  // The picker, then the app's dialog, then one `media.import_images_as_slideshow` (WP-2F.05d).
  {
    id: "bin.import_slideshow",
    label: "contextMenu.command.mediaImportImagesAsSlideshow",
    icon: "film",
    host: true,
    when: "engine.live",
    run: () => pickSlideshow(),
  },
  // The frame on screen as a picture in the bin (WP-2F.05d): one `media.import_frame`.
  {
    id: "preview.add_frame_to_bin",
    label: "contextMenu.command.mediaImportFrame",
    icon: "image",
    when: "engine.live",
    run: async () => void (await addShownFrameToBin()),
  },
  {
    id: "bin.relink",
    label: "contextMenu.item.relink",
    icon: "search",
    run: () => openRelinkDialog(),
  },
  {
    id: "bin.new_folder",
    label: "contextMenu.item.newFolder",
    icon: "folder-plus",
    run: async (args) => {
      await newFolder(arg<{ parent?: FolderId | null }>(args).parent ?? currentOpenFolder());
    },
  },
  {
    id: "bin.rename",
    label: "contextMenu.item.renameItem",
    run: (args) => {
      const { folder, media } = arg<{ folder?: FolderId; media?: MediaId }>(args);
      const ui = folderUiStore.getState();
      if (folder !== undefined) {
        viewStore.getState().setSelection([]);
        ui.setFolders([folder], { kind: "folder", id: folder });
        ui.startRename({ kind: "folder", id: folder });
      } else if (media !== undefined) {
        ui.setFolders([], { kind: "media", id: media });
        selectIds("media", [media]);
        ui.startRename({ kind: "media", id: media });
      }
    },
  },
  {
    id: "bin.move_to",
    label: "contextMenu.item.moveToFolder",
    run: (args) => {
      const { media = [], folders = [] } = arg<{ media?: string[]; folders?: string[] }>(args);
      menuDialogStore.getState().open({ kind: "move-to", media, folders });
    },
  },
  {
    id: "bin.open_folder",
    label: "contextMenu.item.openFolder",
    icon: "folder-open",
    run: (args) => openFolder(arg<{ folder: FolderId }>(args).folder),
  },
  {
    id: "bin.delete_folders",
    label: "contextMenu.item.deleteFolder",
    icon: "trash-2",
    danger: true,
    run: async (args) => {
      const [first, ...rest] = arg<{ folders: FolderId[] }>(args).folders;
      if (first !== undefined) await requestFolderDelete(first, rest);
    },
  },
  {
    id: "bin.remove_selected",
    label: "contextMenu.item.removeMany",
    icon: "trash-2",
    danger: true,
    run: async (args) => requestMediaDelete(arg<{ ids: MediaId[] }>(args).ids),
  },
  {
    id: "layout.apply",
    label: "contextMenu.item.layout",
    run: async (args) => {
      const { ids, layout } = arg<{ ids: string[]; layout: string }>(args);
      await applyLayout(engineStore, ids, layout);
    },
  },
  {
    id: "layout.swap",
    label: "contextMenu.item.swapLayout",
    run: async (args) => {
      await swapLayout(engineStore, arg<{ ids: string[] }>(args).ids);
    },
  },
  {
    id: "bin.place_layout",
    label: "contextMenu.item.placeSideBySide",
    run: async (args) => {
      const { media, layout } = arg<{ media: string[]; layout?: string }>(args);
      await placeAssets(engineStore, media, layout);
    },
  },
  {
    id: "bin.select_all",
    label: "contextMenu.item.selectAll",
    run: () => selectAllInBin(),
  },
  {
    id: "bin.play_in_source",
    label: "contextMenu.item.playInSource",
    icon: "play",
    run: async (args) => {
      await transportController().toggleSource(arg<{ id: MediaId }>(args).id, "preview");
    },
  },
  {
    id: "bin.remove_media",
    label: "contextMenu.command.mediaRemove",
    icon: "trash-2",
    danger: true,
    run: async (args) => requestMediaRemoval(arg<{ id: MediaId }>(args).id),
  },
  {
    id: "bin.set_sort",
    label: "mediaBin.sort.button",
    run: (args) => {
      const { by, order } = arg<{
        by: "imported" | "name" | "duration" | "kind";
        order: "asc" | "desc";
      }>(args);
      binStore.getState().setSort({ by, order });
    },
  },
  {
    id: "bin.set_layout",
    label: "mediaBin.layout.label",
    run: (args) => layoutStore.getState().set(arg<{ layout: MediaLayout }>(args).layout),
  },
  {
    id: "bin.toggle_hover_scrub",
    label: "contextMenu.item.hoverScrub",
    run: async () => {
      await preferences.set("media.hover_scrub", !preferences.get("media.hover_scrub"));
    },
  },
  {
    id: "bin.remove_filter",
    label: "contextMenu.item.removeFilter",
    run: (args) => {
      const { chip } = arg<{ chip: string }>(args);
      const bin = binStore.getState();
      if (chip.startsWith("kind-")) bin.toggleKind(chip.slice("kind-".length) as never);
      else if (chip === "duration") bin.setDuration(null);
      else if (chip === "use") bin.setUse(null);
      else if (chip === "problems") bin.setProblems(false);
    },
  },
  {
    id: "bin.clear_filters",
    label: "mediaBin.filters.clearAll",
    run: () => binStore.getState().clearFilters(),
  },

  // ---- the preview and the transport --------------------------------------------------------
  {
    id: "transport.toggle_play",
    label: "contextMenu.item.playPause",
    icon: "play",
    run: async () => transportController().togglePlay(),
  },
  {
    id: "transport.to_start",
    label: "transport.toStart",
    icon: "skip-back",
    run: async () => transportController().toStart(),
  },
  {
    id: "transport.to_end",
    label: "transport.toEnd",
    icon: "skip-forward",
    run: async () => transportController().toEnd(),
  },
  {
    id: "transport.toggle_mute",
    label: "transport.mute",
    run: () => transportController().toggleMute(),
  },
  {
    id: "transport.reset_volume",
    label: "contextMenu.item.resetVolume",
    run: () => transportController().setVolume(1),
  },
  {
    id: "view.toggle_safe_areas",
    label: "preview.safeAreas",
    run: () => viewStore.getState().setSafeAreas(!viewStore.getState().safeAreas),
  },
  {
    id: "view.set_preview_quality",
    label: "contextMenu.item.quality",
    run: (args) =>
      viewStore.getState().setPreviewQuality(arg<{ quality: PreviewQuality }>(args).quality),
  },
  {
    id: "source.back",
    label: "preview.source.back",
    icon: "arrow-left",
    run: async () => transportController().closeSource(),
  },

  // ---- the timeline -------------------------------------------------------------------------
  {
    id: "timeline.cut",
    label: "contextMenu.item.cut",
    icon: "scissors",
    run: (_args, ctx) => cutClips(ctx.call, ctx.ids),
  },
  {
    id: "timeline.copy_clips",
    label: "contextMenu.item.copy",
    icon: "copy",
    run: async (_args, ctx) => {
      await copyClips(ctx.call, ctx.ids);
    },
  },
  {
    id: "timeline.paste_here",
    label: "contextMenu.item.pasteHere",
    icon: "clipboard",
    run: async (_args, ctx) => {
      const payload = clipboardStore.getState().payload;
      const at = ctx.target.place?.time;
      if (payload === null || at === undefined) return;
      await pasteClips(ctx.call, payload, at, ctx.target.place?.trackId);
    },
  },
  {
    id: "timeline.paste_attributes",
    label: "contextMenu.item.pasteAttributes",
    run: (_args, ctx) => {
      const from = attributesSource();
      if (from === null) return;
      menuDialogStore.getState().open({ kind: "paste-attributes", from, to: ctx.ids });
    },
  },
  {
    // Replace clip… (WP-2A.11): with a bin card picked that fits, replaces right away; otherwise
    // asks for the media. Either way one `clip.replace`.
    id: "timeline.replace_clip",
    label: "contextMenu.item.replaceClip",
    run: async (_args, ctx) => {
      const item = ctx.ids[0];
      if (item === undefined) return;
      const media = pickedMedia();
      if (media !== null && canReplaceWith(item, media)) {
        await replaceClip(ctx.call, { item, media });
        return;
      }
      menuDialogStore.getState().open({ kind: "replace-clip", item });
    },
  },
  {
    // Replace selected clip (the bin card's menu): the last clip picked on the timeline gets
    // this card's media.
    id: "bin.replace_clip",
    label: "contextMenu.item.replaceSelectedClip",
    run: async (args, ctx) => {
      const item = pickedClip();
      const media = arg<{ media?: string }>(args).media ?? ctx.ids[0];
      if (item === null || media === undefined || !canReplaceWith(item, media)) return;
      await replaceClip(ctx.call, { item, media });
    },
  },
  {
    id: "timeline.rename_clip",
    label: "contextMenu.item.renameClip",
    run: (_args, ctx) => {
      const item = ctx.ids[0];
      const fact = item === undefined ? null : clipFact(item);
      if (item === undefined || fact === null) return;
      menuDialogStore.getState().open({
        kind: "rename-clip",
        item,
        name: fact.clip.name ?? "",
      });
    },
  },
  {
    // Speed ▸ Custom… (WP-2D.01): asks for a speed, then sends one `clip.set_speed`.
    id: "timeline.custom_speed",
    label: "contextMenu.item.speedCustom",
    hidden: true,
    run: (_args, ctx) => {
      const facts = clipFacts(ctx.ids);
      if (facts.length === 0) return;
      const speeds = new Set(facts.map((f) => (f.clip.speed?.num ?? 1) / (f.clip.speed?.den ?? 1)));
      const shared = speeds.size === 1 ? [...speeds][0] : undefined;
      menuDialogStore.getState().open({
        kind: "custom-speed",
        items: facts.map((f) => f.clip.id),
        speed: shared === undefined ? "" : String(Number(shared.toFixed(3))),
      });
    },
  },
  {
    id: "timeline.reveal_in_bin",
    label: "contextMenu.item.revealInBin",
    icon: "film",
    run: (_args, ctx) => {
      const fact = ctx.ids[0] === undefined ? null : clipFact(ctx.ids[0]);
      if (fact === null) return;
      const view = viewStore.getState();
      view.setAssetTab("media");
      selectIds("media", [fact.clip.media]);
    },
  },
  {
    id: "timeline.select_all_on_track",
    label: "contextMenu.item.selectAllOnTrack",
    run: (_args, ctx) => {
      const trackId = ctx.target.place?.trackId ?? ctx.ids[0];
      const fact = trackId === undefined ? null : trackFact(trackId);
      if (fact === null) return;
      const clips = Object.values(fact.track.items)
        .sort((a, b) => a.start - b.start)
        .map((clip) => clip.id);
      selectIds("item", clips);
    },
  },
  {
    id: "timeline.move_playhead_here",
    label: "contextMenu.command.viewSetPlayhead",
    icon: "arrow-right",
    run: async (_args, ctx) => {
      const time = ctx.target.place?.time;
      if (time !== undefined) await transportController().seek(time);
    },
  },
  {
    id: "timeline.rename_track",
    label: "contextMenu.item.renameTrack",
    run: (_args, ctx) => {
      const track = ctx.ids[0];
      if (track !== undefined) activeTimelineController()?.beginRename(track);
    },
  },
  {
    id: "timeline.delete_track",
    label: "contextMenu.command.trackRemove",
    icon: "trash-2",
    danger: true,
    run: async (_args, ctx) => {
      const id = ctx.ids[0];
      const fact = id === undefined ? null : trackFact(id);
      if (id === undefined || fact === null) return;
      const clips = Object.keys(fact.track.items).length;
      if (clips > 0) {
        menuDialogStore.getState().open({
          kind: "delete-track",
          track: id,
          name: fact.track.name,
          clips,
        });
        return;
      }
      await ctx.call("track.remove", { track: id });
    },
  },

  // ---- the inspector and settings -----------------------------------------------------------
  {
    id: "inspector.collapse_all",
    label: "contextMenu.item.collapseAll",
    icon: "chevron-down",
    run: () => sectionsStore.getState().collapseAll(),
  },
  {
    id: "inspector.expand_all",
    label: "contextMenu.item.expandAll",
    run: () => sectionsStore.getState().expandAll(),
  },
  {
    id: "settings.reset_row",
    label: "contextMenu.item.resetToDefault",
    icon: "undo-2",
    run: async (args) => {
      const { key } = arg<{ key: string }>(args);
      await preferences.reset([key as never]);
    },
  },

  // ---- the title bar and the toasts ---------------------------------------------------------
  {
    id: "chrome.save_now",
    label: "contextMenu.item.saveNow",
    run: async () => {
      await projectController().save();
    },
  },
  {
    id: "chrome.autosave_settings",
    label: "contextMenu.item.autosaveSettings",
    run: () => openSettings("saving"),
  },
  {
    id: "export.open",
    label: "contextMenu.item.export",
    icon: "upload",
    run: () => exportController().open(),
  },
  {
    id: "toast.dismiss",
    label: "contextMenu.item.dismiss",
    icon: "x",
    run: (args) => toastStore.getState().dismiss(arg<{ id: number }>(args).id),
  },
  {
    id: "toast.dismiss_all",
    label: "contextMenu.item.dismissAll",
    run: () => toastStore.getState().clear(),
  },
];
