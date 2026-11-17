import { Clock, Typer, parseScript, reducedMotion } from "./typer";
import { DemoControl } from "./demo";

/** The hero chat: messages type themselves with real typos, unflub fixes them, then they send. */
const CONVERSATION: { me: string; reply: { who: string; av: string; text: string } }[] = [
  { me: "Running late, {teh|the} train is stuck again.", reply: { who: "Maya", av: "M", text: "Classic. We'll order for you." } },
  { me: "You're the best. {Definately|Definitely} owe you one.", reply: { who: "Theo", av: "T", text: "Dumplings it is." } },
  { me: "Perfect, {thier|their} dumplings are unreal.", reply: { who: "Maya", av: "M", text: "Agreed. Get here!" } },
  { me: "Off the train, {probaly|probably} five minutes.", reply: { who: "Theo", av: "T", text: "Saved you a seat." } },
];

const win = document.querySelector<HTMLElement>("[data-hero]");
const field = win?.querySelector<HTMLElement>(".composer .field");
const track = win?.querySelector<HTMLElement>(".track");

if (win && field && track && !reducedMotion()) {
  const clock = new Clock(win);
  const typer = new Typer(field, clock);
  const control = new DemoControl(clock, document.querySelector<HTMLButtonElement>('[data-demo-toggle="hero"]'));

  /** Add a bubble at the bottom; older bubbles slide up using transforms only. */
  const post = (el: HTMLElement): void => {
    el.style.opacity = "0";
    track.append(el);
    const h = el.offsetHeight + 10;
    track.style.transition = "none";
    track.style.transform = `translateY(${h}px)`;
    void track.offsetHeight;
    track.style.transition = "transform 0.42s cubic-bezier(0.2, 0.8, 0.2, 1)";
    track.style.transform = "";
    el.style.transition = "opacity 0.3s ease 0.08s";
    el.style.opacity = "1";
    while (track.children.length > 9) track.firstElementChild?.remove();
  };

  const bubble = (text: string, from?: { who: string; av: string }): HTMLElement => {
    const msg = document.createElement("div");
    msg.className = from ? "msg them" : "msg me";
    const p = document.createElement("p");
    if (from) {
      const av = document.createElement("span");
      av.className = "av";
      av.textContent = from.av;
      const who = document.createElement("span");
      who.className = "who";
      who.textContent = from.who;
      p.append(who);
      msg.append(av);
    }
    p.append(text);
    msg.append(p);
    return msg;
  };

  const run = async (): Promise<void> => {
    // Let the still frame (a fix mid-flight) register before the loop takes over.
    await clock.wait(1600);
    for (;;) {
      await control.round();
      for (const turn of CONVERSATION) {
        typer.clear();
        await clock.wait(700);
        await typer.play(parseScript(turn.me));
        await clock.wait(1250);
        const text = typer.text();
        typer.clear();
        post(bubble(text));
        await clock.wait(1300);
        post(bubble(turn.reply.text, turn.reply));
        await clock.wait(900);
      }
    }
  };
  void run();
}
