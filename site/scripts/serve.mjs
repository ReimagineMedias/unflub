// A local stand-in for the production host, used by the tests and for Lighthouse runs. It approximates
// vercel.json (trailingSlash, cleanUrls, headers): 308 redirects to the slash form, `.html` and `/index.html`
// redirected to the clean URL, 404.html for misses, and the configured headers on every response. It is not
// Vercel's route compiler; check a real preview deployment for exact behaviour.
// Usage: node scripts/serve.mjs [port]   (0 picks a free port)
import { createServer } from "node:http";
import { existsSync, readFileSync, statSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

const dist = fileURLToPath(new URL("../dist/", import.meta.url));

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".ico": "image/x-icon",
  ".woff2": "font/woff2",
  ".woff": "font/woff",
  ".xml": "application/xml",
  ".txt": "text/plain; charset=utf-8",
  ".webmanifest": "application/manifest+json",
};

/** Header rules from vercel.json, the one source for production headers. Its `source` patterns here are plain
    regular expressions (only `(.*)` and escaped dots), so they are used as they are. Later rules win, as on Vercel. */
function loadHeaders() {
  const cfg = JSON.parse(readFileSync(fileURLToPath(new URL("../vercel.json", import.meta.url)), "utf8"));
  return (cfg.headers ?? []).map((r) => [
    new RegExp(`^${r.source}$`),
    Object.fromEntries(r.headers.map((h) => [h.key, h.value])),
  ]);
}

/** @returns {Promise<import("node:http").Server>} */
export function startServer(port = 0) {
  const rules = loadHeaders();
  const server = createServer((req, res) => {
    const url = new URL(req.url ?? "/", "http://localhost");
    const path = decodeURIComponent(url.pathname);
    const headers = {};
    for (const [re, h] of rules) if (re.test(path)) Object.assign(headers, h);
    const send = (status, file) => {
      res.writeHead(status, { "Content-Type": TYPES[extname(file)] ?? "application/octet-stream", ...headers });
      res.end(req.method === "HEAD" ? undefined : readFileSync(file));
    };
    const redirect = (to) => {
      res.writeHead(308, { Location: `${to}${url.search}`, ...headers });
      res.end();
    };
    const target = normalize(join(dist, path));
    if (!target.startsWith(normalize(dist))) return send(404, join(dist, "404.html"));
    // cleanUrls: /page/index.html and /page.html both have the clean address /page/.
    if (path.endsWith("/index.html")) return redirect(path.slice(0, -"index.html".length));
    if (path.endsWith(".html") && path !== "/404.html") {
      const clean = path.slice(0, -".html".length);
      if (existsSync(join(dist, clean, "index.html"))) return redirect(`${clean}/`);
    }
    if (path.endsWith("/") && existsSync(join(target, "index.html"))) return send(200, join(target, "index.html"));
    if (!path.endsWith("/") && existsSync(join(target, "index.html"))) return redirect(`${path}/`);
    if (existsSync(target) && statSync(target).isFile()) return send(200, target);
    return send(404, join(dist, "404.html"));
  });
  return new Promise((resolve) => server.listen(port, "localhost", () => resolve(server)));
}

if (process.argv[1] && fileURLToPath(import.meta.url) === normalize(process.argv[1])) {
  const server = await startServer(Number(process.argv[2] ?? 4322));
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : process.argv[2];
  console.log(`Serving dist/ with production headers at http://localhost:${port}/`);
}
