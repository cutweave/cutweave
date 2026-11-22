// SPDX-License-Identifier: GPL-3.0-or-later
import { JobList, JobProgress } from "./JobProgress.tsx";

export default { title: "Feedback/JobProgress", component: JobProgress, tags: ["autodocs"] };

export const States = {
  render: () => (
    <JobList aria-label="Background jobs" style={{ width: "var(--layer-width-lg)" }}>
      <JobProgress
        name="Exporting Shorts_final.mp4"
        state="running"
        progress={64}
        eta="20 s left"
        onCancel={() => {}}
      />
      <JobProgress
        name="Generating kling_sunrise_v2.mp4 (Kling 3.0 via fal)"
        state="running"
        progress={18}
        eta="about 1 min"
        onCancel={() => {}}
      />
      <JobProgress
        name="Transcribing interview_A.mov"
        state="queued"
        progress={0}
        eta="Waiting"
        onCancel={() => {}}
      />
      <JobProgress
        name="Rendering proxy for drone_03.mov"
        state="failed"
        progress={42}
        error="Failed: source file not found. Relink media, then retry."
        onRetry={() => {}}
      />
    </JobList>
  ),
};
