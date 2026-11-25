// SPDX-License-Identifier: GPL-3.0-or-later
import { MediaBreadcrumb } from "./MediaBreadcrumb.tsx";

export default { title: "Media/MediaBreadcrumb", component: MediaBreadcrumb, tags: ["autodocs"] };

const noop = () => {};

const box = { padding: "var(--space-4)", background: "var(--surface-1)" } as const;

/** Two levels deep: each segment goes there; the last is the current place. */
export const Nested = {
  render: () => (
    <div style={box}>
      <MediaBreadcrumb
        label="Media folders"
        segments={[
          { id: null, label: "Media" },
          { id: "fld_1", label: "Interviews" },
          { id: "fld_2", label: "Day one" },
        ]}
        onNavigate={noop}
      />
    </div>
  ),
};

/** A drag is over the Interviews segment: it takes the marigold drop state. */
export const DropTarget = {
  render: () => (
    <div style={box}>
      <MediaBreadcrumb
        label="Media folders"
        segments={[
          { id: null, label: "Media" },
          { id: "fld_1", label: "Interviews" },
          { id: "fld_2", label: "Day one" },
        ]}
        dropTarget="fld_1"
        onNavigate={noop}
      />
    </div>
  ),
};

/** The top level alone. */
export const TopLevel = {
  render: () => (
    <div style={box}>
      <MediaBreadcrumb
        label="Media folders"
        segments={[{ id: null, label: "Media" }]}
        onNavigate={noop}
      />
    </div>
  ),
};
