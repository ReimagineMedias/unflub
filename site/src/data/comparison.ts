/** Comparison table. Text only, no logos. "unknown" means we could not find it documented. */
export type Mark = "yes" | "no" | "partial" | "unknown";
export interface Cell { mark: Mark; note?: string }

export const COLUMNS = [
  "Fixes as you type, in every app",
  "Works offline",
  "Free",
  "Open source",
  "Leaves code alone",
  "Undo with Backspace",
] as const;

export const ROWS: { name: string; cells: Cell[]; ours?: boolean }[] = [
  {
    name: "unflub",
    ours: true,
    cells: [
      { mark: "yes" },
      { mark: "yes", note: "Sends usage counts, never text" },
      { mark: "yes" },
      { mark: "yes", note: "MIT" },
      { mark: "yes" },
      { mark: "yes" },
    ],
  },
  {
    name: "Windows typing settings",
    cells: [
      { mark: "partial", note: "Only in apps that use Windows text services" },
      { mark: "yes" },
      { mark: "yes", note: "Built in" },
      { mark: "no" },
      { mark: "unknown" },
      { mark: "unknown" },
    ],
  },
  {
    name: "Grammarly desktop app",
    cells: [
      { mark: "partial", note: "Suggests fixes for you to accept" },
      { mark: "no", note: "Checks text online" },
      { mark: "partial", note: "Free plan, paid tiers" },
      { mark: "no" },
      { mark: "unknown" },
      { mark: "unknown" },
    ],
  },
  {
    name: "Phone keyboards",
    cells: [
      { mark: "partial", note: "On the phone, not your computer" },
      { mark: "yes" },
      { mark: "yes" },
      { mark: "no", note: "Most are closed" },
      { mark: "no" },
      { mark: "yes" },
    ],
  },
];
