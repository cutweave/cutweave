// SPDX-License-Identifier: GPL-3.0-or-later
import * as RadixTabs from "@radix-ui/react-tabs";
import { createContext, type ReactNode, useContext, useEffect, useRef, useState } from "react";
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
 * The tab strip. Left/Right and Home/End move focus and activate (roving tabindex). When the tabs
 * are wider than the strip it scrolls horizontally: the mouse wheel scrolls it sideways, a chevron
 * button appears at each edge that hides tabs, and the selected tab is kept in view.
 */
export function TabList({ tabs, className, ...rest }: TabListProps) {
  const selected = useContext(SelectedTab);
  const listRef = useRef<HTMLDivElement>(null);
  const [hidden, setHidden] = useState({ start: false, end: false });
  useEffect(() => {
    const el = listRef.current?.querySelector<HTMLElement>('[aria-selected="true"]');
    el?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [selected]);
  // Re-measure whenever the tabs change (a different set can overflow differently).
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const measure = () => {
      const max = list.scrollWidth - list.clientWidth;
      const start = list.scrollLeft > 1;
      const end = list.scrollLeft < max - 1;
      setHidden((prev) => (prev.start === start && prev.end === end ? prev : { start, end }));
    };
    const onWheel = (event: WheelEvent) => {
      if (list.scrollWidth <= list.clientWidth) return;
      if (Math.abs(event.deltaY) <= Math.abs(event.deltaX)) return;
      list.scrollLeft += event.deltaY;
      event.preventDefault();
    };
    measure();
    list.addEventListener("scroll", measure, { passive: true });
    list.addEventListener("wheel", onWheel, { passive: false });
    const resize = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    resize?.observe(list);
    return () => {
      list.removeEventListener("scroll", measure);
      list.removeEventListener("wheel", onWheel);
      resize?.disconnect();
    };
  }, [tabs]);
  const page = (direction: 1 | -1) => {
    const list = listRef.current;
    list?.scrollBy?.({ left: direction * list.clientWidth * 0.8, behavior: "smooth" });
  };
  return (
    <div className={cx("cw-tabsbar", className)}>
      {hidden.start ? (
        // Mouse affordance only: keyboard users move through the tabs with the arrow keys.
        <button
          type="button"
          className="cw-tabsbar__scroll cw-tabsbar__scroll--start"
          tabIndex={-1}
          aria-hidden="true"
          onClick={() => page(-1)}
        >
          <Icon name="chevron-left" size="sm" />
        </button>
      ) : null}
      <RadixTabs.List ref={listRef} className="cw-tabs" {...rest}>
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
      {hidden.end ? (
        <button
          type="button"
          className="cw-tabsbar__scroll cw-tabsbar__scroll--end"
          tabIndex={-1}
          aria-hidden="true"
          onClick={() => page(1)}
        >
          <Icon name="chevron-right" size="sm" />
        </button>
      ) : null}
    </div>
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
