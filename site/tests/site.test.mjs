// Checks the built site in dist/: every page answers 200, internal links resolve, metadata and
// JSON-LD are in place, and the production headers keep the CSP honest. Run `npm run build` first.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { parse } from "node-html-parser";
import { startServer } from "../scripts/serve.mjs";

const SITE = "https://unflub.rmgn.ca";
const dist = new URL("../dist/", import.meta.url);
let server;
let base = "";
/** @type {Map<string, import("node-html-parser").HTMLElement>} */
const pages = new Map();

before(async () => {
  assert.ok(existsSync(new URL("index.html", dist)), "dist/ is missing: run npm run build first");
  server = await startServer(0);
  base = `http://localhost:${server.address().port}`;
  for (const path of sitemapPaths()) {
    const res = await fetch(base + path);
    assert.equal(res.status, 200, path);
    pages.set(path, parse(await res.text()));
  }
});

after(() => server?.close());

function sitemapPaths() {
  const xml = readFileSync(new URL("sitemap-0.xml", dist), "utf8");
  return [...xml.matchAll(/<loc>([^<]+)<\/loc>/g)].map((m) => new URL(m[1]).pathname);
}

test("sitemap lists every public page and not the 404", () => {
  assert.deepEqual(sitemapPaths().sort(), [
    "/",
    "/autocorrect-windows/",
    "/download/",
    "/privacy/",
    "/spell-check-everywhere/",
  ]);
});

test("every page has complete metadata", () => {
  for (const [path, doc] of pages) {
    const meta = (sel) => doc.querySelector(sel)?.getAttribute("content") ?? "";
    assert.equal(doc.querySelector("html")?.getAttribute("lang"), "en-CA", `${path} lang`);
    const title = doc.querySelector("title")?.text ?? "";
    assert.ok(title.length >= 20 && title.length <= 70, `${path} title length ${title.length}`);
    const description = meta('meta[name="description"]');
    assert.ok(description.length >= 70 && description.length <= 170, `${path} description length ${description.length}`);
    assert.equal(doc.querySelector('link[rel="canonical"]')?.getAttribute("href"), SITE + path, `${path} canonical`);
    assert.equal(doc.querySelectorAll("h1").length, 1, `${path} has one h1`);
    assert.equal(meta('meta[property="og:url"]'), SITE + path, `${path} og:url`);
    assert.equal(meta('meta[name="twitter:card"]'), "summary_large_image", `${path} twitter card`);
    const og = meta('meta[property="og:image"]');
    assert.ok(og.startsWith(`${SITE}/`), `${path} og:image is absolute`);
    assert.ok(existsSync(new URL(`.${new URL(og).pathname}`, dist)), `${path} og:image file exists`);
    for (const img of doc.querySelectorAll("img")) assert.ok(img.hasAttribute("alt"), `${path} img alt`);
  }
});

test("JSON-LD parses and carries the required types", () => {
  const ld = (path) =>
    pages
      .get(path)
      .querySelectorAll('script[type="application/ld+json"]')
      .map((s) => {
        const data = JSON.parse(s.textContent);
        assert.equal(data["@context"], "https://schema.org", path);
        return data;
      });

  const home = ld("/");
  const app = home.find((d) => d["@type"] === "SoftwareApplication");
  assert.ok(app, "SoftwareApplication on home");
  assert.equal(app.operatingSystem, "Windows");
  assert.equal(app.applicationCategory, "UtilitiesApplication");
  assert.equal(app.offers.price, "0");
  assert.equal(app.softwareVersion, "0.1.0");
  assert.match(app.license, /MIT/);
  assert.ok(app.downloadUrl.startsWith("https://"));
  const faq = home.find((d) => d["@type"] === "FAQPage");
  assert.ok(faq, "FAQPage on home");
  assert.equal(
    faq.mainEntity.length,
    pages.get("/").querySelectorAll("#faq details").length,
    "FAQ JSON-LD matches the visible questions",
  );

  for (const path of pages.keys()) {
    const all = ld(path);
    assert.ok(
      all.some((d) => d["@type"] === "Organization"),
      `${path} Organization`,
    );
    if (path !== "/") {
      const crumbs = all.find((d) => d["@type"] === "BreadcrumbList");
      assert.ok(crumbs, `${path} BreadcrumbList`);
      assert.equal(crumbs.itemListElement.at(-1).item, SITE + path);
    }
  }
});

test("internal links, anchors and assets resolve", async () => {
  const seen = new Set();
  for (const [path, doc] of pages) {
    for (const el of doc.querySelectorAll("a[href], link[href], script[src], img[src]")) {
      const raw = el.getAttribute("href") ?? el.getAttribute("src") ?? "";
      const url = new URL(raw, SITE + path);
      if (url.origin !== SITE) continue;
      if (url.hash) {
        const target = pages.get(url.pathname);
        assert.ok(target?.querySelector(url.hash), `${path} links to a missing anchor ${url.pathname}${url.hash}`);
      }
      seen.add(url.pathname);
    }
  }
  for (const p of seen) {
    const res = await fetch(base + p, { redirect: "manual" });
    assert.equal(res.status, 200, `${p} should answer 200`);
  }
});

test("unknown paths get the 404 page, which is not indexed", async () => {
  const res = await fetch(`${base}/does-not-exist/`);
  assert.equal(res.status, 404);
  const doc = parse(await res.text());
  assert.equal(doc.querySelector('meta[name="robots"]')?.getAttribute("content"), "noindex");
  assert.equal(doc.querySelectorAll("h1").length, 1);
});

test("robots.txt points at the sitemap", async () => {
  const res = await fetch(`${base}/robots.txt`);
  assert.equal(res.status, 200);
  assert.match(await res.text(), /Sitemap: https:\/\/unflub\.rmgn\.ca\/sitemap-index\.xml/);
});

test("security headers are set and no page needs an inline script", async () => {
  const res = await fetch(`${base}/`);
  assert.match(res.headers.get("content-security-policy") ?? "", /script-src 'self';/);
  assert.ok(res.headers.get("strict-transport-security"));
  assert.equal(res.headers.get("x-content-type-options"), "nosniff");
  assert.ok(res.headers.get("referrer-policy"));
  for (const [path, doc] of pages) {
    for (const s of doc.querySelectorAll("script")) {
      const ok = s.hasAttribute("src") || s.getAttribute("type") === "application/ld+json";
      assert.ok(ok, `${path} has an inline script, which the CSP would block`);
    }
  }
  const asset = pages.get("/").querySelector("script[src]")?.getAttribute("src") ?? "";
  const cached = await fetch(base + asset);
  assert.match(cached.headers.get("cache-control") ?? "", /immutable/);
});

test("scroll-driven animations keep their timeline out of the animation shorthand", () => {
  // Browsers reject `animation: ... view()`; the minifier produces it if one rule sets both.
  const css = (path) =>
    pages
      .get(path)
      .querySelectorAll("style")
      .map((s) => s.textContent)
      .join("\n");
  for (const path of pages.keys()) {
    assert.doesNotMatch(css(path), /animation:[^;}]*(view\(|scroll\()/, `${path} folds a timeline into the shorthand`);
  }
  assert.match(css("/"), /animation-timeline:scroll\(root\)/);
});

test("wrangler config serves dist on the custom domain", () => {
  const text = readFileSync(new URL("../wrangler.jsonc", import.meta.url), "utf8")
    .replace(/^\s*\/\/.*$/gm, "")
    .replace(/,(\s*[}\]])/g, "$1");
  const cfg = JSON.parse(text);
  assert.equal(cfg.name, "unflub-site");
  assert.equal(cfg.assets.directory, "./dist");
  assert.equal(cfg.assets.not_found_handling, "404-page");
  assert.deepEqual(cfg.routes, [{ pattern: "unflub.rmgn.ca", custom_domain: true }]);
});

test("privacy copy matches what the app sends", () => {
  // The update check carries no ID, and typo pairs are never collected (owner, 2026-11-17).
  for (const [path, doc] of pages) {
    const text = doc.querySelector("body")?.text ?? "";
    assert.doesNotMatch(text, /random ID|install ID|typo and fix pairs|Help improve corrections/i, `${path} has outdated privacy copy`);
  }
  const privacy = pages.get("/privacy/").querySelector("main")?.text ?? "";
  assert.match(privacy, /Nothing you type leaves your computer/);
});
