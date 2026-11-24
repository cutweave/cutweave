// SPDX-License-Identifier: GPL-3.0-or-later
// Help > About Cutweave (WP-1M.09): the editor's own dialog, so it looks like the rest of the app
// and a person (or a user-flow test) can read and close it. The versions come from main.
import { Button, Dialog, DialogNote, InspectorRow } from "@cutweave/ui";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { type AboutInfo, desktopFromWindow } from "../session/desktop.ts";
import { aboutStore, useAboutOpen } from "./appCommands.ts";

export function AboutDialog() {
  const { t } = useTranslation();
  const open = useAboutOpen();
  const [info, setInfo] = useState<AboutInfo | null>(null);
  useEffect(() => {
    if (!open) return;
    let alive = true;
    desktopFromWindow()
      ?.app?.about()
      .then((about) => {
        if (alive) setInfo(about);
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [open]);
  if (!open) return null;
  const close = (): void => aboutStore.getState().setOpen(false);
  const name = t("app.name");
  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next) close();
      }}
      title={t("about.title", { name })}
      description={t("about.tagline")}
      footer={
        <Button variant="primary" size="lg" onClick={close}>
          {t("about.close")}
        </Button>
      }
    >
      {info === null ? null : (
        <div className="cw-inspector__rows" data-testid="about-versions">
          <InspectorRow label={t("about.version")}>
            <span className="numeric">{info.version}</span>
          </InspectorRow>
          <InspectorRow label={t("about.electron")}>
            <span className="numeric">{info.electron}</span>
          </InspectorRow>
          <InspectorRow label={t("about.chromium")}>
            <span className="numeric">{info.chrome}</span>
          </InspectorRow>
          <InspectorRow label={t("about.node")}>
            <span className="numeric">{info.node}</span>
          </InspectorRow>
          <InspectorRow label={t("about.platform")}>
            <span className="numeric">{info.platform}</span>
          </InspectorRow>
        </div>
      )}
      <DialogNote>{t("about.license")}</DialogNote>
    </Dialog>
  );
}
