import { PERFORMANCE } from "./performance";

const [decide, memory] = PERFORMANCE.stats;

/** Shown on the home page and emitted as FAQPage JSON-LD, so keep answers plain text. */
export const FAQ: { q: string; a: string }[] = [
  {
    q: "What if it changes a word I meant to type?",
    a: "Press Backspace right after the fix. Your word comes back, and unflub stops fixing that word. It goes into Your words in Settings, so you can take it out later.",
  },
  {
    q: "Can it see my passwords?",
    a: "No. unflub ignores password fields, and it skips 1Password, Bitwarden, KeePass and KeePassXC by default. You can add any other app under Apps in Settings.",
  },
  {
    q: "Does unflub send anything over the internet?",
    a: "Never what you type, and never the words you misspell. Once a day it checks for updates, sending only the app version. It also sends anonymous usage counts once a day. They are on by default, and you can turn them off any time in Settings > Privacy. Crash reports are only sent if you say yes after a crash.",
  },
  {
    q: "Will it mess with my code or my terminal?",
    a: "Terminals are skipped by default, so Windows Terminal, PowerShell, Command Prompt and WSL get exactly what you type. In an editor, unflub fixes comments and plain sentences but leaves camelCase, snake_case, file paths, links and anything in backticks alone.",
  },
  {
    q: "Will it slow my computer down?",
    a: `It sits at about ${memory.value.replace("~", "")} ${memory.unit} of memory and uses next to no CPU while you aren't typing. Checking a word takes about ${decide.value} ${decide.unit}, so the fix is in before your next key lands.`,
  },
  {
    q: "How do I uninstall it?",
    a: "Like any other app: Settings › Apps › Installed apps, then Uninstall next to unflub. That removes its login entry too, so nothing starts with Windows afterwards. The uninstaller asks whether to delete your settings and words as well.",
  },
];
