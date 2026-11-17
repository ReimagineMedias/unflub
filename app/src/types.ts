export type EngineMode = "both" | "system-only" | "bundled-only" | "aggressive";
export type PillStyle = "underline-capsule" | "capsule" | "underline" | "off";
export type PillPosition = "above" | "below";
export type Theme = "unflub" | "paper" | "ink" | "system";
export type Appearance = "auto" | "light" | "dark";

/** Mirrors `Settings` in src-tauri/src/settings.rs (camelCase). */
export interface Settings {
  enabled: boolean;
  startAtLogin: boolean;
  mode: EngineMode;
  confidence: number;
  minWordLen: number;
  skipCode: boolean;
  undoLearns: boolean;
  excludedApps: string[];
  personalWords: string[];
  userRules: [string, string][];
  pillStyle: PillStyle;
  pillOpacity: number;
  pillDurationMs: number;
  pillPosition: PillPosition;
  theme: Theme;
  appearance: Appearance;
}

export interface SystemLook {
  accent: string | null;
  platform: string;
}

export const defaultSettings: Settings = {
  enabled: true,
  startAtLogin: true,
  mode: "both",
  confidence: 0.6,
  minWordLen: 2,
  skipCode: true,
  undoLearns: true,
  excludedApps: [],
  personalWords: [],
  userRules: [],
  pillStyle: "underline-capsule",
  pillOpacity: 0.92,
  pillDurationMs: 1100,
  pillPosition: "above",
  theme: "unflub",
  appearance: "auto",
};
