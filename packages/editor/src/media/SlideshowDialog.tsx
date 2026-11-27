// SPDX-License-Identifier: GPL-3.0-or-later
// "Import as slideshow…" (WP-2F.05d, ADR-0035): how long each picture stays and in what order.
// Agreeing sends one `media.import_images_as_slideshow`: the pictures and their clips are one
// undo step.
import type { SlideshowOrder } from "@cutweave/protocol";
import { Button, Dialog, SegmentedControl, TextField } from "@cutweave/ui";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { engineStore } from "../engine/engineStore.ts";
import type { TKey } from "../i18n/index.ts";
import { menuDialogStore } from "../menus/dialogs.ts";
import { useDocStore } from "../state/docStore.ts";
import { selectActiveSequenceId } from "../state/selectors.ts";
import { toastStore } from "../state/toastStore.ts";
import { translator } from "../timeline/errors.ts";
import { importSlideshow, parseSeconds, proposedSeconds, SLIDESHOW_ORDERS } from "./slideshow.ts";

const ORDER_LABEL: Readonly<Record<SlideshowOrder, TKey>> = {
  given: "mediaBin.slideshow.orderGiven",
  name: "mediaBin.slideshow.orderName",
  name_numeric: "mediaBin.slideshow.orderNumber",
};

export function SlideshowDialog({ files }: { readonly files: readonly string[] }) {
  const { t } = useTranslation();
  const project = useDocStore((s) => s.project);
  const [seconds, setSeconds] = useState(() =>
    proposedSeconds(project?.settings?.still_duration ?? null),
  );
  const [order, setOrder] = useState<SlideshowOrder>("given");
  const length = parseSeconds(seconds);
  const close = () => menuDialogStore.getState().close();
  const submit = () => {
    if (length === null) return;
    close();
    void importSlideshow(
      { engine: engineStore, toasts: toastStore, t: translator(t) },
      { files, seconds: length, order, sequence: selectActiveSequenceId(project) },
    );
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
      title={t("mediaBin.slideshow.title")}
      description={t("mediaBin.slideshow.body", { count: files.length })}
      onSubmit={submit}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={close}>
            {t("contextMenu.dialog.cancel")}
          </Button>
          <Button variant="primary" size="lg" disabled={length === null} onClick={submit}>
            {t("mediaBin.slideshow.confirm")}
          </Button>
        </>
      }
    >
      <div className="cw-row">
        <label className="body" htmlFor="slideshow-seconds">
          {t("mediaBin.slideshow.seconds")}
        </label>
        <TextField
          id="slideshow-seconds"
          numeric
          inputMode="decimal"
          value={seconds}
          onChange={setSeconds}
          invalid={length === null}
          autoFocus
          onFocus={(event) => event.currentTarget.select()}
        />
      </div>
      <div className="cw-row">
        <span className="body">{t("mediaBin.slideshow.order")}</span>
        <SegmentedControl
          aria-label={t("mediaBin.slideshow.order")}
          value={order}
          onChange={setOrder}
          options={SLIDESHOW_ORDERS.map((value) => ({ value, label: t(ORDER_LABEL[value]) }))}
        />
      </div>
    </Dialog>
  );
}
