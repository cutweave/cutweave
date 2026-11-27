// SPDX-License-Identifier: GPL-3.0-or-later
// The export dialog's settings (WP-2G.04, ADR-0018 F14, F34): a preset picker grouped by
// platform with the presets made for the sequence's shape first; a summary of what the file will
// be (size, rate, codecs, length, estimated size); the platform's warnings in words (never a
// block); what to export (the whole sequence, in to out, a custom range or one file per marker);
// where it goes and what to do when the file exists; and an Advanced section (size and rate, the
// quality, the hardware encoder and the encoder's own options, from `export.describe_options`).
// Export starts it now; Add to queue leaves it to the export queue. Every refusal the engine
// names a field for shows at that field. Missing media (WP-2F.03c) shows as one line with
// Relink… and Export anyway. Save as preset (WP-2G.04b) keeps what the dialog shows now as a
// preset of the person's, under a name; a preset of theirs can be deleted, and the line that
// says so brings it back.

import type {
  CodecEncoders,
  Collision,
  ExportPresetEntry,
  ExportPresetSpec,
  ExportQuality,
} from "@cutweave/protocol";
import { formatTimecode } from "@cutweave/timeline";
import {
  Button,
  Dialog,
  DialogNote,
  Icon,
  IconButton,
  InspectorRow,
  type JsonSchema,
  SchemaForm,
  SegmentedControl,
  Switch,
  setPath,
  TextField,
  Tooltip,
  type UiHints,
} from "@cutweave/ui";
import { type ReactNode, useEffect, useId, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TKey } from "../i18n/index.ts";
import { openRelinkDialog } from "../media/RelinkDialog.tsx";
import { type Translate, translator } from "../timeline/errors.ts";
import type { ExportSource, ResolveOutcome, StartMode } from "./exportController.ts";
import {
  advancedPath,
  codecsText,
  collisionFor,
  containerText,
  DEFAULT_RANGE,
  EXISTS_CHOICES,
  type ExistsChoice,
  estimateSize,
  flicksToSeconds,
  fpsLabel,
  type GroupLabel,
  groupPresets,
  hasExtension,
  humanBytes,
  isTemplate,
  markerRegions,
  markerTemplate,
  type OfflineFacts,
  overridesFor,
  PRESET_SIZE,
  presetParam,
  presetQuality,
  pruneOptions,
  RANGE_KINDS,
  type RangeDraft,
  type RangeKind,
  rangeAvailability,
  rangeParam,
  rangeSpan,
  type SizeDraft,
  singleFile,
  sizeProblem,
  startingPreset,
  startingQuality,
  warningText,
  withExtension,
} from "./exportModel.ts";
import { baseName, encoderLabel, isAbsolutePath, QUALITIES } from "./exportSettings.ts";
import { type ExportStore, useExportStore } from "./exportStore.ts";
import { nameProblem, presetFromDraft, presetIdFor } from "./presetDraft.ts";
import { exportController } from "./useExport.ts";

const QUALITY_LABEL = {
  low: "export.low",
  medium: "export.medium",
  high: "export.high",
} as const satisfies Record<ExportQuality, TKey>;

const RANGE_LABEL = {
  sequence: "export.range.sequence",
  in_out: "export.range.inOut",
  custom: "export.range.custom",
  markers: "export.range.markers",
} as const satisfies Record<RangeKind, TKey>;

const EXISTS_LABEL = {
  ask: "export.exists.ask",
  rename: "export.exists.rename",
  overwrite: "export.exists.overwrite",
} as const satisfies Record<ExistsChoice, TKey>;

const SIZE_PROBLEM = {
  width: "export.errors.width",
  height: "export.errors.height",
  fps: "export.errors.fps",
} as const satisfies Record<"width" | "height" | "fps", TKey>;

const NAME_PROBLEM = {
  empty: "export.savePreset.empty",
  long: "export.savePreset.long",
} as const satisfies Record<"empty" | "long", TKey>;

/** Where a refusal shows: a field of the form, or the dialog's own line. */
type Spot = "path" | "range" | "size" | "options" | "general";

/** The field a JSON Pointer of the engine's names. */
function spotOf(pointer: string | null): Spot {
  if (pointer === null) return "general";
  if (
    pointer.startsWith("/path") ||
    pointer.startsWith("/overwrite") ||
    pointer.startsWith("/collision")
  ) {
    return "path";
  }
  if (pointer.startsWith("/range")) return "range";
  if (pointer.startsWith("/preset/custom")) return "size";
  if (pointer.startsWith("/advanced/")) return "options";
  return "general";
}

/** What to tell about the hardware encoder under its switch, for the preset's codec. */
function hardwareNote(
  encoders: ExportStore["encoders"],
  codec: string | null,
  on: boolean,
  t: Translate,
): string {
  if (!on) return t("export.hardwareOff");
  if (encoders.status === "loading") return t("export.hardwareChecking");
  if (encoders.status === "ready") {
    const info: CodecEncoders | undefined = encoders.value.find((c) => c.codec === codec);
    const names = info?.hardware.map((h) => encoderLabel(h.implementation)) ?? [];
    return names.length === 0
      ? t("export.hardwareNone")
      : t("export.hardwareFound", { names: [...new Set(names)].join(", ") });
  }
  return t("export.hardwareAuto");
}

/** Field names of the encoder options, in the person's language (the schema has none). */
function optionHints(t: Translate): Record<string, UiHints> {
  const either = (a: string, b: string) => `mode == '${a}' || mode == '${b}'`;
  return {
    video: { label: t("export.options.video") },
    "video.rate_control": { label: t("export.options.rateControl") },
    "video.rate_control.mode": { label: t("export.options.mode"), widget: "select" },
    "video.rate_control.crf": { label: t("export.options.crf"), visibleWhen: either("crf", "cq") },
    "video.rate_control.bitrate_kbps": {
      label: t("export.options.bitrate"),
      unit: "kb/s",
      step: 100,
      visibleWhen: either("cbr", "vbr"),
    },
    "video.rate_control.max_bitrate_kbps": {
      label: t("export.options.maxBitrate"),
      unit: "kb/s",
      step: 100,
      visibleWhen: either("vbr", "cq"),
    },
    "video.rate_control.buffer_kbit": {
      label: t("export.options.buffer"),
      unit: "kbit",
      step: 100,
      visibleWhen: "mode",
    },
    "video.gop": { label: t("export.options.gop") },
    "video.gop.keyint": { label: t("export.options.keyint") },
    "video.gop.keyint_seconds": { label: t("export.options.keyintSeconds"), unit: "s" },
    "video.gop.keyint_min": { label: t("export.options.keyintMin") },
    "video.gop.bframes": { label: t("export.options.bframes") },
    "video.gop.closed_gop": { label: t("export.options.closedGop") },
    "video.gop.scenecut": { label: t("export.options.scenecut") },
    "video.preset": { label: t("export.options.speed"), widget: "select" },
    "video.tune": { label: t("export.options.tune"), widget: "select" },
    "video.profile": { label: t("export.options.profile"), widget: "select" },
    "video.level": { label: t("export.options.level") },
    "video.pix_fmt": { label: t("export.options.pixFmt"), widget: "select" },
    "video.threads": { label: t("export.options.threads") },
    color: { label: t("export.options.color") },
    "color.primaries": { label: t("export.options.primaries"), widget: "select" },
    "color.transfer": { label: t("export.options.transfer"), widget: "select" },
    "color.matrix": { label: t("export.options.matrix"), widget: "select" },
    "color.range": { label: t("export.options.colorRange"), widget: "select" },
    "color.tag_only": { label: t("export.options.tagOnly") },
    audio: { label: t("export.options.audio") },
    "audio.codec": { label: t("export.options.audioCodec"), widget: "select" },
    "audio.bitrate_kbps": { label: t("export.options.audioBitrate"), unit: "kb/s" },
    "audio.sample_rate": { label: t("export.options.sampleRate"), widget: "select", unit: "Hz" },
    "audio.channels": { label: t("export.options.channels"), widget: "select" },
  };
}

/** Codec, rate-control and speed names as editors write them; other values keep their own. */
const OPTION_NAMES: Readonly<Record<string, string>> = {
  crf: "CRF",
  cbr: "CBR",
  vbr: "VBR",
  cq: "CQ",
  aac: "AAC",
  mp3: "MP3",
  opus: "Opus",
  flac: "FLAC",
  pcm_s16: "PCM 16-bit",
  pcm_s24: "PCM 24-bit",
  veryfast: "Very fast",
  veryslow: "Very slow",
};

/** The schema with readable choices: `enum`s become `oneOf` with titles (words translated). */
function labelChoices(schema: JsonSchema, t: Translate): JsonSchema {
  const words: Readonly<Record<string, string>> = {
    mono: t("export.options.mono"),
    stereo: t("export.options.stereo"),
    limited: t("export.options.limited"),
    full: t("export.options.full"),
  };
  const walk = (node: JsonSchema): JsonSchema => {
    const out: JsonSchema = { ...node };
    if (Array.isArray(node.enum)) {
      delete out.enum;
      out.oneOf = node.enum.map((value) => {
        const key = String(value);
        return { const: value, title: words[key] ?? OPTION_NAMES[key] ?? key };
      });
    }
    if (node.properties !== undefined) {
      out.properties = Object.fromEntries(
        Object.entries(node.properties).map(([key, child]) => [key, walk(child)]),
      );
    }
    return out;
  };
  return walk(schema);
}

/** Whether the schema has a field at this dot path (`video.rate_control.mode`). */
function schemaHas(schema: JsonSchema, path: string): boolean {
  let at: JsonSchema | undefined = schema;
  for (const key of path.split(".")) {
    at = at?.properties?.[key];
    if (at === undefined) return false;
  }
  return true;
}

function groupLabel(label: GroupLabel, t: Translate): string {
  return "name" in label ? label.name : t(label.key, label.vars);
}

/** A refusal line under a field (`role="alert"`). */
function FieldError({ children }: { readonly children: ReactNode }) {
  return (
    <p className="cw-note cw-export__error" role="alert">
      <Icon name="circle-alert" size="md" />
      <span>{children}</span>
    </p>
  );
}

/** The line the dialog shows for missing media: one file by name, several by number. */
export function offlineText(facts: OfflineFacts, t: Translate): string {
  return facts.count === 1 && facts.names.length === 1
    ? t("export.offline.one", { name: facts.names[0] ?? "" })
    : t("export.offline.many", { count: String(facts.count) });
}

/**
 * Media files the export plays are missing (WP-2F.03c): what that means for the file, and the
 * two ways on: find the files (the relink dialog) or export with the slate.
 */
function OfflineNotice({
  facts,
  busy,
  onRelink,
  onExportAnyway,
}: {
  readonly facts: OfflineFacts;
  readonly busy: boolean;
  readonly onRelink: () => void;
  readonly onExportAnyway: () => void;
}) {
  const { t: rawT } = useTranslation();
  const t = translator(rawT);
  return (
    <div className="cw-export__offline" role="alert" data-testid="export-offline">
      <p className="cw-note cw-export__error">
        <Icon name="circle-alert" size="md" />
        <span>{offlineText(facts, t)}</span>
      </p>
      <div className="cw-export__offline-actions">
        <Button variant="secondary" size="sm" onClick={onRelink}>
          {t("contextMenu.item.relink")}
        </Button>
        <Button variant="ghost" size="sm" disabled={busy} onClick={onExportAnyway}>
          {t("export.offline.anyway")}
        </Button>
      </div>
    </div>
  );
}

/** The settings form; the presets must be loaded (see {@link ExportForm}). */
function PresetForm({
  source,
  entries,
}: {
  readonly source: ExportSource;
  readonly entries: readonly ExportPresetEntry[];
}) {
  const { t: rawT } = useTranslation();
  const t = translator(rawT);
  const controller = exportController();
  const encoders = useExportStore((s) => s.encoders);
  const seq = source.sequence;
  const facts = source.facts;
  // Settings > Export decides where the form starts; it is read once, as the dialog opens.
  const [defaults] = useState(() => controller.formDefaults());
  const [presetId, setPresetId] = useState(() => startingPreset(entries, defaults.presets));
  const entry = entries.find((e) => e.preset.id === presetId) ?? entries[0];
  const spec = entry?.preset as ExportPresetSpec;
  const [size, setSize] = useState<SizeDraft>(PRESET_SIZE);
  const [quality, setQuality] = useState<ExportQuality | null>(() =>
    startingQuality(spec, defaults.quality),
  );
  const hardwareFor = (s: ExportPresetSpec) => defaults.hardware && s.video?.hw_preferred !== false;
  const [hardware, setHardware] = useState(() => hardwareFor(spec));
  const [range, setRange] = useState<RangeDraft>(DEFAULT_RANGE);
  const [path, setPath_] = useState(() => controller.defaultPath(spec.name, spec.container));
  const [pathTouched, setPathTouched] = useState(false);
  const [exists, setExists] = useState<ExistsChoice>("ask");
  const [options, setOptions] = useState<Record<string, unknown>>({});
  const [schema, setSchema] = useState<JsonSchema | null>(null);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [resolved, setResolved] = useState<ResolveOutcome | null>(null);
  const [stage, setStage] = useState<"form" | "replace">("form");
  const [pendingMode, setPendingMode] = useState<StartMode>("now");
  const [submitted, setSubmitted] = useState(false);
  const [starting, setStarting] = useState<StartMode | null>(null);
  const [refusal, setRefusal] = useState<{ message: string; pointer: string | null } | null>(null);
  // The engine refused because media files are missing: what was being started, to start it
  // again with the slate allowed.
  const [offline, setOffline] = useState<{
    readonly facts: OfflineFacts;
    readonly mode: StartMode;
    readonly collision: Collision | undefined;
  } | null>(null);
  const ids = {
    preset: useId(),
    path: useId(),
    exists: useId(),
    width: useId(),
    height: useId(),
    fps: useId(),
    hardware: useId(),
    start: useId(),
    end: useId(),
    advanced: useId(),
    presetName: useId(),
  };
  // Save as preset: the name being typed, and what the engine said when it refused.
  const [naming, setNaming] = useState<{
    readonly name: string;
    readonly busy: boolean;
    readonly tried: boolean;
    readonly refused: string | null;
  } | null>(null);
  // The preset just deleted: the dialog offers to bring it back until another one is chosen.
  const [deleted, setDeleted] = useState<ExportPresetSpec | null>(null);

  const groups = useMemo(() => groupPresets(entries, facts), [entries, facts]);
  const overrides = useMemo(() => overridesFor(spec, size, quality), [spec, size, quality]);
  const codec = spec.video?.codec ?? null;
  const hasVideo = codec !== null;

  // What the preset writes for this sequence and range (size, rate, estimate, warnings).
  const rangeKey = JSON.stringify(rangeParam(range));
  const overridesKey = JSON.stringify(sizeProblem(size) === null ? overrides : null);
  const span = rangeSpan(seq, range);
  const resolvable = span !== null;
  useEffect(() => {
    if (!resolvable) return;
    let live = true;
    const timer = setTimeout(() => {
      void controller
        .resolve(presetId, JSON.parse(rangeKey), JSON.parse(overridesKey))
        .then((outcome) => {
          if (live) setResolved(outcome);
        });
    }, 120);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [controller, presetId, rangeKey, overridesKey, resolvable]);

  // The encoder options of the preset's codec.
  useEffect(() => {
    setSchema(null);
    if (codec === null || codec === "gif") return;
    let live = true;
    void controller.describeOptions(codec, spec.container).then((found) => {
      if (live) setSchema(found);
    });
    return () => {
      live = false;
    };
  }, [controller, codec, spec.container]);

  // The encoder options as the form shows them: readable choices and field names.
  const formSchema = useMemo(
    () => (schema === null ? null : labelChoices(schema, translator(rawT))),
    [schema, rawT],
  );
  const hints = useMemo(() => optionHints(translator(rawT)), [rawT]);

  const plan = resolved?.ok === true ? resolved.value : null;
  const width = plan?.width ?? facts.width;
  const height = plan?.height ?? facts.height;
  const fps = plan?.frame_rate ?? facts.fps;
  const seconds = span === null ? 0 : flicksToSeconds(span.end - span.start);
  const qualityOverride = quality !== null && quality !== presetQuality(spec) ? quality : null;
  const estimate = estimateSize({
    spec,
    width,
    height,
    fps,
    seconds,
    quality: qualityOverride,
    resolvedKib: plan?.estimated_kib ?? null,
  });
  const warnings = (plan?.warnings ?? []).map((w) =>
    warningText(w, { spec, seq: facts, width, height, fps, seconds, estimate }),
  );
  const regions = range.kind === "markers" ? markerRegions(seq, range.pairMode) : [];
  // Why a kind of range cannot be chosen ("Set in and out points first").
  const unavailable = RANGE_KINDS.map((kind) => rangeAvailability(seq, kind)).filter(
    (key): key is TKey => key !== null,
  );
  // The estimate is for the first file of a marker set; all of them weigh their lengths together.
  const sizeScale =
    regions.length > 1 && seconds > 0
      ? regions.reduce((sum, r) => sum + flicksToSeconds(r.end - r.start), 0) / seconds
      : 1;

  // ---- checks before asking the engine ----
  const single = range.kind !== "markers";
  const trimmed = path.trim();
  const pathProblem: { key: TKey; vars?: Record<string, string> } | null =
    trimmed === ""
      ? { key: "export.errors.path" }
      : !isAbsolutePath(trimmed)
        ? { key: "export.errors.pathFull" }
        : single && !hasExtension(trimmed, spec.container)
          ? { key: "export.errors.pathExtension", vars: { ext: `.${spec.container}` } }
          : null;
  const rangeProblem: TKey | null =
    span === null
      ? range.kind === "custom"
        ? "export.errors.range"
        : range.kind === "markers"
          ? "export.errors.noRegions"
          : "export.errors.empty"
      : null;
  const formatProblem = sizeProblem(size);
  const problem: Spot | null =
    pathProblem !== null
      ? "path"
      : rangeProblem !== null
        ? "range"
        : formatProblem !== null
          ? "size"
          : null;
  const unsupported = entry !== undefined && !entry.supported;

  const refusedAt = refusal === null ? null : spotOf(refusal.pointer);
  const refusedOption = refusal === null ? null : advancedPath(refusal.pointer);
  const optionErrors = useMemo(
    () =>
      refusedOption === null || refusal === null ? undefined : { [refusedOption]: refusal.message },
    [refusal, refusedOption],
  );
  // A refusal of an encoder option shows at its field when that field is on screen.
  const optionShown =
    advancedOpen && schema !== null && refusedOption !== null && schemaHas(schema, refusedOption);

  // ---- changes ----
  const pathFor = (s: ExportPresetSpec, r: RangeDraft): string => {
    const proposed = controller.defaultPath(s.name, s.container);
    return r.kind === "markers" ? markerTemplate(proposed) : proposed;
  };
  /** Starts the form over at a preset: its own size, quality, hardware choice and options. */
  const applyPreset = (next: ExportPresetSpec): void => {
    setPresetId(next.id);
    setSize(PRESET_SIZE);
    setQuality(startingQuality(next, defaults.quality));
    setHardware(hardwareFor(next));
    setOptions({});
    setResolved(null);
    setRefusal(null);
    setPath_(pathTouched ? withExtension(path, next.container) : pathFor(next, range));
  };
  const choosePreset = (id: string): void => {
    const next = entries.find((e) => e.preset.id === id)?.preset;
    if (next === undefined) return;
    setDeleted(null);
    applyPreset(next);
  };

  // ---- presets of the person's ----
  const mine = entry?.source === "user";
  const typedName = naming?.name ?? "";
  const nameIssue = nameProblem(typedName);
  const target = presetIdFor(typedName, entries);
  const undoDelete = async (): Promise<void> => {
    if (deleted === null) return;
    const outcome = await controller.savePreset(deleted);
    if (outcome.ok) {
      setDeleted(null);
      applyPreset(deleted);
    } else {
      setRefusal({ message: outcome.message, pointer: null });
    }
  };
  const savePreset = async (): Promise<void> => {
    if (naming === null || naming.busy) return;
    if (nameIssue !== null || formatProblem !== null) {
      setNaming({ ...naming, tried: true });
      if (formatProblem !== null) {
        setSubmitted(true);
        setAdvancedOpen(true);
      }
      return;
    }
    const saved = presetFromDraft({
      base: spec,
      id: target.id,
      name: typedName,
      summary:
        spec.platform === null || spec.platform === undefined
          ? (spec.summary ?? t("export.savePreset.from", { preset: spec.name }))
          : t("export.savePreset.from", { preset: spec.name }),
      size,
      resolved: { width, height },
      quality,
      hardware,
      options,
    });
    setNaming({ ...naming, busy: true, tried: true, refused: null });
    const outcome = await controller.savePreset(saved);
    if (outcome.ok) {
      setNaming(null);
      applyPreset(saved);
    } else {
      setNaming({ ...naming, busy: false, tried: true, refused: outcome.message });
    }
  };
  const deletePreset = async (): Promise<void> => {
    if (!mine) return;
    const gone = spec;
    const outcome = await controller.deletePreset(gone);
    if (!outcome.ok) {
      setRefusal({ message: outcome.message, pointer: null });
      return;
    }
    setDeleted(gone);
    const left = entries.filter((e) => e.preset.id !== spec.id);
    const next = left.find((e) => e.preset.id === startingPreset(left, []))?.preset;
    if (next !== undefined) applyPreset(next);
  };
  const chooseRange = (kind: RangeKind): void => {
    const next = { ...range, kind };
    setRange(next);
    setRefusal(null);
    if (kind === "markers" && range.kind !== "markers") setPath_(markerTemplate(path));
    if (kind !== "markers" && range.kind === "markers") setPath_(singleFile(path));
  };
  const typePath = (value: string): void => {
    setPath_(value);
    setPathTouched(true);
    if (refusedAt === "path") setRefusal(null);
  };

  const run = async (
    mode: StartMode,
    collision?: Collision,
    allowOffline = false,
  ): Promise<void> => {
    setSubmitted(true);
    setRefusal(null);
    setOffline(null);
    if (problem !== null || unsupported) {
      setStage("form");
      if (problem === "size") setAdvancedOpen(true);
      return;
    }
    setStarting(mode);
    const outcome = await controller.start(
      {
        path: trimmed,
        preset: presetParam(presetId, overrides),
        presetId,
        hw: hasVideo && hardware ? "auto" : "off",
        collision: collision ?? (exists === "ask" ? undefined : collisionFor(exists)),
        range: rangeParam(range),
        advanced: pruneOptions(options),
        allowOffline,
      },
      mode,
    );
    setStarting(null);
    if (outcome.kind === "exists") {
      setPendingMode(mode);
      setStage("replace");
    } else if (outcome.kind === "offline") {
      setStage("form");
      setOffline({ facts: outcome, mode, collision });
    } else if (outcome.kind === "refused") {
      setStage("form");
      setRefusal({ message: outcome.message, pointer: outcome.pointer });
      const at = spotOf(outcome.pointer);
      if (at === "size" || at === "options") setAdvancedOpen(true);
    } else if (mode === "queue") {
      controller.close();
    }
    // "now", started: the dialog now shows the progress (the active export is set).
  };

  if (stage === "replace") {
    const name = baseName(trimmed);
    return (
      <Dialog
        open
        onOpenChange={(open) => {
          if (!open) controller.close();
        }}
        title={t("export.exists.title", { name })}
        description={t("export.exists.body")}
        footer={
          <>
            <Button variant="ghost" size="lg" onClick={() => setStage("form")}>
              {t("export.exists.back")}
            </Button>
            <Button
              variant="danger"
              size="lg"
              disabled={starting !== null}
              onClick={() => void run(pendingMode, "overwrite")}
            >
              {t("export.exists.replace")}
            </Button>
          </>
        }
      >
        {null}
      </Dialog>
    );
  }

  const shown = (spot: Spot): ReactNode => {
    if (submitted && problem === spot) {
      if (spot === "path" && pathProblem !== null) {
        return <FieldError>{t(pathProblem.key, pathProblem.vars)}</FieldError>;
      }
      if (spot === "range" && rangeProblem !== null)
        return <FieldError>{t(rangeProblem)}</FieldError>;
      if (spot === "size" && formatProblem !== null) {
        return <FieldError>{t(SIZE_PROBLEM[formatProblem])}</FieldError>;
      }
    }
    if (problem === null && refusal !== null && refusedAt === spot && spot !== "options") {
      return <FieldError>{t("export.errors.refused", { message: refusal.message })}</FieldError>;
    }
    return null;
  };

  const lengthText =
    span === null ? "—" : formatTimecode(span.end - span.start, seq.format.frame_rate);
  const sizeText =
    estimate === null
      ? t("export.summary.noEstimate")
      : t(estimate.upTo ? "export.summary.upTo" : "export.summary.about", {
          size: humanBytes(estimate.bytes * sizeScale),
        });
  const rateText =
    spec.video?.rate_control.mode === "vbr" || spec.video?.rate_control.mode === "cbr"
      ? t("export.bitrateValue", { mbps: String(spec.video.rate_control.bitrate_kbps / 1000) })
      : null;

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) controller.close();
      }}
      onSubmit={() => void run("now")}
      title={t("export.title")}
      description={t("export.description")}
      className="cw-dialog--export"
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={() => controller.close()}>
            {t("export.cancel")}
          </Button>
          <Button
            variant="secondary"
            size="lg"
            disabled={starting !== null || unsupported}
            onClick={() => void run("queue")}
          >
            {starting === "queue" ? t("export.starting") : t("export.addToQueue")}
          </Button>
          <Button
            variant="primary"
            size="lg"
            disabled={starting !== null || unsupported}
            onClick={() => void run("now")}
          >
            {starting === "now" ? t("export.starting") : t("export.start")}
          </Button>
        </>
      }
    >
      <div className="cw-export__body" data-testid="export-form">
        <InspectorRow label={t("export.preset")} htmlFor={ids.preset}>
          <select
            id={ids.preset}
            className="cw-field body cw-export__preset"
            value={presetId}
            onChange={(event) => choosePreset(event.target.value)}
          >
            {groups.map((group) => (
              <optgroup key={group.id} label={groupLabel(group.label, t)}>
                {group.options.map((o) => (
                  <option key={o.id} value={o.id} disabled={!o.supported}>
                    {o.supported ? o.name : t("export.presets.unavailable", { name: o.name })}
                  </option>
                ))}
              </optgroup>
            ))}
          </select>
          <Tooltip label={t("export.savePreset.open")}>
            <IconButton
              icon="bookmark"
              label={t("export.savePreset.open")}
              pressed={naming !== null}
              disabled={unsupported}
              onClick={() =>
                setNaming(
                  naming === null ? { name: "", busy: false, tried: false, refused: null } : null,
                )
              }
            />
          </Tooltip>
          {mine ? (
            <Tooltip label={t("export.deletePreset.label")}>
              <IconButton
                icon="trash-2"
                label={t("export.deletePreset.label")}
                onClick={() => void deletePreset()}
              />
            </Tooltip>
          ) : null}
        </InspectorRow>
        {deleted !== null ? (
          <div className="cw-export__options-head" data-testid="export-preset-deleted">
            <span className="caption cw-export__hint" role="status">
              {t("export.deletePreset.deleted", { name: deleted.name })}
            </span>
            <Button variant="ghost" size="sm" onClick={() => void undoDelete()}>
              {t("export.deletePreset.undo")}
            </Button>
          </div>
        ) : null}
        {naming !== null ? (
          <>
            <InspectorRow label={t("export.savePreset.name")} htmlFor={ids.presetName}>
              <TextField
                id={ids.presetName}
                className="cw-export__path"
                autoFocus
                value={naming.name}
                maxLength={120}
                invalid={naming.tried && (nameIssue !== null || naming.refused !== null)}
                onChange={(name) => setNaming({ ...naming, name, refused: null })}
                onKeyDown={(event) => {
                  // Enter saves the preset; it does not start the export (the dialog's Enter).
                  if (event.key !== "Enter" || event.nativeEvent.isComposing) return;
                  event.preventDefault();
                  void savePreset();
                }}
              />
              <Tooltip label={t("export.savePreset.save")}>
                <IconButton
                  icon="check"
                  label={t("export.savePreset.save")}
                  disabled={naming.busy}
                  onClick={() => void savePreset()}
                />
              </Tooltip>
              <Tooltip label={t("export.savePreset.cancel")}>
                <IconButton
                  icon="x"
                  label={t("export.savePreset.cancel")}
                  onClick={() => setNaming(null)}
                />
              </Tooltip>
            </InspectorRow>
            {target.replaces !== null && nameIssue === null ? (
              <p className="caption cw-export__hint">
                {t("export.savePreset.replaces", { name: target.replaces.name })}
              </p>
            ) : null}
            {naming.tried && nameIssue !== null ? (
              <FieldError>{t(NAME_PROBLEM[nameIssue])}</FieldError>
            ) : naming.refused !== null ? (
              <FieldError>{t("export.savePreset.refused", { message: naming.refused })}</FieldError>
            ) : null}
          </>
        ) : null}
        {spec.summary !== undefined && spec.summary !== "" ? (
          <p className="caption cw-export__hint">{spec.summary}</p>
        ) : null}
        {unsupported ? (
          <DialogNote icon="triangle-alert">
            {t("export.errors.unsupported", { reason: entry?.unsupported_reason ?? "" })}
          </DialogNote>
        ) : null}

        <section
          className="cw-export__summary"
          aria-label={t("export.summary.label")}
          data-testid="export-summary"
        >
          <span className="body-strong cw-export__specs">
            {[
              hasVideo
                ? t("export.summary.size", { width: String(width), height: String(height) })
                : null,
              hasVideo ? t("export.summary.fps", { fps: fpsLabel(fps) }) : null,
              codecsText(spec),
              containerText(spec.container),
            ]
              .filter((part): part is string => part !== null && part !== "")
              .join(" · ")}
          </span>
          <span className="caption cw-export__length">
            {[
              t("export.summary.length", { length: lengthText }),
              regions.length > 1 ? t("export.files", { count: String(regions.length) }) : null,
              sizeText,
            ]
              .filter((part): part is string => part !== null)
              .join(" · ")}
          </span>
        </section>
        {warnings.length > 0 ? (
          <ul className="cw-export__warnings" aria-label={t("export.warnings.label")}>
            {warnings.map((w) => (
              <li key={w.code} className="cw-note cw-export__warning" data-code={w.code}>
                <Icon name="triangle-alert" size="md" />
                <span>{w.key === null ? w.message : t(w.key, w.vars)}</span>
              </li>
            ))}
          </ul>
        ) : null}
        {resolved?.ok === false && spotOf(resolved.pointer) === "general" ? (
          <DialogNote icon="triangle-alert">{resolved.message}</DialogNote>
        ) : null}

        <InspectorRow label={t("export.range.label")}>
          <SegmentedControl
            aria-label={t("export.range.label")}
            value={range.kind}
            onChange={chooseRange}
            options={RANGE_KINDS.map((kind) => ({
              value: kind,
              label: t(RANGE_LABEL[kind]),
              disabled: rangeAvailability(seq, kind) !== null,
            }))}
          />
        </InspectorRow>
        {range.kind === "custom" ? (
          <div className="cw-export__pair">
            <InspectorRow label={t("export.range.start")} htmlFor={ids.start}>
              <TextField
                id={ids.start}
                numeric
                className="cw-export__time"
                value={range.start}
                placeholder="00:00:00:00"
                onChange={(start) => setRange({ ...range, start })}
                invalid={submitted && rangeProblem !== null}
              />
            </InspectorRow>
            <InspectorRow label={t("export.range.end")} htmlFor={ids.end}>
              <TextField
                id={ids.end}
                numeric
                className="cw-export__time"
                value={range.end}
                placeholder={formatTimecode(span?.end ?? 0, seq.format.frame_rate)}
                onChange={(end) => setRange({ ...range, end })}
                invalid={submitted && rangeProblem !== null}
              />
            </InspectorRow>
          </div>
        ) : null}
        {range.kind === "markers" ? (
          <>
            <InspectorRow label={t("export.range.pairs")}>
              <SegmentedControl
                aria-label={t("export.range.pairs")}
                value={range.pairMode}
                onChange={(pairMode) => setRange({ ...range, pairMode })}
                options={[
                  { value: "duration", label: t("export.range.duration") },
                  { value: "pairs", label: t("export.range.toNext") },
                ]}
              />
            </InspectorRow>
            <DialogNote>
              {regions.length === 0
                ? t("export.range.makesNone")
                : regions.length === 1
                  ? t("export.range.makesOne")
                  : t("export.range.makesMany", { count: String(regions.length) })}
            </DialogNote>
          </>
        ) : null}
        {unavailable.length > 0 ? (
          <p className="caption cw-export__hint">{unavailable.map((key) => t(key)).join(" ")}</p>
        ) : null}
        {resolved?.ok === false && spotOf(resolved.pointer) === "range" ? (
          <DialogNote icon="triangle-alert">{resolved.message}</DialogNote>
        ) : null}
        {shown("range")}

        <InspectorRow label={t("export.output")} htmlFor={ids.path}>
          <TextField
            id={ids.path}
            className="cw-export__path"
            value={path}
            onChange={typePath}
            placeholder={t("export.outputPlaceholder")}
            invalid={(submitted && pathProblem !== null) || refusedAt === "path"}
          />
          <Button
            variant="secondary"
            onClick={() => {
              void controller
                .chooseFile(single ? trimmed : singleFile(trimmed), spec.container)
                .then((picked) => {
                  if (picked !== null) typePath(single ? picked : markerTemplate(picked));
                });
            }}
          >
            {t("export.choose")}
          </Button>
        </InspectorRow>
        {!single ? (
          <p className="caption cw-export__hint">
            {isTemplate(trimmed) ? t("export.range.template") : t("export.range.folder")}
          </p>
        ) : null}
        {shown("path")}
        <InspectorRow label={t("export.exists.label")} htmlFor={ids.exists}>
          <select
            id={ids.exists}
            className="cw-field body"
            value={exists}
            onChange={(event) => setExists(event.target.value as ExistsChoice)}
          >
            {EXISTS_CHOICES.map((choice) => (
              <option key={choice} value={choice}>
                {t(EXISTS_LABEL[choice])}
              </option>
            ))}
          </select>
        </InspectorRow>

        <Button
          variant="ghost"
          icon={advancedOpen ? "chevron-down" : "chevron-right"}
          className="cw-export__disclosure"
          aria-expanded={advancedOpen}
          aria-controls={ids.advanced}
          onClick={() => setAdvancedOpen(!advancedOpen)}
        >
          {t("export.advanced")}
        </Button>
        {advancedOpen ? (
          <div id={ids.advanced} className="cw-export__advanced">
            {hasVideo ? (
              <>
                <InspectorRow label={t("export.width")} htmlFor={ids.width}>
                  <TextField
                    id={ids.width}
                    numeric
                    inputMode="numeric"
                    className="cw-export__num"
                    value={size.width ?? String(width)}
                    onChange={(value) => setSize({ ...size, width: value })}
                    invalid={submitted && formatProblem === "width"}
                  />
                </InspectorRow>
                <InspectorRow label={t("export.height")} htmlFor={ids.height}>
                  <TextField
                    id={ids.height}
                    numeric
                    inputMode="numeric"
                    className="cw-export__num"
                    value={size.height ?? String(height)}
                    onChange={(value) => setSize({ ...size, height: value })}
                    invalid={submitted && formatProblem === "height"}
                  />
                </InspectorRow>
                <InspectorRow label={t("export.fps")} htmlFor={ids.fps}>
                  <TextField
                    id={ids.fps}
                    numeric
                    inputMode="decimal"
                    className="cw-export__num"
                    value={size.fps ?? fpsLabel(fps)}
                    onChange={(value) => setSize({ ...size, fps: value })}
                    invalid={submitted && formatProblem === "fps"}
                  />
                </InspectorRow>
                {shown("size")}
                {quality !== null ? (
                  <InspectorRow label={t("export.quality")}>
                    <SegmentedControl
                      aria-label={t("export.qualityLabel")}
                      value={quality}
                      onChange={setQuality}
                      options={QUALITIES.map((value) => ({
                        value,
                        label: t(QUALITY_LABEL[value]),
                      }))}
                    />
                  </InspectorRow>
                ) : rateText !== null ? (
                  <InspectorRow label={t("export.bitrate")}>
                    <span className="body cw-mono numeric">{rateText}</span>
                  </InspectorRow>
                ) : null}
                <InspectorRow label={t("export.hardware")} htmlFor={ids.hardware}>
                  <Switch id={ids.hardware} checked={hardware} onCheckedChange={setHardware} />
                </InspectorRow>
                <DialogNote icon="cpu">{hardwareNote(encoders, codec, hardware, t)}</DialogNote>
              </>
            ) : null}
            {schema !== null ? (
              <div className="cw-export__options">
                <div className="cw-export__options-head">
                  <span className="overline">{t("export.options.title")}</span>
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={pruneOptions(options) === null}
                    onClick={() => setOptions({})}
                  >
                    {t("export.options.reset")}
                  </Button>
                </div>
                <SchemaForm
                  aria-label={t("export.options.title")}
                  schema={formSchema ?? schema}
                  value={options}
                  hints={hints}
                  errors={optionErrors}
                  onChange={(at, value) => {
                    setOptions((current) => setPath(current, at, value) as Record<string, unknown>);
                    if (refusedAt === "options") setRefusal(null);
                  }}
                />
              </div>
            ) : null}
          </div>
        ) : null}
        {offline !== null ? (
          <OfflineNotice
            facts={offline.facts}
            busy={starting !== null}
            onRelink={() => {
              controller.close();
              openRelinkDialog();
            }}
            onExportAnyway={() => void run(offline.mode, offline.collision, true)}
          />
        ) : null}
        {problem === null &&
        refusal !== null &&
        !optionShown &&
        (refusedAt === "general" || refusedAt === "options") ? (
          <FieldError>{t("export.errors.start", { message: refusal.message })}</FieldError>
        ) : null}
      </div>
    </Dialog>
  );
}

/** The export dialog's settings: waits for the engine's presets, then the form. */
export function ExportForm({ source }: { readonly source: ExportSource }) {
  const { t } = useTranslation();
  const controller = exportController();
  const presets = useExportStore((s) => s.presets);
  if (presets.status === "ready" && presets.value.length > 0) {
    return <PresetForm source={source} entries={presets.value} />;
  }
  const failed = presets.status === "unavailable" || presets.status === "ready";
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) controller.close();
      }}
      title={t("export.title")}
      description={t("export.description")}
      footer={
        <Button variant="ghost" size="lg" onClick={() => controller.close()}>
          {t("export.cancel")}
        </Button>
      }
    >
      {failed ? (
        <p className="cw-note" role="alert">
          {t("export.errors.presets")}
        </p>
      ) : (
        <DialogNote icon="loader-circle">{t("export.loading")}</DialogNote>
      )}
    </Dialog>
  );
}
