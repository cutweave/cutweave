// SPDX-License-Identifier: GPL-3.0-or-later
// The Layout section of the Inspector (WP-2C.08): with 2 to 4 video clips selected, small diagrams
// of the layouts for that many clips, how the pictures fill their cells, and Swap for two clips.
// Each choice is one `item.apply_layout`; the result is ordinary transform and crop values.
import type { LayoutFill } from "@cutweave/protocol";
import { IconButton, InspectorRow, LayoutPicker, SegmentedControl, Tooltip } from "@cutweave/ui";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import { engineStore } from "../engine/engineStore.ts";
import type { TKey } from "../i18n/index.ts";
import {
  applyLayout,
  canLayout,
  canSwap,
  lastApplied,
  swapLayout,
  visualClipIds,
} from "../layout/actions.ts";
import { layoutOptions } from "../layout/names.ts";
import type { SelectedClip } from "./fields.ts";
import { Section } from "./Section.tsx";

export function LayoutSection({
  clips,
  locked,
}: {
  readonly clips: readonly SelectedClip[];
  readonly locked: boolean;
}) {
  const { t } = useTranslation();
  const rpc = useStore(engineStore, (s) => (s.blocked === null ? s.rpc : null));
  const ids = clips.map((c) => c.item.id);
  const visual = visualClipIds(ids);
  const [fill, setFill] = useState<LayoutFill>("crop");
  const [swappable, setSwappable] = useState(false);
  const options = useMemo(
    () => layoutOptions(visual.length, (key: TKey) => t(key)),
    [visual.length, t],
  );
  if (!canLayout(ids)) return null;
  const enabled = rpc !== null && !locked;
  const remember = () => setSwappable(canSwap(ids));
  return (
    <Section id="layout" label={t("layout.title")} ids={ids}>
      <div className="cw-inspector__rows" data-testid="layout-section">
        <LayoutPicker
          aria-label={t("layout.group")}
          options={options}
          disabled={!enabled}
          onPick={(layout) => {
            void applyLayout(engineStore, ids, layout, { fill }).then(remember);
          }}
        />
        <InspectorRow label={t("layout.fill")}>
          <SegmentedControl<LayoutFill>
            aria-label={t("layout.fill")}
            disabled={!enabled}
            value={fill}
            options={[
              { value: "crop", label: t("layout.fillCrop") },
              { value: "fit", label: t("layout.fillFit") },
            ]}
            onChange={(next) => {
              setFill(next);
              const last = lastApplied(visual);
              if (last !== undefined) {
                void applyLayout(engineStore, ids, last.layout, {
                  fill: next,
                  order: last.order,
                });
              }
            }}
          />
        </InspectorRow>
        {visual.length === 2 ? (
          <InspectorRow label={t("layout.swap")}>
            <Tooltip label={canSwap(ids) ? t("layout.swap") : t("layout.swapReason")}>
              <IconButton
                size="sm"
                icon="arrow-left-right"
                label={t("layout.swap")}
                disabled={!enabled || !(swappable || canSwap(ids))}
                onClick={() => {
                  void swapLayout(engineStore, ids).then(remember);
                }}
              />
            </Tooltip>
          </InspectorRow>
        ) : null}
      </div>
    </Section>
  );
}
