// SPDX-License-Identifier: GPL-3.0-or-later
import { InspectorSection } from "@cutweave/ui";
import type { ReactNode } from "react";
import { ContextSurface } from "../menus/host.tsx";
import type { SectionData } from "../menus/surfaces/inspector.ts";
import { type InspectorSectionId, useSection } from "./sections.ts";

/**
 * A collapsible section whose header menu resets it and collapses or expands them all
 * (WP-2H.13). `ids` are the clips it edits and `group` the engine's reset group, which the menu's
 * "Reset section" needs; without them the row is disabled with its reason.
 */
export function Section({
  id,
  label,
  children,
  ids = [],
  group,
  modified,
  onReset,
}: {
  readonly id: InspectorSectionId;
  readonly label: string;
  readonly children: ReactNode;
  readonly ids?: readonly string[];
  readonly group?: SectionData["group"];
  /** Something in the section is not at its default: its reset button shows. */
  readonly modified?: boolean;
  readonly onReset?: () => void;
}) {
  const [expanded, setExpanded] = useSection(id);
  return (
    <ContextSurface
      surface="inspector.section"
      className="cw-context-surface"
      target={() => ({
        name: label,
        ids: [...ids],
        ...(group !== undefined && { data: { group } satisfies SectionData }),
      })}
    >
      <InspectorSection
        label={label}
        expanded={expanded}
        onExpandedChange={setExpanded}
        {...(modified !== undefined && { modified })}
        {...(onReset !== undefined && { onReset })}
      >
        {children}
      </InspectorSection>
    </ContextSurface>
  );
}
