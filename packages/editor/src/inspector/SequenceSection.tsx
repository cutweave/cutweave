// SPDX-License-Identifier: GPL-3.0-or-later
// The Sequence section of the Inspector (WP-2H.03b), shown when nothing is selected: the canvas
// size, frame rate and background of the active sequence. The form is the one Sequence settings
// shows (`sequences/SequenceForm.tsx`, WP-2B.03); here it lists an image fill only while the
// sequence has one, since choosing the image belongs to the dialog.
import type { SequenceFormat, SequenceId } from "@cutweave/protocol";
import { useTranslation } from "react-i18next";
import { SequenceForm } from "../sequences/SequenceForm.tsx";
import type { InspectorEditor } from "./editor.ts";
import { Section } from "./Section.tsx";

export { rateLabel } from "../sequences/SequenceForm.tsx";

export function SequenceSection({
  sequence,
  format,
  editor,
}: {
  readonly sequence: SequenceId;
  readonly format: SequenceFormat;
  readonly editor: InspectorEditor;
}) {
  const { t } = useTranslation();
  return (
    <Section id="sequence" label={t("inspector.sequence.title")}>
      <div data-testid="sequence-inspector">
        <SequenceForm
          sequence={sequence}
          format={format}
          editor={editor}
          label={t("inspector.sequence.title")}
        />
      </div>
    </Section>
  );
}
