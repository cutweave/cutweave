// SPDX-License-Identifier: GPL-3.0-or-later
import { useState } from "react";
import { Button } from "./Button.tsx";
import { Dialog, DialogNote } from "./Dialog.tsx";
import { NumberScrubber } from "./NumberScrubber.tsx";

export default { title: "Overlays/Dialog", component: Dialog, tags: ["autodocs"] };

const stage = {
  padding: "var(--space-4)",
  minHeight: "var(--layer-width-md)",
  background: "var(--surface-0)",
} as const;

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="cw-row">
      <span className="cw-row__label label">{label}</span>
      {children}
    </div>
  );
}

function ExportDialog({ initiallyOpen }: { initiallyOpen: boolean }) {
  const [open, setOpen] = useState(initiallyOpen);
  const [width, setWidth] = useState(1080);
  const [height, setHeight] = useState(1920);
  const [fps, setFps] = useState(23.976);
  const [bitrate, setBitrate] = useState(12);
  return (
    <div style={stage}>
      <Dialog
        open={open}
        onOpenChange={setOpen}
        title="Export video"
        onSubmit={() => setOpen(false)}
        trigger={<Button>Export…</Button>}
        footer={
          <>
            <Button variant="ghost" size="lg" onClick={() => setOpen(false)}>
              Cancel
            </Button>
            <Button variant="primary" size="lg" onClick={() => setOpen(false)}>
              Export
            </Button>
          </>
        }
      >
        <Row label="Resolution">
          <span style={{ display: "inline-flex", gap: "var(--space-1)" }}>
            <NumberScrubber
              aria-label="Width"
              value={width}
              unit="px"
              min={16}
              onChange={setWidth}
            />
            <NumberScrubber
              aria-label="Height"
              value={height}
              unit="px"
              min={16}
              onChange={setHeight}
            />
          </span>
        </Row>
        <Row label="Frame rate">
          <NumberScrubber
            aria-label="Frame rate"
            value={fps}
            unit="fps"
            min={1}
            onChange={setFps}
          />
        </Row>
        <Row label="Bitrate">
          <NumberScrubber
            aria-label="Bitrate"
            value={bitrate}
            unit="Mbps"
            min={1}
            onChange={setBitrate}
          />
        </Row>
        <DialogNote>Renders locally. Free (local). Est. 00:00:58 to Shorts_final.mp4</DialogNote>
      </Dialog>
    </div>
  );
}

export const Export = { render: () => <ExportDialog initiallyOpen /> };

/** Closed: the opener keeps focus on close. */
export const Closed = { render: () => <ExportDialog initiallyOpen={false} /> };

function ConfirmDelete() {
  const [open, setOpen] = useState(true);
  return (
    <div style={stage}>
      <Dialog
        open={open}
        onOpenChange={setOpen}
        title="Delete 3 original files?"
        description="This removes the files from disk. This can't be undone."
        footer={
          <>
            <Button variant="ghost" size="lg" onClick={() => setOpen(false)}>
              Cancel
            </Button>
            <Button variant="danger" size="lg" onClick={() => setOpen(false)}>
              Delete 3 files
            </Button>
          </>
        }
      >
        <DialogNote icon="triangle-alert">
          interview_A.mov, interview_B.mov and drone_03.mov
        </DialogNote>
      </Dialog>
    </div>
  );
}

export const DestructiveConfirm = { render: () => <ConfirmDelete /> };
