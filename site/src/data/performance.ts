/**
 * The three performance numbers on the home page.
 * Keep in step with docs/performance.md once it exists; these are the figures from our own runs.
 */
export const PERFORMANCE = {
  measuredOn: "Measured on a typical laptop running Windows 11.",
  stats: [
    { value: "0.1", unit: "ms", label: "to decide whether a word needs fixing" },
    { value: "~20", unit: "MB", label: "of memory while it waits in the background" },
    { value: "0", unit: "%", label: "CPU when you are not typing" },
  ],
} as const;
