// SPDX-License-Identifier: GPL-3.0-or-later
// The engine's media HTTP endpoints (WP-1I.03, docs/reference/http-media.md) as the editor uses
// them: `get` for `fetch` callers (the timeline's filmstrip and waveform loaders), with the UI
// token in the `Authorization` header, and `sign` for `<img src>` (the media bin's cards), which
// cannot send headers: the engine signs exact URLs with a per-launch key, so the token never
// appears in a URL (desktop `shared/api.ts`).

/** The engine's HTTP origin and UI token (`window.cutweave.engine.getConnection()`). */
export interface EngineHttp {
  readonly httpUrl: string;
  readonly token: string;
}

export interface MediaHttpOptions {
  /** The current engine; `null` while there is none. Asked before every request. */
  readonly connection: () => Promise<EngineHttp | null>;
  readonly fetch?: typeof fetch;
}

/** A failed media request: the HTTP status (0 without an answer) and the engine's error code. */
export class MediaHttpError extends Error {
  readonly status: number;
  readonly code: string | null;
  constructor(status: number, code: string | null, message: string) {
    super(message);
    this.name = "MediaHttpError";
    this.status = status;
    this.code = code;
  }
}

/** URLs signed by `POST /media/sign`: absolute, and valid until `expires` (Unix ms). */
export interface SignedUrls {
  readonly urls: readonly string[];
  readonly expires: number;
}

const trimSlash = (url: string) => url.replace(/\/+$/, "");

/** `?name=value&…` in the order given, skipping `undefined` values. */
export function mediaQuery(params: Readonly<Record<string, string | number | undefined>>): string {
  const q = new URLSearchParams();
  for (const [name, value] of Object.entries(params)) {
    if (value !== undefined) q.set(name, String(value));
  }
  const text = q.toString();
  return text === "" ? "" : `?${text}`;
}

/** `/media/{id}/{route}?…` with the id escaped. */
export function mediaPath(
  id: string,
  route: "thumb" | "filmstrip" | "waveform",
  params: Readonly<Record<string, string | number | undefined>>,
): string {
  return `/media/${encodeURIComponent(id)}/${route}${mediaQuery(params)}`;
}

export class MediaHttp {
  readonly #options: MediaHttpOptions;
  readonly #fetch: typeof fetch;

  constructor(options: MediaHttpOptions) {
    this.#options = options;
    this.#fetch = options.fetch ?? ((input, init) => globalThis.fetch(input, init));
  }

  async #engine(): Promise<EngineHttp> {
    const engine = await this.#options.connection();
    if (engine === null) throw new MediaHttpError(0, null, "the engine is not connected");
    return engine;
  }

  /** `GET path` (`/media/...`) with the token in a header; rejects unless the answer is 2xx. */
  async get(path: string, signal?: AbortSignal): Promise<Response> {
    const engine = await this.#engine();
    const response = await this.#fetch(`${trimSlash(engine.httpUrl)}${path}`, {
      headers: { Authorization: `Bearer ${engine.token}` },
      ...(signal && { signal }),
    });
    if (!response.ok) throw await failure(response, path);
    return response;
  }

  /** Signs `paths` (`/media/...?...`) for `ttlS` seconds (engine default: an hour). */
  async sign(paths: readonly string[], ttlS?: number): Promise<SignedUrls> {
    const engine = await this.#engine();
    const base = trimSlash(engine.httpUrl);
    const response = await this.#fetch(`${base}/media/sign`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${engine.token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify(ttlS === undefined ? { paths } : { paths, ttl_s: ttlS }),
    });
    if (!response.ok) throw await failure(response, "/media/sign");
    const body = (await response.json()) as { urls?: unknown; expires?: unknown };
    if (!Array.isArray(body.urls) || typeof body.expires !== "number") {
      throw new MediaHttpError(response.status, null, "unexpected answer from /media/sign");
    }
    return {
      urls: body.urls.map((url) => `${base}${String(url)}`),
      expires: body.expires * 1000,
    };
  }
}

async function failure(response: Response, path: string): Promise<MediaHttpError> {
  let code: string | null = null;
  let message = `HTTP ${response.status} from ${path.split("?")[0] ?? path}`;
  try {
    const body = (await response.json()) as { error?: { code?: unknown; message?: unknown } };
    if (typeof body.error?.code === "string") code = body.error.code;
    if (typeof body.error?.message === "string") message = `${message}: ${body.error.message}`;
  } catch {
    // Not JSON: the status says enough.
  }
  return new MediaHttpError(response.status, code, message);
}
