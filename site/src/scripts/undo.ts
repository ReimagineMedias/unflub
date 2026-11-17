import { Clock, Typer, parseScript, reducedMotion } from "./typer";
import { DemoControl } from "./demo";

/** "Backspace means no": fix, undo, remember. The numbered steps follow along. */
const win = document.querySelector<HTMLElement>("[data-undo]");
const field = win?.querySelector<HTMLElement>(".field");
const toast = win?.querySelector<HTMLElement>(".toast");
const steps = [...document.querySelectorAll<HTMLElement>("[data-undo-steps] > li")];

if (win && field && toast && !reducedMotion()) {
  const section = win.closest("section");
  const clock = new Clock(win);
  const control = new DemoControl(clock, document.querySelector<HTMLButtonElement>('[data-demo-toggle="undo"]'));
  const step = (i: number): void => steps.forEach((s, n) => s.classList.toggle("is-active", n === i));
  const typer = new Typer(field, clock, {
    holdMs: 1400,
    pace: 1.05,
    onEvent: (e) => {
      if (e === "fix") step(0);
      if (e === "undo") step(1);
    },
  });

  const run = async (): Promise<void> => {
    section?.classList.add("live");
    for (;;) {
      await control.round();
      toast.classList.remove("show");
      step(-1);
      typer.clear();
      await clock.wait(900);
      await typer.play(parseScript(field.dataset["a"] ?? ""));
      toast.classList.add("show");
      step(2);
      await typer.play(parseScript(field.dataset["b"] ?? ""));
      await clock.wait(3200);
    }
  };

  const io = new IntersectionObserver((entries) => {
    if (entries.some((e) => e.isIntersecting)) {
      io.disconnect();
      void run();
    }
  });
  io.observe(win);
}
