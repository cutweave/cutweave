// SPDX-License-Identifier: GPL-3.0-or-later
// `CUTWEAVE_PREVIEW=jpeg` forces the reduced preview path (WP-1M.04, arch §10 fallback): still
// frames from the engine's `/media/frame` in a 2D canvas instead of the shared-memory ring drawn
// with WebGPU. CI machines without a GPU set it; so can a user whose GPU misbehaves. Main reads
// it once and tells the editor through `AppInfo.preview`.

/** `"jpeg"` when the environment forces the reduced preview, else `undefined`. */
export function forcedPreviewPath(env: NodeJS.ProcessEnv): "jpeg" | undefined {
  return env.CUTWEAVE_PREVIEW?.trim().toLowerCase() === "jpeg" ? "jpeg" : undefined;
}
