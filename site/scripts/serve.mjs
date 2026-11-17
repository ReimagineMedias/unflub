// Serves dist/ the way Cloudflare's static assets will: trailing-slash redirects, 404.html for
// misses, and the headers from _headers. Used by the tests and for Lighthouse runs.
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

/** Parse _headers into [pattern, headers] rules. Only `*` wildcards are supported. */
function loadHeaders() {
  const file = join(dist, "_headers");
  if (!existsSync(file)) return [];
  const rules = [];
  for (const line of readFileSync(file, "utf8").split(/\r?\n/)) {
    if (!line.trim() || line.trim().startsWith("#")) continue;
    if (!/^\s/.test(line)) {
      const escaped = line.trim().replace(/[.+?^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*");
      rules.push([new RegExp(`^${escaped}$`), {}]);
    } else if (rules.length) {
      const i = line.indexOf(":");
      rules[rules.length - 1][1][line.slice(0, i).trim()] = line.slice(i + 1).trim();
    }
  }
  return rules;
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
    const target = normalize(join(dist, path));
    if (!target.startsWith(normalize(dist))) return send(404, join(dist, "404.html"));
    if (path.endsWith("/") && existsSync(join(target, "index.html"))) return send(200, join(target, "index.html"));
    if (!path.endsWith("/") && existsSync(join(target, "index.html"))) {
      res.writeHead(307, { Location: `${path}/${url.search}` });
      return res.end();
    }
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
