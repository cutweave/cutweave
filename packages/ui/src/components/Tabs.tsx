// SPDX-License-Identifier: GPL-3.0-or-later
import * as RadixTabs from "@radix-ui/react-tabs";
import { createContext, type ReactNode, useContext, useEffect, useRef } from "react";
import { Icon, type IconName } from "../icons/index.ts";
import { cx } from "./cx.ts";

const SelectedTab = createContext<string | undefined>(undefined);

export interface TabsProps {
  /** Selected tab id. Dispatches `layout.panel.setTab`. */
  value: string;
  onValueChange: (value: string) => void;
  children: ReactNode;
  className?: string | undefined;
}

/** Owns the selection for a `TabList` and its `TabPanel`s. Renders no box of its own. */
export function Tabs({ value, onValueChange, className, children }: TabsProps) {
  return (
    <SelectedTab.Provider value={value}>
      <RadixTabs.Root
        value={value}
        onValueChange={onValueChange}
        className={cx("cw-tabsroot", className)}
      >
        {children}
      </RadixTabs.Root>
    </SelectedTab.Provider>
  );
}

export interface TabItem {
  id: string;
  label: string;
  /** Optional leading icon; AI Studio uses `sparkles`. */
  icon?: IconName | undefined;
  disabled?: boolean | undefined;
}

export interface TabListProps {
  tabs: readonly TabItem[];
  /** Names the tablist, e.g. "Asset browser". */
  "aria-label": string;
  className?: string | undefined;
}

/**
 * The tab strip. Left/Right and Home/End move focus and activate (roving tabindex); overflow
 * scrolls horizontally and keeps the selected tab in view.
 */
export function TabList({ tabs, className, ...rest }: TabListProps) {
  const selected = useContext(SelectedTab);
  const listRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = listRef.current?.querySelector<HTMLElement>('[aria-selected="true"]');
    el?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [selected]);
  return (
    <RadixTabs.List ref={listRef} className={cx("cw-tabs", className)} {...rest}>
      {tabs.map((tab) => (
        <RadixTabs.Trigger
          key={tab.id}
          value={tab.id}
          disabled={tab.disabled}
          className="cw-tabs__tab label"
        >
          {tab.icon ? <Icon name={tab.icon} size="md" /> : null}
          {tab.label}
        </RadixTabs.Trigger>
      ))}
    </RadixTabs.List>
  );
}

export interface TabPanelProps {
  /** The id of the tab this panel belongs to. */
  value: string;
  children: ReactNode;
  className?: string | undefined;
}

export function TabPanel({ value, className, children }: TabPanelProps) {
  return (
    <RadixTabs.Content value={value} className={cx("cw-tabpanel", className)}>
      {children}
    </RadixTabs.Content>
  );
}
