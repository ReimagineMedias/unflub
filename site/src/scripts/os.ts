/** Re-labels download buttons for visitors who can't install unflub on the device they're using: macOS and Linux
    (not out yet) and phones or tablets (no download at all; they get a way to send the link to their PC). */
type Os = "windows" | "mac" | "linux" | "mobile" | "other";

interface UAData {
  platform?: string;
  mobile?: boolean;
}

function detect(): Os {
  const nav = navigator as Navigator & { userAgentData?: UAData };
  const p = (nav.userAgentData?.platform ?? navigator.platform ?? "").toLowerCase();
  const ua = navigator.userAgent.toLowerCase();
  if (nav.userAgentData?.mobile || /android|iphone|ipad|ipod/.test(ua)) return "mobile";
  // iPadOS reports itself as a Mac; a touch screen gives it away.
  if ((p.includes("mac") || ua.includes("mac os")) && navigator.maxTouchPoints > 1) return "mobile";
  if (p.includes("win") || ua.includes("windows")) return "windows";
  if (p.includes("mac") || ua.includes("mac os")) return "mac";
  if (p.includes("linux") || ua.includes("linux") || ua.includes("cros")) return "linux";
  return "other";
}

const os = detect();
document.documentElement.dataset["os"] = os;

const relabel = (full: (a: HTMLAnchorElement) => string, short: string): void => {
  for (const a of document.querySelectorAll<HTMLAnchorElement>("a[data-dl]")) {
    const label = a.querySelector("[data-dl-label]");
    if (!label) continue;
    label.textContent = a.dataset["dl"] === "short" ? short : full(a);
    a.href = "/download/";
  }
  // The night copy of the nav has to match the real one exactly.
  for (const m of document.querySelectorAll("[data-dl-mirror]")) m.textContent = short;
};

if (os === "mac" || os === "linux") {
  const name = os === "mac" ? "Mac" : "Linux";
  relabel(() => `Coming soon for ${name}`, "Get unflub");
}

if (os === "mobile") {
  relabel(() => "Get it on your Windows PC", "Get unflub");
  const onDownload = location.pathname === "/download/";
  const url = new URL("/download/", location.origin).href;
  const canShare = typeof navigator.share === "function";
  for (const b of document.querySelectorAll<HTMLButtonElement>("button[data-share]")) {
    b.hidden = false;
    const label = b.querySelector("[data-share-label]");
    const say = (t: string): void => {
      if (label) label.textContent = t;
    };
    say(canShare ? "Send the link to your PC" : "Copy the link");
    b.addEventListener("click", () => {
      if (canShare) {
        navigator.share({ title: "unflub for Windows", url }).catch(() => undefined);
        return;
      }
      navigator.clipboard.writeText(url).then(
        () => say("Link copied"),
        () => say(url),
      );
    });
    // On the download page itself, the link back to it would go nowhere: only the share button stays.
    if (onDownload) b.parentElement?.querySelector<HTMLElement>("a[data-dl]")?.setAttribute("hidden", "");
  }
}
