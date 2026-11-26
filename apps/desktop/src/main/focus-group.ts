// SPDX-License-Identifier: GPL-3.0-or-later
// The engine's focus record (`project.report_focus`, WP-1J.04, ADR-0021) counts the editor window
// and the popouts it opened (detached panels, ADR-0031) as one: moving the focus between them is
// not "the app lost the focus and got it back", so an agent's default engine never flips while a
// person works in a detached Inspector. Pure; index.ts feeds it the windows' focus and blur.

export interface FocusGroupOptions {
  /** The group gained (`true`) or lost (`false`) the focus. */
  report(focused: boolean): void;
  /** Whether a window of the group has the focus now. */
  anyFocused(): boolean;
  /** Timer seam for tests; `setTimeout` by default. */
  schedule?(run: () => void, ms: number): unknown;
  cancel?(handle: unknown): void;
}

/**
 * How long a blur waits for the focus to land on another window of the group before it counts.
 * The OS blurs one window before it focuses the next.
 */
export const FOCUS_SETTLE_MS = 100;

export class FocusGroup {
  #focused = false;
  #pending: unknown = null;
  readonly #o: FocusGroupOptions;

  constructor(options: FocusGroupOptions) {
    this.#o = options;
  }

  /** Whether the group has the focus, as last reported. */
  get focused(): boolean {
    return this.#focused;
  }

  /** A window of the group gained the focus. */
  focus(): void {
    this.#cancelPending();
    if (this.#focused) return;
    this.#focused = true;
    this.#o.report(true);
  }

  /** A window of the group lost the focus: it counts once no other window of the group has it. */
  blur(): void {
    this.#cancelPending();
    const schedule = this.#o.schedule ?? ((run, ms) => setTimeout(run, ms));
    this.#pending = schedule(() => {
      this.#pending = null;
      const now = this.#o.anyFocused();
      if (now === this.#focused) return;
      this.#focused = now;
      this.#o.report(now);
    }, FOCUS_SETTLE_MS);
  }

  #cancelPending(): void {
    if (this.#pending === null) return;
    const cancel = this.#o.cancel ?? ((handle) => clearTimeout(handle as NodeJS.Timeout));
    cancel(this.#pending);
    this.#pending = null;
  }
}
