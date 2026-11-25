// SPDX-License-Identifier: GPL-3.0-or-later
import { Toast } from "@cutweave/ui";
import { useContextMenu } from "../menus/host.tsx";
import { type Notice, useToastStore } from "../state/toastStore.ts";

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

/** The notices of `toastStore`; mount once inside the shell's `ToastProvider`. */
export function Toasts() {
  const notices = useToastStore((s) => s.notices);
  const dismiss = useToastStore((s) => s.dismiss);
  return (
    <>
      {notices.map((n) => (
        <NoticeToast key={n.id} notice={n} onDismiss={() => dismiss(n.id)} />
      ))}
    </>
  );
}
