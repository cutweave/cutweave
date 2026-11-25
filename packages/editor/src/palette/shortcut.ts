// SPDX-License-Identifier: GPL-3.0-or-later
// Opening the command palette from the keyboard (WP-2H.04, WP-2H.05). Ctrl+K (Cmd+K on macOS) is
// the keymap's `palette.open` binding like any other: the global dispatcher (`keymap/dispatch.ts`)
// runs it, so a person can rebind it, and it works from anywhere, a text field included, as a
// native accelerator does. Another dialog being open (Settings, Export) keeps it out, and pressing
// it again closes the palette.
import { closePalette, openPalette, paletteStore } from "./paletteStore.ts";

/** Whether a dialog other than the palette is showing. */
function otherDialogOpen(): boolean {
  return [...document.querySelectorAll('[role="dialog"]')].some(
    (el) => !el.classList.contains("cw-dialog--palette"),
  );
}

/** Opens the palette, or closes it when it is open; nothing while another dialog is showing. */
export function togglePalette(): void {
  if (paletteStore.getState().open) {
    closePalette();
    return;
  }
  if (otherDialogOpen()) return;
  openPalette();
}
