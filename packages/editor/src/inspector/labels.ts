// SPDX-License-Identifier: GPL-3.0-or-later
// The words the Inspector adds to what the schema says. A property or group the engine adds later
// shows the name made from its schema key until a string is added here.
import type { ProxyStatus } from "@cutweave/protocol";
import type { TKey } from "../i18n/index.ts";

/** Section titles by `x-ui.group`. */
export const GROUP_LABELS: Readonly<Record<string, TKey>> = {
  transform: "inspector.groups.transform",
  crop: "inspector.groups.crop",
  flip: "inspector.groups.flip",
  compositing: "inspector.groups.compositing",
  video: "inspector.groups.video",
  audio: "inspector.groups.audio",
};

/** A property's name in a sentence ("Change position"), by its full dot path. */
export const PROP_NAMES: Readonly<Record<string, TKey>> = {
  "transform.position": "inspector.props.transformPosition",
  "transform.scale": "inspector.props.transformScale",
  "transform.rotation": "inspector.props.transformRotation",
  "transform.anchor": "inspector.props.transformAnchor",
  opacity: "inspector.props.opacity",
  audio_gain: "inspector.props.audioGain",
  audio_pan: "inspector.props.audioPan",
  blend_mode: "inspector.props.blendMode",
  "crop.left": "inspector.props.cropLeft",
  "crop.top": "inspector.props.cropTop",
  "crop.right": "inspector.props.cropRight",
  "crop.bottom": "inspector.props.cropBottom",
  "flip.horizontal": "inspector.props.flipHorizontal",
  "flip.vertical": "inspector.props.flipVertical",
};

/** A property's row label, by its full dot path. */
export const PROP_LABELS: Readonly<Record<string, TKey>> = {
  "transform.position": "inspector.labels.transformPosition",
  "transform.scale": "inspector.labels.transformScale",
  "transform.rotation": "inspector.labels.transformRotation",
  "transform.anchor": "inspector.labels.transformAnchor",
  opacity: "inspector.labels.opacity",
  audio_gain: "inspector.labels.audioGain",
  audio_pan: "inspector.labels.audioPan",
  blend_mode: "inspector.labels.blendMode",
  "crop.left": "inspector.labels.cropLeft",
  "crop.top": "inspector.labels.cropTop",
  "crop.right": "inspector.labels.cropRight",
  "crop.bottom": "inspector.labels.cropBottom",
  "flip.horizontal": "inspector.labels.flipHorizontal",
  "flip.vertical": "inspector.labels.flipVertical",
};

export const PROXY_STATES: Readonly<Record<ProxyStatus, TKey>> = {
  none: "inspector.clipInfo.proxyStates.none",
  queued: "inspector.clipInfo.proxyStates.queued",
  generating: "inspector.clipInfo.proxyStates.generating",
  ready: "inspector.clipInfo.proxyStates.ready",
  incomplete: "inspector.clipInfo.proxyStates.incomplete",
  evicted: "inspector.clipInfo.proxyStates.evicted",
  failed: "inspector.clipInfo.proxyStates.failed",
};

/** The Track section's flag buttons. */
export const TRACK_FLAGS = {
  hidden: "inspector.track.hidden",
  muted: "inspector.track.muted",
  locked: "inspector.track.locked",
} as const satisfies Record<string, TKey>;
