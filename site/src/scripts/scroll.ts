/** Shared scroll helpers. Scroll effects are CSS scroll-driven animations; scripts only pick which scene plays
    and add the IntersectionObserver fallback where those animations are missing. */
import { reducedMotion } from "./typer";

export const motion = !reducedMotion();
/** The pinned, scroll-driven scenes. Reduced motion keeps them (same page), with still keyframes instead of moving ones. */
export const scrollScenes = CSS.supports("animation-timeline: view()");

/** Calls `cb` with 0..1 while `el` is pinned: from its top reaching the viewport top to its bottom reaching the bottom.
    One rect read per frame, nothing written to layout. */
export function pinProgress(el: HTMLElement, cb: (p: number) => void): void {
  let queued = false;
  const read = (): void => {
    queued = false;
    const r = el.getBoundingClientRect();
    const span = r.height - window.innerHeight;
    cb(span > 0 ? Math.min(1, Math.max(0, -r.top / span)) : 0);
  };
  const ask = (): void => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(read);
  };
  window.addEventListener("scroll", ask, { passive: true });
  window.addEventListener("resize", ask);
  read();
}

/** Fallback: without scroll-driven animations, [data-reveal] elements get `.in` once they come into view. */
export function revealFallback(): void {
  if (scrollScenes) return;
  const io = new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        if (!e.isIntersecting) continue;
        e.target.classList.add("in");
        io.unobserve(e.target);
      }
    },
    { rootMargin: "0px 0px -12% 0px" },
  );
  for (const el of document.querySelectorAll("[data-reveal]")) io.observe(el);
}
