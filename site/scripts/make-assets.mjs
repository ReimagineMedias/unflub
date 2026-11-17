// Renders the favicon set and the Open Graph image into public/. Run after changing the
// brand icon or scripts/assets/og.html, then commit the PNGs:  npm run assets
import { readFileSync, writeFileSync } from "node:fs";
import { withChrome } from "./chrome.mjs";

const root = new URL("../", import.meta.url);
const pub = (p) => new URL(`public/${p}`, root);
const icon = readFileSync(new URL("src/assets/brand/unflub-icon.svg", root), "utf8");
// Full-bleed square for Apple and maskable icons: the platform rounds the corners itself.
const square = icon.replace(/rx="232"/, 'rx="0"');

const page = (svg, size, pad = 0) =>
  "data:text/html;base64," +
  Buffer.from(
    `<!doctype html><html><body style="margin:0;width:${size}px;height:${size}px;overflow:hidden;background:transparent">` +
      `<div style="width:${size - 2 * pad}px;height:${size - 2 * pad}px;margin:${pad}px">${svg.replace("<svg ", '<svg width="100%" height="100%" ')}</div>` +
      `</body></html>`,
  ).toString("base64");

/** Wrap PNGs into one .ico (PNG-compressed entries, supported since Windows Vista). */
function ico(images) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(images.length, 4);
  const entries = [];
  let offset = 6 + 16 * images.length;
  for (const { size, png } of images) {
    const e = Buffer.alloc(16);
    e.writeUInt8(size >= 256 ? 0 : size, 0);
    e.writeUInt8(size >= 256 ? 0 : size, 1);
    e.writeUInt16LE(1, 4);
    e.writeUInt16LE(32, 6);
    e.writeUInt32LE(png.length, 8);
    e.writeUInt32LE(offset, 12);
    offset += png.length;
    entries.push(e);
  }
  return Buffer.concat([header, ...entries, ...images.map((i) => i.png)]);
}

await withChrome(async (p) => {
  await p.transparent();
  const render = async (svg, size, pad = 0) => {
    await p.viewport(size, size);
    await p.goto(page(svg, size, pad), 200);
    return p.shot();
  };

  const icoParts = [];
  for (const size of [16, 32, 48]) icoParts.push({ size, png: await render(icon, size) });
  writeFileSync(pub("favicon.ico"), ico(icoParts));
  writeFileSync(pub("apple-touch-icon.png"), await render(square, 180));
  writeFileSync(pub("icon-192.png"), await render(icon, 192));
  writeFileSync(pub("icon-512.png"), await render(icon, 512));
  writeFileSync(pub("icon-maskable-512.png"), await render(square, 512));

  await p.viewport(1200, 630);
  await p.goto(new URL("scripts/assets/og.html", root).href, 600);
  writeFileSync(pub("og/unflub.png"), await p.shot());
});
console.log("Wrote favicon.ico, apple-touch-icon.png, icon-192.png, icon-512.png, icon-maskable-512.png, og/unflub.png");
