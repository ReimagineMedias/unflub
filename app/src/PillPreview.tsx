import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { previewScript as SCRIPT, t } from "./i18n";
import type { PillPosition, PillStyle } from "./types";

interface Segment { text: string; fixed?: boolean; flash?: boolean; underline?: boolean }

const motionPreference = window.matchMedia("(prefers-reduced-motion: reduce)");
const subscribeToMotion = (callback: () => void) => {
  motionPreference.addEventListener("change", callback);
  return () => motionPreference.removeEventListener("change", callback);
};
const prefersReducedMotion = () => motionPreference.matches;
const correctedSentence = SCRIPT.map(([word, fix]) => fix ? fix + word.slice(word.trimEnd().length) : word).join("");

/** Plays a sentence full of real typos and corrects them with the chosen pill settings. */
export function PillPreview({ style, position, opacity, durationMs }: {
  style: PillStyle;
  position: PillPosition;
  opacity: number;
  durationMs: number;
}) {
  const reducedMotion = useSyncExternalStore(subscribeToMotion, prefersReducedMotion);
  const [segments, setSegments] = useState<Segment[]>([]);
  const [pill, setPill] = useState({ from: "", to: "", x: 0, y: 0, show: false });
  const box = useRef<HTMLDivElement>(null);
  const settings = useRef({ style, durationMs, position });
  settings.current = { style, durationMs, position };

  useEffect(() => {
    if (reducedMotion) return;
    let cancelled = false;
    let frame = 0;
    const timers = new Set<number>();
    const later = (callback: () => void, ms: number) => {
      const timer = window.setTimeout(() => {
        timers.delete(timer);
        callback();
      }, ms);
      timers.add(timer);
    };
    const wait = (ms: number) => new Promise<void>((resolve) => later(resolve, ms));
    setPill((p) => ({ ...p, show: false }));
    (async () => {
      while (!cancelled) {
        setSegments([]);
        for (const [word, fix] of SCRIPT) {
          if (cancelled) return;
          const bare = word.trimEnd();
          const tail = word.slice(bare.length);
          for (let i = 1; i <= bare.length; i++) {
            setSegments((s) => [...s.slice(0, s.length - (i > 1 ? 1 : 0)), { text: bare.slice(0, i) }]);
            await wait(40 + Math.random() * 50);
          }
          if (fix) {
            const { style: st, durationMs: dur } = settings.current;
            const underline = st === "underline" || st === "underline-capsule";
            setSegments((s) => [...s.slice(0, -1), { text: fix, fixed: true, flash: st !== "off", underline }]);
            if (st === "capsule" || st === "underline-capsule") {
              frame = requestAnimationFrame(() => {
                const all = box.current?.querySelectorAll<HTMLElement>(".fixed");
                const el = all?.[all.length - 1];
                const b = box.current?.getBoundingClientRect();
                const r = el?.getBoundingClientRect();
                if (!r || !b) return;
                const y = settings.current.position === "above" ? r.top - b.top - 30 : r.bottom - b.top + 6;
                setPill({ from: bare, to: fix, x: r.left - b.left - 6, y, show: true });
              });
              later(() => setPill((p) => ({ ...p, show: false })), dur);
            }
            later(() => setSegments((s) => s.map((x) => (x.text === fix ? { ...x, flash: false, underline: false } : x))), Math.max(700, dur));
          }
          setSegments((s) => [...s, { text: tail }]);
          await wait(tail ? 80 : 0);
        }
        await wait(2400);
      }
    })();
    return () => {
      cancelled = true;
      timers.forEach((timer) => window.clearTimeout(timer));
      cancelAnimationFrame(frame);
    };
  }, [reducedMotion]);

  const label = t("pillPreview.label", { sentence: correctedSentence });
  if (reducedMotion) return <div role="img" aria-label={label} className={"preview " + position}>{correctedSentence}</div>;

  return (
    <div ref={box} className={`preview ${position}`} style={{ ["--pill-opacity" as string]: opacity }} role="img" aria-label={label}>
      {segments.map((s, i) => (
        <span key={i} className={[s.fixed && "fixed", s.flash && "flash", s.underline && "underline"].filter(Boolean).join(" ")}>
          {s.text}
        </span>
      ))}
      <span className="caret" />
      <div className={`pill${pill.show ? " show" : ""}`} style={{ left: Math.max(8, pill.x), top: pill.y }}>
        <s>{pill.from}</s>
        <span>→ {pill.to}</span>
        <kbd>{t("pillPreview.undo")}</kbd>
      </div>
    </div>
  );
}
