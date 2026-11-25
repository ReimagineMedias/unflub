// Screenshots of every page at desktop and mobile, light and dark, into .local/shots/.
// Usage: node scripts/shots.mjs [baseUrl] [--full] [--only=home|download|privacy...]
import { mkdirSync, writeFileSync } from "node:fs";
import { withChrome } from "./chrome.mjs";

const base = process.argv[2] && !process.argv[2].startsWith("--") ? process.argv[2] : "http://localhost:4321";
const full = process.argv.includes("--full");
const only = process.argv.find((a) => a.startsWith("--only="))?.slice(7);
const pages = ["/", "/download/", "/privacy/", "/terms/", "/autocorrect-windows/", "/spell-check-everywhere/", "/nope/"].filter(
  (p) => !only || (p.replaceAll("/", "") || "home") === only,
);
const sizes = [
  { name: "desktop", width: 1440, height: 900, scale: 1, mobile: false },
  { name: "mobile", width: 390, height: 844, scale: 2, mobile: true },
];
const out = new URL("../.local/shots/", import.meta.url);
mkdirSync(out, { recursive: true });

await withChrome(async (page) => {
  for (const size of sizes) {
    for (const scheme of ["light", "dark"]) {
      await page.viewport(size.width, size.height, size);
      await page.media({ scheme });
      for (const path of pages) {
        await page.goto(base + path, 1500);
        const name = `${path.replaceAll("/", "") || "home"}-${size.name}-${scheme}.png`;
        writeFileSync(new URL(name, out), await page.shot({ full }));
        console.log(name);
      }
    }
  }
});
