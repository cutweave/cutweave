// SPDX-License-Identifier: GPL-3.0-or-later
// "In use by <actor>" marks (WP-2A.12, arch §5.2): a dashed agent outline over each clip another
// actor's open gesture or proposal holds, with a lock badge whose tooltip names who. Nothing is
// blocked: the outline lets the pointer through, and an edit of a held clip is refused by the
// engine with a toast that names the same actor (`describeRefusal`). The marks follow the
// engine's `tx.*` events (`locks.ts`) and are gone the moment the holder commits or cancels.
import { Icon, Tooltip } from "@cutweave/ui";
import { type ReactElement, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import { useEngineStore } from "../engine/engineStore.ts";
import { actorDisplayName } from "../session/historyLabels.ts";
import { useDocStore } from "../state/docStore.ts";
import { useViewStore } from "../state/viewStore.ts";
import type { TimelineController } from "./controller.ts";
import { translator } from "./errors.ts";
import { LocksFeed, locksStore } from "./locks.ts";

/** Keeps `locksStore` current while the timeline shows an engine's sequence. */
function useLocksFeed(): void {
  const rpc = useEngineStore((s) => s.rpc);
  const ready = useEngineStore((s) => s.connection === "ready");
  useEffect(() => {
    if (rpc === null || !ready) return undefined;
    // A fresh baseline on every (re)connection: events sent while away are lost.
    const feed = new LocksFeed({ rpc });
    feed.start();
    return () => feed.stop();
  }, [rpc, ready]);
}

/** The marks, laid over the timeline canvas. */
export function InUseMarks({ controller }: { readonly controller: TimelineController }) {
  const { t } = useTranslation();
  useLocksFeed();
  const byItem = useStore(locksStore, (s) => s.byItem);
  // Follow the clips when the timeline scrolls or zooms, or the document changes.
  useViewStore((s) => s.scroll);
  useViewStore((s) => s.zoom);
  useDocStore((s) => s.project);
  if (byItem.size === 0) return null;
  const tr = translator(t);
  const marks: ReactElement[] = [];
  for (const [item, actor] of byItem) {
    const rect = controller.itemRect(item);
    if (rect === null) continue;
    const label = t("timeline.inUse", { who: actorDisplayName(actor, tr) ?? actor });
    marks.push(
      <div
        key={item}
        className="cw-timeline__in-use"
        data-item={item}
        style={{ left: rect.x, top: rect.y, width: rect.w, height: rect.h }}
      >
        <Tooltip label={label}>
          {/* biome-ignore lint/a11y/noNoninteractiveTabindex: focusable so the tooltip also reaches keyboard users */}
          <span className="cw-timeline__in-use-badge" role="img" aria-label={label} tabIndex={0}>
            <Icon name="lock" size="sm" />
          </span>
        </Tooltip>
      </div>,
    );
  }
  return <>{marks}</>;
}
