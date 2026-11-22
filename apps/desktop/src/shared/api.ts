// SPDX-License-Identifier: GPL-3.0-or-later
// Types of the narrow `window.cutweave` surface the preload exposes through `contextBridge`.
// Pure types: importable from main, preload and (later) the renderer. No generic `invoke`.

/** Lifecycle of the supervised engine process (arch §2; restart states arrive in WP-1L.03). */
export type EngineState =
  | "unavailable"
  | "starting"
  | "ready"
  | "crashed"
  | "restarting"
  | "failed"
  | "stopped";

export interface EngineStatus {
  readonly state: EngineState;
  /** Control port while `ready`. */
  readonly port?: number;
  /** Human-readable reason for `unavailable` / `failed` / `crashed`. */
  readonly message?: string;
}

/** How the renderer reaches the engine. The token never leaves the preload closure's callers. */
export interface EngineConnection {
  readonly port: number;
  /** Per-launch UI credential (arch §6). Never put it in a URL, the DOM or storage. */
  readonly token: string;
  readonly httpUrl: string;
  readonly wsUrl: string;
}

/** Commands native menu accelerators send to the renderer (validated, allowlisted). */
export const MENU_COMMANDS = ["undo", "redo", "save", "open", "export"] as const;
export type MenuCommand = (typeof MENU_COMMANDS)[number];

export interface AppInfo {
  readonly name: string;
  readonly version: string;
  readonly platform: NodeJS.Platform;
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
