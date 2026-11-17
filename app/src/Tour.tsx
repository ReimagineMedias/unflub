import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { tourKey, tourReset } from "./api";
import { AppIcon, Row, Switch } from "./components";
import { t } from "./i18n";
import type { AppInfo, Settings, TourAction } from "./types";

type Pill = { from: string; to: string; undone: boolean } | null;

/**
 * The practice box. The keyboard hook leaves unflub's own windows alone, so each key goes to a
 * separate engine in Rust (`tour.rs`) and the answer is applied here, pill included.
 */
function PracticeBox({ onAction }: { onAction: (a: TourAction) => void }) {
  const [text, setText] = useState("");
  const [pill, setPill] = useState<Pill>(null);
  const queue = useRef<Promise<void>>(Promise.resolve());
  const box = useRef<HTMLTextAreaElement>(null);
  const hide = useRef<number | undefined>(undefined);

  useEffect(() => {
    void tourReset();
    box.current?.focus();
    return () => window.clearTimeout(hide.current);
  }, []);

  // The caret stays at the end: the engine only knows what was typed in order.
  useEffect(() => {
    const el = box.current;
    if (el) el.selectionStart = el.selectionEnd = el.value.length;
  }, [text]);

  const apply = (key: string, a: TourAction) => {
    if (a.kind === "pass") {
      setText((s) => (key === "Backspace" ? Array.from(s).slice(0, -1).join("") : s + key));
      return;
    }
    setText((s) => Array.from(s).slice(0, Math.max(0, Array.from(s).length - a.delete)).join("") + a.insert);
    setPill(a.kind === "replace" ? { from: a.original, to: a.corrected, undone: false } : { from: a.corrected, to: a.original, undone: true });
    window.clearTimeout(hide.current);
    hide.current = window.setTimeout(() => setPill(null), 1600);
    onAction(a);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.ctrlKey || e.metaKey || e.altKey || e.key === "Tab") return;
    const key = e.key === "Backspace" ? "Backspace" : e.key === "Enter" ? "\n" : [...e.key].length === 1 ? e.key : null;
    e.preventDefault();
    // Arrows, Home, Delete and the rest would move the caret somewhere the engine can't see.
    const sent = key ?? "Reset";
    queue.current = queue.current.then(async () => {
      const a = await tourKey(sent);
      if (key) apply(key, a);
    });
  };

  return (
    <div className="practice">
      <div className={`pill${pill ? " show" : ""}`} style={{ left: 14, top: -34 }} aria-live="polite">
        {pill && (
          <>
            <s>{pill.from}</s>
            <span>→ {pill.to}</span>
            {!pill.undone && <kbd>{t("pillPreview.undo")}</kbd>}
          </>
        )}
      </div>
      <textarea ref={box} value={text} onChange={() => {}} onKeyDown={onKeyDown} onPaste={(e) => e.preventDefault()}
        onDrop={(e) => e.preventDefault()} placeholder={t("tour.box.placeholder")} aria-label={t("tour.box")}
        spellCheck={false} autoCorrect="off" autoCapitalize="off" rows={3} />
      <button className="link" onClick={() => {
        setText("");
        setPill(null);
        void tourReset();
        box.current?.focus();
      }}>{t("tour.box.clear")}</button>
    </div>
  );
}

const STEPS = 4;

/** First run, and later from the tray or About: try a fix, an undo, see what's skipped, then choose. */
export function Tour({ settings: s, info, update, onClose }: {
  settings: Settings;
  info: AppInfo | null;
  update: (patch: Partial<Settings>) => void;
  onClose: () => void;
}) {
  const [step, setStep] = useState(1);
  const [fixed, setFixed] = useState(false);
  const [undone, setUndone] = useState<string | null>(null);
  const managed = info?.managed;

  const done = () => {
    if (!s.onboarded) update({ onboarded: true });
    onClose();
  };

  return (
    <div className="tour">
      <div className="tour-head">
        <AppIcon className="tour-logo" />
        <small className="muted">{t("tour.step", { n: step, total: STEPS })}</small>
        <div className="dots" aria-hidden="true">
          {Array.from({ length: STEPS }, (_, i) => <i key={i} className={i + 1 <= step ? "on" : undefined} />)}
        </div>
      </div>

      {step === 1 && (
        <>
          <h1>{t("tour.try.title")}</h1>
          <p className="lead">{t("tour.try.lead")}</p>
          <PracticeBox onAction={(a) => a.kind === "replace" && setFixed(true)} />
          <p className={fixed ? "tour-note good" : "tour-note"}>{fixed ? t("tour.try.done") : t("tour.try.waiting")}</p>
        </>
      )}

      {step === 2 && (
        <>
          <h1>{t("tour.undo.title")}</h1>
          <p className="lead">{t("tour.undo.lead")}</p>
          <PracticeBox onAction={(a) => a.kind === "undo" && setUndone(a.original)} />
          <p className={undone ? "tour-note good" : "tour-note"}>
            {undone ? t("tour.undo.done", { word: undone }) : t("tour.undo.waiting")}
          </p>
        </>
      )}

      {step === 3 && (
        <>
          <h1>{t("tour.skips.title")}</h1>
          <p className="lead">{t("tour.skips.lead")}</p>
          <ul className="tour-list">
            <li>{t("tour.skips.code")}</li>
            <li>{t("tour.skips.passwords")}</li>
            <li>{t("tour.skips.apps")}</li>
            <li>{t("tour.skips.yours")}</li>
          </ul>
        </>
      )}

      {step === 4 && (
        <>
          <h1>{t("tour.data.title")}</h1>
          <p className="lead">{t("tour.data.lead")}</p>
          <p className="tour-funding">{t("tour.data.funding")}</p>
          <div className="card">
            <Row label={t("privacy.stats")} hint={managed?.disableUsageStats ? t("privacy.managed") : t("privacy.stats.hint")}>
              <Switch label={t("privacy.stats")} disabled={managed?.disableUsageStats}
                checked={s.shareUsageStats && !managed?.disableUsageStats} onChange={(v) => update({ shareUsageStats: v })} />
            </Row>
            <Row label={t("privacy.crashes")} hint={managed?.disableCrashReports ? t("privacy.managed") : t("privacy.crashes.hint")}>
              <Switch label={t("privacy.crashes")} disabled={managed?.disableCrashReports}
                checked={s.sendCrashReports && !managed?.disableCrashReports} onChange={(v) => update({ sendCrashReports: v })} />
            </Row>
            <Row label={t("general.login")} hint={info && !info.loginAllowed ? t("general.login.unavailable") : t("general.login.hint")}>
              <Switch label={t("general.login")} disabled={info ? !info.loginAllowed : false}
                checked={s.startAtLogin && (info?.loginAllowed ?? true)} onChange={(v) => update({ startAtLogin: v })} />
            </Row>
          </div>
          <small className="muted tour-later">{t("tour.data.later")}</small>
        </>
      )}

      <div className="tour-foot">
        {step > 1 ? <button className="btn ghost" onClick={() => setStep(step - 1)}>{t("tour.back")}</button> : <span />}
        <div className="actions">
          {step < STEPS && <button className="btn ghost" onClick={() => setStep(STEPS)}>{t("tour.skip")}</button>}
          {step < STEPS
            ? <button className="btn" onClick={() => setStep(step + 1)}>{t("tour.next")}</button>
            : <button className="btn" autoFocus onClick={done}>{t("tour.done")}</button>}
        </div>
      </div>
    </div>
  );
}
