// Minimal headless Chrome driver over the DevTools protocol, used by the dev-only
// scripts (screenshots, favicon and OG image rendering). No extra dependencies:
// it finds the installed Chrome, or uses CHROME_PATH.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function findChrome() {
  return [
    process.env.CHROME_PATH,
    "C:/Program Files/Google/Chrome/Application/chrome.exe",
    "C:/Program Files (x86)/Google/Chrome/Application/chrome.exe",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
  ].find((p) => p && existsSync(p));
}

const freePort = () =>
  new Promise((done) => {
    const s = createServer();
    s.listen(0, () => {
      const { port } = /** @type {import("node:net").AddressInfo} */ (s.address());
      s.close(() => done(port));
    });
  });

/**
 * Start Chrome, hand `fn` a page controller, always clean up.
 * @param {(page: Page) => Promise<void>} fn
 */
export async function withChrome(fn) {
  const path = findChrome();
  if (!path) throw new Error("Chrome not found. Set CHROME_PATH.");
  const port = await freePort();
  const work = mkdtempSync(join(tmpdir(), "unflub-chrome-"));
  const proc = spawn(
    path,
    [
      "--headless=new",
      `--remote-debugging-port=${port}`,
      `--user-data-dir=${join(work, "profile")}`,
      "--no-first-run",
      "--no-default-browser-check",
      "--hide-scrollbars",
      "--force-color-profile=srgb",
      "about:blank",
    ],
    { stdio: "ignore" },
  );
  try {
    let target;
    for (let i = 0; i < 75 && !target; i++) {
      try {
        const list = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
        target = list.find((t) => t.type === "page");
      } catch {}
      if (!target) await sleep(200);
    }
    if (!target) throw new Error("Chrome did not start.");
    const socket = new WebSocket(target.webSocketDebuggerUrl);
    await new Promise((r) => socket.addEventListener("open", r));
    let id = 0;
    const pending = new Map();
    socket.addEventListener("message", (m) => {
      const data = JSON.parse(m.data);
      if (data.id && pending.has(data.id)) {
        pending.get(data.id)(data);
        pending.delete(data.id);
      }
    });
    const send = (method, params = {}) =>
      new Promise((done) => {
        const n = ++id;
        pending.set(n, done);
        socket.send(JSON.stringify({ id: n, method, params }));
      });
    await send("Page.enable");
    await send("Runtime.enable");
    await fn(new Page(send));
    socket.close();
  } finally {
    try {
      proc.kill();
    } catch {}
    await sleep(500);
    rmSync(work, { recursive: true, force: true });
  }
}

export class Page {
  constructor(send) {
    this.send = send;
  }

  async viewport(width, height, { scale = 1, mobile = false } = {}) {
    await this.send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: scale, mobile });
    await this.send("Emulation.setTouchEmulationEnabled", { enabled: mobile });
  }

  async media({ scheme = "light", reducedMotion = "no-preference" } = {}) {
    await this.send("Emulation.setEmulatedMedia", {
      features: [
        { name: "prefers-color-scheme", value: scheme },
        { name: "prefers-reduced-motion", value: reducedMotion },
      ],
    });
  }

  async transparent() {
    await this.send("Emulation.setDefaultBackgroundColorOverride", { color: { r: 0, g: 0, b: 0, a: 0 } });
  }

  async goto(url, settleMs = 800) {
    await this.send("Page.navigate", { url });
    await this.evaluate("new Promise(r => document.readyState === 'complete' ? r() : addEventListener('load', r))");
    await this.evaluate("document.fonts.ready.then(() => true)");
    await sleep(settleMs);
  }

  /** Runs `expression` in the page via Runtime.evaluate (our own trusted scripts only). */
  async evaluate(expression) {
    const reply = await this.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    const err = reply.result?.exceptionDetails;
    if (err) throw new Error(err.exception?.description ?? err.text);
    return reply.result?.result?.value;
  }

  /** PNG bytes of the viewport, or of the whole page when `full` is set. */
  async shot({ full = false } = {}) {
    let params = { format: "png" };
    if (full) {
      const h = await this.evaluate("Math.ceil(document.documentElement.scrollHeight)");
      const w = await this.evaluate("document.documentElement.clientWidth");
      params = { ...params, captureBeyondViewport: true, clip: { x: 0, y: 0, width: w, height: h, scale: 1 } };
    }
    const reply = await this.send("Page.captureScreenshot", params);
    if (!reply.result?.data) throw new Error(JSON.stringify(reply.error ?? reply));
    return Buffer.from(reply.result.data, "base64");
  }
}
