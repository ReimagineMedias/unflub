import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { defaultSettings, type Settings, type SystemLook } from "./types";

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

/** Settings changed elsewhere (tray toggle, another window). Returns an unsubscribe function. */
export async function onSettingsChanged(cb: (s: Settings) => void): Promise<() => void> {
  if (!inTauri) return () => {};
  return listen<Settings>("settings-changed", (e) => cb(e.payload));
}
