import { useEffect, useRef, useState } from "react";
import type { PillPosition, PillStyle } from "./types";

/** Words typed by the preview; a second element is the correction applied on the next boundary. */
const SCRIPT: [string, string?][] = [
  ["I "], ["type "], ["fast "], ["and "], ["most "], ["issuses ", "issues"], ["get "], ["caught, "],
  ["but "], ["teh ", "the"], ["fix "], ["never "], ["landed. "], ["Now "], ["it "], ["does, "], ["liek ", "like"], ["magic."],
];

interface Segment { text: string; fixed?: boolean; flash?: boolean; underline?: boolean }

/** Plays a sentence full of real typos and corrects them with the chosen pill settings. */
export function PillPreview({ style, position, opacity, durationMs }: {
  style: PillStyle;
  position: PillPosition;
  opacity: number;
  durationMs: number;
}) {
  const [segments, setSegments] = useState<Segment[]>([]);
  const [pill, setPill] = useState({ from: "", to: "", x: 0, y: 0, show: false });
  const box = useRef<HTMLDivElement>(null);
  const settings = useRef({ style, durationMs, position });
  settings.current = { style, durationMs, position };

  useEffect(() => {
    let cancelled = false;
    const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));
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
              requestAnimationFrame(() => {
                const all = box.current?.querySelectorAll<HTMLElement>(".fixed");
                const el = all?.[all.length - 1];
                const b = box.current?.getBoundingClientRect();
                const r = el?.getBoundingClientRect();
                if (!r || !b) return;
                const y = settings.current.position === "above" ? r.top - b.top - 30 : r.bottom - b.top + 6;
                setPill({ from: bare, to: fix, x: r.left - b.left - 6, y, show: true });
              });
              setTimeout(() => setPill((p) => ({ ...p, show: false })), dur);
            }
            setTimeout(() => setSegments((s) => s.map((x) => (x.text === fix ? { ...x, flash: false, underline: false } : x))), Math.max(700, dur));
          }
          setSegments((s) => [...s, { text: tail }]);
          await wait(tail ? 80 : 0);
        }
        await wait(2400);
      }
    })();
    return () => { cancelled = true; };
  }, []);

  return (
    <div ref={box} className={`preview ${position}`} style={{ ["--pill-opacity" as string]: opacity }} aria-label="Correction preview">
      {segments.map((s, i) => (
        <span key={i} className={[s.fixed && "fixed", s.flash && "flash", s.underline && "underline"].filter(Boolean).join(" ")}>
          {s.text}
        </span>
      ))}
      <span className="caret" />
      <div className={`pill${pill.show ? " show" : ""}`} style={{ left: Math.max(8, pill.x), top: pill.y }}>
        <s>{pill.from}</s>
        <span>→ {pill.to}</span>
        <kbd>⌫ undo</kbd>
      </div>
    </div>
  );
}
