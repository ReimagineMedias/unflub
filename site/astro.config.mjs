// @ts-check
import { defineConfig } from "astro/config";
import sitemap from "@astrojs/sitemap";

export default defineConfig({
  site: "https://unflub.rmgn.net",
  trailingSlash: "always",
  output: "static",
  integrations: [sitemap({ filter: (page) => !page.includes("/404") })],
  build: {
    format: "directory",
    // One small stylesheet per page: inline it so first paint needs no extra request.
    inlineStylesheets: "always",
  },
  vite: {
    build: {
      // Keep every script in its own file so the CSP can stay at script-src 'self'.
      assetsInlineLimit: 0,
    },
  },
});
