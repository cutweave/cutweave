// SPDX-License-Identifier: GPL-3.0-or-later
import { StatusChip } from "./StatusChip.tsx";

export default { title: "Feedback/StatusChip", component: StatusChip, tags: ["autodocs"] };

const row = {
  padding: "var(--space-4)",
  display: "flex",
  flexWrap: "wrap",
  gap: "var(--space-3)",
  alignItems: "center",
  background: "var(--surface-1)",
} as const;

export const Tones = {
  render: () => (
    <div style={row}>
      <StatusChip tone="success">Exported</StatusChip>
      <StatusChip tone="warning">Missing media</StatusChip>
      <StatusChip tone="danger">Failed</StatusChip>
      <StatusChip tone="neutral">Proxy ready</StatusChip>
    </div>
  ),
};

/** Icon only, for the title bar: each state its own icon; hover shows the words. */
export const IconOnly = {
  render: () => (
    <div style={row}>
      <StatusChip tone="success" icon="check" iconOnly>
        Engine connected
      </StatusChip>
      <StatusChip tone="neutral" icon="loader-circle" iconOnly>
        Connecting
      </StatusChip>
      <StatusChip tone="warning" icon="loader-circle" iconOnly>
        Reconnecting
      </StatusChip>
      <StatusChip tone="danger" icon="circle-alert" iconOnly>
        Engine stopped
      </StatusChip>
      <StatusChip tone="neutral" icon="plug" iconOnly>
        Engine not connected
      </StatusChip>
    </div>
  ),
};
