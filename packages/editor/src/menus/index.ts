// SPDX-License-Identifier: GPL-3.0-or-later
// The context-menu framework (WP-2H.12, ADR-0026). Surface owners import from here:
//
//   registerMenuSurface(def)            a surface's table (its menu, as data)
//   menuContributions.register(...)     rows added from outside (feature WPs; plugins in WP-6B.03)
//   registerUiCommand(command)          an editor-side action in the same shape as a catalog command
//   registerCommandInfo(info)           how a registry command reads in a menu
//   useContextMenu / ContextSurface     mark a DOM element as a surface
//   openContextMenu({surface, point})   open one by hand (the canvas, a dock tab, the keyboard)
export { itemSelection, mediaSelection, trackSelection } from "./adapters.ts";
export { requestForCanvasEvent, requestForHit, ZONE_SURFACE } from "./canvas.ts";
export {
  type CommandInfo,
  CommandInfoTable,
  guardedCall,
  MultipleMutationsError,
  type RunContext,
  type UiCommand,
  UiCommandTable,
} from "./commands.ts";
export { menuServices } from "./defaults.ts";
export {
  ContextMenuHost,
  ContextSurface,
  closeContextMenu,
  contextMenuStore,
  installContextMenuHandlers,
  isMenuKey,
  type OpenRequest,
  openContextMenu,
  registerContextSurface,
  type SurfaceHit,
  type SurfaceQuery,
  type SurfaceResolver,
  useContextMenu,
} from "./host.tsx";
export { keysFor, registerKeyHint } from "./keys.ts";
export {
  MAX_PLUGIN_ITEMS,
  MenuContributionRegistry,
  MenuSurfaceRegistry,
  plainLabel,
} from "./registry.ts";
export { createMenuServices, type MenuServices } from "./services.ts";
export { applyTargetRules, resolveTarget } from "./target.ts";
export type {
  LabelSpec,
  MenuContext,
  MenuContribution,
  MenuRow,
  MenuSurfaceDef,
  MenuTarget,
  OpenVia,
  ScreenPoint,
  SelectionAdapter,
} from "./types.ts";
export { CONTEXT_KEYS, type ContextKey, type ContextValues, evaluateWhen } from "./when.ts";

import type { CommandInfo, UiCommand } from "./commands.ts";
import { menuServices } from "./defaults.ts";
import type { MenuSurfaceDef } from "./types.ts";

/** The app's contribution registry. */
export const menuContributions = menuServices.contributions;

/** Registers a surface's table with the app. Returns an unregister function. */
export function registerMenuSurface(def: MenuSurfaceDef): () => void {
  return menuServices.surfaces.register(def);
}

/** Registers an editor-side command with the app (menus, and later the palette and keymap). */
export function registerUiCommand(command: UiCommand): void {
  menuServices.ui.register(command);
}

/** Registers how a registry command reads in a menu. */
export function registerCommandInfo(info: CommandInfo): void {
  menuServices.commands.register(info);
}
