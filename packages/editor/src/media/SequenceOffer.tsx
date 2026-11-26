// SPDX-License-Identifier: GPL-3.0-or-later
// "Import as an image sequence?" (WP-2F.05c, ADR-0035): asked when the files picked in the bin's
// Import… dialog are a numbered run of pictures. One answer, one import: the run as one item
// that plays like video (`media.import_sequence`), or every picture as a still (`media.import`).
import { Button, Dialog, DialogNote } from "@cutweave/ui";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import { sequenceOfferStore } from "./sequence.ts";

export function SequenceOffer() {
  const { t } = useTranslation();
  const offer = useStore(sequenceOfferStore, (s) => s.offer);
  if (offer === null) return null;
  const answer = sequenceOfferStore.getState().answer;
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) answer("cancel");
      }}
      title={t("mediaBin.sequence.offerTitle")}
      description={t("mediaBin.sequence.offerBody", {
        count: String(offer.count),
        first: offer.first,
        last: offer.last,
      })}
      onSubmit={() => answer("sequence")}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={() => answer("cancel")}>
            {t("mediaBin.sequence.cancel")}
          </Button>
          <Button variant="secondary" size="lg" onClick={() => answer("separate")}>
            {t("mediaBin.sequence.asSeparate")}
          </Button>
          <Button variant="primary" size="lg" onClick={() => answer("sequence")}>
            {t("mediaBin.sequence.asSequence")}
          </Button>
        </>
      }
    >
      <DialogNote icon="info">{t("mediaBin.sequence.offerNote")}</DialogNote>
    </Dialog>
  );
}
