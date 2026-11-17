import { Clock, Typer, parseScript, reducedMotion } from "./typer";

/** "Everywhere you type": the step in the middle of the screen picks the window, which replays its fix. */
const section = document.querySelector<HTMLElement>("#everywhere");
const stage = section?.querySelector<HTMLElement>("[data-ew-stage]");

if (section && stage) {
  const steps = [...section.querySelectorAll<HTMLElement>("[data-step]")];
  const wins = [...stage.querySelectorAll<HTMLElement>("[data-ctx]")];
  const motion = !reducedMotion();
  const clock = new Clock(stage);
  const typers = wins.map((w) => {
    const field = w.querySelector<HTMLElement>(".field[data-script]");
    return field
      ? new Typer(field, clock, { position: field.dataset["pill"] === "below" ? "below" : "above", pace: 0.9 })
      : null;
  });
  let active = 0;
  let controller: AbortController | null = null;

  const play = async (i: number): Promise<void> => {
    controller?.abort();
    const typer = typers[i];
    if (!motion || !typer) return;
    const c = new AbortController();
    controller = c;
    try {
      typer.clear();
      await clock.wait(450, c.signal);
      await typer.play(parseScript(typer.field.dataset["script"] ?? ""), c.signal);
    } catch {
      // Interrupted by another step; that step takes over.
    }
  };

  const activate = (i: number): void => {
    if (i === active) return;
    active = i;
    steps.forEach((s, n) => s.classList.toggle("is-active", n === i));
    wins.forEach((w, n) => w.classList.toggle("is-active", n === i));
    void play(i);
  };

  section.classList.add("live");
  // On narrow screens the window sits on top, so read the step from the lower part of the screen.
  const narrow = window.matchMedia("(max-width: 900px)").matches;
  const io = new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        if (e.isIntersecting) activate(Number((e.target as HTMLElement).dataset["step"]));
      }
    },
    { rootMargin: narrow ? "-68% 0px -24% 0px" : "-46% 0px -46% 0px" },
  );
  for (const s of steps) io.observe(s);

  // The first window plays once it comes into view.
  const first = new IntersectionObserver((entries) => {
    if (entries.some((e) => e.isIntersecting)) {
      first.disconnect();
      if (active === 0) void play(0);
    }
  });
  first.observe(stage);
}
