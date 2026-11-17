import type { Clock } from "./typer";

const ICON_PAUSE = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M5.5 3.5v9M10.5 3.5v9"/></svg>';
const ICON_PLAY = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M5 3.2v9.6L12.6 8z"/></svg>';

/**
 * A looping demo's pause control (WCAG 2.2.2). The button pauses and resumes the scene's clock. The loop also stops by
 * itself after `cycles` rounds, ending on its last frame; Play starts it again.
 */
export class DemoControl {
  private done = 0;
  private ended = false;
  private resume: (() => void) | null = null;

  constructor(
    clock: Clock,
    private readonly button: HTMLButtonElement | null,
    private readonly cycles = 3,
  ) {
    if (!button) return;
    button.hidden = false;
    this.render(false);
    button.addEventListener("click", () => {
      if (this.ended) {
        this.ended = false;
        this.done = 0;
        this.render(false);
        this.resume?.();
        this.resume = null;
        return;
      }
      const pause = button.dataset["state"] !== "paused";
      clock.setPaused(pause);
      this.render(pause);
    });
  }

  /** Await before each round. Resolves at once for the first rounds, then waits for Play. */
  async round(): Promise<void> {
    if (this.done++ < this.cycles) return;
    this.ended = true;
    this.render(true);
    await new Promise<void>((r) => (this.resume = r));
    this.done = 1;
  }

  private render(stopped: boolean): void {
    const b = this.button;
    if (!b) return;
    b.dataset["state"] = stopped ? "paused" : "playing";
    b.setAttribute("aria-label", stopped ? (this.ended ? "Play the animation again" : "Play animation") : "Pause animation");
    b.innerHTML = stopped ? ICON_PLAY : ICON_PAUSE;
  }
}
