// SPDX-License-Identifier: GPL-3.0-or-later
// "Set rating" from the command palette (WP-2F.06b): the stars for the cards selected in the bin.
// A click is one `media.set_rating` for all of them (one undo step); the current star again
// clears the rating. The card menu's Set rating ▸ and the media info send the same command.
import type { MediaId } from "@cutweave/protocol";
import { Button, Dialog, DialogNote, RatingStars } from "@cutweave/ui";
import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { useEngineReady } from "../engine/engineStore.ts";
import { menuDialogStore } from "../menus/dialogs.ts";
import { useDocStore } from "../state/docStore.ts";
import { selectMediaCards } from "./cards.ts";
import { selectedTiles } from "./folderUi.ts";
import { binDeps } from "./requestDelete.ts";
import { setRating } from "./tagActions.ts";
import { sharedRating } from "./tags.ts";

export function RatingDialog({ media }: { readonly media: readonly string[] }) {
  const { t } = useTranslation();
  const project = useDocStore((s) => s.project);
  const ready = useEngineReady();
  const cards = useMemo(() => {
    const wanted = new Set<string>(media);
    return selectMediaCards(project).filter((card) => wanted.has(card.id));
  }, [project, media]);
  const shared = sharedRating(cards);
  const close = () => menuDialogStore.getState().close();
  const one = cards.length === 1 ? cards[0] : undefined;
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
      title={
        one !== undefined
          ? t("mediaBin.rating.titleOne", { name: one.name })
          : t("mediaBin.rating.titleMany", { count: cards.length })
      }
      description={t("mediaBin.rating.body")}
      footer={
        <Button variant="primary" size="lg" onClick={close}>
          {t("mediaBin.tags.done")}
        </Button>
      }
    >
      <div className="cw-row">
        <span className="cw-row__label label">{t("mediaBin.rating.label")}</span>
        <RatingStars
          value={shared === "mixed" ? null : shared}
          aria-label={t("mediaBin.rating.label")}
          starLabel={(count) =>
            count === 1 ? t("mediaBin.rating.starOne") : t("mediaBin.rating.starMany", { count })
          }
          disabled={!ready || cards.length === 0}
          onChange={(rating) =>
            void setRating(
              binDeps(),
              cards.map((card) => card.id as MediaId),
              rating,
            )
          }
        />
      </div>
      {shared === "mixed" ? (
        <DialogNote icon="info">{t("mediaBin.rating.mixed")}</DialogNote>
      ) : null}
    </Dialog>
  );
}

/** Opens the stars for the cards selected in the bin; nothing selected: nothing opens. */
export function openRatingDialog(): void {
  const media = selectedTiles().media;
  if (media.length === 0) return;
  menuDialogStore.getState().open({ kind: "rating", media: [...media] });
}
