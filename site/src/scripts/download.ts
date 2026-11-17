/**
 * Download buttons. Each one goes to rmgn-api with the visitor's `ref` (from ?ref= on the page they landed on, if
 * rmgn-api counts it, else "site"). With a Turnstile site key set at build time, a click first passes an invisible
 * Turnstile check and adds its token as `t`. The Turnstile script loads only when someone reaches for a download
 * button, never on page load. No key (local dev): the links go straight to the API.
 *
 * After the check, the page asks the API (`?format=json`) where the installer is, with a short timeout, and opens
 * that address. A check or API call that fails, stalls or is unsupported (including the API's daily limit) never
 * leaves the button stuck: after a one-line message, GitHub's latest release opens instead. If Cloudflare asks for interaction, a labelled panel takes focus, with Cancel and the
 * GitHub link beside the challenge.
 */
import {
  API_TIMEOUT_MS,
  CHECK_TIMEOUT_MS,
  FALLBACK_DELAY_MS,
  SCRIPT_TIMEOUT_MS,
  downloadUrl,
  isDownloadHref,
  pickRef,
  targetFrom,
  withDeadline,
} from "./download-core";

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
  "unsupported-callback": () => void;
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

const REF_KEY = "unflub-ref";
let kept: string | null = null;
try {
  kept = sessionStorage.getItem(REF_KEY);
} catch {
  // Storage off: the ref still applies on this page.
}
const ref = pickRef(location.search, kept, known);
try {
  if (ref !== "site") sessionStorage.setItem(REF_KEY, ref);
} catch {
  // Storage off.
}

/** Download links that still point at the API (os.ts sends phones, Mac and Linux to /download/ instead). */
const isDownload = (a: HTMLAnchorElement): boolean => isDownloadHref(a.href, api);
const links = [...document.querySelectorAll<HTMLAnchorElement>("a[data-dl]")].filter(isDownload);
for (const a of links) a.href = downloadUrl(api, ref);

let script: Promise<TurnstileApi> | null = null;
let scriptTag: HTMLScriptElement | null = null;
function loadTurnstile(): Promise<TurnstileApi> {
  if (script) return script;
  const load = new Promise<TurnstileApi>((resolve, reject) => {
    const s = document.createElement("script");
    s.src = "https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit";
    s.async = true;
    s.addEventListener("load", () => (window.turnstile ? resolve(window.turnstile) : reject(new Error("no turnstile"))), {
      once: true,
    });
    s.addEventListener("error", () => reject(new Error("turnstile failed to load")), { once: true });
    scriptTag = s;
    document.head.append(s);
  });
  script = withDeadline(load, SCRIPT_TIMEOUT_MS, "turnstile script").promise.catch((e: unknown) => {
    // Forget a script that failed or stalled, so the next click starts clean.
    scriptTag?.remove();
    scriptTag = null;
    script = null;
    throw e;
  });
  return script;
}

const box = document.querySelector<HTMLElement>("[data-human-check]");
const title = document.querySelector<HTMLElement>("[data-human-title]");
const slot = document.querySelector<HTMLElement>("[data-human-widget]");
const cancel = document.querySelector<HTMLButtonElement>("[data-human-cancel]");
const status = document.querySelector<HTMLElement>("[data-dl-status]");
let clearTimer = 0;
/** A short visible status near the bottom of the screen, also read out by screen readers. */
const say = (text: string, link?: { href: string; label: string }): void => {
  if (!status) return;
  window.clearTimeout(clearTimer);
  status.replaceChildren(text);
  if (link) {
    const a = document.createElement("a");
    a.href = link.href;
    a.textContent = link.label;
    status.append(" ", a);
  }
  if (text) clearTimer = window.setTimeout(() => status.replaceChildren(), 8000);
};

type Outcome = { token: string } | { failed: true } | { cancelled: true };

let widget: string | undefined;
let busy = false;

function removeWidget(): void {
  if (widget && window.turnstile) window.turnstile.remove(widget);
  widget = undefined;
}

function showPanel(on: boolean): void {
  if (box) box.hidden = !on;
}

/** One Turnstile run per click. Settles once; late callbacks after that are ignored. */
async function check(): Promise<Outcome> {
  const ts = await loadTurnstile();
  if (!slot) return { failed: true };
  removeWidget();
  let settle: (o: Outcome) => void = () => undefined;
  const run = new Promise<Outcome>((resolve) => {
    let settled = false;
    settle = (o) => {
      if (settled) return;
      settled = true;
      resolve(o);
    };
  });
  const deadline = withDeadline(run, CHECK_TIMEOUT_MS, "turnstile check");
  const onCancel = (): void => settle({ cancelled: true });
  cancel?.addEventListener("click", onCancel, { once: true });
  widget = ts.render(slot, {
    sitekey: siteKey,
    action: "download",
    appearance: "interaction-only",
    retry: "never",
    "response-field": false,
    callback: (t) => settle({ token: t }),
    "error-callback": () => settle({ failed: true }),
    "expired-callback": () => settle({ failed: true }),
    "timeout-callback": () => settle({ failed: true }),
    "unsupported-callback": () => settle({ failed: true }),
    "before-interactive-callback": () => {
      // Cloudflare wants the visitor to act: no deadline while they do, and the panel takes focus.
      deadline.stop();
      showPanel(true);
      title?.focus();
    },
    "after-interactive-callback": () => showPanel(false),
  });
  try {
    return await deadline.promise;
  } catch {
    return { failed: true };
  } finally {
    cancel?.removeEventListener("click", onCancel);
    removeWidget();
    showPanel(false);
  }
}

/** Ask the API where the installer is. Null on any failure: timeout, error status, CORS, or an unexpected answer. */
async function installerUrl(token: string): Promise<string | null> {
  const url = new URL(downloadUrl(api, ref, token));
  url.searchParams.set("format", "json");
  const abort = new AbortController();
  const timer = window.setTimeout(() => abort.abort(), API_TIMEOUT_MS);
  try {
    const res = await fetch(url, { signal: abort.signal, credentials: "omit", cache: "no-store", referrerPolicy: "origin" });
    if (!res.ok) return null;
    return targetFrom(await res.json());
  } catch {
    return null;
  } finally {
    window.clearTimeout(timer);
  }
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
      say("Checking, then your download starts.");
      check()
        .catch((): Outcome => ({ failed: true }))
        .then(async (o) => {
          if ("token" in o) {
            const to = await installerUrl(o.token);
            if (to) {
              say("Your download is starting.");
              location.href = to;
              return;
            }
            say("Our download server didn't answer, so GitHub is opening instead.", {
              href: releases,
              label: "Open GitHub",
            });
            window.setTimeout(() => {
              location.href = releases;
            }, FALLBACK_DELAY_MS);
            return;
          }
          if ("cancelled" in o) {
            say("Download cancelled.");
            a.focus();
            return;
          }
          // The check failed or stalled: say so plainly, then open GitHub releases, which has every version.
          say("The download check didn't finish, so GitHub is opening instead.", {
            href: releases,
            label: "Open GitHub",
          });
          window.setTimeout(() => {
            location.href = releases || downloadUrl(api, ref);
          }, FALLBACK_DELAY_MS);
        })
        .finally(() => {
          busy = false;
          a.removeAttribute("aria-busy");
        });
    });
  }
}

export {};
