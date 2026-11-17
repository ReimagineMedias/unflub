/**
 * Plays a typing script into a `.field` and corrects it the way unflub does:
 * the fix lands when the word ends, the word gets a brief rose underline and a capsule
 * pops above it, and Backspace right after a fix puts the original back.
 *
 * Script syntax: plain text, with `{typed|fix}` for a corrected word and `{typed|fix|undo}`
 * for one the user takes back. Each scene pauses while it is off screen.
 */

export type PillStyle = "underline-capsule" | "capsule" | "underline" | "off";

export interface Token {
  text: string;
  fix?: string;
  undo?: boolean;
}

export type TyperEvent = "fix" | "undo" | "word";

export interface TyperOptions {
  pill?: PillStyle;
  position?: "above" | "below";
  /** How long the capsule and underline stay, like the app's "Stays for" (default 1100 ms). */
  holdMs?: number;
  /** Multiplies every typing delay. */
  pace?: number;
  onEvent?: (event: TyperEvent, token: Token) => void;
}

export const reducedMotion = (): boolean => window.matchMedia("(prefers-reduced-motion: reduce)").matches;

export function parseScript(src: string): Token[] {
  const out: Token[] = [];
  const re = /\{([^|}]+)\|([^|}]+)(\|undo)?\}/g;
  let last = 0;
  for (const m of src.matchAll(re)) {
    if (m.index > last) out.push({ text: src.slice(last, m.index) });
    const typed = m[1] ?? "";
    const fix = m[2] ?? "";
    out.push(m[3] ? { text: typed, fix, undo: true } : { text: typed, fix });
    last = m.index + m[0].length;
  }
  if (last < src.length) out.push({ text: src.slice(last) });
  return out;
}

/** Keeps time only while its element is on screen, the tab is visible and nobody pressed pause. */
export class Clock {
  private visible = false;
  private paused = false;
  private waiters: (() => void)[] = [];

  constructor(target: Element) {
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) this.set(e.isIntersecting);
      },
      { rootMargin: "80px 0px" },
    );
    io.observe(target);
    document.addEventListener("visibilitychange", () => {
      if (document.hidden) this.visible = false;
    });
  }

  private set(v: boolean): void {
    this.visible = v && !document.hidden;
    this.wake();
  }

  private wake(): void {
    if (!this.running) return;
    const w = this.waiters;
    this.waiters = [];
    for (const f of w) f();
  }

  /** Freeze or resume every scene on this clock, mid-word if need be. */
  setPaused(p: boolean): void {
    this.paused = p;
    this.wake();
  }

  get running(): boolean {
    return this.visible && !this.paused;
  }

  /** Whether the scene is on screen in a visible tab, paused or not. */
  get onScreen(): boolean {
    return this.visible;
  }

  async wait(ms: number, signal?: AbortSignal): Promise<void> {
    await new Promise<void>((resolve, reject) => {
      const t = window.setTimeout(resolve, ms);
      signal?.addEventListener(
        "abort",
        () => {
          window.clearTimeout(t);
          reject(signal.reason);
        },
        { once: true },
      );
    });
    while (!this.running) {
      await new Promise<void>((resolve) => this.waiters.push(resolve));
      if (signal?.aborted) throw signal.reason;
    }
  }
}

export class Typer {
  readonly field: HTMLElement;
  private readonly ink: HTMLElement;
  private readonly caret: HTMLElement;
  private readonly pill: HTMLElement | null;
  private readonly placeholder: string;
  private node: Text | null = null;
  private hideTimer = 0;
  opts: TyperOptions;

  constructor(
    field: HTMLElement,
    private readonly clock: Clock,
    opts: TyperOptions = {},
  ) {
    this.field = field;
    this.opts = opts;
    const ink = field.querySelector<HTMLElement>(".ink");
    if (!ink) throw new Error("field without .ink");
    this.ink = ink;
    // The field's own capsule, not a still-frame one inside the text.
    this.pill = field.querySelector<HTMLElement>(":scope > .pill");
    this.placeholder = field.dataset["placeholder"] ?? "";
    this.caret = document.createElement("span");
    this.caret.className = "caret";
    this.caret.setAttribute("aria-hidden", "true");
  }

  /** Empty the field, leaving the placeholder (if any) and the caret. */
  clear(): void {
    window.clearTimeout(this.hideTimer);
    this.pill?.classList.remove("show");
    this.ink.replaceChildren();
    if (this.placeholder) {
      const ph = document.createElement("span");
      ph.className = "ph";
      ph.textContent = this.placeholder;
      this.ink.append(this.caret, ph);
    } else {
      this.ink.append(this.caret);
    }
    this.node = null;
    this.field.classList.add("focused");
  }

  /** The text typed so far, without the placeholder. */
  text(): string {
    let s = "";
    for (const n of this.ink.childNodes) {
      if (n instanceof HTMLElement && (n.classList.contains("ph") || n === this.caret)) continue;
      s += n.textContent ?? "";
    }
    return s;
  }

  async play(script: Token[], signal?: AbortSignal): Promise<void> {
    // Boundaries are taken from the following token, so work on a copy.
    const tokens = script.map((t) => ({ ...t }));
    this.field.classList.add("typing");
    try {
      for (let i = 0; i < tokens.length; i++) {
        const tok = tokens[i];
        if (!tok) continue;
        if (tok.fix === undefined) {
          await this.typeText(tok.text, signal);
          continue;
        }
        // Type the word as written, then the boundary key fires the fix.
        const span = document.createElement("span");
        this.insert(span);
        this.node = null;
        for (const ch of tok.text) {
          span.textContent += ch;
          await this.clock.wait(this.delay(ch), signal);
        }
        const next = tokens[i + 1];
        const boundary = next && next.fix === undefined ? (next.text[0] ?? "") : "";
        if (next && boundary) tokens[i + 1] = { text: next.text.slice(1) };
        await this.clock.wait(this.delay(" ") * 0.6, signal);
        this.applyFix(span, tok, boundary);
        this.opts.onEvent?.("fix", tok);
        if (tok.undo) {
          await this.clock.wait(820 * this.pace(), signal);
          this.applyUndo(span, tok);
          this.opts.onEvent?.("undo", tok);
          await this.clock.wait(900 * this.pace(), signal);
        }
      }
    } finally {
      this.field.classList.remove("typing");
    }
  }

  private pace(): number {
    return this.opts.pace ?? 1;
  }

  private delay(ch: string): number {
    const base = 52 + Math.random() * 58;
    const extra = ch === " " ? 30 : /[,.?!]/.test(ch) ? 150 : 0;
    return (base + extra) * this.pace();
  }

  private insert(n: Node): void {
    this.ink.querySelector(".ph")?.remove();
    this.ink.insertBefore(n, this.caret);
  }

  private async typeText(text: string, signal?: AbortSignal): Promise<void> {
    for (const ch of text) {
      if (!this.node) {
        this.node = document.createTextNode("");
        this.insert(this.node);
      }
      this.node.data += ch;
      if (ch === " ") this.opts.onEvent?.("word", { text: "" });
      await this.clock.wait(this.delay(ch), signal);
    }
  }

  private applyFix(span: HTMLElement, tok: Token, boundary: string): void {
    span.textContent = tok.fix ?? tok.text;
    if (boundary) {
      this.node = document.createTextNode(boundary);
      this.insert(this.node);
    }
    const style = this.opts.pill ?? "underline-capsule";
    if (style === "off") return;
    if (style !== "capsule") {
      span.classList.add("fx");
      // Next frame, so the underline fades in rather than popping.
      requestAnimationFrame(() => span.classList.add("lit"));
    }
    if (style !== "underline") this.showPill(span, tok.text, tok.fix ?? "", false);
    const hold = this.opts.holdMs ?? 1100;
    window.clearTimeout(this.hideTimer);
    this.hideTimer = window.setTimeout(() => {
      this.pill?.classList.remove("show");
      span.classList.remove("lit");
    }, hold);
  }

  private applyUndo(span: HTMLElement, tok: Token): void {
    span.textContent = tok.text;
    span.classList.remove("lit");
    const style = this.opts.pill ?? "underline-capsule";
    if (style === "off" || style === "underline") return;
    this.showPill(span, tok.fix ?? "", tok.text, true);
    window.clearTimeout(this.hideTimer);
    this.hideTimer = window.setTimeout(() => this.pill?.classList.remove("show"), this.opts.holdMs ?? 1100);
  }

  private showPill(span: HTMLElement, from: string, to: string, undone: boolean): void {
    const pill = this.pill;
    if (!pill) return;
    const s = document.createElement("s");
    s.textContent = from;
    const arr = document.createElement("span");
    arr.className = "arr";
    arr.textContent = "→";
    const b = document.createElement("b");
    b.textContent = to;
    const label = document.createElement(undone ? "span" : "kbd");
    label.className = undone ? "done" : "";
    label.textContent = undone ? "undone" : "⌫ undo";
    pill.replaceChildren(s, arr, b, label);
    placePill(this.field, pill, span, this.opts.position ?? "above");
    pill.classList.add("show");
  }
}

/** Position a pill over (or under) a word, inside the field, using transforms only. */
export function placePill(field: HTMLElement, pill: HTMLElement, word: HTMLElement, position: "above" | "below"): void {
  const f = field.getBoundingClientRect();
  const r = word.getBoundingClientRect();
  // Scenes can sit inside a scaled parent; work in the field's own (unscaled) pixels.
  const k = field.offsetWidth ? f.width / field.offsetWidth : 1;
  const left = (r.left - f.left) / k;
  const top = (r.top - f.top) / k;
  const bottom = (r.bottom - f.top) / k;
  const pw = pill.offsetWidth;
  const ph = pill.offsetHeight;
  const min = Number(field.dataset["pillMin"] ?? 0);
  const x = Math.max(min, Math.min(left - 10, field.offsetWidth - pw));
  const y = position === "above" ? top - ph - 5 : bottom + 7;
  pill.style.transform = `translate(${Math.round(x)}px, ${Math.round(y)}px)`;
}

/** Run `fn` once the element first comes near the viewport, unless the visitor prefers less motion. */
export function whenNear(el: Element, fn: () => void): void {
  if (reducedMotion()) return;
  const io = new IntersectionObserver(
    (entries) => {
      if (entries.some((e) => e.isIntersecting)) {
        io.disconnect();
        fn();
      }
    },
    { rootMargin: "200px 0px" },
  );
  io.observe(el);
}
