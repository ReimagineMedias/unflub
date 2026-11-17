// Renders brand SVGs to the PNGs the app and bundler need. Run: npm run icons
import { Resvg } from "@resvg/resvg-js";
import { readFileSync, writeFileSync, mkdirSync, rmSync } from "node:fs";
import { execSync } from "node:child_process";

const brand = new URL("../../assets/brand/", import.meta.url);
const out = new URL("../src-tauri/icons/", import.meta.url);
const tray = new URL("../src-tauri/icons/tray/", import.meta.url);
mkdirSync(tray, { recursive: true });

const render = (file, width) =>
  new Resvg(readFileSync(new URL(file, brand)), { fitTo: { mode: "width", value: width } }).render().asPng();

writeFileSync(new URL("source-1024.png", out), render("unflub-icon.svg", 1024));
for (const name of ["tray-on-dark", "tray-on-light", "tray-paused-dark", "tray-paused-light"]) {
  writeFileSync(new URL(`${name}.png`, tray), render(`${name}.svg`, 32));
}
// The settings window's taskbar icon while unflub is paused: the app icon in greys.
const paused = readFileSync(new URL("unflub-icon.svg", brand), "utf8")
  .replace("#2D1916", "#2B2B2B").replace("#F2E6E3", "#D6D6D6").replace("#B06A5C", "#8A8A8A");
writeFileSync(new URL("icon-paused.png", out), new Resvg(paused, { fitTo: { mode: "width", value: 256 } }).render().asPng());
execSync("npx tauri icon src-tauri/icons/source-1024.png -o src-tauri/icons", { stdio: "inherit" });
for (const d of ["android", "ios"]) rmSync(new URL(d, out), { recursive: true, force: true });
