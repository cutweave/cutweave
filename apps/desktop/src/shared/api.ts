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

/** Commands native menu accelerators send to the renderer (validated, allowlisted). */
export const MENU_COMMANDS = ["undo", "redo", "save", "open", "export"] as const;
export type MenuCommand = (typeof MENU_COMMANDS)[number];

export interface AppInfo {
  readonly name: string;
  readonly version: string;
  readonly platform: NodeJS.Platform;
  /**
   * True only in unpackaged runs with `CUTWEAVE_TEST_HOOKS=1` (the e2e suites): the editor then
   * exposes its test handle (`window.__cutweaveEditor`). Absent otherwise.
   */
  readonly testHooks?: true;
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
}

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
