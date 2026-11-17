/**
 * The three performance numbers on the home page.
 * Keep `measuredOn` in step with docs/launch/plan.md and docs/performance.md (on the perf-gate branch): the figures come
 * from the development laptop, and the CI perf gate holds the same limits on GitHub's standard 4-core runners.
 */
export const PERFORMANCE = {
  measuredOn: "Measured on a high-end laptop. CI enforces the same limits on standard 4-core cloud machines.",
  stats: [
    { value: "0.1", unit: "ms", label: "to decide whether a word needs fixing" },
    { value: "~20", unit: "MB", label: "of memory while it waits in the background" },
    { value: "0", unit: "%", label: "CPU when you are not typing" },
  ],
} as const;
