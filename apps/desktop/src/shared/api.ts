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

/** One decoded preview frame from the SHM ring (WP-1H.03). `buffer` is valid until the next call. */
export interface PreviewFrame {
  readonly pts: number;
  readonly width: number;
  readonly height: number;
  readonly format: PreviewPixelFormat;
  readonly seq: number;
  readonly buffer: ArrayBuffer;
}

export interface PreviewClock {
  /** Audio clock in flicks, or null when the engine runs on the null clock. */
  readonly ptsFlicks: number | null;
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
  /** Frame bridge. Stubs until WP-1H.03 loads the napi addon; they throw `ERR_BRIDGE_UNAVAILABLE`. */
  readonly preview: {
    isAvailable(): boolean;
    open(ringName: string): void;
    getLatestFrame(maxPtsFlicks?: number): PreviewFrame | null;
    getClock(): PreviewClock;
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
  };
}
