// SPDX-License-Identifier: GPL-3.0-or-later
import { KeyframeGroup, KeyframeToggle } from "./KeyframeToggle.tsx";

export default { title: "Inputs/KeyframeToggle", component: KeyframeToggle, tags: ["autodocs"] };

const row = {
  padding: "var(--space-4)",
  display: "flex",
  gap: "var(--space-3)",
  alignItems: "center",
} as const;

const noop = () => {};

export const States = {
  render: () => (
    <div style={row}>
      <KeyframeToggle state="off" onToggle={noop} />
      <KeyframeToggle state="on" onToggle={noop} />
      <KeyframeToggle state="elsewhere" onToggle={noop} />
      <KeyframeToggle state="off" disabled onToggle={noop} />
    </div>
  ),
};

export const WithPrevNext = {
  render: () => (
    <div style={row}>
      <span className="label" style={{ color: "var(--text-secondary)" }}>
        Position X
      </span>
      <KeyframeGroup state="on" property="Position X" onPrev={noop} onNext={noop} onToggle={noop} />
    </div>
  ),
};
