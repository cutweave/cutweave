// SPDX-License-Identifier: GPL-3.0-or-later
import { Toast, ToastProvider } from "@cutweave/ui";
import { useTranslation } from "react-i18next";
import { useToastStore } from "../state/toastStore.ts";

/** The toast viewport and the notices of `toastStore` (mounted once by the shell). */
export function Toasts() {
  const { t } = useTranslation();
  const notices = useToastStore((s) => s.notices);
  const dismiss = useToastStore((s) => s.dismiss);
  return (
    <ToastProvider label={t("toasts.label")}>
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
    </ToastProvider>
  );
}
