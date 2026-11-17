/** Pure parts of the download flow, kept free of the DOM so tests/download.test.mjs can run them in Node. */

/** How long the Turnstile script may take to load before we give up on it. */
export const SCRIPT_TIMEOUT_MS = 8000;
/** How long a non-interactive check may take, script included, before the GitHub fallback. */
export const CHECK_TIMEOUT_MS = 12000;
/** How long the "opening GitHub instead" message shows before the fallback opens. */
export const FALLBACK_DELAY_MS = 2500;

/** The ref to count: the landing page's ?ref= if rmgn-api counts it, else the one kept for this visit, else "site". */
export function pickRef(search: string, kept: string | null, known: ReadonlySet<string>): string {
  const fromUrl = new URLSearchParams(search).get("ref") ?? "";
  if (known.has(fromUrl)) return fromUrl;
  if (kept && known.has(kept)) return kept;
  return "site";
}

/** The API link, with the ref and (when there is one) the Turnstile token as `t`. */
export function downloadUrl(api: string, ref: string, token?: string): string {
  const url = new URL(api);
  url.searchParams.set("ref", ref);
  if (token) url.searchParams.set("t", token);
  return url.href;
}

/** Whether a link points at the download endpoint itself (same origin and path), not somewhere else. */
export function isDownloadHref(href: string, api: string): boolean {
  try {
    const a = new URL(href);
    const b = new URL(api);
    return a.origin === b.origin && a.pathname === b.pathname;
  } catch {
    return false;
  }
}

export class TimeoutError extends Error {
  constructor(what: string) {
    super(`${what} timed out`);
    this.name = "TimeoutError";
  }
}

/** Rejects with TimeoutError if `p` hasn't settled within `ms`. `stop()` cancels the deadline. */
export function withDeadline<T>(p: Promise<T>, ms: number, what: string): { promise: Promise<T>; stop: () => void } {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const deadline = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new TimeoutError(what)), ms);
  });
  const promise = Promise.race([p, deadline]).finally(() => clearTimeout(timer));
  return { promise, stop: () => clearTimeout(timer) };
}
