/** Shown as Careful (both), Balanced, Phone-like (aggressive); the single-engine modes are under Advanced. */
export type EngineMode = "both" | "system-only" | "bundled-only" | "balanced" | "aggressive";
export type PillStyle = "underline-capsule" | "capsule" | "underline" | "off";
export type PillPosition = "above" | "below";
export type Theme = "unflub" | "paper" | "ink" | "system";
export type Appearance = "auto" | "light" | "dark";
export type Channel = "stable" | "beta";
export type UndoMode = "twice" | "once" | "off";

/**
 * Mirrors `Settings` in src-tauri/src/settings.rs (camelCase). Fields this version doesn't know
 * (`schemaVersion`, ones from a newer unflub) ride along untouched because saves spread the
 * loaded object.
 */
export interface Settings {
  enabled: boolean;
  /** Windows shortcut. Empty means off. */
  pauseShortcut: string;
  startAtLogin: boolean;
  /** Download updates in the background. Installing waits for a restart or quit. */
  autoUpdate: boolean;
  channel: Channel;
  /** Daily anonymous counts; on by default. */
  shareUsageStats: boolean;
  /** Scrubbed crash reports without asking; on by default. */
  sendCrashReports: boolean;
  /** The Stats page keeps which words were fixed. */
  statsKeepWords: boolean;
  /** The welcome screen was closed. */
  onboarded: boolean;
  mode: EngineMode;
  confidence: number;
  minWordLen: number;
  skipCode: boolean;
  /** What Backspace right after a fix does. */
  undoMode: UndoMode;
  /** Stop fixing a typo after this many undos of the same fix; 0 never. */
  learnAfterUndos: number;
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
  /** English dictionary fixes even when `language` isn't English. */
  englishForOtherLanguages: boolean;
}

export interface PauseControls {
  paused: boolean;
  shortcutUnavailable: boolean;
}

/** Mirrors `UpdateStatus` in src-tauri/src/updates.rs. */
export type UpdateStatus =
  | { state: "idle" | "checking" | "up-to-date" }
  | { state: "updated"; version: string }
  | { state: "available" | "ready"; version: string; notes: string; critical: boolean }
  | { state: "downloading" | "installing"; version: string }
  | { state: "error"; phase: "check" | "install"; message: string; retry: boolean };

export interface SystemLook {
  accent: string | null;
  platform: string;
}

/** Mirrors `Day` in src-tauri/src/stats.rs. */
export interface StatsDay {
  corrections: number;
  undos: number;
  wordsChecked: number;
  keystrokesSaved: number;
}

/** Mirrors `StatsView` in src-tauri/src/stats.rs. */
export interface StatsView {
  today: StatsDay;
  week: StatsDay;
  allTime: StatsDay;
  /** Fixes per day, oldest first, today last. */
  last30: number[];
  topWords: { typo: string; fix: string; count: number }[];
  daysCounted: number;
}

/** Mirrors `InstallSource` in src-tauri/src/install.rs. */
export type InstallSource = "installer" | "winget" | "chocolatey" | "scoop" | "store" | "portable";

/** Mirrors `AppInfo` in src-tauri/src/lib.rs and `Managed` in managed.rs. */
export interface AppInfo {
  installSource: InstallSource;
  /** Runs from an installed location, so start at login is available. */
  loginAllowed: boolean;
  managed: {
    disableUpdates: boolean;
    disableUsageStats: boolean;
    disableCrashReports: boolean;
    disableRemoteConfig: boolean;
  };
}

/** Mirrors `TourAction` in src-tauri/src/tour.rs. */
export type TourAction =
  | { kind: "pass" }
  | { kind: "replace" | "undo"; delete: number; insert: string; original: string; corrected: string };

/** Mirrors `RemoteMessage` in src-tauri/src/remote.rs. */
export interface RemoteMessage {
  id: string;
  text: string;
  url: string | null;
}

export type CrashAnswer = "send" | "dont-send" | "always-send";

/** The longest entries the window accepts. Keep in step with MAX_*_CHARS in src-tauri/src/settings.rs
 * (a test there reads this line). */
export const LIMITS = { word: 64, replacement: 256, app: 260 } as const;

export const defaultSettings: Settings = {
  enabled: true,
  pauseShortcut: "",
  startAtLogin: true,
  autoUpdate: true,
  channel: "stable",
  shareUsageStats: true,
  sendCrashReports: true,
  statsKeepWords: true,
  onboarded: false,
  mode: "both",
  confidence: 0.6,
  minWordLen: 2,
  skipCode: true,
  undoMode: "twice",
  learnAfterUndos: 2,
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
  englishForOtherLanguages: false,
};
