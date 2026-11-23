// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from "vitest";
import type { EngineStatus } from "../../shared/api.ts";
import { engineFailedDialogOptions, showEngineFailedDialog } from "./failed-dialog.ts";

const crashed: EngineStatus = {
  state: "failed",
  message: "The engine stopped 4 times in 60 s. Last error: exit code 70",
  lastError: { code: "crashed", message: "exit code 70", exitCode: 70 },
};

describe("engine failed dialog", () => {
  it("says what happened and offers Restart (default) or Quit", () => {
    const o = engineFailedDialogOptions(crashed, "/work/film/film.cutweave");
    expect(o).toMatchObject({ buttons: ["Restart", "Quit"], defaultId: 0, cancelId: 1 });
    expect(o.message).toBe("The engine stopped");
    expect(o.detail).toContain("4 times in 60 s");
    expect(o.detail).toContain("film.cutweave are kept in its journal");
    expect(o.detail).not.toContain("/work/film");
    expect(engineFailedDialogOptions(crashed, null).detail).toContain("Restart the engine");
  });

  it("names the other process holding the project", () => {
    const o = engineFailedDialogOptions(
      {
        state: "failed",
        message: "held",
        lastError: {
          code: "project_locked",
          message: "held",
          exitCode: 10,
          holder: { pid: 4242, host: "studio", started: 1 },
        },
      },
      "/p/a.cutweave",
    );
    expect(o.message).toMatch(/open in another Cutweave window/);
    expect(o.detail).toContain("process 4242 on studio");
  });

  it("maps the button to a choice", async () => {
    const dialog = { showMessageBox: vi.fn(async () => ({ response: 1, checkboxChecked: false })) };
    expect(await showEngineFailedDialog(dialog, null, crashed, null)).toBe("quit");
    dialog.showMessageBox.mockResolvedValueOnce({ response: 0, checkboxChecked: false });
    expect(await showEngineFailedDialog(dialog, null, crashed, null)).toBe("restart");
  });
});
