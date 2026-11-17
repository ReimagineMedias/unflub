/** Comparison table. Text only, no logos. "unknown" means we could not find it documented.
 *  Rows marked `more` appear only on /alternatives/; the home table keeps the shorter list. */
export type Mark = "yes" | "no" | "partial" | "unknown";
export interface Cell { mark: Mark; note?: string }

export const COLUMNS = [
  "Fixes as you type, in every app",
  "Works offline",
  "Free",
  "Open source",
  "Leaves code alone",
  "Undo with Backspace",
  "Checks grammar and tone",
] as const;

export const ROWS: { name: string; cells: Cell[]; ours?: boolean; more?: boolean }[] = [
  {
    name: "unflub",
    ours: true,
    cells: [
      { mark: "yes" },
      { mark: "yes", note: "Updates and anonymous stats need a connection" },
      { mark: "yes" },
      { mark: "yes", note: "MIT" },
      { mark: "yes" },
      { mark: "yes" },
      { mark: "no", note: "Typos only" },
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
      { mark: "no" },
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
      { mark: "yes" },
    ],
  },
  {
    name: "tinySpell",
    more: true,
    cells: [
      { mark: "partial", note: "Beeps at a typo in any app; you pick the fix" },
      { mark: "yes" },
      { mark: "partial", note: "Free, or tinySpell+ from $10" },
      { mark: "no" },
      { mark: "unknown" },
      { mark: "no" },
      { mark: "no" },
    ],
  },
  {
    name: "AutoHotkey AutoCorrect script",
    more: true,
    cells: [
      { mark: "partial", note: "Only the misspellings on its list" },
      { mark: "yes" },
      { mark: "yes" },
      { mark: "partial", note: "Plain-text script, no stated licence" },
      { mark: "no", note: "A listed word is fixed anywhere" },
      { mark: "no" },
      { mark: "no" },
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
      { mark: "no" },
    ],
  },
];
