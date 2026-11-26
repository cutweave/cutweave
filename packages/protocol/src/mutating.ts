// SPDX-License-Identifier: GPL-3.0-or-later
// Which methods need a `_meta.request_id`: "every mutating call (an entry not annotated
// read-only, `view.*` aside)" (rpc.md, Envelope). A repeated request id within 10 minutes
// returns the first result, so a read must never carry one: it would come back stale.

/**
 * Registry entries annotated read-only, plus the reserved methods. A test checks this list
 * against `schema/commands.json` (`readOnlyHint`), so a new query cannot be missed.
 */
export const READ_ONLY_METHODS: ReadonlySet<string> = new Set([
  "edit.history",
  "edit.list_checkpoints",
  "edit.locks",
  "export.list_encoders",
  "export.list_presets",
  "export.queue",
  "export.resolve_preset",
  "item.get",
  "item.group_select",
  "item.layouts",
  "job.list",
  "job.status",
  "job.wait",
  "keyframe.copy",
  "keyframe.eval",
  "media.cache_status",
  "media.capabilities",
  "media.list",
  "media.list_folders",
  "media.proxy_status",
  "project.connections",
  "project.info",
  "project.snapshot",
  "rpc.hello",
  "rpc.subscribe",
  "rpc.unsubscribe",
  "sequence.list",
  "timeline.copy",
  "timeline.get",
  "timeline.list_markers",
  "track.list",
]);

/** Whether a call to `method` must carry a `request_id`. */
export function needsRequestId(method: string): boolean {
  return !READ_ONLY_METHODS.has(method) && !method.startsWith("view.");
}
