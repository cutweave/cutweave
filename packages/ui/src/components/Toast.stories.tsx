// SPDX-License-Identifier: GPL-3.0-or-later
import { useState } from "react";
import { Button } from "./Button.tsx";
import { Toast, type ToastProps, ToastProvider } from "./Toast.tsx";

export default { title: "Feedback/Toast", component: Toast, tags: ["autodocs"] };

type Spec = Omit<ToastProps, "open" | "onOpenChange"> & { id: string };

const SPECS: Spec[] = [
  {
    id: "success",
    tone: "success",
    title: "Export complete",
    message: "Exported 00:00:58 to Shorts_final.mp4",
    actions: [{ label: "Show in folder", onClick: () => {} }],
    duration: Number.POSITIVE_INFINITY,
  },
  {
    id: "warning",
    tone: "warning",
    title: "Media missing",
    message: "drone_03.mov is offline; 2 clips are unlinked.",
  },
  {
    id: "danger",
    tone: "danger",
    title: "Generation failed",
    message: "Kling 3.0 via fal returned a timeout. No cost was charged.",
    actions: [
      { label: "Retry", onClick: () => {} },
      { label: "View log", onClick: () => {}, variant: "ghost" },
    ],
  },
  {
    id: "job",
    tone: "job",
    title: "Exporting Shorts_final.mp4",
    message: (
      <>
        <span className="cw-mono numeric">64%</span> · about 20 s left
      </>
    ),
    progress: 64,
    onCancel: () => {},
    cancelLabel: "Cancel export",
  },
];

function Stack() {
  const [closed, setClosed] = useState<Record<string, boolean>>({});
  return (
    <ToastProvider>
      <div
        style={{
          padding: "var(--space-4)",
          minHeight: "var(--layer-width-lg)",
          background: "var(--surface-0)",
        }}
      >
        <Button onClick={() => setClosed({})}>Show all toasts</Button>
      </div>
      {SPECS.map(({ id, ...spec }) => (
        <Toast
          key={id}
          {...spec}
          open={!closed[id]}
          onOpenChange={(open) => setClosed((c) => ({ ...c, [id]: !open }))}
        />
      ))}
    </ToastProvider>
  );
}

/** All four tones stacked in the viewport; Escape dismisses the focused toast, F8 focuses the stack. */
export const Tones = { render: () => <Stack /> };

/** Long unbroken text (a file path) wraps inside the toast instead of overflowing it. */
export const LongPath = {
  render: () => (
    <ToastProvider>
      <div style={{ minHeight: "var(--layer-width-sm)", background: "var(--surface-0)" }} />
      <Toast
        tone="warning"
        title="Media missing: Interview_with_the_coffee_shop_owner_final_v3_color_graded.mov"
        message="Cutweave can't find E:\projects\cutweave-demo\media\2026-11-24_shoot\camera_A\Interview_with_the_coffee_shop_owner_final_v3_color_graded.mov. Relink it or remove its clips."
        duration={Number.POSITIVE_INFINITY}
        open
        onOpenChange={() => {}}
      />
    </ToastProvider>
  ),
};
