// SPDX-License-Identifier: GPL-3.0-or-later
// Real OS mouse input and screen pixels for the window-chrome flow (WP-1M.09, ADR-0018).
//
// Dragging a title bar and double-clicking it are handled by the OS (the drag region is a
// caption hit-test), not by the page, so Playwright's page-level mouse cannot exercise them.
// These helpers move the real cursor and press the real button with Win32 `SetCursorPos` /
// `SendInput`, and read real screen pixels with GDI, the way a person's hand and eyes would.
// Windows only, and opt-in (`osInputAvailable`); otherwise the steps that need it are skipped.
//
// Safety: every press first checks that the window under the cursor belongs to the app under
// test (`owner`, its main process id) and aborts otherwise, so a covered window never sends a
// click to someone else's app. The cursor is put back where it was afterwards.
//
// Coordinates are physical screen pixels: convert page points (CSS px) with `screenPoint()`.

import { spawnSync } from "node:child_process";
import type { ElectronApplication } from "@playwright/test";

/**
 * Real-cursor steps run on Windows only, and only when asked for with
 * `CUTWEAVE_E2E_REAL_INPUT=1` (`just e2e-real-input`): they move the person's own cursor and
 * fail whenever anything else on the desktop is in the way (the person working, another
 * suite's windows), so the everyday gate leaves them out.
 */
export const osInputAvailable =
  process.platform === "win32" && process.env.CUTWEAVE_E2E_REAL_INPUT === "1";

export interface Point {
  readonly x: number;
  readonly y: number;
}

/** One step of OS input. Pauses pace the input like a hand does (the OS's own drag loop). */
export type OsStep =
  | { readonly op: "move"; readonly x: number; readonly y: number }
  | { readonly op: "down" }
  | { readonly op: "up" }
  /** One click (down and up queued together). */
  | { readonly op: "click" }
  /** Two clicks queued in one `SendInput` call: a double-click whatever the machine's load. */
  | { readonly op: "doubleClick" }
  | { readonly op: "pause"; readonly ms: number }
  | { readonly op: "pixel"; readonly x: number; readonly y: number };

const CSHARP = `
using System;
using System.Drawing;
using System.Runtime.InteropServices;
public static class CwOsInput {
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
  [DllImport("user32.dll")] static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] static extern bool GetCursorPos(out POINT p);
  [StructLayout(LayoutKind.Sequential)] struct MOUSEINPUT {
    public int dx; public int dy; public uint mouseData; public uint dwFlags; public uint time; public IntPtr extra;
  }
  [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public MOUSEINPUT mi; }
  [DllImport("user32.dll")] static extern uint SendInput(uint n, INPUT[] inputs, int size);
  static void Buttons(params uint[] flags) {
    var inputs = new INPUT[flags.Length];
    for (int i = 0; i < flags.Length; i++) { inputs[i].type = 0; inputs[i].mi.dwFlags = flags[i]; }
    if (SendInput((uint)inputs.Length, inputs, Marshal.SizeOf(typeof(INPUT))) != inputs.Length) {
      throw new InvalidOperationException("SendInput was blocked");
    }
  }
  [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(POINT p);
  [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr h, uint flags);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  public static void Init() { SetProcessDPIAware(); }
  public static void Move(int x, int y) { SetCursorPos(x, y); }
  public static void Down() { Buttons(0x0002); }
  public static void Up() { Buttons(0x0004); }
  public static void Click() { Buttons(0x0002, 0x0004); }
  public static void DoubleClick() { Buttons(0x0002, 0x0004, 0x0002, 0x0004); }
  public static int[] Cursor() { POINT p; GetCursorPos(out p); return new int[] { p.X, p.Y }; }
  public static uint OwnerUnderCursor() {
    POINT p; GetCursorPos(out p);
    IntPtr h = WindowFromPoint(p);
    if (h == IntPtr.Zero) return 0;
    uint pid; GetWindowThreadProcessId(GetAncestor(h, 2), out pid);
    return pid;
  }
  public static string Pixel(int x, int y) {
    using (var bmp = new Bitmap(1, 1)) using (var g = Graphics.FromImage(bmp)) {
      g.CopyFromScreen(x, y, 0, 0, new Size(1, 1));
      var c = bmp.GetPixel(0, 0);
      return string.Format("#{0:x2}{1:x2}{2:x2}", c.R, c.G, c.B);
    }
  }
}`;

const SCRIPT = (planJson: string, owner: number) => `
$ErrorActionPreference = 'Stop'
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
${CSHARP}
'@
[CwOsInput]::Init()
$start = [CwOsInput]::Cursor()
$pressed = $false
try {
  foreach ($s in (ConvertFrom-Json '${planJson.replace(/'/g, "''")}')) {
    switch ($s.op) {
      'move' { [CwOsInput]::Move([int]$s.x, [int]$s.y) }
      'down' {
        $pid0 = [CwOsInput]::OwnerUnderCursor()
        if ($pid0 -ne ${owner}) { throw "the window under the cursor belongs to process $pid0, not the app (${owner})" }
        [CwOsInput]::Down(); $pressed = $true
      }
      'up' { [CwOsInput]::Up(); $pressed = $false }
      'click' {
        $pid0 = [CwOsInput]::OwnerUnderCursor()
        if ($pid0 -ne ${owner}) { throw "the window under the cursor belongs to process $pid0, not the app (${owner})" }
        [CwOsInput]::Click()
      }
      'doubleClick' {
        $pid0 = [CwOsInput]::OwnerUnderCursor()
        if ($pid0 -ne ${owner}) { throw "the window under the cursor belongs to process $pid0, not the app (${owner})" }
        [CwOsInput]::DoubleClick()
      }
      'pause' { Start-Sleep -Milliseconds ([int]$s.ms) }
      'pixel' { Write-Output ([CwOsInput]::Pixel([int]$s.x, [int]$s.y)) }
    }
  }
} finally {
  if ($pressed) { [CwOsInput]::Up() }
  [CwOsInput]::Move($start[0], $start[1])
}
`;

/**
 * Runs the steps in one PowerShell process; returns the lines `pixel` steps printed. `owner` is
 * the app's main process id: a press over any other process's window aborts the run.
 */
export function runOsInput(steps: readonly OsStep[], owner: number): string[] {
  if (!osInputAvailable) throw new Error("OS input injection is implemented for Windows only");
  const encoded = Buffer.from(SCRIPT(JSON.stringify(steps), owner), "utf16le").toString("base64");
  const result = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", encoded],
    { encoding: "utf8", timeout: 60_000, windowsHide: true },
  );
  if (result.status !== 0) {
    throw new Error(`OS input failed (${result.status}): ${result.stderr || result.stdout}`);
  }
  return result.stdout
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line !== "");
}

/**
 * The process that owns the app's windows: Electron's main process (`app.process()` can be a
 * launcher in front of it).
 */
export function ownerOf(app: ElectronApplication): Promise<number> {
  return app.evaluate(() => process.pid);
}

/** A point in the editor window's page (CSS px = DIPs) as a physical screen point. */
export async function screenPoint(app: ElectronApplication, page: Point): Promise<Point> {
  return app.evaluate(({ BrowserWindow, screen }, p) => {
    const win = BrowserWindow.getAllWindows()[0];
    if (win === undefined) throw new Error("no window");
    const content = win.getContentBounds();
    const dip = { x: Math.round(content.x + p.x), y: Math.round(content.y + p.y) };
    // Windows and Linux only; DIPs elsewhere are already screen points.
    return typeof screen.dipToScreenPoint === "function" ? screen.dipToScreenPoint(dip) : dip;
  }, page);
}

/**
 * Press, nudge past the OS drag threshold, drag the rest in small steps, release: a hand
 * dragging the title bar. The OS starts moving the window after the nudge, so the window moves
 * by the full distance or by the distance less the nudge.
 */
export function osDrag(owner: number, from: Point, to: Point, nudge: Point): void {
  const steps: OsStep[] = [
    { op: "move", x: from.x, y: from.y },
    { op: "pause", ms: 200 },
    { op: "down" },
    { op: "pause", ms: 200 },
    { op: "move", x: from.x + nudge.x, y: from.y + nudge.y },
    { op: "pause", ms: 300 },
  ];
  const n = 12;
  const sx = from.x + nudge.x;
  const sy = from.y + nudge.y;
  for (let i = 1; i <= n; i += 1) {
    steps.push({
      op: "move",
      x: Math.round(sx + ((to.x - sx) * i) / n),
      y: Math.round(sy + ((to.y - sy) * i) / n),
    });
    steps.push({ op: "pause", ms: 40 });
  }
  steps.push({ op: "pause", ms: 300 }, { op: "up" }, { op: "pause", ms: 200 });
  runOsInput(steps, owner);
}

/**
 * One click: what a person does to bring a window to the front before dragging it (Windows
 * does not let a background process take the foreground itself).
 */
export function osClick(owner: number, at: Point): void {
  runOsInput(
    [
      { op: "move", x: at.x, y: at.y },
      { op: "pause", ms: 200 },
      { op: "click" },
      { op: "pause", ms: 200 },
    ],
    owner,
  );
}

/** A double-click at one spot (both clicks queued together, so the OS always pairs them). */
export function osDoubleClick(owner: number, at: Point): void {
  runOsInput(
    [
      { op: "move", x: at.x, y: at.y },
      { op: "pause", ms: 300 },
      { op: "doubleClick" },
      { op: "pause", ms: 300 },
    ],
    owner,
  );
}

/** The color of one screen pixel (`#rrggbb`), with the cursor parked out of the way first. */
export function osPixel(owner: number, at: Point, park: Point): string {
  const out = runOsInput(
    [
      { op: "move", x: park.x, y: park.y },
      { op: "pause", ms: 300 },
      { op: "pixel", x: at.x, y: at.y },
    ],
    owner,
  );
  const color = out.at(-1);
  if (color === undefined) throw new Error("no pixel read");
  return color;
}
