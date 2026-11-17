/** Facts used across the site. Change a link or version here, not in the pages. */
export const SITE = {
  name: "unflub",
  url: "https://unflub.rmgn.net",
  version: "0.1.0",
  tagline: "Phone-style autocorrect for every text box on your computer.",
  repo: "https://github.com/ReimagineMedias/unflub",
  releases: "https://github.com/ReimagineMedias/unflub/releases",
  /** The Windows download: rmgn-api redirects to the current setup and counts the download by `ref`.
   *  scripts/download.ts adds the visitor's `ref` and a Turnstile token (`t`) when a site key is set. */
  downloadApi: "https://api.rmgn.net/unflub/v1/download/windows",
  download: "https://api.rmgn.net/unflub/v1/download/windows?ref=site",
  /** `ref` values rmgn-api counts separately; anything else is sent as "site". Keep in step with rmgn-api. */
  refs: ["site", "github", "readme", "hn", "ph", "reddit", "x", "linkedin", "alternativeto", "winget"],
  license: "https://github.com/ReimagineMedias/unflub/blob/main/LICENSE",
  trademarks: "https://github.com/ReimagineMedias/unflub/blob/main/TRADEMARKS.md",
  issues: "https://github.com/ReimagineMedias/unflub/issues",
  /** GitHub Sponsors for now; swap here if sponsorship moves. */
  sponsor: "https://github.com/sponsors/ReimagineMedias",
  /** Said wherever we ask for support. */
  sponsorPitch: "Sponsors fund unflub's development and get priority on feature requests and bug reports.",
  ogImage: "/og/unflub.png",
  org: {
    name: "RMGN",
    url: "https://rmgn.ca",
  },
} as const;
