import { useCallback, useEffect, useRef, useState } from "react";
import {
  answerCrashReport, appInfo, appVersion, checkForUpdates, crashReportPending, dismissMessage, getSettings, inTauri,
  inputInterference, onInputInterference,
  installUpdate, onCrashReport, onOpenPage, onRemoteMessage, onSettingsChanged, onUpdateStatus, remoteMessage,
  resetSettings, setSettings, systemLook, updateStatus,
} from "./api";
import { AppIcon, ChipList, ExternalLink, Logo, NavIcon, PowerSwitch, Row, RuleList, Segmented, Slider, Switch } from "./components";
import { t, type StringKey } from "./i18n";
import { LINKS, isExternalUrl } from "./links";
import { PillPreview } from "./PillPreview";
import { PrivacyPage } from "./PrivacyPage";
import { StatsPage } from "./StatsPage";
import { Tour } from "./Tour";
import type { AppInfo, EngineMode, RemoteMessage, Settings, SystemLook, Theme, UndoMode, UpdateStatus } from "./types";

type Page = "general" | "correction" | "appearance" | "apps" | "dictionary" | "stats" | "privacy" | "about";
const PAGES: { id: Page; label: StringKey }[] = [
  { id: "general", label: "nav.general" },
  { id: "correction", label: "nav.correction" },
  { id: "appearance", label: "nav.appearance" },
  { id: "apps", label: "nav.apps" },
  { id: "dictionary", label: "nav.dictionary" },
  { id: "stats", label: "nav.stats" },
  { id: "privacy", label: "nav.privacy" },
  { id: "about", label: "nav.about" },
];
const isPage = (p: string | null): p is Page => PAGES.some((x) => x.id === p);

const THEMES: { id: Theme; name: StringKey; note: StringKey; swatch: string[]; bg: string }[] = [
  { id: "unflub", name: "appearance.theme.unflub", note: "appearance.theme.unflub.note", swatch: ["#2D1916", "#79453A", "#B06A5C"], bg: "#F2E6E3" },
  { id: "paper", name: "appearance.theme.paper", note: "appearance.theme.paper.note", swatch: ["#292827", "#4e4843", "#a77a55"], bg: "#e7e2da" },
  { id: "ink", name: "appearance.theme.ink", note: "appearance.theme.ink.note", swatch: ["#ededef", "#8a8f98", "#c69c72"], bg: "#121214" },
  { id: "system", name: "appearance.theme.system", note: "appearance.theme.system.note", swatch: [], bg: "linear-gradient(135deg,#dfe7f2,#ece6f0)" },
];

function updateHint(u: UpdateStatus, auto: boolean, info: AppInfo | null): string {
  if (info?.managed.disableUpdates) return t("about.check.managed");
  switch (u.state) {
    case "checking": return t("about.check.checking");
    case "up-to-date": return t("about.check.latest");
    case "error": return t("about.check.error", { message: u.message });
    default: return auto ? t("about.check.auto") : t("about.check.manual");
  }
}

const initialPage = (): Page => {
  const p = new URLSearchParams(window.location.search).get("page");
  return isPage(p) ? p : "general";
};

/** The three modes on the main row; the single-engine ones live under Advanced. */
const MAIN_MODES: { value: EngineMode; label: StringKey; hint: StringKey; tip: StringKey }[] = [
  { value: "both", label: "correction.engine.careful", hint: "correction.engine.careful.hint", tip: "correction.engine.careful.tip" },
  { value: "balanced", label: "correction.engine.balanced", hint: "correction.engine.balanced.hint", tip: "correction.engine.balanced.tip" },
  { value: "aggressive", label: "correction.engine.phone", hint: "correction.engine.phone.hint", tip: "correction.engine.phone.tip" },
];

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

/** Shown above every page: a crash report waiting for an answer, unflub pausing itself, or an
 * important update. */
function Banners({ crash, onCrash, interference, update, autoUpdate, onInstall, onDetails }: {
  crash: boolean;
  interference: boolean;
  onCrash: (a: "send" | "dont-send" | "always-send") => void;
  update: UpdateStatus;
  autoUpdate: boolean;
  onInstall: () => void;
  onDetails: () => void;
}) {
  const critical = update.state === "available" && update.critical && !autoUpdate;
  if (!crash && !interference && !critical) return null;
  return (
    <div className="banners">
      {interference && (
        <div className="banner important" role="status">
          <span>{t("banner.interference")}</span>
        </div>
      )}
      {crash && (
        <div className="banner" role="status">
          <span>{t("banner.crash")}</span>
          <div className="actions">
            <button className="btn" onClick={() => onCrash("send")}>{t("banner.crash.send")}</button>
            <button className="btn ghost" onClick={() => onCrash("dont-send")}>{t("banner.crash.dontSend")}</button>
            <button className="btn ghost" onClick={() => onCrash("always-send")}>{t("banner.crash.always")}</button>
          </div>
        </div>
      )}
      {critical && (
        <div className="banner important" role="status">
          <span>{t("banner.critical", { version: update.version })}</span>
          <div className="actions">
            <button className="btn" onClick={onInstall}>{t("banner.critical.install")}</button>
            <button className="btn ghost" onClick={onDetails}>{t("banner.critical.details")}</button>
          </div>
        </div>
      )}
    </div>
  );
}

export default function App() {
  const [settings, setLocal] = useState<Settings | null>(null);
  const [look, setLook] = useState<SystemLook>({ accent: null, platform: "windows" });
  const [page, setPage] = useState<Page>(initialPage);
  const [version, setVersion] = useState("");
  const [updateState, setUpdateState] = useState<UpdateStatus>({ state: "idle" });
  const [laterFor, setLaterFor] = useState<string | null>(null);
  const [crash, setCrash] = useState(false);
  const [interference, setInterference] = useState(false);
  const [message, setMessage] = useState<RemoteMessage | null>(null);
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [touring, setTouring] = useState(() => new URLSearchParams(window.location.search).get("page") === "tour");
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
    crashReportPending().then(setCrash);
    inputInterference().then(setInterference);
    remoteMessage().then(setMessage);
    appInfo().then(setInfo);
    const offs: Promise<() => void>[] = [
      onSettingsChanged(setLocal),
      onUpdateStatus(setUpdateState),
      onCrashReport(setCrash),
      onInputInterference(setInterference),
      onRemoteMessage(setMessage),
      onOpenPage((p) => {
        if (p === "tour") setTouring(true);
        else if (isPage(p)) setPage(p);
      }),
    ];
    return () => offs.forEach((off) => void off.then((f) => f()));
  }, []);

  // A page change replaces the content under the focused nav button: move focus to the new page's title.
  const shownPage = useRef(page);
  useEffect(() => {
    if (shownPage.current === page) return;
    shownPage.current = page;
    const h1 = document.querySelector<HTMLElement>("main h1");
    h1?.setAttribute("tabindex", "-1");
    h1?.focus();
  }, [page]);

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

  const install = () => void installUpdate().catch((e) => setUpdateState({ state: "error", message: String(e) }));
  const answerCrash = (a: "send" | "dont-send" | "always-send") => {
    setCrash(false);
    void answerCrashReport(a);
  };

  if (!settings) return null;
  const s = settings;
  if (!s.onboarded || touring) {
    return (
      <div className="shell single">
        <main><Tour settings={s} info={info} update={update} onClose={() => setTouring(false)} /></main>
      </div>
    );
  }
  const systemName = look.platform === "macos" ? "macOS" : look.platform === "linux" ? "System" : "Windows";
  const capsule = s.pillStyle === "capsule" || s.pillStyle === "underline-capsule";
  const u = updateState;
  const pending = (u.state === "ready" || u.state === "available") && laterFor !== u.version ? u : null;
  const source = info?.installSource ?? "installer";
  const selfUpdates = source === "installer";
  const mainMode = MAIN_MODES.find((m) => m.value === s.mode);
  const singleEngine = s.mode === "system-only" || s.mode === "bundled-only";

  return (
    <div className="shell">
      <nav className="nav" aria-label={t("nav.label")}>
        <div className="brand"><Logo />unflub</div>
        {PAGES.map((p) => (
          <button key={p.id} aria-current={page === p.id ? "page" : undefined} onClick={() => setPage(p.id)}>
            <NavIcon name={p.id} />
            {t(p.label)}
            {p.id === "about" && (u.state === "ready" || u.state === "available") && <span className="badge" aria-hidden="true" />}
          </button>
        ))}
        <div className="spacer" />
        <PowerSwitch checked={s.enabled} onChange={(v) => update({ enabled: v })} />
      </nav>

      <main>
        <Banners crash={crash} onCrash={answerCrash} interference={interference} update={u} autoUpdate={s.autoUpdate}
          onInstall={install} onDetails={() => setPage("about")} />

        {page === "general" && (
          <>
            <h1>{t("general.title")}</h1>
            <p className="lead">{t("general.lead")}</p>
            <div className="card">
              <Row label={t("general.autocorrect")} hint={t("general.autocorrect.hint")}>
                <Switch label={t("general.autocorrect")} checked={s.enabled} onChange={(v) => update({ enabled: v })} />
              </Row>
              <Row label={t("general.login")} hint={info && !info.loginAllowed ? t("general.login.unavailable") : t("general.login.hint")}>
                <Switch label={t("general.login")} disabled={info ? !info.loginAllowed : false}
                  checked={s.startAtLogin && (info?.loginAllowed ?? true)} onChange={(v) => update({ startAtLogin: v })} />
              </Row>
            </div>
          </>
        )}

        {page === "correction" && (
          <>
            <h1>{t("correction.title")}</h1>
            <p className="lead">{t("correction.lead")}</p>
            <div className="card">
              <Row stack label={t("correction.engine")} hint={
                mainMode ? t(mainMode.hint, { system: systemName }) : t("correction.engine.custom")
              }>
                <Segmented label={t("correction.engine")} value={s.mode} onChange={(v) => update({ mode: v })}
                  options={MAIN_MODES.map((m) => ({ value: m.value, label: t(m.label), title: t(m.tip, { system: systemName }) }))} />
              </Row>
              <Row label={t("correction.confidence")} hint={t("correction.confidence.hint")}>
                <Slider label={t("correction.confidence")} min={0} max={1} step={0.05} value={s.confidence}
                  onChange={(v) => update({ confidence: v })}
                  format={(v) => (v < 0.35 ? t("correction.confidence.eager") : v > 0.75 ? t("correction.confidence.cautious") : t("correction.confidence.balanced"))} />
              </Row>
              <Row label={t("correction.minLen")} hint={t("correction.minLen.hint")}>
                <Slider label={t("correction.minLen.label")} min={1} max={6} step={1} value={s.minWordLen}
                  onChange={(v) => update({ minWordLen: v })} format={(v) => t(v === 1 ? "correction.minLen.one" : "correction.minLen.many", { n: v })} />
              </Row>
              <Row label={t("correction.code")} hint={t("correction.code.hint")}>
                <Switch label={t("correction.code")} checked={s.skipCode} onChange={(v) => update({ skipCode: v })} />
              </Row>
              <Row stack label={t("correction.undo")} hint={t("correction.undo.hint")}>
                <Segmented<UndoMode> label={t("correction.undo")} value={s.undoMode} onChange={(v) => update({ undoMode: v })}
                  options={[
                    { value: "twice", label: t("correction.undo.twice"), title: t("correction.undo.twice.tip") },
                    { value: "once", label: t("correction.undo.once"), title: t("correction.undo.once.tip") },
                    { value: "off", label: t("correction.undo.off"), title: t("correction.undo.off.tip") },
                  ]} />
              </Row>
              <Row stack label={t("correction.learn")} hint={t("correction.learn.hint")}>
                <Segmented label={t("correction.learn")} value={String(s.learnAfterUndos)}
                  onChange={(v) => update({ learnAfterUndos: Number(v) })}
                  options={[
                    { value: "0", label: t("correction.learn.never") },
                    ...[1, 2, 3, 4, 5].map((n) => ({ value: String(n), label: t(n === 1 ? "correction.learn.one" : "correction.learn.many", { n }) })),
                  ]} />
              </Row>
            </div>
            <details className="exact" open={singleEngine || undefined}>
              <summary>{t("correction.advanced")}</summary>
              <div className="exact-body">
                <Row label={t("correction.single")} hint={t("correction.single.hint")}>
                  <Segmented<EngineMode | "off"> label={t("correction.single")} value={singleEngine ? s.mode : "off"}
                    onChange={(v) => update({ mode: v === "off" ? "both" : v })} options={[
                      { value: "off", label: t("correction.single.off"), title: t("correction.single.off.tip") },
                      { value: "system-only", label: t("correction.single.system", { system: systemName }), title: t("correction.single.system.tip", { system: systemName }) },
                      { value: "bundled-only", label: t("correction.single.bundled"), title: t("correction.single.bundled.tip") },
                    ]} />
                </Row>
              </div>
            </details>
          </>
        )}

        {page === "appearance" && (
          <>
            <h1>{t("appearance.title")}</h1>
            <p className="lead">{t("appearance.lead")}</p>
            <div className="section-title">{t("appearance.theme")}</div>
            <div className="themes">
              {THEMES.map((th) => (
                <button key={th.id} className="theme-card" aria-pressed={s.theme === th.id} onClick={() => update({ theme: th.id })}>
                  <div className="swatch" style={{ background: th.bg }}>
                    {(th.id === "system" ? [look.accent ?? "#005fb8", "#1b1b1b", "#ffffff"] : th.swatch).map((c) => <i key={c} style={{ background: c }} />)}
                  </div>
                  <div className="name">{t(th.name)}<small>{t(th.note)}</small></div>
                </button>
              ))}
            </div>
            <div className="card" style={{ marginTop: 14 }}>
              <Row label={t("appearance.mode")}>
                <Segmented label={t("appearance.mode")} value={s.appearance} onChange={(v) => update({ appearance: v })} options={[
                  { value: "auto", label: t("appearance.mode.auto") }, { value: "light", label: t("appearance.mode.light") }, { value: "dark", label: t("appearance.mode.dark") },
                ]} />
              </Row>
            </div>

            <div className="section-title">{t("appearance.afterFix")}</div>
            <div className="card">
              <Row stack label={t("appearance.show")} hint={t("appearance.show.hint")}>
                <Segmented label={t("appearance.show.label")} value={s.pillStyle} onChange={(v) => update({ pillStyle: v })} options={[
                  { value: "underline-capsule", label: t("appearance.show.underlineCapsule") },
                  { value: "capsule", label: t("appearance.show.capsule") },
                  { value: "underline", label: t("appearance.show.underline") },
                  { value: "off", label: t("appearance.show.off") },
                ]} />
                <PillPreview style={s.pillStyle} position={s.pillPosition} opacity={s.pillOpacity} durationMs={s.pillDurationMs} />
              </Row>
              {capsule && (
                <>
                  <Row label={t("appearance.position")}>
                    <Segmented label={t("appearance.position")} value={s.pillPosition} onChange={(v) => update({ pillPosition: v })} options={[
                      { value: "above", label: t("appearance.position.above") }, { value: "below", label: t("appearance.position.below") },
                    ]} />
                  </Row>
                  <Row label={t("appearance.opacity")}>
                    <Slider label={t("appearance.opacity")} min={0.3} max={1} step={0.02} value={s.pillOpacity}
                      onChange={(v) => update({ pillOpacity: v })} format={(v) => `${Math.round(v * 100)}%`} />
                  </Row>
                </>
              )}
              <Row label={t("appearance.duration")} hint={t("appearance.duration.hint")}>
                <Slider label={t("appearance.duration")} min={300} max={5000} step={100} value={s.pillDurationMs}
                  onChange={(v) => update({ pillDurationMs: v })} format={(v) => t("appearance.duration.value", { s: (v / 1000).toFixed(1) })} />
              </Row>
            </div>
          </>
        )}

        {page === "apps" && (
          <>
            <h1>{t("apps.title")}</h1>
            <p className="lead">{t("apps.lead")}</p>
            <div className="card">
              <Row stack label={t("apps.never")} hint={t("apps.never.hint")}>
                <ChipList items={s.excludedApps} onChange={(v) => update({ excludedApps: v })}
                  placeholder={t("apps.placeholder")} empty={t("apps.empty")} normalise={(v) => v.trim().toLowerCase()} />
              </Row>
            </div>
          </>
        )}

        {page === "dictionary" && (
          <>
            <h1>{t("dictionary.title")}</h1>
            <p className="lead">{t("dictionary.lead")}</p>
            <div className="card">
              <Row stack label={t("dictionary.words")} hint={t("dictionary.words.hint")}>
                <ChipList items={s.personalWords} onChange={(v) => update({ personalWords: v })}
                  placeholder={t("dictionary.words.placeholder")} empty={t("dictionary.words.empty")} />
              </Row>
              <Row stack label={t("dictionary.shortcuts")} hint={t("dictionary.shortcuts.hint")}>
                <RuleList rules={s.userRules} onChange={(v) => update({ userRules: v })} />
              </Row>
            </div>
          </>
        )}

        {page === "stats" && <StatsPage settings={s} update={update} />}

        {page === "privacy" && <PrivacyPage settings={s} info={info} update={update} />}

        {page === "about" && (
          <>
            <h1>{t("about.title")}</h1>
            <div className="card">
              <Row label={<span className="about"><AppIcon className="logo" /><span>unflub {version}<br /><small className="muted">{t("about.tagline")}</small></span></span>}>
                <span />
              </Row>
              {message && (
                <Row label={message.text} hint={message.url && isExternalUrl(message.url)
                  ? <ExternalLink href={message.url}>{t("about.message.more")}</ExternalLink> : undefined}>
                  <button className="btn ghost" onClick={() => { setMessage(null); void dismissMessage(message.id); }}>{t("about.message.dismiss")}</button>
                </Row>
              )}
              {pending && (
                <div className="row update-row">
                  <div className="label">
                    <span>
                      {t(pending.state === "ready" ? "about.ready" : "about.available", { version: pending.version })}
                      {pending.critical && <em className="tag">{t("about.critical")}</em>}
                    </span>
                    <small>{
                      !selfUpdates ? t(source === "scoop" ? "about.via.scoop" : source === "winget" ? "about.via.winget" : source === "chocolatey" ? "about.via.chocolatey" : source === "store" ? "about.via.store" : "about.via.portable")
                      : t(pending.state === "ready" ? "about.ready.hint" : "about.available.hint")
                    }</small>
                    {pending.notes && (
                      <>
                        <small className="notes-title">{t("about.notes")}</small>
                        <div className="notes">{pending.notes}</div>
                      </>
                    )}
                  </div>
                  <div className="actions">
                    {selfUpdates ? (
                      <>
                        <button className="btn" onClick={install}>{t("about.restart")}</button>
                        <button className="btn ghost" onClick={() => setLaterFor(pending.version)}>{t("about.later")}</button>
                      </>
                    ) : source === "portable" ? (
                      <ExternalLink className="btn" href={`${LINKS.site}download/`}>{t("about.via.download")}</ExternalLink>
                    ) : null}
                  </div>
                </div>
              )}
              {(u.state === "downloading" || u.state === "installing") && (
                <Row label={u.state === "downloading" ? t("about.downloading", { version: u.version }) : t("about.installing")}>
                  <span className="spinner" aria-hidden="true" />
                </Row>
              )}
              {!pending && u.state !== "downloading" && u.state !== "installing" && (
                <Row label={t("about.check")} hint={<span role="status">{updateHint(u, s.autoUpdate, info)}</span>}>
                  <button className="btn ghost" disabled={u.state === "checking" || info?.managed.disableUpdates}
                    onClick={() => void checkForUpdates().then(setUpdateState)}>
                    {t("about.check.button")}
                  </button>
                </Row>
              )}
              <Row label={t("about.tour")} hint={t("about.tour.hint")}>
                <button className="btn ghost" onClick={() => setTouring(true)}>{t("about.tour.button")}</button>
              </Row>
              <Row label={t("about.support")} hint={t("about.support.hint")}>
                <ExternalLink className="btn" href={LINKS.sponsor}>{t("about.support.button")}</ExternalLink>
              </Row>
              <Row label={t("about.star")} hint={t("about.star.hint")}>
                <ExternalLink className="btn ghost" href={LINKS.repo}>{t("about.star.button")}</ExternalLink>
              </Row>
              <Row label={t("about.reset")} hint={t("about.reset.hint")}>
                <button className="btn ghost" onClick={() => void resetSettings().then(setLocal)}>{t("about.reset.button")}</button>
              </Row>
            </div>
          </>
        )}
      </main>
    </div>
  );
}
