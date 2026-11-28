// SPDX-License-Identifier: GPL-3.0-or-later
// Sequence settings… (WP-2B.03): one sequence's canvas size, how its clips follow a new size,
// its frame rate and its background (the color, a blurred copy of the picture, a still image
// from the media bin with how it is fitted, or a transparent canvas). The form is the
// Inspector's (`SequenceForm`), so every change is one `sequence.set_format` or
// `sequence.set_background` and one undo step, and since it shows the document, a change an
// agent makes (or an undo) shows at once. Never taller than 60% of the window.
import type { MediaAsset, MediaId } from "@cutweave/protocol";
import { Button, Dialog, DialogNote } from "@cutweave/ui";
import { useEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { engineStore } from "../engine/engineStore.ts";
import { InspectorEditor } from "../inspector/editor.ts";
import { useDocStore } from "../state/docStore.ts";
import { mediaName } from "../state/selectors.ts";
import { toastStore } from "../state/toastStore.ts";
import { translator } from "../timeline/errors.ts";
import { type BackgroundImage, SequenceForm } from "./SequenceForm.tsx";
import { sequenceUiStore, useSequenceUi } from "./uiStore.ts";

/** The project's still images by name: what a background image can be. */
export function backgroundImages(
  media: Readonly<Partial<Record<MediaId, MediaAsset>>> | undefined,
): readonly BackgroundImage[] {
  return Object.values(media ?? {})
    .filter((asset): asset is MediaAsset => asset !== undefined && asset.kind === "image")
    .map((asset) => ({ id: asset.id, name: mediaName(asset, asset.id) }))
    .sort((a, b) => a.name.localeCompare(b.name) || (a.id < b.id ? -1 : 1));
}

export function SequenceSettingsDialog() {
  const { t } = useTranslation();
  const id = useSequenceUi((s) => s.settings);
  const sequence = useDocStore((s) => (id === null ? undefined : s.project?.sequences[id]));
  const media = useDocStore((s) => (id === null ? undefined : s.project?.media));
  const images = useMemo(() => backgroundImages(media), [media]);
  const latest = useRef(translator(t));
  latest.current = translator(t);
  const editor = useMemo(
    () =>
      new InspectorEditor({
        engine: engineStore,
        toasts: toastStore,
        t: (key, vars) => latest.current(key, vars),
      }),
    [],
  );
  const close = () => sequenceUiStore.getState().closeSettings();
  // The sequence was deleted (by an agent, or an undo of its creation): nothing to show.
  const gone = id !== null && sequence === undefined;
  useEffect(() => {
    if (gone) sequenceUiStore.getState().closeSettings();
  }, [gone]);
  if (id === null || sequence === undefined) return null;
  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next) close();
      }}
      title={t("sequences.settingsDialog.title")}
      description={t("sequences.settingsDialog.hint", { name: sequence.name })}
      className="cw-sequence-settings"
      footer={
        <Button variant="primary" size="lg" onClick={close}>
          {t("sequences.settingsDialog.done")}
        </Button>
      }
    >
      <div data-testid="sequence-settings">
        <SequenceForm
          key={id}
          sequence={id}
          format={sequence.format}
          editor={editor}
          images={images}
          label={t("sequences.settingsDialog.formLabel")}
        />
      </div>
      {images.length === 0 ? <DialogNote>{t("sequences.settingsDialog.noImages")}</DialogNote> : null}
    </Dialog>
  );
}
