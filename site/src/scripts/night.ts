import { revealFallback, scrollScenes } from "./scroll";

revealFallback();

/** The home page's night stretch (.story). With scroll-driven animations CSS handles the fade (see .dusk), with or
    without reduced motion; this is the fallback for browsers without them: night is a class while the story crosses
    mid-screen. */
const story = document.querySelector<HTMLElement>(".story");
if (story && !scrollScenes) {
  new IntersectionObserver(([e]) => document.documentElement.classList.toggle("is-night", e?.isIntersecting ?? false), {
    rootMargin: "-50% 0px -50% 0px",
  }).observe(story);
}
