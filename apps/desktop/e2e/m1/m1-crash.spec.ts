// SPDX-License-Identifier: GPL-3.0-or-later
// M1 exit criterion 3 (WP-1O.04, docs/milestones/M1-exit.md): kill the engine mid-edit, and the
// UI recovers with no lost commands. Trials, each against an uninterrupted run:
//
// 1. Engine killed mid-edit: a 30-command script (inserts, trims, splits, moves, renames, new
//    tracks; most of them change the document again if applied twice) is sent through the
//    editor's own engine connection, as `e2e/resync.spec.ts` does, so the outbox, the resend
//    under the original request_id, the journal replay and the resync are the real ones. The
//    engine is SIGKILLed after command k (k spread over the script), with command k still in
//    flight in odd trials and answered in even ones; the next two commands are sent while it is
//    down. The document must equal the uninterrupted run's: nothing lost, nothing twice.
// 2. Engine killed during a pointer drag (a gesture transaction): the drag is rolled back to the
//    last committed state, the editor is back online, and the same drag afterwards lands once.
// 3. The whole app killed after k acknowledged edits: relaunch, open the project from Recent,
//    Restore in the recovery dialog: the document equals the last acknowledged state, ids
//    included.
//
// The crash is fault injection (`killEngine`, `killTree`); between engine kills the supervisor's
// crash-loop guard (four crashes in 60 s) is reset with the hook resync.spec.ts uses, since it is
// not what is measured here. F11 and F12 cover the same journeys with a person's clicks and drags.
// Trials per kind: `CUTWEAVE_M1_TRIALS` (the gate passes 20), else 4.
import { closeSync, existsSync, openSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import type { ElectronApplication, Page } from "@playwright/test";
import {
  editorCall,
  editorState,
  engineStatus,
  expect,
  killEngine,
  killTree,
  type LaunchedApp,
  test,
  unsavedPrompt,
} from "../fixtures.ts";
import {
  clipBox,
  closeSettings,
  dirtyDot,
  drag,
  editorCaughtUp,
  engineChip,
  hasRealEngine,
  importWithButton,
  launchFlowApp,
  namingDialog,
  openRecent,
  openSettingsPage,
  profileDir,
  projectDoc,
  recoveryDialog,
  removeDir,
  type SequenceView,
  sec,
  sequence,
  shortcut,
  timelineGeometry,
  toast,
  type Workspace,
  workspace,
} from "../flows/helpers.ts";
import { keepJson, need, trialCount } from "./m1-support.ts";

const TRIALS = trialCount(4);
/** The media the script places, by file name: `media[i]` of the script is `FILES[i]`. */
const FILES = ["interview.mp4", "broll-city.mp4", "broll-coffee.mp4"] as const;

// ---- the script ------------------------------------------------------------------------------

interface Ctx {
  sequence: string;
  video: string;
  /** Media ids in FILES order. */
  media: string[];
  /** The three placed clips (the left halves after their splits). */
  clips: string[];
}

interface Step {
  readonly name: string;
  call(c: Ctx): [string, Record<string, unknown>];
  keep?(c: Ctx, out: unknown): void;
}

const QUARTER = sec(0.25);
const insert = (i: number, at: number): Step => ({
  name: `insert clip ${i} at ${at} s`,
  call: (c) => [
    "timeline.insert",
    { sequence: c.sequence, track: c.video, media: c.media[i], at: sec(at) },
  ],
  keep: (c, out) => {
    c.clips[i] = (out as { clips: string[] }).clips[0] ?? "";
  },
});
const trim = (i: number): Step => ({
  name: `trim clip ${i} by -0.25 s`,
  call: (c) => ["clip.trim", { item: c.clips[i], edge: "end", delta: -QUARTER }],
});
const addTrack = (kind: "video" | "audio"): Step => ({
  name: `add a ${kind} track`,
  call: (c) => ["track.add", { sequence: c.sequence, kind }],
});
const rename = (i: number, n: number): Step => ({
  name: `rename clip ${i}`,
  call: (c) => ["item.set_props", { item: c.clips[i], props: { name: `Step ${n}` } }],
});
const move = (i: number, at: number): Step => ({
  name: `move clip ${i} to ${at} s`,
  call: (c) => ["item.move", { item: c.clips[i], start: sec(at) }],
});
const split = (i: number, at: number): Step => ({
  name: `split clip ${i} at ${at} s`,
  call: (c) => ["clip.split", { item: c.clips[i], at: sec(at) }],
});

/**
 * Thirty commands. Each clip keeps to its own 20 s of the timeline, so every command is valid in
 * every run. Inserts, delta trims, splits and new tracks change the document again when applied
 * twice; a lost command of any kind changes the result.
 */
const SCRIPT: readonly Step[] = [
  insert(0, 0),
  insert(1, 20),
  insert(2, 40),
  trim(0),
  addTrack("video"),
  rename(1, 6),
  move(2, 42),
  trim(1),
  addTrack("video"),
  rename(2, 10),
  move(0, 1),
  trim(2),
  addTrack("video"),
  rename(0, 14),
  move(1, 23),
  split(0, 4),
  trim(0),
  addTrack("audio"),
  rename(1, 19),
  split(1, 26),
  trim(1),
  move(2, 44),
  rename(2, 23),
  split(2, 47),
  trim(2),
  addTrack("video"),
  trim(0),
  trim(1),
  rename(0, 29),
  trim(2),
];

/** Whether step `i` can be sent before the replies still pending (it needs no clip id from them). */
const ready = (i: number, c: Ctx): boolean => i < 3 || c.clips.filter(Boolean).length === 3;

// ---- reading the document --------------------------------------------------------------------

interface Snapshot {
  tracks: {
    kind: string;
    name: string;
    items: {
      start: number;
      duration: number;
      media: number;
      group: number | null;
      enabled: boolean;
      name: string | null;
    }[];
  }[];
  /** Entries in the undo history. */
  history: number;
}

/** The document as the engine has it, ids replaced by positions (media by FILES order). */
async function snapshot(page: Page, c: Ctx): Promise<Snapshot> {
  const seq = await editorCall<
    SequenceView & { tracks: (SequenceView["tracks"][number] & { kind: string })[] }
  >(page, "timeline.get", { sequence: c.sequence, depth: 2 });
  const groups = new Map<string, number>();
  const group = (id: string | null) => {
    if (id === null) return null;
    if (!groups.has(id)) groups.set(id, groups.size);
    return groups.get(id) ?? null;
  };
  const tracks: Snapshot["tracks"] = [];
  for (const t of seq.tracks) {
    const items: Snapshot["tracks"][number]["items"] = [];
    for (const i of [...t.items].sort((a, b) => a.start - b.start)) {
      const got = await editorCall<{ name: string | null }>(page, "item.get", { item: i.id });
      items.push({
        start: i.start,
        duration: i.duration,
        media: c.media.indexOf(i.media),
        group: group(i.linked_group),
        enabled: i.enabled,
        name: got.name,
      });
    }
    tracks.push({ kind: t.kind, name: t.name, items });
  }
  const history = await editorCall<{ entries: unknown[] }>(page, "edit.history", { limit: 500 });
  return { tracks, history: history.entries.length };
}

/** The open project's sequence, V1 and media (FILES order). */
async function context(page: Page): Promise<Ctx> {
  const info = await editorCall<{ sequences: { id: string }[] }>(page, "project.info");
  const id = info.sequences[0]?.id ?? "";
  const seq = await editorCall<{ tracks: { id: string; name: string }[] }>(page, "timeline.get", {
    sequence: id,
    depth: 1,
  });
  const listed = await editorCall<{ media: { id: string; path: string | null }[] }>(
    page,
    "media.list",
    {},
  );
  const byName = new Map(listed.media.map((m) => [(m.path ?? "").split(/[\\/]/).pop(), m.id]));
  return {
    sequence: id,
    video: seq.tracks.find((t) => t.name === "V1")?.id ?? "",
    media: FILES.map((f) => byName.get(f) ?? ""),
    clips: [],
  };
}

// ---- the engine ------------------------------------------------------------------------------

/** Waits until main's engine `generation` is ready and the editor is connected to it again. */
async function backOnline(app: ElectronApplication, page: Page, generation: number) {
  await expect
    .poll(async () => (await engineStatus(app))?.generation === generation, { timeout: 30_000 })
    .toBe(true);
  await expect
    .poll(async () => {
      const s = await editorState(page).catch(() => null);
      return s?.connection === "ready" && s.generation === generation;
    })
    .toBe(true);
  await expect(engineChip(page)).toHaveText("Engine connected");
}

const generationOf = async (app: ElectronApplication) => (await engineStatus(app))?.generation ?? 0;

/** A new saved project (the journal records every commit) with FILES imported. */
async function freshProject(page: Page, work: Workspace, name: string): Promise<Ctx> {
  const info = await editorCall<{ sequences: unknown[] }>(page, "project.info");
  if (info.sequences.length > 0) await editorCall(page, "project.close", { save: true });
  await editorCall(page, "project.new", { name });
  await editorCall(page, "project.save_as", { path: path.join(work.dir, name) });
  await editorCall(page, "media.import", { paths: FILES.map((f) => work.file(f)) });
  return context(page);
}

interface Kill {
  readonly app: ElectronApplication;
  /** 1-based: the engine dies right after command `at` is sent. */
  readonly at: number;
  /** Command `at` is still in flight (else it was answered first). */
  readonly inFlight: boolean;
}

/** Runs SCRIPT[0..count) through the editor's connection; with `kill`, the engine dies once. */
async function runScript(
  page: Page,
  c: Ctx,
  count: number,
  kill?: Kill,
): Promise<{ recoveredMs: number | null; sentDuringOutage: number }> {
  let recoveredMs: number | null = null;
  let sentDuringOutage = 0;
  let i = 0;
  while (i < count) {
    const step = SCRIPT[i];
    if (step === undefined) break;
    const [method, params] = step.call(c);
    const reply = editorCall(page, method, params);
    if (kill === undefined || kill.at !== i + 1) {
      step.keep?.(c, await reply);
      i += 1;
      continue;
    }
    if (!kill.inFlight) step.keep?.(c, await reply);
    const generation = await generationOf(kill.app);
    await kill.app.evaluate(() => globalThis.__cutweaveTest?.resetEngineCrashCount());
    const killedAt = Date.now();
    await killEngine(kill.app);
    // The next commands go out while the engine is down: they wait in the outbox.
    const during: { step: Step; reply: Promise<unknown> }[] = [];
    for (let j = i + 1; j < Math.min(i + 3, count) && ready(j, c); j += 1) {
      const next = SCRIPT[j];
      if (next === undefined) break;
      const [m, p] = next.call(c);
      during.push({ step: next, reply: editorCall(page, m, p) });
    }
    sentDuringOutage = during.length;
    await backOnline(kill.app, page, generation + 1);
    recoveredMs = Date.now() - killedAt;
    if (kill.inFlight) step.keep?.(c, await reply);
    for (const d of during) d.step.keep?.(c, await d.reply);
    // Nothing is left waiting: the outbox drains once the editor's own resync calls are answered.
    await expect.poll(async () => (await editorState(page)).outbox).toEqual([]);
    i += 1 + during.length;
  }
  return { recoveredMs, sentDuringOutage };
}

/**
 * File > New project (Ctrl+N) from any state: when the editor asks about unsaved changes, Discard
 * (the trial before has checked and saved what it needed).
 */
async function newProject(app: ElectronApplication, page: Page, name: string, parent: string) {
  await shortcut(app, "Control+N");
  const prompt = unsavedPrompt(page);
  const naming = namingDialog(page);
  await expect(prompt.dialog.or(naming.dialog)).toBeVisible();
  if (await prompt.dialog.isVisible()) await prompt.discard.click();
  await expect(naming.dialog).toBeVisible();
  await naming.name.fill(name);
  await naming.location.fill(parent);
  await naming.create.click();
  await expect(naming.dialog).toBeHidden();
  await expect(page).toHaveTitle(`${name} — Cutweave`);
  await expect(dirtyDot(page)).toBeHidden();
}

/**
 * Whether no process of an app killed on profile `home` still holds Chromium's single-instance
 * lock (`<userData>/lockfile`, opened exclusively and deleted when its holder exits).
 */
function profileFree(home: string): boolean {
  const lock = path.join(home, "userData", "lockfile");
  if (!existsSync(lock)) return true;
  try {
    closeSync(openSync(lock, "r+"));
    return true;
  } catch {
    return false;
  }
}

/** Timeline items in the project file on disk. */
function itemsOnDisk(doc: string): number {
  const saved = JSON.parse(readFileSync(doc, "utf8")) as {
    sequences: Record<string, { tracks: Record<string, { items: Record<string, unknown> }> }>;
  };
  return Object.values(saved.sequences).flatMap((s) =>
    Object.values(s.tracks).flatMap((t) => Object.keys(t.items)),
  ).length;
}

// ---- the trials ------------------------------------------------------------------------------

test.describe("M1 exit 3: crash recovery trials", () => {
  test.beforeEach(() => need(hasRealEngine, "cutweave-engine-serve is not built"));

  test("the engine killed mid-edit, or mid-drag: every command lands once", async () => {
    // About 10 s a trial; the drag trials take a few more.
    test.setTimeout(120_000 + TRIALS * 20_000);
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    const rows: Record<string, unknown>[] = [];
    try {
      // The uninterrupted run everything is compared with.
      const reference = await freshProject(page, work, "Reference");
      await runScript(page, reference, SCRIPT.length);
      const expected = await snapshot(page, reference);
      expect(expected.tracks.flatMap((t) => t.items)).toHaveLength(12);

      // Script trials: one in four trials is a drag trial instead.
      const dragTrials = Math.max(1, Math.round(TRIALS / 5));
      const scriptTrials = TRIALS - dragTrials;
      for (let t = 1; t <= scriptTrials; t += 1) {
        const at = scriptTrials === 1 ? 15 : 1 + Math.round(((t - 1) * 29) / (scriptTrials - 1));
        const inFlight = t % 2 === 1;
        const c = await freshProject(page, work, `Engine kill ${t}`);
        const { recoveredMs, sentDuringOutage } = await runScript(page, c, SCRIPT.length, {
          app,
          at,
          inFlight,
        });
        const got = await snapshot(page, c);
        rows.push({
          trial: t,
          kind: "script",
          killedAfter: at,
          command: SCRIPT[at - 1]?.name,
          inFlight,
          sentDuringOutage,
          recoveredMs,
          generation: await generationOf(app),
          equal: JSON.stringify(got) === JSON.stringify(expected),
        });
        expect(got, `trial ${t}: killed after command ${at}`).toEqual(expected);
      }

      // Drag trials: a project with one clip on V1 (the script's new video tracks would push V1
      // down the timeline), dragged by the pointer; the engine dies mid-drag.
      for (let d = 1; d <= dragTrials; d += 1) {
        const c = await freshProject(page, work, `Drag kill ${d}`);
        await runScript(page, c, 1);
        await editorCaughtUp(page);
        const tracks = await editorCall<SequenceView>(page, "timeline.get", {
          sequence: c.sequence,
          depth: 2,
        });
        const first = tracks.tracks.find((t) => t.name === "V1")?.items[0];
        if (first === undefined) throw new Error("no clip on V1 to drag");
        const before = await snapshot(page, c);
        const g = await timelineGeometry(page);
        const box = clipBox(g, first.id);
        const from = { x: box.x + Math.min(box.w / 2, 60), y: box.y };
        const generation = await generationOf(app);
        await app.evaluate(() => globalThis.__cutweaveTest?.resetEngineCrashCount());
        // Press, move: the gesture transaction is open and the engine previews the move.
        await page.mouse.move(from.x, from.y);
        await page.mouse.down();
        await page.mouse.move(from.x + 4, from.y, { steps: 2 });
        await page.mouse.move(from.x + 24, from.y, { steps: 6 });
        await expect
          .poll(
            async () =>
              (
                await editorCall<{ item: { start: number } }>(page, "item.get", {
                  item: first.id,
                })
              ).item.start,
          )
          .toBeGreaterThan(first.start);
        // The engine dies mid-drag; the hand keeps moving and lets go.
        const killedAt = Date.now();
        await killEngine(app);
        await page.mouse.move(from.x + 40, from.y, { steps: 4 });
        await page.mouse.up();
        await backOnline(app, page, generation + 1);
        const recoveredMs = Date.now() - killedAt;
        // Rolled back to the last committed state: nothing half-applied, nothing lost.
        await expect.poll(() => snapshot(page, c)).toEqual(before);
        const notice = await page
          .locator(".cw-toast")
          .allInnerTexts()
          .catch(() => []);
        // The same drag with the engine up lands, once.
        await editorCaughtUp(page);
        const g2 = await timelineGeometry(page);
        const box2 = clipBox(g2, first.id);
        const from2 = { x: box2.x + Math.min(box2.w / 2, 60), y: box2.y };
        await drag(page, from2, { x: from2.x + 24, y: from2.y });
        await expect.poll(async () => (await snapshot(page, c)).history).toBe(before.history + 1);
        const after = await snapshot(page, c);
        expect(after.tracks.flatMap((t) => t.items)).toHaveLength(
          before.tracks.flatMap((t) => t.items).length,
        );
        const moved = await editorCall<{ item: { start: number } }>(page, "item.get", {
          item: first.id,
        });
        expect(moved.item.start).toBeGreaterThan(first.start);
        rows.push({
          trial: scriptTrials + d,
          kind: "drag",
          rolledBack: true,
          recoveredMs,
          generation: await generationOf(app),
          notices: notice,
          redoneOnce: true,
        });
      }
      expect(rows).toHaveLength(TRIALS);
    } finally {
      keepJson(path.join("crash", "engine-kill.json"), { trials: TRIALS, rows });
      await close();
      work.remove();
    }
  });

  test("the whole app killed after k edits: Restore brings back every acknowledged edit", async () => {
    // About 20 s a trial: two launches each, but the relaunch serves the next trial.
    test.setTimeout(120_000 + TRIALS * 45_000);
    const work = workspace();
    const home = profileDir();
    const rows: Record<string, unknown>[] = [];
    let launched: LaunchedApp | null = await launchFlowApp(work, home);
    try {
      // Autosave off (Settings > Saving and backups), as a person may have it: an idle save
      // between the last edit and the crash would leave nothing to restore. The journal records
      // every edit either way.
      const settings = await openSettingsPage(launched.page, launched.app, "Saving and backups");
      const automatic = settings.getByRole("switch", { name: "Save automatically" });
      await automatic.click();
      await expect(automatic).not.toBeChecked();
      await closeSettings(settings);
      for (let t = 1; t <= TRIALS; t += 1) {
        const { app, page } = launched;
        const name = `Crashy ${t}`;
        // A saved project, three clips imported, then k commands of the script (each answered).
        await newProject(app, page, name, work.dir);
        await importWithButton(
          app,
          page,
          FILES.map((f) => work.file(f)),
        );
        const c = await context(page);
        // k from 3 to 30, a different k in each of the first 28 trials (11 and 28 are coprime).
        const k = 3 + ((t * 11) % 28);
        await runScript(page, c, k);
        await editorCaughtUp(page);
        await expect(dirtyDot(page)).toBeVisible();
        const acked = await sequence(page);
        const ackedSnapshot = await snapshot(page, c);

        // Everything dies at once: the window and the engine.
        const child = app.process();
        const exited = new Promise<void>((resolve) => {
          if (child.exitCode !== null || child.signalCode !== null) resolve();
          else child.once("exit", () => resolve());
        });
        killTree(child.pid);
        await exited;
        await launched.close();
        launched = null;
        // A relaunch on the same profile needs Chromium's single-instance lock, which the killed
        // app's last helper process holds for a moment after the main process has gone.
        await expect
          .poll(() => profileFree(home), { timeout: 30_000, message: "the profile stays locked" })
          .toBe(true);

        // Relaunch, open it from Recent, Restore.
        launched = await launchFlowApp(work, home);
        await openRecent(launched.page, `${name}.cutweave`);
        const recovery = recoveryDialog(launched.page);
        await expect(recovery.dialog).toBeVisible();
        const offered = (await recovery.restore.textContent()) ?? "";
        // The import and the k commands.
        expect(offered).toContain(`${k + 1} edits`);
        await recovery.restore.click();
        await expect(recovery.dialog).toBeHidden();
        await expect(toast(launched.page, "Changes restored")).toBeVisible();
        const restored = await sequence(launched.page);
        expect(restored, `trial ${t}: after ${k} edits`).toEqual(acked);
        const c2 = await context(launched.page);
        expect(c2.media).toEqual(c.media);
        const restoredSnapshot = await snapshot(launched.page, c2);
        expect(restoredSnapshot.tracks).toEqual(ackedSnapshot.tracks);
        // Still unsaved after the restore. Ctrl+S writes the restored edits to the project file.
        await expect(dirtyDot(launched.page)).toBeVisible();
        const doc = projectDoc(work.dir, name);
        const before = statSync(doc).mtimeMs;
        await shortcut(launched.app, "Control+S");
        await expect.poll(() => statSync(doc).mtimeMs).toBeGreaterThan(before);
        await expect.poll(() => itemsOnDisk(doc)).toBe(acked.tracks.flatMap((x) => x.items).length);
        // Known defect (reported with WP-1O.04): after a restore the unsaved dot stays on after
        // the save. Recorded, not asserted: it is not a lost edit.
        const dirtyAfterSave = await dirtyDot(launched.page).isVisible();
        rows.push({
          trial: t,
          edits: k,
          lastCommand: SCRIPT[k - 1]?.name,
          offered: offered.trim(),
          equal: true,
          savedToDisk: true,
          dirtyAfterSave,
        });
      }
    } finally {
      keepJson(path.join("crash", "app-kill.json"), { trials: TRIALS, rows });
      await launched?.close();
      work.remove();
      removeDir(home);
    }
  });
});
