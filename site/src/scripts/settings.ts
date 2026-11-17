import { Clock, Typer, parseScript, reducedMotion, type PillStyle } from "./typer";
import { DemoControl } from "./demo";

/** Makes the settings recreation respond: pages, themes, light or dark, switches, pill styles. */
const app = document.querySelector<HTMLElement>("[data-settings]");

if (app) {
  const dark = window.matchMedia("(prefers-color-scheme: dark)");
  let modePick = "auto";
  const applyMode = (): void => {
    app.dataset["mode"] = modePick === "auto" ? (dark.matches ? "dark" : "light") : modePick;
  };
  applyMode();
  dark.addEventListener("change", applyMode);

  /** Wire a segmented control: one pressed button at a time. */
  const segmented = (group: Element | null, onPick: (value: string, button: HTMLButtonElement) => void): void => {
    if (!group) return;
    const buttons = [...group.querySelectorAll<HTMLButtonElement>("button")];
    for (const b of buttons) {
      b.addEventListener("click", () => {
        for (const o of buttons) o.setAttribute("aria-pressed", String(o === b));
        onPick(b.dataset["value"] ?? b.dataset["themePick"] ?? "", b);
      });
    }
  };

  // Pages
  const navButtons = [...app.querySelectorAll<HTMLButtonElement>("[data-page]")];
  const panels = [...app.querySelectorAll<HTMLElement>("[data-panel]")];
  for (const b of navButtons) {
    b.addEventListener("click", () => {
      for (const o of navButtons) {
        if (o === b) o.setAttribute("aria-current", "page");
        else o.removeAttribute("aria-current");
      }
      for (const p of panels) p.hidden = p.dataset["panel"] !== b.dataset["page"];
    });
  }

  segmented(app.querySelector(".themes"), (v) => (app.dataset["theme"] = v));
  segmented(app.querySelector("[data-mode-pick]"), (v) => {
    modePick = v;
    applyMode();
  });

  for (const s of app.querySelectorAll<HTMLButtonElement>(".switch")) {
    s.addEventListener("click", () => s.setAttribute("aria-checked", String(s.getAttribute("aria-checked") !== "true")));
  }

  const hint = app.querySelector<HTMLElement>("[data-engine-hint]");
  segmented(app.querySelector("[data-engine]"), (_v, b) => {
    if (hint) hint.textContent = b.dataset["hint"] ?? "";
  });

  const range = app.querySelector<HTMLInputElement>("[data-confidence]");
  const out = range?.nextElementSibling;
  range?.addEventListener("input", () => {
    const v = Number(range.value);
    if (out) out.textContent = v < 0.35 ? "eager" : v > 0.75 ? "cautious" : "balanced";
  });

  const form = app.querySelector<HTMLFormElement>("[data-add-word]");
  const chips = app.querySelector<HTMLElement>("[data-words]");
  form?.addEventListener("submit", (e) => {
    e.preventDefault();
    const input = form.querySelector("input");
    const word = input?.value.trim();
    if (!input || !word || !chips) return;
    const chip = document.createElement("span");
    chip.className = "chip";
    chip.textContent = word;
    chips.append(chip);
    input.value = "";
  });

  // Live preview of the chosen pill style, like the app's own preview.
  const field = app.querySelector<HTMLElement>(".preview .field");
  const clock = field ? new Clock(field) : null;
  const typer = field && clock && !reducedMotion() ? new Typer(field, clock, { pill: "underline-capsule", pace: 0.9 }) : null;
  // The buttons respond even when the preview stays still.
  segmented(app.querySelector("[data-pill-style]"), (v) => {
    if (typer) typer.opts.pill = v as PillStyle;
  });
  if (field && clock && typer) {
    const control = new DemoControl(clock, document.querySelector<HTMLButtonElement>('[data-demo-toggle="settings"]'));
    let started = false;
    const io = new IntersectionObserver((entries) => {
      if (started || !entries.some((e) => e.isIntersecting)) return;
      started = true;
      void (async () => {
        for (;;) {
          await control.round();
          typer.clear();
          await clock.wait(500);
          await typer.play(parseScript(field.dataset["script"] ?? ""));
          await clock.wait(2400);
        }
      })();
    });
    io.observe(field);
  }
}
