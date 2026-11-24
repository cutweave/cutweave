// SPDX-License-Identifier: GPL-3.0-or-later
// Types of the narrow `window.cutweave` surface the preload exposes through `contextBridge`.
// Pure types: importable from main, preload and (later) the renderer. No generic `invoke`.

/**
 * Lifecycle of the supervised engine process (arch §2, WP-1L.03):
 * `starting → ready → crashed → restarting → ready …`; `failed` after a start error that a
 * retry cannot fix, or after more than 3 crashes in 60 s (main then offers Restart / Quit);
 * `stopped` on quit; `unavailable` when there is no engine binary.
 */
export type EngineState =
  | "unavailable"
  | "starting"
  | "ready"
  | "crashed"
  | "restarting"
  | "failed"
  | "stopped";

/** Why the last engine process stopped or failed to start. Never contains a credential. */
export interface EngineErrorInfo {
  /**
   * The engine's error-line code (`project_locked`, `port_in_use`, `bad_handshake`, `usage`,
   * `gpu_init_failed`, `start_failed`, …) or the supervisor's own: `crashed` (exit after
   * ready), `ready_timeout`, `spawn_failed`, `recovery_failed`.
   */
  readonly code: string;
  readonly message: string;
  readonly exitCode?: number;
  readonly signal?: string;
  /** Who holds the project lock (`project_locked`, exit 10). */
  readonly holder?: { readonly pid: number; readonly host: string; readonly started: number };
  /** The engine panicked: what its crash marker (`crash-<pid>.json`) recorded. */
  readonly panic?: { readonly message: string; readonly location?: string };
}

/** Journal recovery main ran after a supervised restart (no prompt; arch §17). */
export interface EngineRecovery {
  /** The project document that was reopened. */
  readonly project: string;
  /** Transactions restored from the journal (0 when nothing was pending). */
  readonly applied: number;
  /** Replay stopped at a damaged record. */
  readonly damaged: boolean;
}

export interface EngineStatus {
  readonly state: EngineState;
  /** Control port while `ready`. */
  readonly port?: number;
  /** Human-readable reason for `unavailable` / `failed` / `crashed` / `restarting`. */
  readonly message?: string;
  /**
   * Engine launch counter since the app started (1 = the first engine). A higher value means
   * the engine restarted: fetch `getConnection()` again (the UI token is per engine launch and
   * the port may have changed), then resync.
   */
  readonly generation?: number;
  /** Why the previous engine stopped (`crashed`, `restarting`, `failed`, and `ready` after a restart). */
  readonly lastError?: EngineErrorInfo;
  /** While `ready` after a supervised restart: what the automatic journal recovery restored. */
  readonly recovery?: EngineRecovery;
  /** While `crashed`: the backoff before the next start, in ms. */
  readonly restartInMs?: number;
}

/** How the renderer reaches the engine. The token never leaves the preload closure's callers. */
export interface EngineConnection {
  readonly port: number;
  /**
   * UI credential of the current engine launch (arch §6): a new one after every restart.
   * Never put it in a URL, the DOM, storage or logs.
   */
  readonly token: string;
  readonly httpUrl: string;
  readonly wsUrl: string;
  /**
   * The engine launch this connection belongs to (`EngineStatus.generation`): a connection made
   * with a lower one was to an engine that has since restarted.
   */
  readonly generation: number;
}

/**
 * Commands the native menu sends to the renderer (validated, allowlisted). Only macOS has a
 * native menu; on Windows and Linux the editor draws its own menu bar (WP-1M.09).
 */
export const MENU_COMMANDS = [
  "undo",
  "redo",
  "save",
  "saveAs",
  "new",
  "open",
  "export",
  "settings",
] as const;
export type MenuCommand = (typeof MENU_COMMANDS)[number];

/** The editor's themes: the OS window-controls overlay follows the one it shows (WP-1M.09). */
export const THEMES = ["dark", "light"] as const;
export type ThemeId = (typeof THEMES)[number];

/**
 * Edit > Cut, Copy, Paste and Select all of the in-app menu bar: main runs them on the focused
 * element of the editor window, as the native menu's edit roles did (WP-1M.09).
 */
export const EDIT_ACTIONS = ["cut", "copy", "paste", "selectAll"] as const;
export type EditAction = (typeof EDIT_ACTIONS)[number];

/** What Help > About shows (WP-1M.09). Versions only; nothing about the user or the machine. */
export interface AboutInfo {
  readonly name: string;
  readonly version: string;
  readonly electron: string;
  readonly chrome: string;
  readonly node: string;
  readonly v8: string;
  /** `win32 x64`, `darwin arm64`, … */
  readonly platform: string;
}

export interface AppInfo {
  readonly name: string;
  readonly version: string;
  readonly platform: NodeJS.Platform;
  /**
   * True only in unpackaged runs with `CUTWEAVE_TEST_HOOKS=1` (the e2e suites): the editor then
   * exposes its test handle (`window.__cutweaveEditor`). Absent otherwise.
   */
  readonly testHooks?: true;
  /**
   * True in unpackaged (development) runs: the View menu then offers Reload and Toggle developer
   * tools, whose channels refuse to run in packaged builds. Absent otherwise.
   */
  readonly dev?: true;
  /**
   * `jpeg` when `CUTWEAVE_PREVIEW=jpeg` forces the reduced preview path (still frames from the
   * engine's `/media/frame`, used on CI machines without a GPU; WP-1M.04). Absent otherwise.
   */
  readonly preview?: "jpeg";
}

export type PreviewPixelFormat = "rgba8" | "nv12";

/**
 * One preview frame from the SHM ring (WP-1H.03). `bytes` is the slot payload exactly as the
 * engine published it: rows may be padded (use `stride`; NV12's UV plane starts at `plane1Offset`).
 * Through `contextBridge` it is a structured clone, i.e. a second full copy: present from the
 * preload's present loop (`startPresenting`) and use this only for tests and thumbnails.
 */
export interface PreviewFrame {
  /** Presentation timestamp in flicks. */
  readonly pts: number;
  readonly duration: number;
  /** Strictly increasing, also across successor rings. */
  readonly seq: number;
  readonly width: number;
  readonly height: number;
  readonly format: PreviewPixelFormat;
  readonly bytes: ArrayBuffer;
  readonly stride: number;
  readonly plane1Offset: number;
  readonly plane1Stride: number;
  /** Reduced-size scrub preview; a full-size frame of the same `pts` follows. */
  readonly lowres: boolean;
  readonly rev: number;
  readonly configGen: number;
  readonly renderAgeMs: number;
  readonly publishAgeMs: number;
}

export interface PreviewClock {
  /** Audio clock in flicks (extrapolated to now), or null when the engine runs on the null clock. */
  readonly ptsFlicks: number | null;
  readonly state?: "stopped" | "playing" | "scrubbing";
  /** Rate in thousandths: 1000 = 1x, negative = reverse. */
  readonly rateMilli?: number;
}

/** Static facts about the ring the preload has open. */
export interface PreviewRingInfo {
  readonly name: string;
  readonly epoch: string;
  readonly writerPid: number;
  readonly slotCount: number;
  readonly slotCapacity: number;
}

/**
 * Liveness and progress of the ring. `ended` means the mapping is released: the writer closed
 * the ring, sent no heartbeat for 500 ms, or the ring moved twice before it could be followed
 * (`successor-gone`); ask the engine for its current ring and `open` it again.
 */
export interface PreviewStatus {
  readonly state: "live" | "starting" | "ended";
  readonly alive: boolean;
  readonly reason?:
    | "writer-closed"
    | "heartbeat-stale"
    | "successor-gone"
    | "successor-rejected"
    | "closed";
  readonly name?: string;
  readonly heartbeatAgeMs?: number;
  readonly writeIndex: number;
  readonly lastSeq: number;
  /** Times the ring moved to a larger successor. */
  readonly switches: number;
  readonly tornRetries: number;
  readonly framesDropped: number;
  readonly configGen: number;
  readonly previewWidth: number;
  readonly previewHeight: number;
}

export type PreviewWaitResult = "frame" | "changed" | "ended" | "timeout";

/** Counters of the preload present loop (small plain values; pixels never cross the bridge). */
export interface PresentStats {
  readonly running: boolean;
  readonly framesPresented: number;
  /** Frames the engine published that were never presented (gaps in `seq`). */
  readonly seqGaps: number;
  readonly lastSeq: number;
  readonly lastPts: number;
  readonly lastWidth: number;
  readonly lastHeight: number;
  readonly lastFormat: PreviewPixelFormat | null;
  /** Frames presented in the last second. */
  readonly presentFps: number;
  /** Ticks that threw (ring closed or corrupt). */
  readonly errors: number;
  readonly lastError: string | null;
  /** Display ticks (`requestAnimationFrame` callbacks) since the loop started. */
  readonly ticks: number;
  /** Ticks in the last second: the rate the canvas is refreshed at. */
  readonly tickFps: number;
  /** Ticks that came more than 1.5 display frames (25 ms) after the previous one. */
  readonly longTicks: number;
  /** The attached presenter (`webgpu`) or `none` (frames are counted but not drawn). */
  readonly presenter: "webgpu" | "none";
  /** Frames the presenter failed to upload or draw. */
  readonly presenterErrors: number;
  /** The presenter's GPU device was lost: nothing is drawn any more. */
  readonly presenterLost: boolean;
  readonly presenterError: string | null;
}

/** Result of `preview.attachCanvas`. */
export type AttachCanvasResult =
  | {
      readonly ok: true;
      /** The WebGPU adapter, e.g. `nvidia / ampere / NVIDIA GeForce RTX 3060`. */
      readonly adapter: string;
      /** The upload path (Spike S1's choice). */
      readonly upload: "writeTexture";
    }
  | {
      readonly ok: false;
      readonly reason:
        | "no-addon"
        | "bad-canvas"
        | "no-webgpu"
        | "no-adapter"
        | "no-context"
        | "device-failed";
      readonly message: string;
    };

export interface PresentOptions {
  /**
   * Clock margin: present the newest frame with `pts <= audioClock - latencyMs` (default 20,
   * the value Spike S1 measured as skip-free at 60 fps).
   */
  readonly latencyMs?: number;
}

/** Unsubscribe function returned by every `on*` subscription. */
export type Unsubscribe = () => void;

export interface CutweaveApi {
  readonly app: {
    getInfo(): Promise<AppInfo>;
    /** Versions for Help > About (the editor draws the dialog). */
    about(): Promise<AboutInfo>;
    /** File > Exit: quit the app; unsaved changes are settled first (the close guard asks). */
    quit(): Promise<void>;
  };
  readonly engine: {
    /** Null until the engine is ready (or when no engine is available). */
    getConnection(): Promise<EngineConnection | null>;
    getStatus(): Promise<EngineStatus>;
    onStatus(listener: (status: EngineStatus) => void): Unsubscribe;
  };
  /**
   * Frame bridge to the engine's SHM preview ring (WP-1H.03; transport (a): the addon lives in the
   * main window's non-sandboxed preload). Throws `ERR_BRIDGE_UNAVAILABLE` when the addon could not
   * be loaded; ring errors carry an `ERR_RING_*` code at the start of their message.
   */
  readonly preview: {
    isAvailable(): boolean;
    /** Open the ring the engine announced (name and epoch); replaces any open ring. */
    open(ringName: string, epoch: string): PreviewRingInfo;
    /** The newest frame not returned yet (optionally with `pts <= maxPtsFlicks`), or null. */
    getLatestFrame(maxPtsFlicks?: number): PreviewFrame | null;
    /** Resolves after at most `timeoutMs` when a new frame arrives or the ring changes state. */
    waitForFrame(timeoutMs: number): Promise<PreviewWaitResult>;
    getClock(): PreviewClock;
    getStatus(): PreviewStatus;
    /** Start the present loop in the preload (clock-scheduled frame choice). */
    startPresenting(options?: PresentOptions): void;
    /**
     * Draw presented frames into `canvas` with WebGPU (WP-1M.04): the preload creates the device
     * and the canvas's `webgpu` context and uploads each frame itself, so pixels never cross the
     * bridge. Replaces a canvas attached before. Resolves `{ok: false, reason}` when WebGPU cannot
     * present here (the page then falls back to JPEG frames).
     */
    attachCanvas(canvas: HTMLCanvasElement): Promise<AttachCanvasResult>;
    /** Release the canvas and its GPU device; frames are counted but not drawn. */
    detachCanvas(): void;
    /** Draw the last frame again (the page resized the canvas, which cleared it). */
    redraw(): boolean;
    stopPresenting(): void;
    getPresentStats(): PresentStats | null;
    close(): void;
  };
  readonly menu: {
    onCommand(listener: (command: MenuCommand) => void): Unsubscribe;
  };
  readonly dialog: {
    /** Native folder picker; resolves to the chosen path or null when cancelled. */
    openProjectFolder(): Promise<string | null>;
    /**
     * Native multi-file picker for media import (WP-1M.03). Resolves to the absolute paths the
     * user chose (at most 256) or null when cancelled; the paths are then sent to `media.import`.
     */
    pickMediaFiles(): Promise<string[] | null>;
    /**
     * Native "save as" for the exported video (WP-1M.08), starting at `defaultPath` (a full path
     * or a file name). Resolves to the chosen absolute `.mp4` path, or null when cancelled.
     */
    chooseExportFile(defaultPath: string): Promise<string | null>;
  };
  readonly files: {
    /**
     * The filesystem path of a `File` dropped from the OS (`webUtils.getPathForFile`, run in the
     * preload), or `""` when it has none (not a file on disk). The renderer validates the paths
     * it sends on to `media.import`; the engine decides what it may read.
     */
    pathForFile(file: File): string;
  };
  /** Recent projects, newest first, kept by main (WP-1M.07). Paths are absolute. */
  readonly recent: {
    list(): Promise<readonly string[]>;
    add(path: string): Promise<void>;
    remove(path: string): Promise<void>;
  };
  /**
   * Closing the window with unsaved changes (WP-1M.07): the editor reports whether the project is
   * dirty; main then holds a close request, asks the editor (`onCloseRequested`) and closes only
   * after `confirmClose`.
   */
  readonly window: {
    setDirty(dirty: boolean): Promise<void>;
    onCloseRequested(listener: () => void): Unsubscribe;
    confirmClose(): Promise<void>;
    /** View > Toggle full screen (F11). */
    toggleFullScreen(): Promise<void>;
    /** The theme the editor shows: the OS window-controls overlay takes its colors (Windows, Linux). */
    setTheme(theme: ThemeId): Promise<void>;
    /** View > Reload; development builds only (refused when packaged). */
    reload(): Promise<void>;
    /** View > Toggle developer tools; development builds only (refused when packaged). */
    toggleDevTools(): Promise<void>;
  };
  /** Edit > Cut, Copy, Paste, Select all on the focused element (WP-1M.09). */
  readonly edit: {
    run(action: EditAction): Promise<void>;
  };
  readonly shell: {
    showItemInFolder(path: string): Promise<void>;
    /**
     * Open an `https:` link in the OS browser. Main re-validates the URL (https only, no
     * credentials); the app itself never navigates away from `app://cutweave`.
     */
    openExternal(url: string): Promise<void>;
  };
}
