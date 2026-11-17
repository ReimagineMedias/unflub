import { Clock, Typer, parseScript } from "./typer";
import { motion, pinProgress, scrollDriven } from "./scroll";

/** "Everywhere you type": one app window types at a time. With scroll timelines the rail is pinned and the scroll
    position picks the card; otherwise the card in the middle of the screen does. */
const section = document.querySelector<HTMLElement>("#everywhere");

if (section) {
  const figures = [...section.querySelectorAll<HTMLElement>("[data-app]")];
  const names = [...section.querySelectorAll<HTMLButtonElement>("[data-goto]")];
  const clock = new Clock(section);
  const typers = figures.map((f) => {
    const field = f.querySelector<HTMLElement>(".field[data-script]");
    return field
      ? new Typer(field, clock, { position: field.dataset["pill"] === "below" ? "below" : "above", pace: 0.9 })
      : null;
  });
  let current = -1;
  let controller: AbortController | null = null;

  const play = async (i: number): Promise<void> => {
    controller?.abort();
    const typer = typers[i];
    if (!motion || !typer) return;
    const c = new AbortController();
    controller = c;
    try {
      typer.clear();
      await clock.wait(380, c.signal);
      await typer.play(parseScript(typer.field.dataset["script"] ?? ""), c.signal);
    } catch {
      // Another card took over.
    }
  };

  const show = (i: number): void => {
    if (i === current) return;
    current = i;
    figures.forEach((f, n) => f.classList.toggle("is-active", n === i));
    names.forEach((b, n) => {
      b.classList.toggle("on", n === i);
      b.setAttribute("aria-pressed", String(n === i));
    });
    void play(i);
  };

  // Matches the `slide` keyframes: each card holds for 12% of the pin, each move takes 10%.
  const holds = [6, 28, 50, 72, 94];
  if (scrollDriven) {
    pinProgress(section, (p) => show(Math.max(0, Math.min(figures.length - 1, Math.floor((p * 100 + 5) / 22)))));
  } else {
    show(0);
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) if (e.isIntersecting) show(figures.indexOf(e.target as HTMLElement));
      },
      { rootMargin: "-45% 0px -45% 0px" },
    );
    for (const f of figures) io.observe(f);
  }

  for (const b of names) {
    b.addEventListener("click", () => {
      const i = Number(b.dataset["goto"]);
      const span = section.offsetHeight - window.innerHeight;
      window.scrollTo({ top: section.offsetTop + (span * (holds[i] ?? 0)) / 100 });
    });
  }
}
