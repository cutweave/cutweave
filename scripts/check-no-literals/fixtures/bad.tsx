// SPDX-License-Identifier: GPL-3.0-or-later
// Planted violations for the no-literals self-test. Each `expect:` names the rule that must fire.
export const a = { color: "#fff" }; // expect: literal-color
export const b = { background: `rgba(0, 0, 0, 0.5)` }; // expect: literal-color
export const c = { fontSize: 12 }; // expect: raw-font-size
export const d = { fontSize: "12px" }; // expect: raw-font-size
export const e = <p className="text-[11px]">x</p>; // expect: raw-font-size
export const f = { fontFamily: "Arial, sans-serif" }; // expect: font-family-stack
