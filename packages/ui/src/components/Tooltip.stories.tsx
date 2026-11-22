// SPDX-License-Identifier: GPL-3.0-or-later
import { IconButton } from "./IconButton.tsx";
import { Tooltip, TooltipProvider } from "./Tooltip.tsx";

export default { title: "Feedback/Tooltip", component: Tooltip, tags: ["autodocs"] };

const row = {
  padding: "var(--space-4)",
  display: "flex",
  gap: "var(--space-8)",
  alignItems: "flex-start",
  background: "var(--surface-1)",
} as const;

/** Hover or tab to each button; the shortcut comes from the keymap as real characters. */
export const WithShortcuts = {
  render: () => (
    <TooltipProvider>
      <div style={row}>
        <Tooltip label="Split at playhead" keys={["S"]}>
          <IconButton icon="scissors" label="Split" />
        </Tooltip>
        <Tooltip label="Snapping on" keys={["N"]}>
          <IconButton icon="magnet" label="Snapping" pressed />
        </Tooltip>
        <Tooltip label="Ripple delete" keys={["Shift", "Backspace"]}>
          <IconButton icon="ripple" label="Ripple delete" />
        </Tooltip>
        <Tooltip label="Lock track">
          <IconButton icon="lock" label="Lock track" />
        </Tooltip>
      </div>
    </TooltipProvider>
  ),
};
