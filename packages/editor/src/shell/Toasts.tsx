// SPDX-License-Identifier: GPL-3.0-or-later
import { Toast, ToastProvider } from "@cutweave/ui";
import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { WindowLayers } from "../dock/panelWindow.tsx";
import { useContextMenu } from "../menus/host.tsx";
import { type Notice, useToastStore } from "../state/toastStore.ts";
import { onDidAddWindow, onDidRemoveWindow, registeredWindows } from "./windows.ts";

/** One notice. Right-click or the Menu key: Copy message, Dismiss, Dismiss all (WP-2H.13). */
function NoticeToast({ notice, onDismiss }: { notice: Notice; onDismiss: () => void }) {
  const ref = useContextMenu<HTMLLIElement>("chrome.toast", () => ({
    ids: [String(notice.id)],
    data: { text: [notice.title, notice.message].filter(Boolean).join("\n") },
  }));
  return (
    <Toast
      open
      rootRef={ref}
      tone={notice.tone}
      title={notice.title}
      message={notice.message}
      actions={notice.actions}
      duration={notice.duration}
      onOpenChange={(open) => {
        if (!open) onDismiss();
      }}
    />
  );
}

/** The detached panels' windows that are open now, kept current as they open and close. */
function useOpenPopouts(): readonly Window[] {
  const read = () =>
    registeredWindows()
      .filter((w) => w.role === "popout")
      .map((w) => w.window);
  const [wins, setWins] = useState(read);
  useEffect(() => {
    const update = () => setWins(read());
    update();
    const offAdd = onDidAddWindow(update);
    const offRemove = onDidRemoveWindow(update);
    return () => {
      offAdd();
      offRemove();
    };
  }, []);
  return wins;
}

/** The notices raised from one detached window, in a viewport of its own in that window. */
function PopoutToasts({
  win,
  notices,
  onDismiss,
}: {
  win: Window;
  notices: readonly Notice[];
  onDismiss: (id: number) => void;
}) {
  const { t } = useTranslation();
  return createPortal(
    <WindowLayers win={win}>
      <ToastProvider label={t("app.notifications")}>
        {notices.map((n) => (
          <NoticeToast key={n.id} notice={n} onDismiss={() => onDismiss(n.id)} />
        ))}
      </ToastProvider>
    </WindowLayers>,
    win.document.body,
  );
}

/**
 * The notices of `toastStore`; mount once inside the shell's `ToastProvider`. A notice raised
 * while the person worked in a detached panel's window shows in that window; the others, and
 * those of a window that has closed, show in the main window.
 */
export function Toasts() {
  const notices = useToastStore((s) => s.notices);
  const dismiss = useToastStore((s) => s.dismiss);
  const popouts = useOpenPopouts();
  const inMain = notices.filter((n) => n.window === undefined || !popouts.includes(n.window));
  return (
    <>
      {inMain.map((n) => (
        <NoticeToast key={n.id} notice={n} onDismiss={() => dismiss(n.id)} />
      ))}
      {popouts.map((win, i) => (
        <PopoutToasts
          key={win.name || String(i)}
          win={win}
          notices={notices.filter((n) => n.window === win)}
          onDismiss={dismiss}
        />
      ))}
    </>
  );
}
