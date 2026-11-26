// SPDX-License-Identifier: GPL-3.0-or-later
import { cx } from "./cx.ts";
import { Tooltip } from "./Tooltip.tsx";

/** A cell of a layout, in fractions of the frame (`0..1`). */
export interface LayoutCell {
  readonly x: number;
  readonly y: number;
  readonly w: number;
  readonly h: number;
}

export interface LayoutOption {
  readonly id: string;
  /** Names the layout in the tooltip and for assistive technology: "Side by side". */
  readonly label: string;
  /** The cells in the order clips fill them; the first one is drawn in the accent. */
  readonly cells: readonly LayoutCell[];
}

export interface LayoutPickerProps {
  readonly options: readonly LayoutOption[];
  readonly onPick: (id: string) => void;
  /** Names the group: "Layout". */
  readonly "aria-label": string;
  readonly disabled?: boolean;
  readonly className?: string;
}

const W = 24;
const H = 16;
const GAP = 0.75;

/** A small diagram of a layout: the frame with its cells, the first clip's cell in the accent. */
export function LayoutDiagram({ cells }: { readonly cells: readonly LayoutCell[] }) {
  return (
    <svg className="cw-layoutpicker__svg" viewBox={`0 0 ${W} ${H}`} aria-hidden="true">
      <rect
        className="cw-layoutpicker__frame"
        x="0.5"
        y="0.5"
        width={W - 1}
        height={H - 1}
        rx="1"
      />
      {cells.map((c, i) => (
        <rect
          key={i}
          className={cx("cw-layoutpicker__cell", i === 0 && "cw-layoutpicker__cell--first")}
          x={c.x * W + GAP}
          y={c.y * H + GAP}
          width={Math.max(c.w * W - GAP * 2, 1)}
          height={Math.max(c.h * H - GAP * 2, 1)}
          rx="0.75"
        />
      ))}
    </svg>
  );
}

/** A row of small icon buttons, one per layout, each showing its diagram (WP-2C.08). */
export function LayoutPicker({
  options,
  onPick,
  disabled,
  className,
  "aria-label": ariaLabel,
}: LayoutPickerProps) {
  return (
    // biome-ignore lint/a11y/useSemanticElements: a fieldset would bring its own border and layout
    <div role="group" aria-label={ariaLabel} className={cx("cw-layoutpicker", className)}>
      {options.map((o) => (
        <Tooltip key={o.id} label={o.label}>
          <button
            type="button"
            className="cw-iconbtn cw-iconbtn--sm cw-layoutpicker__btn"
            aria-label={o.label}
            data-layout={o.id}
            disabled={disabled}
            onClick={() => onPick(o.id)}
          >
            <LayoutDiagram cells={o.cells} />
          </button>
        </Tooltip>
      ))}
    </div>
  );
}
