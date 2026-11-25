// SPDX-License-Identifier: GPL-3.0-or-later
// Settings > General > "Workspace for new projects" (`general.workspace`, WP-2H.01): a new project
// opens in Long-form or Short-form, or (the default) in whichever workspace was used last, which
// is simply the one already showing.
import type { WorkspaceChoice } from "@cutweave/preferences";
import { prefsStore } from "../prefs/prefsStore.ts";
import { viewStore, type WorkspaceId } from "../state/viewStore.ts";

/** The workspace a new project opens in, or `null` to stay where the person is. */
export function workspaceForNewProject(choice: WorkspaceChoice): WorkspaceId | null {
  return choice === "last" ? null : choice;
}

/** Called once a new project exists: applies the preference. */
export function applyNewProjectWorkspace(): void {
  const target = workspaceForNewProject(prefsStore.getState().values["general.workspace"]);
  if (target !== null) viewStore.getState().switchWorkspace(target);
}
