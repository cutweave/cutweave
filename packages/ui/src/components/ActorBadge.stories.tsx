// SPDX-License-Identifier: GPL-3.0-or-later
import { ActorBadge } from "./ActorBadge.tsx";

export default { title: "AI & agents/ActorBadge", component: ActorBadge, tags: ["autodocs"] };

const pad = { padding: "var(--space-4)", background: "var(--surface-1)" } as const;

const rowStyle = {
  display: "flex",
  alignItems: "center",
  gap: "var(--space-3)",
  padding: "var(--space-2) var(--space-3)",
  borderBottom: "var(--border-thin) solid var(--border-subtle)",
} as const;

export const Kinds = {
  render: () => (
    <div style={{ ...pad, display: "flex", gap: "var(--space-3)", flexWrap: "wrap" }}>
      <ActorBadge kind="user" />
      <ActorBadge kind="agent" id="claude-code" />
      <ActorBadge kind="plugin" id="acme.lut-pack" />
    </div>
  ),
};

const HISTORY = [
  {
    key: "1",
    actor: <ActorBadge kind="user" />,
    what: "Split interview_A.mov",
    when: "00:01:24:17",
  },
  {
    key: "2",
    actor: <ActorBadge kind="agent" id="claude-code" />,
    what: "Removed 14 silences",
    when: "00:02:03:00",
  },
  {
    key: "3",
    actor: <ActorBadge kind="plugin" id="acme.lut-pack" />,
    what: "Applied Teal-orange grade",
    when: "00:00:00:00",
  },
];

export const InHistoryList = {
  render: () => (
    <ul
      aria-label="History"
      style={{ ...pad, margin: 0, listStyle: "none", width: "var(--layer-width-lg)" }}
    >
      {HISTORY.map((row) => (
        <li key={row.key} style={rowStyle}>
          {row.actor}
          <span className="body" style={{ flex: 1, color: "var(--text-primary)" }}>
            {row.what}
          </span>
          <span className="numeric" style={{ color: "var(--text-muted)" }}>
            {row.when}
          </span>
        </li>
      ))}
    </ul>
  ),
};
