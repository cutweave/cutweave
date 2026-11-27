// SPDX-License-Identifier: GPL-3.0-or-later
// The "This computer" pages of WP-1D.08: Cache and storage, and Performance and hardware. The
// values are machine preferences (stored in the `machine` group, never exported); the engine
// applies them after every handshake and on change (`engine/runtime.ts`), and main passes the
// cache folder as `--cache-dir` at the next start. Usage, the GPU and the hardware encoders are
// read from the engine while a page is open (`cacheStatus.ts`).

import {
  FRAME_MEMORY_CHOICES,
  PROXY_JOB_CHOICES,
  PROXY_LIMITS,
  type PreferenceKey,
  THUMBNAIL_LIMITS,
  WAVEFORM_LIMITS,
} from "@cutweave/preferences";
import type { CacheKind } from "@cutweave/protocol";
import { Button, TextField } from "@cutweave/ui";
import { engineStore, useEngineReady } from "../engine/engineStore.ts";
import { refreshProxies } from "../media/proxies.ts";
import {
  clearCaches,
  formatBytes,
  refreshCacheStatus,
  useCacheStatus,
  useCapabilities,
} from "./cacheStatus.ts";
import {
  type ChoiceOption,
  registerSettingsPage,
  type SettingsContext,
  type SettingsPageDef,
} from "./registry.ts";

const bytesOption = (bytes: number): ChoiceOption => ({ value: bytes, text: formatBytes(bytes) });

const thumbnailOptions = THUMBNAIL_LIMITS.map(bytesOption);
const waveformOptions = WAVEFORM_LIMITS.map(bytesOption);
const proxyOptions = PROXY_LIMITS.map(bytesOption);
// Counts are numbers: nothing to translate.
const proxyJobOptions: readonly ChoiceOption[] = PROXY_JOB_CHOICES.map((value) => ({
  value,
  text: String(value),
}));

/** What each cache is called, and its "Clear" button. */
const KIND_NAME = {
  thumbnails: "settings.cache.thumbnails",
  waveforms: "settings.cache.waveforms",
  proxies: "settings.cache.proxies",
} as const;
const KIND_CLEAR = {
  thumbnails: "settings.cache.clearThumbnails",
  waveforms: "settings.cache.clearWaveforms",
  proxies: "settings.cache.clearProxies",
} as const;

/** The bin's proxy badges follow a cache change made here (a lower limit, "Clear proxies"). */
const proxiesChanged = (kind: CacheKind): void => {
  if (kind === "proxies") void refreshProxies({ engine: engineStore });
};
const frameMemoryOptions: readonly ChoiceOption[] = FRAME_MEMORY_CHOICES.map((bytes) =>
  bytes === 0 ? { value: 0, label: "settings.rows.frameMemory.auto" } : bytesOption(bytes),
);

interface KindControlProps {
  kind: CacheKind;
  prefKey: PreferenceKey;
  options: readonly ChoiceOption[];
  controlId: string;
  ctx: SettingsContext;
}

/** One cache kind's row: what it holds, its limit (a preference) and its "Clear" button. */
function CacheKindControl({ kind, prefKey, options, controlId, ctx }: KindControlProps) {
  const status = useCacheStatus();
  const engineReady = useEngineReady();
  const { t } = ctx;
  const used = status?.kinds.find((k) => k.kind === kind)?.bytes;
  const limit = ctx.values[prefKey];
  const usage =
    used === undefined
      ? t("settings.cache.reading")
      : used === 0
        ? t("settings.cache.empty")
        : t("settings.cache.inUse", { used: formatBytes(used) });
  const name = t(KIND_NAME[kind]);
  return (
    <>
      <span className="caption" data-testid={`cache-usage-${kind}`} data-bytes={used ?? ""}>
        {usage}
      </span>
      <select
        id={controlId}
        className="cw-field body"
        aria-label={t("settings.cache.limitOf", { name })}
        value={String(limit)}
        disabled={!engineReady}
        onChange={(event) => {
          const option = options.find((o) => String(o.value) === event.target.value);
          if (option === undefined) return;
          void ctx.client
            .set(prefKey, option.value as never)
            .then(() =>
              window.setTimeout(() => {
                void refreshCacheStatus();
                proxiesChanged(kind);
              }, 300),
            )
            .catch(() => undefined);
        }}
      >
        {options.map((o) => (
          <option key={String(o.value)} value={String(o.value)}>
            {o.label !== undefined ? t(o.label) : (o.text ?? String(o.value))}
          </option>
        ))}
      </select>
      <Button
        size="sm"
        disabled={!engineReady || used === undefined || used === 0}
        onClick={() => {
          void clearCaches([kind]).then((done) => {
            proxiesChanged(kind);
            ctx.say(
              done
                ? t("settings.cache.cleared", { name })
                : t("settings.cache.clearFailed", { name }),
            );
          });
        }}
      >
        {t(KIND_CLEAR[kind])}
      </Button>
    </>
  );
}

/**
 * The cache folder: the chosen one, or (while none is chosen) the folder the engine is using,
 * and "Choose…" for the OS folder picker. A new choice applies the next time Cutweave starts.
 */
function CacheFolderControl({ ctx, controlId }: { ctx: SettingsContext; controlId: string }) {
  const status = useCacheStatus();
  const { t } = ctx;
  const chosen = ctx.values["cache.folder"];
  const inUse = status?.folder ?? undefined;
  const shown = chosen !== "" ? chosen : inUse === undefined ? "" : inUse;
  const picker = ctx.desktop?.dialog;
  return (
    <>
      <TextField
        id={controlId}
        aria-label={t("settings.rows.cacheFolder.label")}
        value={shown}
        readOnly
        title={shown}
        onChange={() => undefined}
      />
      <Button
        size="sm"
        disabled={picker === undefined}
        onClick={() => {
          void picker?.openProjectFolder().then((folder) => {
            if (folder !== null) void ctx.client.set("cache.folder", folder).catch(() => undefined);
          });
        }}
      >
        {t("settings.rows.cacheFolder.choose")}
      </Button>
    </>
  );
}

async function clearAll(ctx: SettingsContext): Promise<void> {
  const yes = await ctx.confirm({
    title: "settings.rows.clearCaches.confirmTitle",
    message: "settings.rows.clearCaches.confirmMessage",
    confirm: "settings.rows.clearCaches.confirm",
    danger: true,
  });
  if (!yes) return;
  const done = await clearCaches(["thumbnails", "waveforms"]);
  ctx.say(ctx.t(done ? "settings.cache.clearedAll" : "settings.cache.clearAllFailed"));
}

const cache: SettingsPageDef = {
  id: "cache",
  title: "settings.pages.cache",
  icon: "folder",
  group: "machine",
  order: 10,
  rows: [
    {
      id: "cache.folder",
      key: "cache.folder",
      label: "settings.rows.cacheFolder.label",
      description: "settings.rows.cacheFolder.description",
      keywords: "settings.rows.cacheFolder.keywords",
      control: {
        kind: "custom",
        render: (ctx, controlId) => <CacheFolderControl ctx={ctx} controlId={controlId} />,
      },
      applies: "restart",
    },
    {
      id: "cache.thumbnails_limit",
      key: "cache.thumbnails_limit",
      label: "settings.rows.thumbnailsLimit.label",
      description: "settings.rows.thumbnailsLimit.description",
      keywords: "settings.rows.thumbnailsLimit.keywords",
      control: {
        kind: "custom",
        render: (ctx, controlId) => (
          <CacheKindControl
            kind="thumbnails"
            prefKey="cache.thumbnails_limit"
            options={thumbnailOptions}
            controlId={controlId}
            ctx={ctx}
          />
        ),
      },
      applies: "engine",
    },
    {
      id: "cache.waveforms_limit",
      key: "cache.waveforms_limit",
      label: "settings.rows.waveformsLimit.label",
      description: "settings.rows.waveformsLimit.description",
      keywords: "settings.rows.waveformsLimit.keywords",
      control: {
        kind: "custom",
        render: (ctx, controlId) => (
          <CacheKindControl
            kind="waveforms"
            prefKey="cache.waveforms_limit"
            options={waveformOptions}
            controlId={controlId}
            ctx={ctx}
          />
        ),
      },
      applies: "engine",
    },
    {
      id: "cache.proxies_limit",
      key: "cache.proxies_limit",
      label: "settings.rows.proxiesLimit.label",
      description: "settings.rows.proxiesLimit.description",
      keywords: "settings.rows.proxiesLimit.keywords",
      control: {
        kind: "custom",
        render: (ctx, controlId) => (
          <CacheKindControl
            kind="proxies"
            prefKey="cache.proxies_limit"
            options={proxyOptions}
            controlId={controlId}
            ctx={ctx}
          />
        ),
      },
      applies: "engine",
    },
    {
      id: "cache.clear_all",
      label: "settings.rows.clearCaches.label",
      description: "settings.rows.clearCaches.description",
      keywords: "settings.rows.clearCaches.keywords",
      control: {
        kind: "action",
        label: "settings.rows.clearCaches.button",
        run: clearAll,
        danger: true,
      },
      applies: "engine",
    },
  ],
};

function GpuInfo({ ctx }: { ctx: SettingsContext }) {
  const caps = useCapabilities();
  const { t } = ctx;
  const gpu = caps?.gpu;
  const text =
    caps === null
      ? t("settings.performance.reading")
      : gpu === undefined || gpu === null
        ? t("settings.performance.noGpu")
        : gpu.software
          ? t("settings.performance.softwareGpu", { name: gpu.name })
          : `${gpu.name} (${gpu.backend})`;
  return (
    <span className="body" data-testid="gpu-name">
      {text}
    </span>
  );
}

function HardwareList({ ctx, which }: { ctx: SettingsContext; which: "encode" | "decode" }) {
  const caps = useCapabilities();
  const { t } = ctx;
  const list = caps === null ? null : which === "encode" ? caps.hw_encode : caps.hw_decode;
  const names = list === null ? [] : [...new Set(list.map((c) => c.implementation || c.device))];
  const text =
    list === null
      ? t("settings.performance.reading")
      : names.length === 0
        ? t("settings.performance.noneFound")
        : names.join(", ");
  return (
    <span className="body" data-testid={`hw-${which}`}>
      {text}
    </span>
  );
}

const performance: SettingsPageDef = {
  id: "performance",
  title: "settings.pages.performance",
  icon: "cpu",
  group: "machine",
  order: 20,
  rows: [
    {
      id: "performance.hw_decode",
      key: "performance.hw_decode",
      label: "settings.rows.hwDecode.label",
      description: "settings.rows.hwDecode.description",
      keywords: "settings.rows.hwDecode.keywords",
      control: {
        kind: "segmented",
        options: [
          { value: "auto", label: "settings.rows.hwDecode.auto" },
          { value: "off", label: "settings.rows.hwDecode.off" },
        ],
      },
      applies: "engine",
    },
    {
      id: "performance.frame_memory",
      key: "performance.frame_memory",
      label: "settings.rows.frameMemory.label",
      description: "settings.rows.frameMemory.description",
      keywords: "settings.rows.frameMemory.keywords",
      control: { kind: "select", options: frameMemoryOptions },
      applies: "engine",
    },
    {
      id: "performance.proxy_jobs",
      key: "performance.proxy_jobs",
      label: "settings.rows.proxyJobs.label",
      description: "settings.rows.proxyJobs.description",
      keywords: "settings.rows.proxyJobs.keywords",
      control: { kind: "select", options: proxyJobOptions },
      applies: "engine",
    },
    {
      id: "performance.gpu",
      label: "settings.rows.gpu.label",
      description: "settings.rows.gpu.description",
      keywords: "settings.rows.gpu.keywords",
      control: { kind: "custom", render: (ctx) => <GpuInfo ctx={ctx} /> },
      applies: "engine",
    },
    {
      id: "performance.hw_encoders",
      label: "settings.rows.hwEncoders.label",
      description: "settings.rows.hwEncoders.description",
      keywords: "settings.rows.hwEncoders.keywords",
      control: { kind: "custom", render: (ctx) => <HardwareList ctx={ctx} which="encode" /> },
      applies: "engine",
    },
    {
      id: "performance.hw_decoders",
      label: "settings.rows.hwDecoders.label",
      description: "settings.rows.hwDecoders.description",
      keywords: "settings.rows.hwDecoders.keywords",
      control: { kind: "custom", render: (ctx) => <HardwareList ctx={ctx} which="decode" /> },
      applies: "engine",
    },
  ],
};

export const MACHINE_PAGES: readonly SettingsPageDef[] = [cache, performance];

/** Registers the pages of this work package (idempotent). */
export function registerMachinePages(): void {
  for (const page of MACHINE_PAGES) registerSettingsPage(page);
}
registerMachinePages();
