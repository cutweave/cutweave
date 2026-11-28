// SPDX-License-Identifier: GPL-3.0-or-later
// The sequence tabs above the timeline (WP-2B.03): one tab per open sequence. A click shows the
// sequence (the timeline, the preview and playback follow), a double-click or F2 renames it in
// place, a drag reorders, the small x (or a middle click) closes the tab without deleting the
// sequence, and "+" adds a sequence or reopens a closed one. Right-click or Shift+F10 opens the
// tab's menu (`sequence.tab`).
//
// It renders only when the list of sequences (their IDs and names), the active sequence, the tab
// order or the tab being renamed changes: never on a pointer move, a drag update or a document
// patch that leaves those alone (issue #241; `SequenceTabs.test.ts` counts the renders).
import type { SequenceId } from "@cutweave/protocol";
import { IconButton, InlineRename, Menu, type MenuEntry, Tooltip } from "@cutweave/ui";
import { type DragEvent, type KeyboardEvent, memo, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useContextMenu } from "../menus/host.tsx";
import { useDocStore } from "../state/docStore.ts";
import { selectActiveSequenceId } from "../state/selectors.ts";
import { useViewStore } from "../state/viewStore.ts";
import {
  beginRename,
  closeTab,
  createSequence,
  moveTab,
  renameSequence,
  showSequence,
} from "./actions.ts";
import { ASPECT_PRESETS } from "./presets.ts";
import { closedSequences, openTabs, sequenceListKey, sequencesOfKey } from "./tabs.ts";
import { sequenceUiStore, useSequenceUi } from "./uiStore.ts";

/** The drag payload of a tab: its sequence ID. Only tabs accept it. */
const TAB_MIME = "application/x-cutweave-sequence-tab";

/** Counts renders for the render-count test; nothing reads it in the app. */
export const tabStripRenders = { count: 0 };

export const SequenceTabs = memo(function SequenceTabs() {
  const { t } = useTranslation();
  const listKey = useDocStore((s) => sequenceListKey(s.project));
  const active = useDocStore((s) => selectActiveSequenceId(s.project));
  const saved = useViewStore((s) => s.sequenceTabs);
  const renaming = useSequenceUi((s) => s.renaming);
  tabStripRenders.count += 1;

  const all = useMemo(() => sequencesOfKey(listKey), [listKey]);
  const tabs = useMemo(() => openTabs(all, saved, active), [all, saved, active]);
  const shut = useMemo(() => closedSequences(all, saved, active), [all, saved, active]);
  const listRef = useRef<HTMLDivElement>(null);
  const dragged = useRef<SequenceId | null>(null);

  const surface = useContextMenu<HTMLDivElement>("sequence.tab", (query) => {
    const id = query.source.closest<HTMLElement>("[data-sequence]")?.dataset.sequence;
    const tab = tabs.find((s) => s.id === id) ?? tabs.find((s) => s.id === active);
    return tab === undefined ? {} : { ids: [tab.id], name: tab.name };
  });

  const addItems = useMemo<readonly MenuEntry[]>(
    () => [
      { id: "new", label: t("sequences.new"), icon: "plus", onSelect: () => void createSequence() },
      { kind: "separator", id: "sep-presets" },
      { kind: "label", id: "presets", label: t("sequences.newWithSize") },
      ...ASPECT_PRESETS.map((preset) => ({
        id: `new-${preset.id}`,
        label: t("sequences.presetSize", {
          ratio: preset.ratio,
          width: preset.width,
          height: preset.height,
        }),
        onSelect: () => void createSequence(preset.id),
      })),
      ...(shut.length === 0
        ? []
        : [
            { kind: "separator" as const, id: "sep-closed" },
            { kind: "label" as const, id: "closed", label: t("sequences.reopen") },
            ...shut.map((s) => ({
              id: `open-${s.id}`,
              label: s.name,
              onSelect: () => showSequence(s.id),
            })),
          ]),
    ],
    [t, shut],
  );

  if (tabs.length === 0) return null;

  const focusTab = (id: SequenceId): void => {
    // After the render that follows the switch, the tab of the sequence that shows has the focus.
    queueMicrotask(() =>
      listRef.current?.querySelector<HTMLElement>(`[data-sequence="${id}"] [role="tab"]`)?.focus(),
    );
  };
  const onKeyDown = (event: KeyboardEvent<HTMLElement>, id: SequenceId): void => {
    const at = tabs.findIndex((s) => s.id === id);
    const to =
      event.key === "ArrowRight"
        ? tabs[(at + 1) % tabs.length]
        : event.key === "ArrowLeft"
          ? tabs[(at - 1 + tabs.length) % tabs.length]
          : event.key === "Home"
            ? tabs[0]
            : event.key === "End"
              ? tabs[tabs.length - 1]
              : undefined;
    if (event.key === "F2") {
      event.preventDefault();
      beginRename(id);
      return;
    }
    if (to === undefined || event.altKey || event.ctrlKey || event.metaKey) return;
    event.preventDefault();
    event.stopPropagation();
    showSequence(to.id);
    focusTab(to.id);
  };
  const onDragStart = (event: DragEvent<HTMLElement>, id: SequenceId): void => {
    dragged.current = id;
    event.dataTransfer.setData(TAB_MIME, id);
    event.dataTransfer.effectAllowed = "move";
  };
  const onDragOver = (event: DragEvent<HTMLElement>): void => {
    if (dragged.current === null) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
  };
  const onDrop = (event: DragEvent<HTMLElement>, onto: SequenceId | null): void => {
    const id = dragged.current;
    dragged.current = null;
    if (id === null) return;
    event.preventDefault();
    if (onto === null) {
      moveTab(id, null);
      return;
    }
    // Dropped on a tab to its right: it lands after that tab; to its left: before it.
    const from = tabs.findIndex((s) => s.id === id);
    const to = tabs.findIndex((s) => s.id === onto);
    moveTab(id, from < to ? (tabs[to + 1]?.id ?? null) : onto);
  };

  return (
    <div className="cw-seqtabs" ref={surface}>
      {/* A tab dropped past the last one goes to the end; the tab menu's Move left and Move right
          do the same from the keyboard. */}
      <div
        ref={listRef}
        className="cw-tabs cw-seqtabs__list"
        role="tablist"
        aria-label={t("sequences.tabsLabel")}
        onDragOver={onDragOver}
        onDrop={(event) => onDrop(event, null)}
      >
        {tabs.map((tab) => {
          const selected = tab.id === active;
          return (
            // biome-ignore lint/a11y/noStaticElementInteractions: the wrapper only takes the middle click and the drop; the tab inside has the keys, and the tab menu closes and moves from the keyboard
            <div
              key={tab.id}
              role="presentation"
              className="cw-seqtabs__tab"
              data-sequence={tab.id}
              data-active={selected}
              onAuxClick={(event) => {
                if (event.button === 1) closeTab(tab.id);
              }}
              onDragOver={onDragOver}
              onDrop={(event) => {
                event.stopPropagation();
                onDrop(event, tab.id);
              }}
            >
              {renaming === tab.id ? (
                <InlineRename
                  value={tab.name}
                  label={t("sequences.renameLabel")}
                  className="cw-seqtabs__rename"
                  onCommit={(name) => {
                    sequenceUiStore.getState().endRename();
                    void renameSequence(tab.id, name);
                  }}
                  onCancel={() => sequenceUiStore.getState().endRename()}
                />
              ) : (
                <button
                  type="button"
                  role="tab"
                  tabIndex={selected ? 0 : -1}
                  aria-selected={selected}
                  className="cw-tabs__tab label"
                  draggable
                  onClick={() => showSequence(tab.id)}
                  onDoubleClick={() => beginRename(tab.id)}
                  onKeyDown={(event) => onKeyDown(event, tab.id)}
                  onDragStart={(event) => onDragStart(event, tab.id)}
                  onDragEnd={() => {
                    dragged.current = null;
                  }}
                >
                  {tab.name}
                </button>
              )}
              {tabs.length > 1 && renaming !== tab.id ? (
                // A pointer shortcut only: the tab menu's Close does the same from the keyboard.
                <Tooltip label={t("sequences.close")}>
                  <IconButton
                    icon="x"
                    size="sm"
                    label={t("sequences.close")}
                    className="cw-seqtabs__close"
                    tabIndex={-1}
                    aria-hidden="true"
                    onClick={() => closeTab(tab.id)}
                  />
                </Tooltip>
              ) : null}
            </div>
          );
        })}
      </div>
      <Menu mode="dropdown" items={addItems} aria-label={t("sequences.addMenu")}>
        <Tooltip label={t("sequences.addMenu")}>
          <IconButton
            icon="plus"
            size="sm"
            label={t("sequences.addMenu")}
            data-testid="sequence-add"
          />
        </Tooltip>
      </Menu>
    </div>
  );
});
