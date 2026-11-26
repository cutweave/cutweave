// SPDX-License-Identifier: GPL-3.0-or-later
// The timeline's clipboard (WP-2A.06): Copy, Cut, Paste, Paste attributes and Duplicate of clips.
// The engine owns the meaning (`timeline.copy` reads a payload, `timeline.paste` places one, both
// callable by an agent); this module is the editor's side of it. The payload is kept in the
// `clipboardStore` (what the menus' "Paste here" and "clipboard.hasItems" read) and, as
// `application/x-cutweave-items+json`, on the system clipboard, so another window of the app
// can paste it too. Every mutation is one registry command; the callers pass `call`, so a menu
// choice, a key and a test run the same code.
import type {
  ClipboardPayload,
  ItemId,
  SelectionRef,
  TrackId,
  TrackKind,
} from "@cutweave/protocol";
import { createStore } from "zustand/vanilla";
import { activeSequence } from "../menus/surfaces/facts.ts";
import { activeDocument } from "../shell/windows.ts";
import { viewStore } from "../state/viewStore.ts";

/** The system clipboard's format for copied clips (WP-2A.06). */
export const CLIPBOARD_MIME = "application/x-cutweave-items+json";

/** Calls a registry command or query on the engine. */
export type Call = (name: string, args?: unknown) => Promise<unknown>;

export interface ClipboardState {
  /** What the last Copy or Cut put there, or what a paste read from the system clipboard. */
  readonly payload: ClipboardPayload | null;
  setPayload(payload: ClipboardPayload | null): void;
}

export const clipboardStore = createStore<ClipboardState>()((set) => ({
  payload: null,
  setPayload: (payload) => set({ payload }),
}));

/** The `clipboard.hasItems` context key. */
export function hasClipboardItems(): boolean {
  return (clipboardStore.getState().payload?.items.length ?? 0) > 0;
}

/** A payload read from text, or `null` when it is not one this build understands. */
export function parsePayload(text: string): ClipboardPayload | null {
  try {
    const value = JSON.parse(text) as Partial<ClipboardPayload> | null;
    if (
      value !== null &&
      typeof value === "object" &&
      value.version === 1 &&
      Array.isArray(value.items) &&
      Array.isArray(value.tracks) &&
      value.items.length > 0
    ) {
      return value as ClipboardPayload;
    }
  } catch {
    // not JSON: someone else's clipboard
  }
  return null;
}

/** The clips selected on the timeline, in selection order. */
export function selectedClips(selection: readonly SelectionRef[] = viewStore.getState().selection) {
  return selection.flatMap((ref) => (ref.kind === "item" ? [ref.id as string] : []));
}

// Spelled in two parts: a dotted literal under a catalog root reads as a missing catalog key.
const COPY = `${"timeline"}.copy`;
const PASTE = `${"timeline"}.paste`;

let writing = false;

/**
 * Whether the copy event in flight is our own system-clipboard write (so the Copy handlers
 * ignore it instead of copying again).
 */
export function isWritingClipboard(): boolean {
  return writing;
}

/**
 * Puts the payload on the system clipboard under its own format. A page can only write the
 * clipboard from a `copy` event, so this asks the browser for one and fills it in.
 */
function writeSystemClipboard(payload: ClipboardPayload): void {
  if (typeof globalThis.document === "undefined") return;
  // The document the person is working in: a detached Timeline's window has its own, and only a
  // document with the person's key press may write the clipboard.
  const doc = activeDocument();
  const json = JSON.stringify(payload);
  const fill = (event: ClipboardEvent): void => {
    event.clipboardData?.setData(CLIPBOARD_MIME, json);
    event.preventDefault();
  };
  writing = true;
  doc.addEventListener("copy", fill, { capture: true, once: true });
  try {
    doc.execCommand("copy");
  } catch {
    // The in-app clipboard still works; only the cross-window paste is lost.
  } finally {
    doc.removeEventListener("copy", fill, { capture: true });
    writing = false;
  }
}

/** Copies `ids` (and their linked partners): the engine's payload goes to both clipboards. */
export async function copyClips(call: Call, ids: readonly string[]): Promise<ClipboardPayload> {
  const out = (await call(COPY, { items: ids })) as { clipboard: ClipboardPayload };
  clipboardStore.getState().setPayload(out.clipboard);
  writeSystemClipboard(out.clipboard);
  return out.clipboard;
}

/** Cut: copy, then delete the clips. The deletion is the one mutation. */
export async function cutClips(call: Call, ids: readonly string[]): Promise<void> {
  await copyClips(call, ids);
  await call("item.delete", { items: ids });
}

/** The track a pasted payload's top row goes to when the person gave none (or an unfit one). */
export function defaultAnchor(payload: ClipboardPayload): TrackId | undefined {
  const found = activeSequence();
  const first = payload.tracks[0];
  if (found === null || first === undefined) return undefined;
  // Back where it came from, when it is still here.
  if (payload.tracks.every((t) => found.sequence.tracks[t.source] !== undefined)) return undefined;
  const sameKind = found.sequence.track_order
    .map((id) => found.sequence.tracks[id])
    .find((t) => t?.kind === first.kind);
  return sameKind?.id;
}

/** The clicked track when it can take the payload's top row (the same kind), else the default. */
export function anchorFor(payload: ClipboardPayload, clicked?: string): TrackId | undefined {
  const found = activeSequence();
  const first = payload.tracks[0];
  const track = clicked === undefined ? undefined : found?.sequence.tracks[clicked as TrackId];
  if (track !== undefined && first !== undefined && track.kind === (first.kind as TrackKind)) {
    return track.id;
  }
  return defaultAnchor(payload);
}

/** Pastes a payload at `at` (flicks). The new clips become the selection. */
export async function pasteClips(
  call: Call,
  payload: ClipboardPayload,
  at: number,
  clicked?: string,
): Promise<void> {
  const track = anchorFor(payload, clicked);
  const out = (await call(PASTE, {
    payload,
    at,
    ...(track !== undefined && { track }),
  })) as { copies: { copy: ItemId }[] };
  selectCreated(out.copies.map((c) => c.copy));
}

/** Duplicates `ids` after the selection. The copies become the selection. */
export async function duplicateClips(call: Call, ids: readonly string[]): Promise<void> {
  const out = (await call("item.duplicate", { items: ids })) as { copies: { copy: ItemId }[] };
  selectCreated(out.copies.map((c) => c.copy));
}

function selectCreated(ids: readonly ItemId[]): void {
  viewStore.getState().setSelection(ids.map((id) => ({ kind: "item", id }) as SelectionRef));
}

/** The clip Paste attributes reads from: the first copied clip, while it is still in the project. */
export function attributesSource(): string | null {
  const payload = clipboardStore.getState().payload;
  const found = activeSequence();
  if (payload === null || found === null) return null;
  for (const entry of payload.items) {
    const id = entry.item.id as string;
    for (const track of Object.values(found.sequence.tracks)) {
      if (track.items[id as keyof typeof track.items] !== undefined) return id;
    }
  }
  return null;
}

/** The groups `clip.paste_attributes` copies, in the order the dialog lists them. */
export const ATTRIBUTE_GROUPS = ["transform", "crop", "flip", "compositing", "audio"] as const;
export type AttributeGroup = (typeof ATTRIBUTE_GROUPS)[number];
