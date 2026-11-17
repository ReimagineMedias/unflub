/**
 * Download buttons. Each one goes to rmgn-api with the visitor's `ref` (from ?ref= on the page they landed on, if
 * rmgn-api counts it, else "site"). With a Turnstile site key set at build time, a click first passes an invisible
 * Turnstile check and adds its token as `t`. The Turnstile script loads only when someone reaches for a download
 * button, never on page load. No key (local dev): the links go straight to the API.
 */

interface TurnstileOptions {
  sitekey: string;
  action: string;
  appearance: "interaction-only";
  retry: "never";
  "response-field": false;
  callback: (token: string) => void;
  "error-callback": () => void;
  "expired-callback": () => void;
  "timeout-callback": () => void;
  "before-interactive-callback": () => void;
  "after-interactive-callback": () => void;
}

interface TurnstileApi {
  render(container: HTMLElement, options: TurnstileOptions): string | undefined;
  remove(widgetId: string): void;
}

declare global {
  interface Window {
    turnstile?: TurnstileApi;
  }
}

const body = document.body;
const api = body.dataset["downloadApi"] ?? "";
const known = new Set((body.dataset["downloadRefs"] ?? "").split(" ").filter(Boolean));
const siteKey = body.dataset["turnstile"] ?? "";
const releases = body.dataset["releases"] ?? "";

/** The landing page's ?ref=, kept for the visit so it survives a click through to /download/. */
function pickRef(): string {
  const fromUrl = new URLSearchParams(location.search).get("ref") ?? "";
  if (known.has(fromUrl)) {
    try {
      sessionStorage.setItem("unflub-ref", fromUrl);
    } catch {
      // Storage off: the ref still applies on this page.
    }
    return fromUrl;
  }
  try {
    const kept = sessionStorage.getItem("unflub-ref") ?? "";
    if (known.has(kept)) return kept;
  } catch {
    // Storage off.
  }
  return "site";
}

const ref = pickRef();
const target = (token?: string): string => {
  const url = new URL(api);
  url.searchParams.set("ref", ref);
  if (token) url.searchParams.set("t", token);
  return url.href;
};

/** Download links that still point at the API (os.ts sends phones, Mac and Linux to /download/ instead). */
const isDownload = (a: HTMLAnchorElement): boolean => a.href.startsWith(api);
const links = [...document.querySelectorAll<HTMLAnchorElement>("a[data-dl]")].filter(isDownload);
for (const a of links) a.href = target();

let script: Promise<TurnstileApi> | null = null;
function loadTurnstile(): Promise<TurnstileApi> {
  script ??= new Promise<TurnstileApi>((resolve, reject) => {
    const s = document.createElement("script");
    s.src = "https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit";
    s.async = true;
    s.addEventListener("load", () => (window.turnstile ? resolve(window.turnstile) : reject(new Error("no turnstile"))), {
      once: true,
    });
    s.addEventListener("error", () => reject(new Error("turnstile failed to load")), { once: true });
    document.head.append(s);
  }).catch((e: unknown) => {
    script = null;
    throw e;
  });
  return script;
}

const box = document.querySelector<HTMLElement>("[data-human-check]");
const slot = document.querySelector<HTMLElement>("[data-human-widget]");
let widget: string | undefined;
let busy = false;

/** One Turnstile run per click. Resolves with a token, or null if the check could not finish. */
async function token(): Promise<string | null> {
  const ts = await loadTurnstile();
  if (!slot) return null;
  if (widget) ts.remove(widget);
  return new Promise<string | null>((resolve) => {
    const done = (t: string | null): void => {
      if (box) box.hidden = true;
      resolve(t);
    };
    widget = ts.render(slot, {
      sitekey: siteKey,
      action: "download",
      appearance: "interaction-only",
      retry: "never",
      "response-field": false,
      callback: (t) => done(t),
      "error-callback": () => done(null),
      "expired-callback": () => done(null),
      "timeout-callback": () => done(null),
      "before-interactive-callback": () => {
        if (box) box.hidden = false;
      },
      "after-interactive-callback": () => {
        if (box) box.hidden = true;
      },
    });
  });
}

if (siteKey && api) {
  const warm = (): void => {
    loadTurnstile().catch(() => undefined);
  };
  for (const a of links) {
    a.addEventListener("pointerenter", warm, { once: true });
    a.addEventListener("focus", warm, { once: true });
    a.addEventListener("click", (e) => {
      // New tab, new window or a download-as: let the browser follow the plain link.
      if (e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || !isDownload(a)) return;
      e.preventDefault();
      if (busy) return;
      busy = true;
      a.setAttribute("aria-busy", "true");
      token()
        .catch(() => null)
        .then((t) => {
          // If the check can't finish, the GitHub releases page still has every version.
          location.href = t ? target(t) : releases || target();
        })
        .finally(() => {
          busy = false;
          a.removeAttribute("aria-busy");
        });
    });
  }
}

export {};
