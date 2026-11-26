import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { reportUsage, tourKey, tourReset } from "./api";
import { AppIcon, Disclosure, ExternalLink, Row, Switch, SuccessNote } from "./components";
import { t } from "./i18n";
import { LINKS } from "./links";
import { practiceInput } from "./tourInput";
import type { AppInfo, Settings, TourAction } from "./types";

/** `asking`: a first Backspace is held and the pill asks "Undo?" until the next key. */
type Pill = { from: string; to: string; kind: "fixed" | "undone" | "asking" } | null;

/**
 * The practice box. The keyboard hook leaves unflub's own windows alone, so each key goes to a
 * separate engine in Rust (`tour.rs`) and the answer is applied here, pill included.
 */
function PracticeBox({ onAction }: { onAction: (a: TourAction) => void }) {
  const [text, setText] = useState("");
  const [pill, setPill] = useState<Pill>(null);
  const queue = useRef<Promise<void>>(Promise.resolve());
  const needsReset = useRef(false);
  const box = useRef<HTMLTextAreaElement>(null);
  const hide = useRef<number | undefined>(undefined);

  useEffect(() => {
    queue.current = queue.current.then(tourReset).catch(() => {
      needsReset.current = true;
    });
    box.current?.focus();
    return () => window.clearTimeout(hide.current);
  }, []);

  // The caret stays at the end: the engine only knows what was typed in order.
  useEffect(() => {
    const el = box.current;
    if (el) el.selectionStart = el.selectionEnd = el.value.length;
  }, [text]);

  const dropLast = (s: string) => Array.from(s).slice(0, -1).join("");

  const apply = (key: string, a: TourAction) => {
    if (a.kind === "pass") {
      setText((s) => (key === "Backspace" ? dropLast(s) : s + key));
      return;
    }
    window.clearTimeout(hide.current);
    if (a.kind === "release") {
      // The held Backspace goes on: delete one character, and the question is answered.
      setText(dropLast);
      setPill(null);
      return;
    }
    if (a.kind === "hold") {
      // Nothing deleted yet; the pill asks until the next key, as the real one does.
      setPill({ from: a.original, to: a.corrected, kind: "asking" });
      return;
    }
    const { delete: remove, insert, original, corrected } = a;
    setText((s) => Array.from(s).slice(0, Math.max(0, Array.from(s).length - remove)).join("") + insert);
    setPill(a.kind === "replace" ? { from: original, to: corrected, kind: "fixed" } : { from: corrected, to: original, kind: "undone" });
    hide.current = window.setTimeout(() => setPill(null), 1600);
    onAction(a);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    const input = practiceInput(e.nativeEvent);
    if (!input) return;
    const { sent, key, preventDefault } = input;
    if (preventDefault) e.preventDefault();
    queue.current = queue.current.then(async () => {
      let actions: TourAction[] = [{ kind: "pass" }];
      try {
        if (needsReset.current) {
          for (const a of await tourKey("Reset")) if (a.kind !== "pass") apply("", a);
          needsReset.current = false;
        }
        actions = await tourKey(sent);
      } catch {
        // The failed call may have reached Rust. Reset before trusting another correction.
        needsReset.current = true;
      }
      // A release comes first even for keys the box doesn't type (arrows).
      for (const a of actions) if (key || a.kind !== "pass") apply(key ?? "", a);
    });
  };

  return (
    <div className="practice">
      <div className={`pill${pill ? " show" : ""}${pill?.kind === "asking" ? " asking" : ""}`} style={{ left: 14, top: -34 }} aria-live="polite">
        {pill && (
          <>
            <s>{pill.from}</s>
            <span>→ {pill.to}</span>
            {pill.kind === "fixed" && <kbd>{t("pillPreview.undo")}</kbd>}
            {pill.kind === "asking" && <strong>{t("tour.undo.ask")}</strong>}
          </>
        )}
      </div>
      <textarea ref={box} value={text} onChange={() => {}} onKeyDown={onKeyDown} onPaste={(e) => e.preventDefault()}
        onDrop={(e) => e.preventDefault()} placeholder={t("tour.box.placeholder")} aria-label={t("tour.box")}
        spellCheck={false} autoCorrect="off" autoCapitalize="off" rows={3} />
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
  const learn = s.learnAfterUndos;
  const root = useRef<HTMLDivElement>(null);
  const shownStep = useRef(step);

  // Steps 1 and 2 move focus into their practice box. On the others the button that was pressed is
  // gone or reused, so focus would fall to the page: put it on the new step's title instead.
  useEffect(() => {
    if (shownStep.current === step) return;
    shownStep.current = step;
    if (step <= 2) return;
    const h1 = root.current?.querySelector<HTMLElement>("h1");
    h1?.setAttribute("tabindex", "-1");
    h1?.focus();
  }, [step]);

  // The practice box starts empty on every step, so its success notes must too.
  const go = (n: number) => {
    setStep(n);
    setFixed(false);
    setUndone(null);
  };

  // The first-run tour reports how it ended; any later one counts as a replay.
  const [skipped, setSkipped] = useState(false);
  const firstRun = useRef(!s.onboarded);
  useEffect(() => {
    if (!firstRun.current) reportUsage("tour_replay");
  }, []);

  const done = () => {
    if (firstRun.current) reportUsage(skipped ? "tour_skipped" : "tour_completed");
    if (!s.onboarded) update({ onboarded: true });
    onClose();
  };

  return (
    <div className="tour" ref={root}>
      <div className="tour-head">
        <AppIcon className="tour-logo" />
        <small className="muted" role="status">{t("tour.step", { n: step, total: STEPS })}</small>
        <div className="dots" aria-hidden="true">
          {Array.from({ length: STEPS }, (_, i) => <i key={i} className={i + 1 <= step ? "on" : undefined} />)}
        </div>
      </div>

      {step === 1 && (
        <>
          <h1 className="tour-instruction">{t("tour.try.before")}<strong>{t("tour.try.word")}</strong>{t("tour.try.after")}</h1>
          <p className="lead">{t("tour.try.lead")}</p>
          <PracticeBox onAction={(a) => a.kind === "replace" && a.original.toLowerCase() === "teh" && a.corrected.toLowerCase() === "the" && setFixed(true)} />
          <div className="tour-note-slot">{fixed && <SuccessNote>{t("tour.try.done")}</SuccessNote>}</div>
        </>
      )}

      {step === 2 && (
        <>
          <h1>{t("tour.undo.title")}</h1>
          <p className="lead">{t("tour.undo.lead")}</p>
          <PracticeBox onAction={(a) => a.kind === "undo" && setUndone(a.original)} />
          {undone
            ? <SuccessNote>{learn === 0
              ? t("tour.undo.done.never")
              : t(learn === 1 ? "tour.undo.done.one" : "tour.undo.done.many", { n: learn })}
            </SuccessNote>
            : <p className="tour-note">{t("tour.undo.waiting")}</p>}
        </>
      )}

      {step === 3 && (
        <>
          <h1>{t("tour.skips.title")}</h1>
          <p className="lead">{t("tour.skips.lead")}</p>
        </>
      )}

      {step === 4 && (
        <>
          <h1>{t("tour.data.title")}</h1>
          <p className="lead">{t("tour.data.lead")}</p>
          <Disclosure />
          <div className="card">
            <Row label={t("privacy.crashes")} hint={managed?.disableCrashReports ? t("privacy.managed") : undefined}>
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

      <div className="tour-bottom">
        <div className="tour-foot">
          {step > 1 ? <button className="btn ghost" onClick={() => go(step - 1)}>{t("tour.back")}</button> : <span />}
          <div className="actions">
            {step < STEPS && <button className="btn ghost" onClick={() => { setSkipped(true); go(STEPS); }}>{t("tour.skip")}</button>}
            {step < STEPS
              ? <button className="btn" disabled={(step === 1 && !fixed) || (step === 2 && !undone)} onClick={() => go(step + 1)}>{t("tour.next")}</button>
              : <button className="btn" onClick={done}>{t(s.onboarded ? "tour.done" : "tour.continue")}</button>}
          </div>
        </div>
        {step === STEPS && !s.onboarded && (
          <p className="tour-terms">
            {t("tour.terms.before")}<ExternalLink href={LINKS.terms}>{t("tour.terms.link")}</ExternalLink>{t("tour.terms.after")}
            <ExternalLink className="link quiet" href={LINKS.privacy}>{t("tour.terms.collect")}</ExternalLink>
          </p>
        )}
      </div>
    </div>
  );
}
