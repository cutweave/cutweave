// SPDX-License-Identifier: GPL-3.0-or-later
import { InspectorRow, InspectorSection } from "@cutweave/ui";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { EmptyState } from "./EmptyState.tsx";

/** Right dock panel: context inspector. Nothing is selectable yet, so it shows project settings. */
export function InspectorPanel() {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(true);
  return (
    <div className="cw-inspector">
      <EmptyState
        icon="sliders-horizontal"
        title={t("inspector.emptyTitle")}
        body={t("inspector.emptyBody")}
      />
      <InspectorSection
        label={t("inspector.project")}
        expanded={expanded}
        onExpandedChange={setExpanded}
      >
        <InspectorRow label={t("inspector.resolution")}>
          <span className="numeric">{t("inspector.resolutionValue")}</span>
        </InspectorRow>
        <InspectorRow label={t("inspector.frameRate")}>
          <span className="numeric">{t("inspector.frameRateValue")}</span>
        </InspectorRow>
        <InspectorRow label={t("inspector.duration")}>
          <span className="numeric">{t("inspector.durationValue")}</span>
        </InspectorRow>
      </InspectorSection>
    </div>
  );
}
