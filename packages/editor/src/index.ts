// SPDX-License-Identifier: GPL-3.0-or-later
/** Package name; also the key the desktop shell uses to find this app. */
export const packageName = "@cutweave/editor";

export { createI18n, i18n, type TKey } from "./i18n/index.ts";
export { EditorShell } from "./shell/EditorShell.tsx";
export {
  createViewStore,
  type ThemeId,
  useViewStore,
  type ViewStore,
  viewStore,
  type WorkspaceId,
} from "./state/viewStore.ts";
