// SPDX-License-Identifier: GPL-3.0-or-later
import { type DragEvent, Fragment } from "react";
import { Icon } from "../icons/index.ts";
import { cx } from "./cx.ts";

export interface BreadcrumbSegment {
  /** The folder's ID; `null` for the top level. */
  id: string | null;
  label: string;
}

type SegmentDrag = (segment: BreadcrumbSegment, event: DragEvent<HTMLElement>) => void;

export interface MediaBreadcrumbProps {
  /** Names the nav: "Media folders". */
  label: string;
  /** From the top level down to the open folder; the last one is the current place. */
  segments: readonly BreadcrumbSegment[];
  onNavigate: (id: string | null) => void;
  /** The segment a drag is over right now and may drop on. */
  dropTarget?: string | null | undefined;
  /** Drag events of a segment, so the bin decides what a drop means. */
  onSegmentDragOver?: SegmentDrag | undefined;
  onSegmentDragLeave?: SegmentDrag | undefined;
  onSegmentDrop?: SegmentDrag | undefined;
  className?: string | undefined;
}

/**
 * Where the bin is: "Media › Interviews › Day 1". Each segment is a button that goes there and a
 * drop target for dragged tiles; the last one is the current place (`aria-current="page"`).
 */
export function MediaBreadcrumb({
  label,
  segments,
  onNavigate,
  dropTarget,
  onSegmentDragOver,
  onSegmentDragLeave,
  onSegmentDrop,
  className,
}: MediaBreadcrumbProps) {
  return (
    <nav aria-label={label} className={cx("cw-breadcrumb", className)}>
      <ol className="cw-breadcrumb__list">
        {segments.map((segment, index) => {
          const last = index === segments.length - 1;
          const over = dropTarget !== undefined && dropTarget === segment.id;
          return (
            <Fragment key={segment.id ?? "top"}>
              {index === 0 ? null : (
                <li className="cw-breadcrumb__sep" aria-hidden="true">
                  <Icon name="chevron-right" size="sm" />
                </li>
              )}
              <li className="cw-breadcrumb__item">
                <button
                  type="button"
                  className="cw-breadcrumb__segment label"
                  aria-current={last ? "page" : undefined}
                  data-drop-target={over ? "true" : undefined}
                  data-drop-folder={segment.id ?? ""}
                  onClick={() => onNavigate(segment.id)}
                  onDragEnter={(event) => onSegmentDragOver?.(segment, event)}
                  onDragOver={(event) => onSegmentDragOver?.(segment, event)}
                  onDragLeave={(event) => onSegmentDragLeave?.(segment, event)}
                  onDrop={(event) => onSegmentDrop?.(segment, event)}
                >
                  {segment.label}
                </button>
              </li>
            </Fragment>
          );
        })}
      </ol>
    </nav>
  );
}
