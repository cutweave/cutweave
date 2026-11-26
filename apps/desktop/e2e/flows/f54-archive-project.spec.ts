// SPDX-License-Identifier: GPL-3.0-or-later
// F54 Archive project (docs/user-flows.md, WP-2F.04b): import the demo media into a saved
// project; File > Archive project…; type a name, choose a location (the OS folder picker is
// stubbed), keep "Zip file"; the dialog says how many files and how much; Archive; the dialog
// follows the job and names the zip. On disk there is one zip and nothing half-written. The zip
// is extracted here the way any unzip tool does it (its entries are stored, so Node's own modules
// are enough), every SHA-256 of its `archive.json` is checked, and a second archive is made as a
// folder. Then the originals go away (the media folder is renamed) and the extracted project is
// opened: nothing is missing, no relink dialog, no Offline badge.
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  renameSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures.ts";
import {
  chooseMenu,
  createProject,
  DEMO_FILES,
  dirtyDot,
  hasRealEngine,
  importWithButton,
  launchFlowApp,
  mediaCards,
  pickFiles,
  profileDir,
  removeDir,
  shortcut,
  snapshot,
  workspace,
} from "./helpers.ts";

const dialog = (page: Page, title: string) => page.getByRole("dialog", { name: title });

/** Every file below `dir`, as paths relative to it with `/`. */
function filesBelow(dir: string, prefix = ""): string[] {
  return readdirSync(path.join(dir, prefix), { withFileTypes: true }).flatMap((entry) =>
    entry.isDirectory() ? filesBelow(dir, `${prefix}${entry.name}/`) : [`${prefix}${entry.name}`],
  );
}

interface ZipEntry {
  readonly name: string;
  /** 0 is "stored": the bytes as they are. */
  readonly method: number;
  readonly bytes: Buffer;
}

/** The entries of a zip without Zip64 parts, read from its central directory. */
function readZip(file: string): ZipEntry[] {
  const zip = readFileSync(file);
  const end = zip.lastIndexOf(Buffer.from([0x50, 0x4b, 0x05, 0x06]));
  expect(end, "the zip has its end record").toBeGreaterThanOrEqual(0);
  const count = zip.readUInt16LE(end + 10);
  let at = zip.readUInt32LE(end + 16);
  const entries: ZipEntry[] = [];
  for (let i = 0; i < count; i += 1) {
    expect(zip.readUInt32LE(at)).toBe(0x02014b50);
    const method = zip.readUInt16LE(at + 10);
    const size = zip.readUInt32LE(at + 20);
    const nameLength = zip.readUInt16LE(at + 28);
    const local = zip.readUInt32LE(at + 42);
    const name = zip.toString("utf8", at + 46, at + 46 + nameLength);
    const data = local + 30 + zip.readUInt16LE(local + 26) + zip.readUInt16LE(local + 28);
    entries.push({ name, method, bytes: zip.subarray(data, data + size) });
    at += 46 + nameLength + zip.readUInt16LE(at + 30) + zip.readUInt16LE(at + 32);
  }
  return entries;
}

interface Manifest {
  readonly format: string;
  readonly hash: string;
  readonly project: string;
  readonly files: readonly { path: string; size: number; hash: string }[];
}

/** Checks every file of the archive folder against its `archive.json`; returns the manifest. */
function verifyArchive(folder: string): Manifest {
  const manifest = JSON.parse(readFileSync(path.join(folder, "archive.json"), "utf8")) as Manifest;
  expect(manifest.format).toBe("cutweave-archive");
  expect(manifest.hash).toBe("sha256");
  for (const file of manifest.files) {
    const bytes = readFileSync(path.join(folder, file.path));
    expect(bytes.length, file.path).toBe(file.size);
    expect(createHash("sha256").update(bytes).digest("hex"), file.path).toBe(file.hash);
  }
  // The list is the whole folder but the list itself.
  expect([...manifest.files.map((f) => f.path), "archive.json"].sort()).toEqual(
    filesBelow(folder).sort(),
  );
  return manifest;
}

test.describe("F54 archive project", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("a project is archived as a zip and as a folder; the extracted zip opens complete", async () => {
    test.setTimeout(300_000);
    const info = test.info();
    const work = workspace();
    const home = profileDir();
    try {
      const { app, page, close } = await launchFlowApp(work, home);
      try {
        await createProject(app, page, "Promo", work.dir);
        await importWithButton(
          app,
          page,
          DEMO_FILES.map((f) => work.file(f)),
        );
        await expect(mediaCards(page)).toHaveCount(DEMO_FILES.length);
        await shortcut(app, "Control+S");
        await expect(dirtyDot(page)).toBeHidden();

        // File > Archive project…: the name is proposed; Archive waits for a location.
        await chooseMenu(page, app, "File", "Archive project…");
        const form = dialog(page, "Archive project");
        await expect(form).toBeVisible();
        const name = form.getByRole("textbox", { name: "Archive name" });
        await expect(name).toHaveValue("Promo archive");
        const archive = form.getByRole("button", { name: "Archive", exact: true });
        await expect(archive).toBeDisabled();
        await expect(form.getByRole("radio", { name: "Zip file" })).toBeChecked();
        await name.fill("Vault copy");
        const out = path.join(work.dir, "Vault");
        mkdirSync(out);
        const asked = await pickFiles(app, [out]);
        await form.getByRole("button", { name: "Choose…" }).click();
        await expect(form.getByRole("textbox", { name: "Location" })).toHaveValue(out);
        expect((await asked()).at(-1)?.properties).toContain("openDirectory");
        // The estimate: how many files and how much.
        await expect(form.getByRole("status")).toContainText(`${DEMO_FILES.length} media files`);
        await snapshot(page, info, "estimate");
        await expect(archive).toBeEnabled();
        await archive.click();

        // The dialog follows the job and names the zip file.
        const done = dialog(page, "Project archived");
        await expect(done).toBeVisible({ timeout: 60_000 });
        const zip = path.join(out, "Vault copy.zip");
        await expect(done).toContainText(zip);
        await expect(done.getByRole("button", { name: "Show the file" })).toBeVisible();
        await snapshot(page, info, "archived");
        await done.getByRole("button", { name: "Close" }).click();
        await expect(done).toBeHidden();
        // On disk: the zip and nothing else (no half file, no leftover folder).
        expect(filesBelow(out)).toEqual(["Vault copy.zip"]);

        // The zip holds one folder; its entries are stored; extracted, every hash verifies.
        const entries = readZip(zip);
        const extracted = path.join(work.dir, "Extracted");
        for (const entry of entries) {
          expect(entry.method, entry.name).toBe(0);
          expect(entry.name.startsWith("Vault copy/"), entry.name).toBe(true);
          const target = path.join(extracted, entry.name);
          mkdirSync(path.dirname(target), { recursive: true });
          writeFileSync(target, entry.bytes);
        }
        const project = path.join(extracted, "Vault copy");
        const manifest = verifyArchive(project);
        expect(manifest.project).toBe("Vault copy.cutweave");
        const listed = manifest.files.map((f) => f.path);
        for (const file of DEMO_FILES) expect(listed).toContain(`media/media/${file}`);

        // The same as a folder: the location is remembered, the format is chosen.
        await chooseMenu(page, app, "File", "Archive project…");
        await expect(form).toBeVisible();
        await expect(form.getByRole("textbox", { name: "Location" })).toHaveValue(out);
        await form.getByRole("textbox", { name: "Archive name" }).fill("Vault folder");
        await form.getByRole("radio", { name: "Folder" }).click();
        await expect(form.getByRole("radio", { name: "Folder" })).toBeChecked();
        await expect(form.getByRole("status")).toContainText(`${DEMO_FILES.length} media files`);
        await archive.click();
        await expect(done).toBeVisible({ timeout: 60_000 });
        const folder = path.join(out, "Vault folder");
        await expect(done).toContainText(folder);
        await expect(done.getByRole("button", { name: "Show the folder" })).toBeVisible();
        await done.getByRole("button", { name: "Close" }).click();
        await expect(done).toBeHidden();
        expect(verifyArchive(folder).project).toBe("Vault folder.cutweave");

        // The open project is the original, untouched: nothing unsaved.
        await expect(page).toHaveTitle("Promo — Cutweave");
        await expect(dirtyDot(page)).toBeHidden();

        // The originals go away; the extracted project opens complete.
        renameSync(work.media, path.join(work.dir, "media-gone"));
        expect(existsSync(work.media)).toBe(false);
        await pickFiles(app, [project]);
        await chooseMenu(page, app, "File", "Open project…");
        await expect(page).toHaveTitle("Vault copy — Cutweave");
        await expect(mediaCards(page)).toHaveCount(DEMO_FILES.length);
        await expect(mediaCards(page).getByText("Offline", { exact: true })).toHaveCount(0);
        await expect(page.getByRole("dialog")).toHaveCount(0);
      } finally {
        await close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
