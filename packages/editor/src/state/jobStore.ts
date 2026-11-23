// SPDX-License-Identifier: GPL-3.0-or-later
import type { JobId, JobInfo } from "@cutweave/protocol";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

export interface JobState {
  /** Every job the engine told us about, by ID. */
  jobs: Readonly<Record<JobId, JobInfo>>;
}

export interface JobActions {
  /** A `job.progress` event: the job's newest state. */
  applyProgress(info: JobInfo): void;
  /** Replaces everything with a `job.list` answer (after a connect or reconnect). */
  reset(jobs: readonly JobInfo[]): void;
}

export type JobStore = JobState & JobActions;

const FINAL: readonly JobInfo["state"][] = ["succeeded", "failed", "cancelled"];

/** Whether the job is still queued or running. */
export const isActive = (job: JobInfo): boolean => !FINAL.includes(job.state);

export function createJobStore() {
  return createStore<JobStore>()((set) => ({
    jobs: {},
    applyProgress: (info) => set((s) => ({ jobs: { ...s.jobs, [info.id]: info } })),
    reset: (jobs) => set({ jobs: Object.fromEntries(jobs.map((job) => [job.id, job])) }),
  }));
}

export const jobStore = createJobStore();

/** The unfinished jobs, in the order the engine first reported them. */
export function selectActiveJobs(state: JobState): JobInfo[] {
  return Object.values(state.jobs).filter(isActive);
}

export function useJobStore<T>(selector: (state: JobStore) => T): T {
  return useStore(jobStore, selector);
}
