import { revealFallback, scrollDriven } from "./scroll";

revealFallback();

/** The home page's night stretch (.story). With scroll-driven animations CSS handles the fade (see .dusk); this is the
    fallback for browsers without them and for reduced motion: night is a class while the story crosses mid-screen. */
const story = document.querySelector<HTMLElement>(".story");
if (story && !scrollDriven) {
  new IntersectionObserver(([e]) => document.documentElement.classList.toggle("is-night", e?.isIntersecting ?? false), {
    rootMargin: "-50% 0px -50% 0px",
  }).observe(story);
}
