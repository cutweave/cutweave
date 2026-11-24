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
import { InspectorEditor } from "../inspector/editor.ts";
import { FIELDS, type FieldDef, type SelectedClip } from "../inspector/fields.ts";
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
      const opacity = field("opacity");

      for (const v of [95, 90, 80, 70, 60, 50]) {
        s.editor.live(opacity, clips, v);
        await sleep(25);
      }
      await s.editor.commit(opacity, clips, 50);
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

  live("a drag back to where it started leaves no undo entry", async () => {
    const s = await open();
    try {
      await createProject(s, "Cancelled");
      const { clip } = await seedClip(s);
      const clips = select(s, clip);
      const before = (await history(s)).length;
      const rotation = field("rotation");
      s.editor.live(rotation, clips, 20);
      await sleep(40);
      s.editor.live(rotation, clips, 0); // Escape in the scrubber reports the start value
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
        await s.editor.apply(field("rotation"), select(s, clip, partner), 15);
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
      await s.editor.apply(field("rotation"), select(s, clip), 10);
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
      const reopened = await s.rpc.call("timeline.get", { sequence, depth: 2 });
      expect(reopened).toEqual(saved);
      expect(s.stores.session.getState().recovery).toBeNull();

      // Discard closes without saving; the work stays in the journal for the next open.
      await s.editor.apply(field("rotation"), select(s, clip), 33);
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
    await a.editor.apply(field("opacity"), select(a, clip), 40);
    await a.editor.apply(field("start"), select(a, clip), sec(2));
    await a.editor.apply(field("rotation"), select(a, clip, partner), 7);
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
