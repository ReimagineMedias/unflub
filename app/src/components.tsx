import { useId, useState, type ReactNode } from "react";

export function Logo({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 64 64" aria-hidden="true">
      <path d="M14 18 V36 a10.5 10.5 0 0 0 21 0 V18" fill="none" stroke="currentColor" strokeWidth="7.5" strokeLinecap="round" />
      <rect x="42" y="9" width="4.5" height="46" rx="2.25" fill="var(--accent)" />
    </svg>
  );
}

export function AppIcon({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 1024 1024" aria-hidden="true">
      <rect width="1024" height="1024" rx="232" fill="#2D1916" />
      <path d="M300 330 V560 a150 150 0 0 0 300 0 V330" fill="none" stroke="#F2E6E3" strokeWidth="104" strokeLinecap="round" />
      <rect x="694" y="232" width="60" height="560" rx="30" fill="#B06A5C" />
    </svg>
  );
}

export function Row({ label, hint, children, stack }: { label: ReactNode; hint?: ReactNode; children: ReactNode; stack?: boolean }) {
  return (
    <div className={stack ? "row stack" : "row"}>
      <div className="label">
        <span>{label}</span>
        {hint && <small>{hint}</small>}
      </div>
      {children}
    </div>
  );
}

export function Switch({ checked, onChange, label }: { checked: boolean; onChange: (v: boolean) => void; label: string }) {
  return <button className="switch" role="switch" aria-checked={checked} aria-label={label} onClick={() => onChange(!checked)} />;
}

export function Segmented<T extends string>({ value, options, onChange, label }: {
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
  label: string;
}) {
  return (
    <div className="seg" role="group" aria-label={label}>
      {options.map((o) => (
        <button key={o.value} aria-pressed={o.value === value} onClick={() => onChange(o.value)}>
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Slider({ value, min, max, step, onChange, format, label }: {
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (v: number) => void;
  format: (v: number) => string;
  label: string;
}) {
  const id = useId();
  return (
    <div className="slider">
      <input id={id} type="range" aria-label={label} min={min} max={max} step={step} value={value} onChange={(e) => onChange(Number(e.target.value))} />
      <output htmlFor={id}>{format(value)}</output>
    </div>
  );
}

const X = () => (
  <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
    <path d="M2 2l6 6M8 2l-6 6" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
  </svg>
);

export function ChipList({ items, onChange, placeholder, empty, normalise = (s) => s.trim() }: {
  items: string[];
  onChange: (items: string[]) => void;
  placeholder: string;
  empty: string;
  normalise?: (s: string) => string;
}) {
  const [draft, setDraft] = useState("");
  const add = () => {
    const v = normalise(draft);
    if (v && !items.includes(v)) onChange([...items, v]);
    setDraft("");
  };
  return (
    <>
      {items.length ? (
        <div className="chips">
          {items.map((it) => (
            <span className="chip" key={it}>
              {it}
              <button aria-label={`Remove ${it}`} onClick={() => onChange(items.filter((x) => x !== it))}>
                <X />
              </button>
            </span>
          ))}
        </div>
      ) : (
        <div className="empty">{empty}</div>
      )}
      <form className="add" onSubmit={(e) => { e.preventDefault(); add(); }}>
        <input value={draft} onChange={(e) => setDraft(e.target.value)} placeholder={placeholder} spellCheck={false} />
        <button className="btn" type="submit" disabled={!draft.trim()}>Add</button>
      </form>
    </>
  );
}

export function RuleList({ rules, onChange }: { rules: [string, string][]; onChange: (r: [string, string][]) => void }) {
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const add = () => {
    const f = from.trim(), t = to.trim();
    if (!f || !t) return;
    onChange([...rules.filter(([k]) => k.toLowerCase() !== f.toLowerCase()), [f, t]]);
    setFrom("");
    setTo("");
  };
  return (
    <div className="rules">
      {rules.length === 0 && <div className="empty">No shortcuts yet.</div>}
      {rules.map(([f, t], i) => (
        <div className="rule" key={f}>
          <input value={f} aria-label="Typed" spellCheck={false} onChange={(e) => onChange(rules.map((r, j) => (j === i ? [e.target.value, r[1]] : r)))} />
          <span className="arrow">→</span>
          <input value={t} aria-label="Becomes" spellCheck={false} onChange={(e) => onChange(rules.map((r, j) => (j === i ? [r[0], e.target.value] : r)))} />
          <button className="icon-btn" aria-label={`Remove ${f}`} onClick={() => onChange(rules.filter((_, j) => j !== i))}>
            <X />
          </button>
        </div>
      ))}
      <form className="rule" onSubmit={(e) => { e.preventDefault(); add(); }}>
        <input value={from} onChange={(e) => setFrom(e.target.value)} placeholder="brb" aria-label="New shortcut typed" spellCheck={false} />
        <span className="arrow">→</span>
        <input value={to} onChange={(e) => setTo(e.target.value)} placeholder="be right back" aria-label="New shortcut becomes" spellCheck={false} />
        <button className="icon-btn" type="submit" aria-label="Add shortcut">+</button>
      </form>
    </div>
  );
}

const paths: Record<string, string> = {
  general: "M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M3.4 12.6l1.4-1.4M11.2 4.8l1.4-1.4M8 5.5a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5z",
  correction: "M2 11c1.2-2 2.4-2 3.6 0s2.4 2 3.6 0M10.5 11H14M11 3v6",
  appearance: "M8 1.8a6.2 6.2 0 1 0 0 12.4c.9 0 1.3-.6 1.3-1.3 0-.8-.6-1-.6-1.8 0-.7.6-1.3 1.3-1.3h1.5A2.8 2.8 0 0 0 14.2 7 6.2 5.6 0 0 0 8 1.8zM4.8 7.2h.01M6.4 4.6h.01M9.6 4.6h.01",
  apps: "M2.5 2.5h4v4h-4zM9.5 2.5h4v4h-4zM2.5 9.5h4v4h-4zM9.5 9.5h4v4h-4z",
  dictionary: "M3 2.5h7.5a2 2 0 0 1 2 2v9H5a2 2 0 0 1-2-2zM3 11.5a2 2 0 0 1 2-2h7.5M6 5.5h4",
  about: "M8 1.8a6.2 6.2 0 1 0 0 12.4A6.2 6.2 0 0 0 8 1.8zM8 7.3v3.9M8 5h.01",
};

export function NavIcon({ name }: { name: string }) {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path d={paths[name]} fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}
