// SPDX-License-Identifier: GPL-3.0-or-later
// The blocking screen when the math module does not match this build (ADR-0033).

import { useTranslation } from "react-i18next";
import type { MathStartup } from "./startup.ts";

export function MathStartupError({ failure }: { failure: Extract<MathStartup, { ok: false }> }) {
  const { t } = useTranslation();
  const version = failure.kind === "version";
  return (
    <main className="cw-startup-error" role="alert" aria-labelledby="cw-startup-error-title">
      <h1 id="cw-startup-error-title" className="cw-startup-error__title">
        {t(version ? "mathStartup.versionTitle" : "mathStartup.loadTitle")}
      </h1>
      <p className="cw-startup-error__body">
        {t(version ? "mathStartup.versionBody" : "mathStartup.loadBody")}
      </p>
      <pre className="cw-startup-error__detail">{failure.detail}</pre>
    </main>
  );
}
