import { useCallback, useEffect, useRef, useState } from "react";
import {
  appVersion, checkForUpdates, getSettings, inTauri, installUpdate, onSettingsChanged, onUpdateStatus, resetSettings,
  setSettings, systemLook, updateStatus,
} from "./api";
import { AppIcon, ChipList, Logo, NavIcon, Row, RuleList, Segmented, Slider, Switch } from "./components";
import { PillPreview } from "./PillPreview";
import type { Settings, SystemLook, Theme, UpdateStatus } from "./types";

type Page = "general" | "correction" | "appearance" | "apps" | "dictionary" | "about";
const PAGES: { id: Page; label: string }[] = [
  { id: "general", label: "General" },
  { id: "correction", label: "Correction" },
  { id: "appearance", label: "Appearance" },
  { id: "apps", label: "Apps" },
  { id: "dictionary", label: "Dictionary" },
  { id: "about", label: "About" },
];

const THEMES: { id: Theme; name: string; note: string; swatch: string[]; bg: string }[] = [
  { id: "unflub", name: "unflub", note: "default", swatch: ["#2D1916", "#79453A", "#B06A5C"], bg: "#F2E6E3" },
  { id: "paper", name: "Paper", note: "rmgn", swatch: ["#292827", "#4e4843", "#a77a55"], bg: "#e7e2da" },
  { id: "ink", name: "Quiet ink", note: "dark", swatch: ["#ededef", "#8a8f98", "#c69c72"], bg: "#121214" },
  { id: "system", name: "System", note: "os accent", swatch: [], bg: "linear-gradient(135deg,#dfe7f2,#ece6f0)" },
];

function updateHint(u: UpdateStatus, auto: boolean): string {
  switch (u.state) {
    case "checking": return "Checking…";
    case "up-to-date": return "You have the latest version.";
    case "error": return `Couldn't check: ${u.message}`;
    default: return auto ? "unflub checks once a day." : "Automatic checks are off.";
  }
}

const initialPage = (): Page => {
  const p = new URLSearchParams(window.location.search).get("page");
  return PAGES.some((x) => x.id === p) ? (p as Page) : "general";
};

function useResolvedMode(appearance: Settings["appearance"] | undefined) {
  const query = "(prefers-color-scheme: dark)";
  const [systemDark, setSystemDark] = useState(() => window.matchMedia(query).matches);
  useEffect(() => {
    const mq = window.matchMedia(query);
    const on = (e: MediaQueryListEvent) => setSystemDark(e.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);
  if (appearance === "light") return "light";
  if (appearance === "dark") return "dark";
  return systemDark ? "dark" : "light";
}

export default function App() {
  const [settings, setLocal] = useState<Settings | null>(null);
  const [look, setLook] = useState<SystemLook>({ accent: null, platform: "windows" });
  const [page, setPage] = useState<Page>(initialPage);
  const [version, setVersion] = useState("");
  const [updateState, setUpdateState] = useState<UpdateStatus>({ state: "idle" });
  const saveTimer = useRef<number | undefined>(undefined);

  useEffect(() => {
    getSettings().then((s) => {
      // Preview hooks for screenshots in a plain browser: ?theme=ink&mode=dark
      const q = new URLSearchParams(window.location.search);
      const theme = q.get("theme") as Theme | null;
      const appearance = q.get("mode") as Settings["appearance"] | null;
      setLocal({ ...s, ...(theme && { theme }), ...(appearance && { appearance }) });
    });
    systemLook().then(setLook);
    appVersion().then(setVersion);
    updateStatus().then(setUpdateState);
    let off: (() => void) | undefined;
    let offUpdate: (() => void) | undefined;
    onSettingsChanged(setLocal).then((u) => (off = u));
    onUpdateStatus(setUpdateState).then((u) => (offUpdate = u));
    return () => {
      off?.();
      offUpdate?.();
    };
  }, []);

  const mode = useResolvedMode(settings?.appearance);
  useEffect(() => {
    const root = document.documentElement;
    if (!settings) return;
    root.dataset.theme = settings.theme;
    root.dataset.mode = mode;
    if (inTauri) root.dataset.tauri = "";
    if (look.accent) root.style.setProperty("--os-accent", look.accent);
  }, [settings, mode, look.accent]);

  // Optimistic local update, debounced persist so sliders don't write on every step.
  const update = useCallback((patch: Partial<Settings>) => {
    setLocal((prev) => {
      if (!prev) return prev;
      const next = { ...prev, ...patch };
      window.clearTimeout(saveTimer.current);
      saveTimer.current = window.setTimeout(() => void setSettings(next), 200);
      return next;
    });
  }, []);

  if (!settings) return null;
  const s = settings;
  const systemName = look.platform === "macos" ? "macOS" : look.platform === "linux" ? "System" : "Windows";
  const capsule = s.pillStyle === "capsule" || s.pillStyle === "underline-capsule";

  return (
    <div className="shell">
      <nav className="nav" aria-label="Settings sections">
        <div className="brand"><Logo />unflub</div>
        {PAGES.map((p) => (
          <button key={p.id} aria-current={page === p.id ? "page" : undefined} onClick={() => setPage(p.id)}>
            <NavIcon name={p.id} />
            {p.label}
          </button>
        ))}
        <div className="spacer" />
        <div className={s.enabled ? "status" : "status off"}>
          <span className="dot" />
          {s.enabled ? "Fixing typos" : "Paused"}
        </div>
      </nav>

      <main>
        {page === "general" && (
          <>
            <h1>General</h1>
            <p className="lead">unflub lives in your tray and fixes words the moment you finish them, in any app.</p>
            <div className="card">
              <Row label="Autocorrect" hint="Also in the tray menu. While paused, unflub doesn't read your keys at all.">
                <Switch label="Autocorrect" checked={s.enabled} onChange={(v) => update({ enabled: v })} />
              </Row>
              <Row label="Start at login" hint="Starts quietly in the tray.">
                <Switch label="Start at login" checked={s.startAtLogin} onChange={(v) => update({ startAtLogin: v })} />
              </Row>
              <Row label="Undo teaches unflub" hint="Backspace right after a fix restores your word and stops fixing it.">
                <Switch label="Undo teaches unflub" checked={s.undoLearns} onChange={(v) => update({ undoLearns: v })} />
              </Row>
            </div>
          </>
        )}

        {page === "correction" && (
          <>
            <h1>Correction</h1>
            <p className="lead">Decide who spots a typo and how sure unflub must be before it changes a word.</p>
            <div className="card">
              <Row stack label="Engine" hint={
                s.mode === "both" ? `${systemName} spots the typo, unflub's own ranking picks the fix. Fewest wrong fixes.`
                : s.mode === "system-only" ? `Only words ${systemName} underlines in red get fixed.`
                : s.mode === "bundled-only" ? "unflub's built-in dictionary only. Works the same on every computer."
                : "Fixes anything either engine flags, like a phone. More fixes, more surprises."
              }>
                <Segmented label="Engine" value={s.mode} onChange={(v) => update({ mode: v })} options={[
                  { value: "both", label: `${systemName} + unflub` },
                  { value: "system-only", label: `${systemName} only` },
                  { value: "bundled-only", label: "unflub only" },
                  { value: "aggressive", label: "Aggressive" },
                ]} />
              </Row>
              <Row label="Confidence" hint="Cautious leaves ambiguous words alone. Eager fixes more.">
                <Slider label="Confidence" min={0} max={1} step={0.05} value={s.confidence}
                  onChange={(v) => update({ confidence: v })}
                  format={(v) => (v < 0.35 ? "eager" : v > 0.75 ? "cautious" : "balanced")} />
              </Row>
              <Row label="Shortest word to fix" hint="Very short words are often intentional.">
                <Slider label="Shortest word" min={1} max={6} step={1} value={s.minWordLen}
                  onChange={(v) => update({ minWordLen: v })} format={(v) => `${v} letter${v === 1 ? "" : "s"}`} />
              </Row>
              <Row label="Leave code alone" hint="Skips camelCase, snake_case, paths, links and anything in `backticks`.">
                <Switch label="Leave code alone" checked={s.skipCode} onChange={(v) => update({ skipCode: v })} />
              </Row>
            </div>
          </>
        )}

        {page === "appearance" && (
          <>
            <h1>Appearance</h1>
            <p className="lead">How unflub looks, and how it tells you it fixed something.</p>
            <div className="section-title">Theme</div>
            <div className="themes">
              {THEMES.map((t) => (
                <button key={t.id} className="theme-card" aria-pressed={s.theme === t.id} onClick={() => update({ theme: t.id })}>
                  <div className="swatch" style={{ background: t.bg }}>
                    {(t.id === "system" ? [look.accent ?? "#005fb8", "#1b1b1b", "#ffffff"] : t.swatch).map((c) => <i key={c} style={{ background: c }} />)}
                  </div>
                  <div className="name">{t.name}<small>{t.note}</small></div>
                </button>
              ))}
            </div>
            <div className="card" style={{ marginTop: 14 }}>
              <Row label="Light or dark">
                <Segmented label="Light or dark" value={s.appearance} onChange={(v) => update({ appearance: v })} options={[
                  { value: "auto", label: "Match system" }, { value: "light", label: "Light" }, { value: "dark", label: "Dark" },
                ]} />
              </Row>
            </div>

            <div className="section-title">After a fix</div>
            <div className="card">
              <Row stack label="Show" hint="The capsule never takes focus, and clicks pass through it.">
                <Segmented label="Show after a fix" value={s.pillStyle} onChange={(v) => update({ pillStyle: v })} options={[
                  { value: "underline-capsule", label: "Underline + capsule" },
                  { value: "capsule", label: "Capsule" },
                  { value: "underline", label: "Underline" },
                  { value: "off", label: "Nothing" },
                ]} />
                <PillPreview style={s.pillStyle} position={s.pillPosition} opacity={s.pillOpacity} durationMs={s.pillDurationMs} />
              </Row>
              {capsule && (
                <>
                  <Row label="Position">
                    <Segmented label="Position" value={s.pillPosition} onChange={(v) => update({ pillPosition: v })} options={[
                      { value: "above", label: "Above the word" }, { value: "below", label: "Below" },
                    ]} />
                  </Row>
                  <Row label="Opacity">
                    <Slider label="Opacity" min={0.3} max={1} step={0.02} value={s.pillOpacity}
                      onChange={(v) => update({ pillOpacity: v })} format={(v) => `${Math.round(v * 100)}%`} />
                  </Row>
                  <Row label="Stays for">
                    <Slider label="Stays for" min={400} max={3000} step={100} value={s.pillDurationMs}
                      onChange={(v) => update({ pillDurationMs: v })} format={(v) => `${(v / 1000).toFixed(1)} s`} />
                  </Row>
                </>
              )}
            </div>
          </>
        )}

        {page === "apps" && (
          <>
            <h1>Apps</h1>
            <p className="lead">unflub stays off in these apps. Password fields are always skipped.</p>
            <div className="card">
              <Row stack label="Never fix in" hint="The program's file name, like code.exe or windowsterminal.exe.">
                <ChipList items={s.excludedApps} onChange={(v) => update({ excludedApps: v })}
                  placeholder="app.exe" empty="No apps excluded." normalise={(v) => v.trim().toLowerCase()} />
              </Row>
            </div>
          </>
        )}

        {page === "dictionary" && (
          <>
            <h1>Dictionary</h1>
            <p className="lead">Words unflub should leave alone, and shortcuts it should expand.</p>
            <div className="card">
              <Row stack label="Your words" hint="Names, slang, product names. Words you undo land here too.">
                <ChipList items={s.personalWords} onChange={(v) => update({ personalWords: v })} placeholder="Add a word" empty="No words yet." />
              </Row>
              <Row stack label="Shortcuts" hint="Typed exactly, then replaced when you finish the word.">
                <RuleList rules={s.userRules} onChange={(v) => update({ userRules: v })} />
              </Row>
            </div>
          </>
        )}

        {page === "about" && (
          <>
            <h1>About</h1>
            <div className="card">
              <Row label={<span className="about"><AppIcon className="logo" /><span>unflub {version}<br /><small className="muted">Phone-style autocorrect for every text field.</small></span></span>}>
                <span />
              </Row>
              {(updateState.state === "available" || updateState.state === "installing") && (
                <Row label="Update available — restart to install" hint={`unflub ${updateState.version} is ready. unflub closes for a moment and opens again.`}>
                  <button className="btn" disabled={updateState.state === "installing"}
                    onClick={() => void installUpdate().catch((e) => setUpdateState({ state: "error", message: String(e) }))}>
                    {updateState.state === "installing" ? "Installing…" : "Restart"}
                  </button>
                </Row>
              )}
              <Row label="Automatic updates" hint="Checks for a new version once a day. Nothing installs until you restart.">
                <Switch label="Automatic updates" checked={s.autoUpdate} onChange={(v) => update({ autoUpdate: v })} />
              </Row>
              {updateState.state !== "available" && updateState.state !== "installing" && (
                <Row label="Check for updates" hint={updateHint(updateState, s.autoUpdate)}>
                  <button className="btn ghost" disabled={updateState.state === "checking"} onClick={() => void checkForUpdates().then(setUpdateState)}>
                    Check now
                  </button>
                </Row>
              )}
              <Row label="Private by design" hint="Nothing you type is stored or sent anywhere. The only connection unflub makes is the update check.">
                <span />
              </Row>
              <Row label="Open source" hint="MIT licensed. An RMGN product.">
                <a href="https://github.com/ReimagineMedias/unflub" target="_blank" rel="noreferrer">GitHub</a>
              </Row>
              <Row label="Reset settings" hint="Back to defaults. Your words and shortcuts are cleared.">
                <button className="btn ghost" onClick={() => void resetSettings().then(setLocal)}>Reset</button>
              </Row>
            </div>
          </>
        )}
      </main>
    </div>
  );
}
