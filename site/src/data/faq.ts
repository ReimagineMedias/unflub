/** Shown on the home page and emitted as FAQPage JSON-LD, so keep answers plain text. */
export const FAQ: { q: string; a: string }[] = [
  {
    q: "Is unflub free?",
    a: "Yes. unflub is free and open source under the MIT licence. There is no paid tier, no trial and no account.",
  },
  {
    q: "Does unflub see my passwords?",
    a: "No. unflub skips password fields, and password managers such as KeePass, KeePassXC, 1Password and Bitwarden are on its skip list from the start. Everywhere else it only keeps the word you are typing in memory, never on disk, and nothing you type leaves your computer.",
  },
  {
    q: "Does unflub connect to the internet?",
    a: "Only to check for updates. Once a day it asks GitHub whether a new version is out. That request carries nothing you typed, no account and no identifier, and you can turn it off in Settings. Corrections themselves work fully offline.",
  },
  {
    q: "Does it work in games and terminals?",
    a: "Terminals are skipped by default, including Windows Terminal, PowerShell, Command Prompt and WSL, so your commands stay exactly as typed. If a game or any other app should be left alone, add it under Apps in settings, or pause unflub from the tray icon.",
  },
  {
    q: "Is there a Mac or Linux version?",
    a: "Not yet. Windows comes first. macOS and Linux are planned on the same engine, so they will correct the same way. Watch the releases page on GitHub to hear when they land.",
  },
  {
    q: "How do I uninstall unflub?",
    a: "Open Windows Settings, go to Apps, then Installed apps, find unflub and choose Uninstall. Nothing is left running afterwards.",
  },
  {
    q: "How do I stop unflub starting at login?",
    a: "Open unflub settings from the tray icon and turn off Start at login on the General page. You can also switch it off under Startup apps in Windows Settings.",
  },
  {
    q: "Which languages does unflub support?",
    a: "English today. unflub picks fixes with its own English dictionary and can use the Windows spell checker for the language you choose to spot typos. More bundled languages are planned.",
  },
];
