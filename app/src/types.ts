export type EngineMode = "both" | "system-only" | "bundled-only" | "aggressive";
export type PillStyle = "underline-capsule" | "capsule" | "underline" | "off";
export type PillPosition = "above" | "below";
export type Theme = "unflub" | "paper" | "ink" | "system";
export type Appearance = "auto" | "light" | "dark";

/** Mirrors `Settings` in src-tauri/src/settings.rs (camelCase). */
export interface Settings {
  enabled: boolean;
  startAtLogin: boolean;
  /** Check for updates once a day and download them quietly. Installing waits for the user. */
  autoUpdate: boolean;
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
  /** BCP 47 tag for the system spell checker, like "en-US". */
  language: string;
}

/** Mirrors `UpdateStatus` in src-tauri/src/updates.rs. */
export type UpdateStatus =
  | { state: "idle" | "checking" | "up-to-date" }
  | { state: "available" | "installing"; version: string }
  | { state: "error"; message: string };

export interface SystemLook {
  accent: string | null;
  platform: string;
}

export const defaultSettings: Settings = {
  enabled: true,
  startAtLogin: true,
  autoUpdate: true,
  mode: "both",
  confidence: 0.6,
  minWordLen: 2,
  skipCode: true,
  undoLearns: true,
  // Keep identical to DEFAULT_EXCLUDED_APPS in src-tauri/src/settings.rs (its test pins the list).
  excludedApps: [
    "windowsterminal.exe", "openconsole.exe", "conhost.exe", "cmd.exe", "powershell.exe", "pwsh.exe", "wsl.exe",
    "mintty.exe", "wezterm-gui.exe", "alacritty.exe", "keepass.exe", "keepassxc.exe", "1password.exe", "bitwarden.exe",
  ],
  personalWords: [],
  userRules: [],
  pillStyle: "underline-capsule",
  pillOpacity: 0.92,
  pillDurationMs: 1100,
  pillPosition: "above",
  theme: "unflub",
  appearance: "auto",
  language: "en-US",
};
