// SPDX-License-Identifier: GPL-3.0-or-later
// WP-1M.07 end to end against REAL headless engines (`--dev-stub-media`): the inspector's edits,
// undo and redo, the project flows (new, save, dirty, close prompt, reopen) and crash recovery
// (SIGKILL the engine with unsaved edits, open the project again, Restore or Discard). The stores,
// `startEngineRuntime`, `InspectorEditor` and `ProjectController` are the app's own; only the
// native dialogs are fakes (the test answers the naming and unsaved prompts the way
// `ProjectDialogs` does). Skips with a message when the engine cannot be built.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ItemId, RpcClient, SequenceId, TrackId } from "@cutweave/protocol";
import {
  type EngineHandle,
  resolveEngineBinary,
  startEngine,
} from "@cutweave/protocol/test-support/engine";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { createEngineStore } from "../engine/engineStore.ts";
import { type EngineRuntime, startEngineRuntime } from "../engine/runtime.ts";
import { i18n } from "../i18n/index.ts";
import { selectSelectedClips } from "../inspector/clips.ts";
import { InspectorEditor, type PropEdit } from "../inspector/editor.ts";
import { FIELDS, type FieldDef, type SelectedClip } from "../inspector/fields.ts";
import { propsPatch, propsSchema } from "../inspector/video.ts";
import { commandCatalog } from "../palette/catalog.ts";
import { createDocStore } from "../state/docStore.ts";
import { createJobStore } from "../state/jobStore.ts";
import { createPlaybackStore } from "../state/playbackStore.ts";
import { createToastStore } from "../state/toastStore.ts";
import { createViewStore } from "../state/viewStore.ts";
import { translator } from "../timeline/errors.ts";
import type { DesktopBridge } from "./desktop.ts";
import { historyLabel } from "./historyLabels.ts";
import { ProjectController } from "./projectController.ts";
import { createSessionStore, documentHasContent } from "./sessionStore.ts";
import { createSessionUiStore } from "./sessionUiStore.ts";

const F = 705_600_000;
const sec = (s: number) => Math.round(s * F);
const t = translator(i18n.t.bind(i18n));

let skipReason: string | undefined;
let binary = "";
let workDir: string;
let mediaFile: string;
const handles: EngineHandle[] = [];
const runtimes: EngineRuntime[] = [];

/** One editor session: stores, the runtime on a real engine, and the app's controllers. */
interface Session {
  readonly handle: EngineHandle;
  readonly rpc: RpcClient;
  readonly stores: ReturnType<typeof makeStores>;
  readonly ui: ReturnType<typeof createSessionUiStore>;
  readonly toasts: ReturnType<typeof createToastStore>;
  readonly controller: ProjectController;
  readonly editor: InspectorEditor;
  readonly errors: unknown[];
  stop(): Promise<void>;
}

function makeStores() {
  return {
    doc: createDocStore(),
    view: createViewStore(),
    playback: createPlaybackStore(),
    jobs: createJobStore(),
    engine: createEngineStore(),
    session: createSessionStore(),
  };
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

async function until(
  condition: () => boolean,
  timeoutMs = 30_000,
  what = "the editor",
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!condition()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await sleep(5);
  }
}

/** Starts an engine and connects a full editor session to it. */
async function open(extraArgs: readonly string[] = []): Promise<Session> {
  const handle = await startEngine(binary, [
    "--dev-stub-media",
    "--allow-root",
    workDir,
    ...extraArgs,
  ]);
  handles.push(handle);
  const stores = makeStores();
  const errors: unknown[] = [];
  const runtime = startEngineRuntime(
    { getConnection: () => Promise.resolve({ ...handle.info, port: handle.port }) },
    stores,
    { view: { debounceMs: 50 }, onError: (e) => errors.push(e) },
  );
  runtimes.push(runtime);
  await runtime.connection.whenReady();
  await until(() => stores.doc.getState().status === "ready" && stores.session.getState().known);
  const ui = createSessionUiStore();
  const toasts = createToastStore();
  const desktop: DesktopBridge = {
    dialog: { openProjectFolder: () => Promise.resolve(workDir) },
  };
  const controller = new ProjectController({
    engine: stores.engine,
    session: stores.session,
    doc: stores.doc,
    ui,
    toasts,
    t,
    desktop,
  });
  const editor = new InspectorEditor({ engine: stores.engine, toasts, t });
  return {
    handle,
    rpc: runtime.rpc,
    stores,
    ui,
    toasts,
    controller,
    editor,
    errors,
    async stop() {
      await editor.dispose();
      await runtime.stop();
    },
  };
}

beforeAll(() => {
  const resolved = resolveEngineBinary();
  if (resolved.unavailable !== undefined) {
    skipReason = resolved.unavailable;
    console.warn(`session e2e tests skipped: ${skipReason}`);
    return;
  }
  binary = resolved.path;
  workDir = mkdtempSync(join(tmpdir(), "cutweave-session-e2e-"));
  mediaFile = join(workDir, "clip.mp4");
  // Generated here, never committed: the stub backend probes by extension (10 s, video + audio).
  writeFileSync(mediaFile, Buffer.alloc(4096, 7));
}, 20 * 60_000);

afterAll(async () => {
  for (const runtime of runtimes) await runtime.stop().catch(() => undefined);
  for (const handle of handles) await handle.stop().catch(() => undefined);
  if (workDir !== undefined) rmSync(workDir, { recursive: true, force: true });
});

const live = (name: string, body: () => Promise<void>) =>
  it(name, async (ctx) => {
    if (skipReason !== undefined) {
      ctx.skip();
      return;
    }
    await body();
  }, 120_000);

const field = (id: string): FieldDef => {
  const found = FIELDS.find((f) => f.id === id);
  if (found === undefined) throw new Error(`no field ${id}`);
  return found;
};

/** A property edit as the Video sections build it, for `path` (dot path into `props`). */
function propEdit(
  clips: SelectedClip[],
  path: string,
  value: unknown,
  current: unknown,
  at = 0,
): PropEdit {
  const input = commandCatalog.getState().commands.find((c) => c.name === "item.set_props");
  if (input === undefined) throw new Error("item.set_props is not in the catalog");
  return {
    clips,
    path,
    value,
    props: propsPatch(propsSchema(input.inputSchema as never), path, value),
    at,
    name: path.split(".").at(-1) ?? path,
    current: clips.map(() => current),
  };
}

/** Answers the naming prompt the way the dialog does, then waits for the flow. */
async function createProject(s: Session, name: string): Promise<string> {
  const flow = s.controller.newProject();
  await until(() => s.ui.getState().naming !== null, 5000, "the naming dialog");
  const parent = join(workDir, `p-${name}`);
  mkdirSync(parent, { recursive: true });
  s.ui.getState().naming?.resolve(s.controller.target(name, parent));
  expect(await flow).toBe(true);
  const path = s.stores.session.getState().path;
  if (path === null) throw new Error("the project has no path after New");
  return path;
}

interface Seeded {
  readonly sequence: SequenceId;
  readonly clip: ItemId;
  readonly partner: ItemId;
}

/** One 4 s clip (video + linked audio) at 1 s from the stub file. */
async function seedClip(s: Session): Promise<Seeded> {
  const seq = Object.values(s.stores.doc.getState().project?.sequences ?? {})[0];
  if (seq === undefined) throw new Error("no sequence");
  const video = seq.track_order.find((id) => seq.tracks[id]?.kind === "video") as TrackId;
  const imported = await s.rpc.call("media.import", { paths: [mediaFile] });
  expect(imported.failed).toEqual([]);
  const inserted = await s.rpc.call("timeline.insert", {
    sequence: seq.id,
    track: video,
    media: imported.media_ids[0] as never,
    at: "1s",
    duration: "4s",
    source_in: 0,
  });
  const [clip, partner] = inserted.clips;
  if (clip === undefined || partner === undefined) throw new Error("expected video + linked audio");
  // The insert's patch reaches the store a moment after the call returns.
  await until(
    () => select(s, clip as ItemId, partner as ItemId).length === 2,
    10_000,
    "the inserted clips in the document",
  );
  return { sequence: seq.id, clip: clip as ItemId, partner: partner as ItemId };
}

const select = (s: Session, ...ids: ItemId[]): SelectedClip[] =>
  selectSelectedClips(
    s.stores.doc.getState().project,
    ids.map((id) => ({ kind: "item", id }) as const),
  );

const itemOf = (s: Session, id: ItemId) => {
  for (const sequence of Object.values(s.stores.doc.getState().project?.sequences ?? {})) {
    for (const track of Object.values(sequence.tracks)) {
      const item = track.items[id];
      if (item !== undefined) return item;
    }
  }
  throw new Error(`no item ${id}`);
};

/** The session after a close: untitled, and the store has taken the engine's empty document. */
const untitledAndEmpty = (s: Session) =>
  until(
    () =>
      s.stores.session.getState().path === null &&
      !documentHasContent(s.stores.doc.getState().project),
    10_000,
    "the closed project to leave the stores",
  );

const history = async (s: Session) => (await s.rpc.call("edit.history", { limit: 500 })).entries;

describe("the inspector, undo and redo against the real engine", () => {
  live("an opacity drag is one gesture and one undo entry; Undo and Redo name it", async () => {
    const s = await open();
    try {
      await createProject(s, "Inspector");
      const { clip } = await seedClip(s);
      const clips = select(s, clip);
      const before = (await history(s)).length;

      for (const v of [0.95, 0.9, 0.8, 0.7, 0.6, 0.5]) {
        s.editor.liveProp(propEdit(clips, "opacity", v, 1));
        await sleep(25);
      }
      await s.editor.commitProp(propEdit(clips, "opacity", 0.5, 1));
      await until(() => itemOf(s, clip).visual.opacity === 0.5);

      const entries = await history(s);
      expect(entries.length).toBe(before + 1);
      expect(entries[0]).toMatchObject({ label: "Change opacity", kind: "gesture" });
      await until(() => s.stores.session.getState().stacks.nextUndo === "Change opacity");
      expect(s.stores.session.getState().stacks.undoDepth).toBeGreaterThan(0);

      expect(await s.controller.undo()).toBe(true);
      await until(() => itemOf(s, clip).visual.opacity === 1);
      await until(() => s.stores.session.getState().stacks.nextRedo === "Change opacity");
      expect(await s.controller.redo()).toBe(true);
      await until(() => itemOf(s, clip).visual.opacity === 0.5);
      expect(s.toasts.getState().notices).toEqual([]);
      expect(s.errors).toEqual([]);
    } finally {
      await s.stop();
    }
  });

  live("two clips take one edit as one entry, and a position keys at the playhead", async () => {
    const s = await open();
    try {
      await createProject(s, "VideoEdits");
      const { clip, partner } = await seedClip(s);
      const both = select(s, clip, partner);
      const before = (await history(s)).length;
      await s.editor.applyProp(propEdit(both, "opacity", 0.5, 1));
      await until(() => itemOf(s, clip).visual.opacity === 0.5);
      await until(() => itemOf(s, partner).visual.opacity === 0.5);
      expect((await history(s)).length).toBe(before + 1);

      // The clip starts at 1 s: a key at 2 s is item time 1 s; typing at 4 s keys it there.
      const one = select(s, clip);
      await s.editor.toggleKey(one, "transform.position", sec(2), false, "position");
      await until(() => "keyframes" in itemOf(s, clip).visual.transform.position);
      await s.editor.applyProp(
        propEdit(select(s, clip), "transform.position", { x: 100, y: 0 }, { x: 0, y: 0 }, sec(4)),
      );
      await until(() => {
        const p = itemOf(s, clip).visual.transform.position as { keyframes?: unknown[] };
        return p.keyframes?.length === 2;
      });
      const keys = (
        itemOf(s, clip).visual.transform.position as {
          keyframes: { t: number; value: { x: number } }[];
        }
      ).keyframes;
      expect(keys.map((k) => [k.t, k.value.x])).toEqual([
        [sec(1), 0],
        [sec(3), 100],
      ]);
      // An edit to an animated property without the playhead inside the clip is refused, not lost.
      await s.editor.applyProp(
        propEdit(select(s, clip), "transform.position", { x: 5, y: 5 }, { x: 0, y: 0 }, sec(9)),
      );
      expect(s.toasts.getState().notices).toHaveLength(1);
      // Removing the key at 2 s leaves one key, then the reset makes the property static again.
      await s.editor.toggleKey(select(s, clip), "transform.position", sec(2), true, "position");
      await until(() => {
        const p = itemOf(s, clip).visual.transform.position as { keyframes?: unknown[] };
        return p.keyframes?.length === 1;
      });
      await s.editor.resetProps(select(s, clip), { props: ["transform.position"] }, "position");
      await until(() => !("keyframes" in itemOf(s, clip).visual.transform.position));
    } finally {
      await s.stop();
    }
  });

  live("typing a value reaches the document within 100 ms (p95)", async () => {
    const s = await open();
    try {
      await createProject(s, "Latency");
      const { clip } = await seedClip(s);
      const clips = select(s, clip);
      const times: number[] = [];
      for (let degrees = 1; degrees <= 20; degrees++) {
        const started = performance.now();
        await s.editor.applyProp(propEdit(clips, "transform.rotation", degrees, degrees - 1));
        await until(() => itemOf(s, clip).visual.transform.rotation === degrees, 5000);
        times.push(performance.now() - started);
      }
      times.sort((a, b) => a - b);
      const p95 = times[Math.ceil(times.length * 0.95) - 1] ?? Number.POSITIVE_INFINITY;
      expect(p95, `p95 of ${times.map((t) => Math.round(t)).join(", ")} ms`).toBeLessThan(100);
    } finally {
      await s.stop();
    }
  });

  live("a drag back to where it started leaves no undo entry", async () => {
    const s = await open();
    try {
      await createProject(s, "Cancelled");
      const { clip } = await seedClip(s);
      const clips = select(s, clip);
      const before = (await history(s)).length;
      s.editor.liveProp(propEdit(clips, "transform.rotation", 20, 0));
      await sleep(40);
      s.editor.liveProp(propEdit(clips, "transform.rotation", 0, 0)); // Escape reports the start value
      await until(() => !s.editor.dragging, 3000, "the drag to cancel itself");
      await s.editor.settled();
      expect(itemOf(s, clip).visual.transform.rotation).toBe(0);
      expect((await history(s)).length).toBe(before);
      expect(await s.rpc.call("edit.locks", {})).toMatchObject({ transactions: [] });
    } finally {
      await s.stop();
    }
  });

  live(
    "start and duration inputs dispatch item.move and clip.trim; several clips edit as one step",
    async () => {
      const s = await open();
      try {
        await createProject(s, "Timecodes");
        const { clip, partner } = await seedClip(s);

        await s.editor.apply(field("start"), select(s, clip), sec(2));
        await until(() => itemOf(s, clip).start === sec(2));
        expect(itemOf(s, partner).start).toBe(sec(2)); // the linked audio follows
        let [entry] = await history(s);
        expect(entry).toMatchObject({ label: "Move clip" });

        await s.editor.apply(field("duration"), select(s, clip), sec(3));
        await until(() => itemOf(s, clip).duration === sec(3));
        expect(itemOf(s, clip).start).toBe(sec(2));
        [entry] = await history(s);
        expect(entry).toMatchObject({ label: "Trim clip" });

        // Both clips at once: one entry, both change.
        const before = (await history(s)).length;
        await s.editor.applyProp(propEdit(select(s, clip, partner), "transform.rotation", 15, 0));
        await until(() => itemOf(s, partner).visual.transform.rotation === 15);
        expect(itemOf(s, clip).visual.transform.rotation).toBe(15);
        expect((await history(s)).length).toBe(before + 1);
        expect(s.toasts.getState().notices).toEqual([]);

        // A name, then an engine refusal shows a toast and changes nothing.
        await s.editor.apply(field("name"), select(s, clip), "Interview");
        await until(() => itemOf(s, clip).name === "Interview");
        await s.editor.apply(field("start"), select(s, clip), -sec(5));
        expect(s.toasts.getState().notices).toHaveLength(1);
        expect(itemOf(s, clip).start).toBe(sec(2));
      } finally {
        await s.stop();
      }
    },
  );

  live(
    "single commands appear in undo and redo by their command name, mapped to words",
    async () => {
      const s = await open();
      try {
        await createProject(s, "Labels");
        const { clip } = await seedClip(s);
        await s.rpc.call("clip.split", { item: clip, at: "3s" });
        await until(() => s.stores.session.getState().stacks.nextUndo === "clip.split");
        expect(historyLabel("clip.split", t)).toBe("Split clip");
        await s.controller.undo();
        await until(() => s.stores.session.getState().stacks.nextRedo === "clip.split");
        await s.rpc.call("item.delete", { items: [clip] });
        await until(() => s.stores.session.getState().stacks.nextUndo === "item.delete");
        expect(historyLabel("item.delete", t)).toBe("Delete clip");
      } finally {
        await s.stop();
      }
    },
  );
});

describe("save, dirty, close and reopen against the real engine", () => {
  live("Save clears the dirty flag; the close prompt offers Save, Discard and Cancel", async () => {
    const s = await open();
    try {
      const path = await createProject(s, "Saving");
      const { clip, sequence } = await seedClip(s);
      await until(() => s.stores.session.getState().dirty, 5000, "the dirty flag");

      // Ctrl+S
      expect(await s.controller.save()).toBe(true);
      await until(() => !s.stores.session.getState().dirty, 5000, "the dirty flag to clear");

      // Closing with unsaved changes: Cancel stays.
      await s.editor.applyProp(propEdit(select(s, clip), "transform.rotation", 10, 0));
      await until(() => s.stores.session.getState().dirty);
      let settled = s.controller.settleUnsaved();
      await until(() => s.ui.getState().unsaved !== null, 5000, "the unsaved prompt");
      expect(s.ui.getState().unsaved?.project).toBe("Saving");
      s.ui.getState().unsaved?.resolve("cancel");
      expect(await settled).toBe(false);
      expect(s.stores.session.getState().dirty).toBe(true);

      // Save saves, then goes on.
      settled = s.controller.settleUnsaved();
      await until(() => s.ui.getState().unsaved !== null);
      s.ui.getState().unsaved?.resolve("save");
      expect(await settled).toBe(true);
      await until(() => !s.stores.session.getState().dirty);

      // Reopening what the app saved restores the identical timeline.
      const saved = await s.rpc.call("timeline.get", { sequence, depth: 2 });
      expect(await s.controller.open(path)).toBe(true); // already open: nothing changes
      await s.rpc.call("project.close", { save: false });
      await untitledAndEmpty(s);
      expect(await s.controller.open(path)).toBe(true);
      await until(() => s.stores.session.getState().path === path);
      // The session store learns the path before the document store takes the reopened
      // document; an edit before that selects nothing and never makes the project dirty.
      await until(() => select(s, clip).length === 1, 30_000, "the reopened document");
      const reopened = await s.rpc.call("timeline.get", { sequence, depth: 2 });
      expect(reopened).toEqual(saved);
      expect(s.stores.session.getState().recovery).toBeNull();

      // Discard closes without saving; the work stays in the journal for the next open.
      await s.editor.applyProp(propEdit(select(s, clip), "transform.rotation", 33, 0));
      await until(() => s.stores.session.getState().dirty);
      settled = s.controller.settleUnsaved();
      await until(() => s.ui.getState().unsaved !== null);
      s.ui.getState().unsaved?.resolve("discard");
      expect(await settled).toBe(true);
      await untitledAndEmpty(s);
      expect(await s.controller.open(path)).toBe(true);
      expect(s.stores.session.getState().recovery).not.toBeNull();
    } finally {
      await s.stop();
    }
  });
});

describe("crash recovery against real engines", () => {
  /** Edits a saved project, then kills the engine before anything more is saved. */
  async function crashWithUnsavedEdits() {
    const a = await open();
    const path = await createProject(a, `Crash${handles.length}`);
    const { clip, partner, sequence } = await seedClip(a);
    expect(await a.controller.save()).toBe(true);
    await until(() => !a.stores.session.getState().dirty, 5000, "the save");
    const saved = await a.rpc.call("timeline.get", { sequence, depth: 2 });

    // Edits that autosave has not written yet (it waits 2 s of quiet).
    await a.editor.applyProp(propEdit(select(a, clip), "opacity", 0.4, 1));
    await a.editor.apply(field("start"), select(a, clip), sec(2));
    await a.editor.applyProp(propEdit(select(a, clip, partner), "transform.rotation", 7, 0));
    await a.editor.apply(field("name"), select(a, clip), "Unsaved");
    await until(() => itemOf(a, clip).name === "Unsaved");
    // `project.dirty` follows the first commit within the autosave poll (250 ms); the save itself
    // waits for 2 s without edits, so killing right after is before it.
    await until(() => a.stores.session.getState().dirty, 5000, "the unsaved flag");
    const unsaved = await a.rpc.call("timeline.get", { sequence, depth: 2 });
    expect(unsaved).not.toEqual(saved);

    process.kill(a.handle.pid, "SIGKILL");
    await a.stop();
    await a.handle.stop();
    return { path, sequence, saved, unsaved };
  }

  live("Restore brings back every unsaved edit (0 lost)", async () => {
    const { path, sequence, unsaved } = await crashWithUnsavedEdits();
    const b = await open();
    try {
      expect(await b.controller.open(path)).toBe(true);
      const recovery = b.stores.session.getState().recovery;
      expect(recovery, "a cold open of a project with unsaved work offers recovery").not.toBeNull();
      expect(recovery?.records).toBeGreaterThanOrEqual(4);
      expect(recovery?.damaged).toBe(false);

      expect(await b.controller.recover()).toBe(true);
      expect(b.stores.session.getState().recovery).toBeNull();
      await until(() => b.stores.doc.getState().status === "ready");
      expect(await b.rpc.call("timeline.get", { sequence, depth: 2 })).toEqual(unsaved);
      // The restored edits carry their undo history and are unsaved until the next save.
      await until(() => b.stores.session.getState().stacks.undoDepth >= 4, 5000, "the undo stack");
      await until(() => b.stores.session.getState().dirty, 5000, "the dirty flag");
      expect(b.toasts.getState().notices.some((n) => n.tone === "success")).toBe(true);
    } finally {
      await b.stop();
    }
  });

  live("Discard yields the last saved state", async () => {
    const { path, sequence, saved } = await crashWithUnsavedEdits();
    const b = await open();
    try {
      expect(await b.controller.open(path)).toBe(true);
      expect(b.stores.session.getState().recovery).not.toBeNull();
      expect(await b.controller.discardRecovery()).toBe(true);
      expect(b.stores.session.getState().recovery).toBeNull();
      await until(() => b.stores.doc.getState().status === "ready");
      expect(await b.rpc.call("timeline.get", { sequence, depth: 2 })).toEqual(saved);
      expect(b.stores.session.getState().dirty).toBe(false);
    } finally {
      await b.stop();
    }
  });
});
