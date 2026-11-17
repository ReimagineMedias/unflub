import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { defaultSettings, type Settings, type SystemLook, type UpdateStatus } from "./types";

/** True inside the Tauri shell; false when the UI runs in a plain browser for previews. */
export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

let mock: Settings = { ...defaultSettings, personalWords: ["rmgn", "Tauri"], userRules: [["omw", "on my way"]] };

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
  mock = structuredClone(defaultSettings);
  return structuredClone(mock);
}

export async function systemLook(): Promise<SystemLook> {
  return inTauri ? invoke<SystemLook>("system_look") : { accent: "#0078D4", platform: "windows" };
}

export async function appVersion(): Promise<string> {
  return inTauri ? getVersion() : "preview";
}

export async function updateStatus(): Promise<UpdateStatus> {
  return inTauri ? invoke<UpdateStatus>("update_status") : { state: "idle" };
}

/** Checks now. A newer version comes back as "available"; nothing is downloaded yet. */
export async function checkForUpdates(): Promise<UpdateStatus> {
  return inTauri ? invoke<UpdateStatus>("check_for_updates") : { state: "up-to-date" };
}

/** Downloads, verifies and installs the available update. unflub closes and the installer opens it again. */
export async function installUpdate(): Promise<void> {
  if (inTauri) await invoke("install_update");
}

/** The update status changed (background check, install, failure). Returns an unsubscribe function. */
export async function onUpdateStatus(cb: (s: UpdateStatus) => void): Promise<() => void> {
  if (!inTauri) return () => {};
  return listen<UpdateStatus>("update-status", (e) => cb(e.payload));
}

/** Settings changed elsewhere (tray toggle, another window). Returns an unsubscribe function. */
export async function onSettingsChanged(cb: (s: Settings) => void): Promise<() => void> {
  if (!inTauri) return () => {};
  return listen<Settings>("settings-changed", (e) => cb(e.payload));
}
