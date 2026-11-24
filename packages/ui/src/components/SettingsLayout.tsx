// SPDX-License-Identifier: GPL-3.0-or-later
import * as RadixTabs from "@radix-ui/react-tabs";
import type { ReactNode } from "react";
import { Icon, type IconName } from "../icons/index.ts";
import { cx } from "./cx.ts";

export interface SettingsPageItem {
  id: string;
  label: string;
  icon?: IconName | undefined;
}

export interface SettingsPageGroup {
  id: string;
  /** Small heading above the group's pages: "Preferences". Leave out for an unnamed group. */
  label?: string | undefined;
  pages: readonly SettingsPageItem[];
}

export interface SettingsLayoutProps {
  /** Names the page list: "Settings pages". */
  navLabel: string;
  /** The search field, above the page list. */
  search: ReactNode;
  groups: readonly SettingsPageGroup[];
  /**
   * The open page. `null` while a search shows its results: no page is selected and `children`
   * are the results.
   */
  activeId: string | null;
  onSelect: (id: string) => void;
  /** The heading of the open page ("Appearance"), or of the results ("Results for "auto""). */
  heading: string;
  /** Next to the heading: "Reset page". */
  actions?: ReactNode;
  /** Names the results region while searching: "Search results". */
  resultsLabel: string;
  /** The page's rows, or the search results. */
  children: ReactNode;
  className?: string | undefined;
}

/**
 * The frame of the Settings dialog: a search field over a vertical list of pages on the left,
 * the open page (heading, its reset, its rows) on the right. Place it in a `Dialog` with
 * `className="cw-dialog--settings"`. The page list is a vertical tablist: Up and Down move
 * between pages, Home and End jump to the first and last.
 */
export function SettingsLayout({
  navLabel,
  search,
  groups,
  activeId,
  onSelect,
  heading,
  actions,
  resultsLabel,
  children,
  className,
}: SettingsLayoutProps) {
  const body = (
    <>
      <div className="cw-settings__head">
        <h3 className="cw-settings__heading title">{heading}</h3>
        {actions}
      </div>
      <div className="cw-settings__rows">{children}</div>
    </>
  );
  return (
    <RadixTabs.Root
      orientation="vertical"
      value={activeId ?? ""}
      onValueChange={onSelect}
      activationMode="automatic"
      className={cx("cw-settings", className)}
    >
      <div className="cw-settings__side">
        {search}
        <RadixTabs.List aria-label={navLabel} className="cw-settings__nav">
          {groups.map((group) => (
            <div key={group.id} role="presentation" className="cw-settings__group">
              {group.label ? (
                <div role="presentation" className="cw-settings__grouplabel overline">
                  {group.label}
                </div>
              ) : null}
              {group.pages.map((page) => (
                <RadixTabs.Trigger key={page.id} value={page.id} className="cw-settings__tab">
                  {page.icon ? <Icon name={page.icon} size="md" /> : null}
                  <span className="cw-settings__tablabel body">{page.label}</span>
                </RadixTabs.Trigger>
              ))}
            </div>
          ))}
        </RadixTabs.List>
      </div>
      {activeId === null ? (
        <section aria-label={resultsLabel} className="cw-settings__panel">
          {body}
        </section>
      ) : (
        <RadixTabs.Content value={activeId} className="cw-settings__panel">
          {body}
        </RadixTabs.Content>
      )}
    </RadixTabs.Root>
  );
}

/** A heading that groups result rows under their page while searching. */
export function SettingsResultGroup({ label, children }: { label: string; children: ReactNode }) {
  return (
    <section aria-label={label} className="cw-settings__result">
      <h4 className="cw-settings__resulthead overline">{label}</h4>
      {children}
    </section>
  );
}
