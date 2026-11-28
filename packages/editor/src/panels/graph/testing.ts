// SPDX-License-Identifier: GPL-3.0-or-later
// Tests only: a document with an animated clip for the graph editor's tests.
import type { Project } from "@cutweave/protocol";
import { sampleProject, sec } from "../../testing/fixtures.ts";

const EASE = { kind: "bezier", x1: 0.42, y1: 0, x2: 0.58, y2: 1, preset: "ease_in_out" };
const LINEAR = { kind: "linear" };

/** A clip at 1 s with three position keys (eased) and two opacity keys. */
export function keyedProject(extra: object = {}): Project {
  return sampleProject({
    itm_v: {
      visual: {
        transform: {
          position: {
            keyframes: [
              { t: 0, value: { x: -400, y: 0 }, interp: EASE },
              { t: sec(1), value: { x: 200, y: -150 }, interp: EASE },
              { t: sec(2), value: { x: 0, y: 300 }, interp: LINEAR },
            ],
          },
          scale: { x: 1, y: 1 },
          rotation: 0,
          anchor: { x: 0, y: 0 },
        },
        opacity: {
          keyframes: [
            { t: 0, value: 0, interp: LINEAR },
            { t: sec(2), value: 0.8, interp: LINEAR },
          ],
        },
      },
      ...extra,
    },
  });
}
