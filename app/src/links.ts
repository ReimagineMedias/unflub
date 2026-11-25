/** Addresses the window opens. Keep in step with src-tauri/src/links.rs (a test checks). */
export const LINKS = {
  sponsor: "https://github.com/sponsors/ReimagineMedias",
  repo: "https://github.com/ReimagineMedias/unflub",
  site: "https://unflub.rmgn.net/",
} as const;

/**
 * A link the opener plugin may open: the scope in capabilities/default.json allows our GitHub
 * owner, the sponsors page and the site only. Use `<ExternalLink>`; raw `<a>` links do nothing in
 * the app window.
 */
export type ExternalUrl = `https://github.com/${string}` | `https://unflub.rmgn.net/${string}`;

const PREFIXES = ["https://github.com/ReimagineMedias/", "https://unflub.rmgn.net/"];

export function isExternalUrl(url: string): url is ExternalUrl {
  const allowed = url === LINKS.sponsor || PREFIXES.some((p) => url.startsWith(p));
  return allowed && !/\s/.test(url) && !url.includes("/..") && !/%2e/i.test(url);
}
