// SPDX-License-Identifier: GPL-3.0-or-later
import { SegmentedControl } from "@cutweave/ui";
import type { CSSProperties } from "react";
import { useTranslation } from "react-i18next";
import { ASPECTS, type AspectId, useViewStore, viewStore } from "../state/viewStore.ts";
import { EmptyState } from "./EmptyState.tsx";
import { TransportBar } from "./TransportBar.tsx";

/** Centre dock panel: aspect switcher, the frame on `surface-preview`, and the transport bar. */
export function PreviewPanel() {
  const { t } = useTranslation();
  const aspectId = useViewStore((s) => s.aspect);
  const aspect = ASPECTS.find((a) => a.id === aspectId) ?? ASPECTS[0];
  const frameStyle = { "--cw-aspect": `${aspect.width} / ${aspect.height}` } as CSSProperties;
  return (
    <div className="cw-preview">
      <div className="cw-preview__bar">
        <SegmentedControl<AspectId>
          aria-label={t("preview.aspectLabel")}
          value={aspect.id}
          onChange={(next) => viewStore.getState().setAspect(next)}
          options={ASPECTS.map((a) => ({ value: a.id, label: t(a.labelKey) }))}
        />
      </div>
      <div className="cw-preview__stage">
        <div className="cw-preview__frame" style={frameStyle}>
          <EmptyState
            icon="file-video"
            title={t("preview.emptyTitle")}
            body={t("preview.emptyBody")}
          />
        </div>
      </div>
      <TransportBar />
    </div>
  );
}
