/** Facts used across the site. Change a link or version here, not in the pages. */
export const SITE = {
  name: "unflub",
  url: "https://unflub.rmgn.ca",
  version: "0.1.0",
  tagline: "Phone-style autocorrect for every text box on your computer.",
  repo: "https://github.com/ReimagineMedias/unflub",
  releases: "https://github.com/ReimagineMedias/unflub/releases",
  /** The Windows download: rmgn-api redirects to the current setup and counts the download by `ref`. */
  download: "https://api.rmgn.ca/unflub/download/windows?ref=site",
  license: "https://github.com/ReimagineMedias/unflub/blob/main/LICENSE",
  trademarks: "https://github.com/ReimagineMedias/unflub/blob/main/TRADEMARKS.md",
  issues: "https://github.com/ReimagineMedias/unflub/issues",
  /** GitHub Sponsors for now; swap here if sponsorship moves. */
  sponsor: "https://github.com/sponsors/ReimagineMedias",
  ogImage: "/og/unflub.png",
  org: {
    name: "RMGN",
    url: "https://rmgn.ca",
  },
} as const;
