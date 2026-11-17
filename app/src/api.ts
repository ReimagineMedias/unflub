import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { ExternalUrl } from "./links";
import {
  defaultSettings, type AppInfo, type CrashAnswer, type RemoteMessage, type SendPreview, type Settings, type StatsView,
  type SystemLook, type TourAction, type UpdateStatus,
} from "./types";

/** True inside the Tauri shell; false when the UI runs in a plain browser for previews. */
export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

const query = () => new URLSearchParams(window.location.search);

let mock: Settings = {
  ...defaultSettings,
  onboarded: !query().has("welcome"),
  personalWords: ["rmgn", "Tauri"],
  userRules: [["omw", "on my way"]],
};

export async function getSettings(): Promise<Settings> {
  return inTauri ? invoke<Settings>("get_settings") : structuredClone(mock);
}

export async function setSettings(settings: Settings): Promise<Settings> {
  if (inTauri) return invoke<Settings>("set_settings", { settings });
  mock = structuredClone(settings);
  return structuredClone(mock);
}

export async function resetSettings(): Promise<Settings> {
  if (inTauri) return invoke<Settings>("reset_settings");
  mock = { ...structuredClone(defaultSettings), onboarded: mock.onboarded };
  return structuredClone(mock);
}

export async function systemLook(): Promise<SystemLook> {
  return inTauri ? invoke<SystemLook>("system_look") : { accent: "#0078D4", platform: "windows" };
}

export async function appVersion(): Promise<string> {
  return inTauri ? getVersion() : "preview";
}

const mockUpdate = (): UpdateStatus => {
  const v = query().get("update");
  const notes = "Fixes a crash when a window closes mid-word.\nThe pill follows the caret in more apps.";
  if (v === "ready" || v === "available") return { state: v, version: "0.2.0", notes, critical: query().has("critical") };
  return { state: "idle" };
};

export async function updateStatus(): Promise<UpdateStatus> {
  return inTauri ? invoke<UpdateStatus>("update_status") : mockUpdate();
}

/** Checks now. With automatic updates on, a newer version downloads and comes back as "ready". */
export async function checkForUpdates(): Promise<UpdateStatus> {
  return inTauri ? invoke<UpdateStatus>("check_for_updates") : { state: "up-to-date" };
}

/** Installs the update (downloading it first if needed). unflub closes and the installer opens it again. */
export async function installUpdate(): Promise<void> {
  if (inTauri) await invoke("install_update");
}

/** The update status changed (background check, download, install, failure). Returns an unsubscribe function. */
export async function onUpdateStatus(cb: (s: UpdateStatus) => void): Promise<() => void> {
  if (!inTauri) return () => {};
  return listen<UpdateStatus>("update-status", (e) => cb(e.payload));
}

/** Settings changed elsewhere (tray toggle, another window). Returns an unsubscribe function. */
export async function onSettingsChanged(cb: (s: Settings) => void): Promise<() => void> {
  if (!inTauri) return () => {};
  return listen<Settings>("settings-changed", (e) => cb(e.payload));
}

/** The tray asked the open window to show a page. */
export async function onOpenPage(cb: (page: string) => void): Promise<() => void> {
  if (!inTauri) return () => {};
  return listen<string>("open-page", (e) => cb(e.payload));
}

/** Opens one of our sites in the browser. The only way the window opens a link. */
export async function openExternal(url: ExternalUrl): Promise<void> {
  if (inTauri) await openUrl(url);
  else console.info("would open", url);
}

const mockPreview = (): SendPreview => ({
  stats: mock.shareUsageStats
    ? JSON.stringify({
      schema: 1, version: "0.1.0", target: "windows", period: "day",
      counters: { corrections: 42, undos: 3, suggestions_shown: 40 },
      settings: { engine: mock.mode, pill: mock.pillStyle, theme: mock.theme, appearance: mock.appearance,
        excluded_count: mock.excludedApps.length, personal_words_count: mock.personalWords.length,
        rules_count: mock.userRules.length, auto_update: mock.autoUpdate },
      perf: { decide_p99_us: 90, inject_p99_us: 400, idle_private_mb: 20 },
      health: { crashes: 0, hook_reinstalls: 0 },
    }, null, 2)
    : null,
  headers: [["User-Agent", "unflub/0.1.0"], ["X-OS-Version", "10.0.26300"], ["X-Rollout-Bucket", "37"], ["X-First", "day"]],
  crashReportsAvailable: true,
});

/** The payloads the next daily send would post, for the Privacy page. */
export async function sendPreview(): Promise<SendPreview> {
  return inTauri ? invoke<SendPreview>("send_preview") : mockPreview();
}

const mockStats: StatsView = {
  today: { corrections: 23, undos: 1, wordsChecked: 1840, keystrokesSaved: 96 },
  week: { corrections: 141, undos: 6, wordsChecked: 11230, keystrokesSaved: 590 },
  allTime: { corrections: 812, undos: 31, wordsChecked: 64110, keystrokesSaved: 3390 },
  last30: [12, 18, 9, 0, 0, 22, 25, 19, 30, 14, 8, 0, 17, 21, 26, 18, 20, 9, 0, 15, 24, 28, 31, 19, 12, 0, 16, 22, 27, 23],
  topWords: [
    { typo: "teh", fix: "the", count: 64 }, { typo: "adn", fix: "and", count: 41 }, { typo: "taht", fix: "that", count: 22 },
    { typo: "recieve", fix: "receive", count: 9 }, { typo: "wierd", fix: "weird", count: 6 },
  ],
  daysCounted: 30,
};

/** The local Stats page. Never sent anywhere. */
export async function statsView(): Promise<StatsView> {
  return inTauri ? invoke<StatsView>("stats_view") : structuredClone(mockStats);
}

export async function clearStats(): Promise<StatsView> {
  if (inTauri) return invoke<StatsView>("clear_stats");
  const zero = { corrections: 0, undos: 0, wordsChecked: 0, keystrokesSaved: 0 };
  return { today: zero, week: zero, allTime: zero, last30: Array<number>(30).fill(0), topWords: [], daysCounted: 0 };
}

/** A crash report from the last run is waiting for an answer. */
export async function crashReportPending(): Promise<boolean> {
  return inTauri ? invoke<boolean>("crash_report_pending") : query().has("crash");
}

export async function answerCrashReport(answer: CrashAnswer): Promise<void> {
  if (inTauri) await invoke("answer_crash_report", { answer });
}

export async function onCrashReport(cb: (pending: boolean) => void): Promise<() => void> {
  if (!inTauri) return () => {};
  return listen<boolean>("crash-report", (e) => cb(e.payload));
}

/** A note from unflub's server (remote config), if there is one to show. */
export async function remoteMessage(): Promise<RemoteMessage | null> {
  return inTauri ? invoke<RemoteMessage | null>("remote_message") : null;
}

export async function dismissMessage(id: string): Promise<void> {
  if (inTauri) await invoke("dismiss_message", { id });
}

export async function onRemoteMessage(cb: (m: RemoteMessage | null) => void): Promise<() => void> {
  if (!inTauri) return () => {};
  return listen<RemoteMessage | null>("remote-message", (e) => cb(e.payload));
}

const mockInfo = (): AppInfo => {
  const managed = query().has("managed");
  return {
    installSource: (query().get("source") as AppInfo["installSource"] | null) ?? "installer",
    loginAllowed: !query().has("portable"),
    managed: { disableUpdates: managed, disableUsageStats: managed, disableCrashReports: managed, disableRemoteConfig: managed },
  };
};

/** Where this copy came from (who updates it) and any machine policy. */
export async function appInfo(): Promise<AppInfo> {
  return inTauri ? invoke<AppInfo>("app_info") : mockInfo();
}

/** A browser-preview stand-in for the tour's engine: three typos, undo, and remembering. */
const mockTour = (() => {
  const fixes: Record<string, string> = { teh: "the", adn: "and", recieve: "receive", taht: "that" };
  let word = "";
  let learned = new Set<string>();
  let last: { original: string; corrected: string; boundary: string } | null = null;
  return {
    reset() { word = ""; learned = new Set(); last = null; },
    key(key: string): TourAction {
      const undo = last;
      last = null;
      if (key === "Backspace") {
        if (undo) {
          learned.add(undo.original.toLowerCase());
          return { kind: "undo", delete: undo.corrected.length + 1, insert: undo.original + undo.boundary, ...undo };
        }
        word = word.slice(0, -1);
        return { kind: "pass" };
      }
      if (/^[\p{L}\p{N}'-]$/u.test(key)) { word += key; return { kind: "pass" }; }
      const typed = word;
      word = "";
      const fix = fixes[typed.toLowerCase()];
      if (!fix || learned.has(typed.toLowerCase())) return { kind: "pass" };
      last = { original: typed, corrected: fix, boundary: key };
      return { kind: "replace", delete: typed.length, insert: fix + key, original: typed, corrected: fix };
    },
  };
})();

/** A fresh practice box for the tour. */
export async function tourReset(): Promise<void> {
  if (inTauri) await invoke("tour_reset");
  else mockTour.reset();
}

/** One key typed in the tour's practice box: what to do with it. */
export async function tourKey(key: string): Promise<TourAction> {
  return inTauri ? invoke<TourAction>("tour_key", { key }) : mockTour.key(key);
}
