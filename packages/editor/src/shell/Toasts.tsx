// SPDX-License-Identifier: GPL-3.0-or-later
import { Toast } from "@cutweave/ui";
import { useToastStore } from "../state/toastStore.ts";

/** The notices of `toastStore`; mount once inside the shell's `ToastProvider`. */
export function Toasts() {
  const notices = useToastStore((s) => s.notices);
  const dismiss = useToastStore((s) => s.dismiss);
  return (
    <>
      {notices.map((n) => (
        <Toast
          key={n.id}
          open
          tone={n.tone}
          title={n.title}
          message={n.message}
          onOpenChange={(open) => {
            if (!open) dismiss(n.id);
          }}
        />
      ))}
    </>
  );
}
