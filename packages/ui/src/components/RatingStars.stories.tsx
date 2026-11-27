// SPDX-License-Identifier: GPL-3.0-or-later
import { useState } from "react";
import { RatingStars } from "./RatingStars.tsx";

export default { title: "Inputs/RatingStars", component: RatingStars, tags: ["autodocs"] };

const col = {
  padding: "var(--space-4)",
  display: "flex",
  flexDirection: "column",
  gap: "var(--space-3)",
  width: "var(--layer-width-sm)",
  background: "var(--surface-1)",
} as const;

const starLabel = (n: number) => (n === 1 ? "1 star" : `${n} stars`);

function Stars({ initial, disabled }: { initial: number | null; disabled?: boolean }) {
  const [value, setValue] = useState(initial);
  return (
    <div className="cw-row">
      <span className="cw-row__label label">Rating</span>
      <RatingStars
        value={value}
        onChange={setValue}
        aria-label="Rating"
        starLabel={starLabel}
        disabled={disabled}
      />
    </div>
  );
}

/** The media info's rating: unrated, three stars, five stars. Click the current star to clear. */
export const Ratings = {
  render: () => (
    <div style={col}>
      <Stars initial={null} />
      <Stars initial={3} />
      <Stars initial={5} />
    </div>
  ),
};

export const Disabled = {
  render: () => (
    <div style={col}>
      <Stars initial={2} disabled />
    </div>
  ),
};
