/** Re-labels download buttons for visitors on macOS or Linux, where unflub is not out yet. */
type Os = "windows" | "mac" | "linux" | "other";

interface UAData {
  platform?: string;
}

function detect(): Os {
  const nav = navigator as Navigator & { userAgentData?: UAData };
  const p = (nav.userAgentData?.platform ?? navigator.platform ?? "").toLowerCase();
  const ua = navigator.userAgent.toLowerCase();
  if (/android|iphone|ipad|ipod/.test(ua)) return "other";
  if (p.includes("win") || ua.includes("windows")) return "windows";
  if (p.includes("mac") || ua.includes("mac os")) return "mac";
  if (p.includes("linux") || ua.includes("linux") || ua.includes("cros")) return "linux";
  return "other";
}

const os = detect();
document.documentElement.dataset["os"] = os;

if (os === "mac" || os === "linux") {
  const name = os === "mac" ? "Mac" : "Linux";
  for (const a of document.querySelectorAll<HTMLAnchorElement>("a[data-dl]")) {
    const label = a.querySelector("[data-dl-label]");
    if (!label) continue;
    label.textContent = a.dataset["dl"] === "short" ? "Get unflub" : `Coming soon for ${name}`;
    a.href = "/download/";
  }
}
