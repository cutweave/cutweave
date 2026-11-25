// SPDX-License-Identifier: GPL-3.0-or-later
// Ctrl+K (Cmd+K on macOS) opens the command palette (WP-2H.04). It works from anywhere, a text
// field included, as a native accelerator does; another dialog being open (Settings, Export)
// keeps it out, and pressing it again closes the palette. The keymap engine (WP-2H.05) takes
// this binding over as `palette.open`.
import { useEffect } from "react";
import { menuPlatform } from "../shell/appMenu.ts";
import type { KeyCombo } from "../shortcuts/bindings.ts";
import { closePalette, openPalette, paletteStore } from "./paletteStore.ts";

/** The key that opens the palette, as the shortcut list names it. */
export const PALETTE_COMBO: KeyCombo = { key: "K", mod: true };

/** Whether `event` is the palette's key on `platform`. */
export function isPaletteKey(
  event: Pick<KeyboardEvent, "key" | "code" | "ctrlKey" | "metaKey" | "altKey" | "shiftKey">,
  platform: ReturnType<typeof menuPlatform>,
): boolean {
  const mod =
    platform === "mac" ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
  if (!mod || event.altKey || event.shiftKey) return false;
  // A non-Latin layout reports another `key`; the physical key still says K.
  return event.key.toLowerCase() === "k" || event.code === "KeyK";
}

/** Whether a dialog other than the palette is showing. */
function otherDialogOpen(): boolean {
  return [...document.querySelectorAll('[role="dialog"]')].some(
    (el) => !el.classList.contains("cw-dialog--palette"),
  );
}

/** Listens for the palette's key; mount once in the shell. */
export function usePaletteShortcut(): void {
  useEffect(() => {
    const platform = menuPlatform(typeof navigator === "undefined" ? "" : navigator.platform);
    const onKey = (event: KeyboardEvent): void => {
      if (event.defaultPrevented || event.repeat || !isPaletteKey(event, platform)) return;
      if (paletteStore.getState().open) {
        event.preventDefault();
        closePalette();
        return;
      }
      if (otherDialogOpen()) return;
      event.preventDefault();
      openPalette();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}
