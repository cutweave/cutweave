// SPDX-License-Identifier: GPL-3.0-or-later
// Must produce zero findings. Literals in comments (#fff, fontSize: 12) are ignored.
export const a = { color: "var(--text-primary)", fontFamily: "var(--font-mono)" };
export const b = { fontSize: "var(--icon-md)", gap: 12 };
export const c = <a href="#top">https://example.com/#fff</a>;
export const d = "// not a comment #notcolor";
class Q {
  #priv = 1;
  get() {
    return this.#priv;
  }
}
export { Q };
