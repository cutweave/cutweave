// SPDX-License-Identifier: GPL-3.0-or-later
// The window-close guard (WP-1M.07): closing the editor window (or quitting) with unsaved
// changes asks the editor first. The editor reports its dirty flag (`window:setDirty`); while it
// is set, main holds the first close, tells the editor (`window:closeRequested`) and closes only
// once the editor answers `window:confirmClose` (saved, or the person chose to discard).
// A renderer that is gone or hung cannot answer, so it never holds the window.

export class CloseGuard {
  #dirty = false;
  #confirmed = false;

  setDirty(dirty: boolean): void {
    this.#dirty = dirty;
    // New unsaved work after a confirmation (a window that stayed open) asks again.
    if (dirty) this.#confirmed = false;
  }

  /** The editor settled its unsaved changes: the next close goes through. */
  confirm(): void {
    this.#confirmed = true;
  }

  /** A new window starts clean. */
  reset(): void {
    this.#dirty = false;
    this.#confirmed = false;
  }

  /**
   * Whether a close request must wait for the editor's answer. `rendererAlive` is false for a
   * crashed or destroyed renderer.
   */
  holds(rendererAlive: boolean): boolean {
    return this.#dirty && !this.#confirmed && rendererAlive;
  }
}
