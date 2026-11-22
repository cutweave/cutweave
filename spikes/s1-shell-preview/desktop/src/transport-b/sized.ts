// SPDX-License-Identifier: GPL-3.0-or-later
// Transport (b) pages keep the canvas backing store at the frame size, as WP-0C.06 measured it.
// Since WP-0C.02 the shared presenter (shared/present.ts) scales frames to whatever backing size
// the canvas has and leaves sizing to transport (a)'s stage (shared/stage.ts, which also
// renegotiates the engine's render size). The (b) page has no stage, so without this its canvas
// would stay at the default 300x150 and every frame would be downscaled to it.

import type { PresentTarget } from "../shared/loop.ts";
import type { Presenter } from "../shared/present.ts";
import type { Frame } from "../shared/types.ts";

export function sizedToFrame(
  presenter: Presenter,
  canvas: { width: number; height: number },
): PresentTarget {
  return {
    kind: presenter.kind,
    adapter: presenter.adapter,
    draw(f: Frame): void {
      if (canvas.width !== f.width || canvas.height !== f.height) {
        canvas.width = f.width;
        canvas.height = f.height;
      }
      presenter.draw(f);
    },
  };
}
