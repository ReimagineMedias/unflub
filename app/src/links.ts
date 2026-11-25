/** Addresses the window opens. Keep in step with src-tauri/src/links.rs (a test checks). */
export const LINKS = {
  sponsor: "https://github.com/sponsors/ReimagineMedias",
  repo: "https://github.com/ReimagineMedias/unflub",
  site: "https://unflub.rmgn.net/",
  terms: "https://unflub.rmgn.net/terms/",
  privacy: "https://unflub.rmgn.net/privacy/",
} as const;

/**
 * A link the opener plugin may open: the scope in capabilities/default.json allows these two
 * sites only. Use `<ExternalLink>`; raw `<a>` links do nothing in the app window.
 */
export type ExternalUrl = `https://github.com/${string}` | `https://unflub.rmgn.net/${string}`;

export function isExternalUrl(url: string): url is ExternalUrl {
  return (url.startsWith("https://github.com/") || url.startsWith("https://unflub.rmgn.net/")) && !/\s/.test(url);
}
