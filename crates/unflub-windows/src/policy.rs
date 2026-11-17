//! Hook-side rules that decide what reaches the corrector and when an action may be injected.
//! Pure logic, tested on every OS.

use unflub_core::Key;

use crate::translate::Modifiers;

/// What to do with a translated key, given whether corrections are blocked right now (secure or
/// pending field, IME layout, excluded app).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    /// Give the key to the corrector.
    Feed,
    /// Send the corrector a single `Key::Reset` instead, and pass the key through.
    ResetOnly,
    /// Don't tell the corrector anything; pass the key through.
    Drop,
}

/// Keeps characters typed in blocked fields away from the corrector.
///
/// While blocked, the corrector gets one `Reset` and then nothing, so it never sees text from a
/// password field. When the block lifts in the middle of a word, the rest of that word is
/// dropped too, so the corrector never mistakes a word's tail for a whole word. Normal feeding
/// resumes at the next boundary, or at a `Reset` (caret moved, focus changed).
#[derive(Debug, Default)]
pub struct Gate {
    suppressing: bool,
}

impl Gate {
    pub fn decide(&mut self, key: Key, blocked: bool) -> GateDecision {
        if blocked {
            return if std::mem::replace(&mut self.suppressing, true) {
                GateDecision::Drop
            } else {
                GateDecision::ResetOnly
            };
        }
        if !self.suppressing {
            return GateDecision::Feed;
        }
        match key {
            Key::Char(_) | Key::Backspace => GateDecision::Drop,
            Key::Boundary(_) | Key::Reset | Key::DeleteWordLeft => {
                self.suppressing = false;
                GateDecision::ResetOnly
            }
        }
    }

    /// A caret move or focus change is a fresh start.
    pub fn on_reset(&mut self) {
        self.suppressing = false;
    }
}

/// The veto reason for every action in a dry run.
pub const DRY_RUN: &str = "dry run";

/// Why an action the corrector asked for must not be injected, if it mustn't. A dry run vetoes
/// everything, so the hook never injects input or swallows a key.
pub fn injection_veto(
    key: Key,
    mods: Modifiers,
    auto_repeat: bool,
    dry_run: bool,
) -> Option<&'static str> {
    if dry_run {
        Some(DRY_RUN)
    } else if key == Key::Reset {
        Some("not a typed key")
    } else if mods.shortcut() {
        // Backspaces sent with Ctrl/Alt/Win held would become Ctrl+Backspace (delete word) etc.
        Some("modifier held")
    } else if auto_repeat {
        // Swallowing a repeat would leave the app with a key-down and no key-up.
        Some("auto-repeat")
    } else if key == Key::Boundary('\n') && mods.shift {
        // Shift+Enter is a soft line break in chat apps; we'd re-send it as a plain Enter.
        Some("Shift+Enter")
    } else {
        None
    }
}

/// Virtual machine consoles and remote viewers that forward scan codes to the guest. Our fixes
/// type text as Unicode packets, which they drop: the Backspaces would arrive and the fixed
/// word wouldn't. Never corrected, whatever the exclude list says (review S3).
pub const SCANCODE_CONSOLES: &[&str] = &[
    "vmware.exe",
    "vmware-vmx.exe",
    "vmplayer.exe",
    "vmrc.exe",
    "virtualboxvm.exe",
    "vboxsdl.exe",
    "vmconnect.exe",
    "qemu-system-x86_64.exe",
    "qemu-system-x86_64w.exe",
    "qemu-system-aarch64.exe",
    "qemu-system-aarch64w.exe",
    "vncviewer.exe",
    "tvnviewer.exe",
    "uvnc_viewer.exe",
];

/// Terminals and SSH clients the default exclude list doesn't have. A password prompt there
/// can't be detected (it is just text after Enter), so they are never corrected (review S4).
/// The usual Windows consoles are in the user's exclude list instead (see
/// [`is_terminal_app`]).
pub const TERMINAL_APPS: &[&str] = &[
    "putty.exe",
    "kitty.exe",
    "kitty_portable.exe",
    "mobaxterm.exe",
    "tabby.exe",
    "hyper.exe",
    "conemu.exe",
    "conemu64.exe",
    "cmder.exe",
    "termius.exe",
    "warp.exe",
    "securecrt.exe",
    "xshell.exe",
    "terminus.exe",
    "ssh.exe",
    "bash.exe",
];

/// Consoles the app's default exclude list holds. Users may take them off that list; fixes
/// there still never record their words (see [`keep_words`]).
const DEFAULT_EXCLUDED_TERMINALS: &[&str] = &[
    "windowsterminal.exe",
    "openconsole.exe",
    "conhost.exe",
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "wsl.exe",
    "mintty.exe",
    "wezterm-gui.exe",
    "alacritty.exe",
];

/// Why unflub never corrects in `app` (a lowercased exe name), if it doesn't.
pub fn never_correct(app: &str) -> Option<&'static str> {
    if SCANCODE_CONSOLES.contains(&app) {
        Some("virtual machine or remote console")
    } else if TERMINAL_APPS.contains(&app) {
        Some("terminal")
    } else {
        None
    }
}

/// A terminal or SSH client, where a password prompt is plausible.
pub fn is_terminal_app(app: &str) -> bool {
    TERMINAL_APPS.contains(&app) || DEFAULT_EXCLUDED_TERMINALS.contains(&app)
}

/// May the local Stats page keep the words of a fix made in `app`? Never for terminals (the
/// "word" may be a password typed at a prompt) or when the app is unknown.
pub fn keep_words(app: Option<&str>) -> bool {
    app.is_some_and(|a| !is_terminal_app(a))
}

/// A UI Automation class name of a terminal's input inside an otherwise ordinary app: xterm.js,
/// the terminal in VS Code, Cursor, Hyper and other Electron apps. Treated as a secure field.
pub fn is_terminal_input_class(class: &str) -> bool {
    class
        .split_whitespace()
        .any(|c| c == "xterm-helper-textarea")
}

/// While a first Backspace is held (double-Backspace undo), does this key-down release it (send
/// it on, ahead of the key)? Every key does except a fresh, plain Backspace from the user, which
/// undoes the fix: an auto-repeat or a Backspace with Ctrl, Alt, Shift or Win releases it too,
/// and so does every replayed key (one of ours sent again after an injection), so no key ever
/// reaches the app or the engine ahead of a Backspace typed before it. There is no timeout, and
/// nothing else waits on the hold.
pub fn releases_held_backspace(vk: u16, auto_repeat: bool, modifier: bool, replayed: bool) -> bool {
    const BACK: u16 = 0x08;
    replayed || vk != BACK || auto_repeat || modifier
}

/// May a first Backspace be held for a double-Backspace undo? Only one straight from the user:
/// a replayed Backspace was typed while an injection was in flight, and holding it would put
/// it behind keys already on their way to the app.
pub fn may_hold_backspace(replayed: bool) -> bool {
    !replayed
}

/// Primary-language ids of layouts that normally come with an IME (Chinese, Japanese, Korean).
/// Keys there are IME composition input, not the text that ends up in the field.
pub fn is_ime_language(langid: u16) -> bool {
    const LANG_CHINESE: u16 = 0x04;
    const LANG_JAPANESE: u16 = 0x11;
    const LANG_KOREAN: u16 = 0x12;
    matches!(langid & 0x3FF, LANG_CHINESE | LANG_JAPANESE | LANG_KOREAN)
}

/// Pairs swallowed key-downs with their key-ups, and spots auto-repeat.
#[derive(Debug, Default)]
pub struct SwallowTracker {
    up_pending: Option<u16>,
}

impl SwallowTracker {
    /// A key-down arrived. Returns true if it is an auto-repeat: the key is already down as far
    /// as Windows knows (`async_down`, read before this event updates it), or it is a repeat of
    /// a down we swallowed (no key-up seen since). Either way its key-up now belongs to the app.
    pub fn on_down(&mut self, vk: u16, async_down: bool) -> bool {
        let repeat_of_swallowed = self.up_pending.take_if(|v| *v == vk).is_some();
        async_down || repeat_of_swallowed
    }

    /// We swallowed this key-down; swallow its key-up too.
    pub fn swallowed(&mut self, vk: u16) {
        self.up_pending = Some(vk);
    }

    /// A key-up arrived. Returns true if it should be swallowed.
    pub fn on_up(&mut self, vk: u16) -> bool {
        self.up_pending.take_if(|v| *v == vk).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review S3 and S4: VM consoles and terminals are never corrected, and terminal fixes
    /// never keep their words.
    #[test]
    fn vm_consoles_and_terminals_are_never_corrected() {
        for app in [
            "vmware.exe",
            "virtualboxvm.exe",
            "vmconnect.exe",
            "vncviewer.exe",
        ] {
            assert_eq!(
                never_correct(app),
                Some("virtual machine or remote console")
            );
        }
        for app in [
            "putty.exe",
            "kitty.exe",
            "mobaxterm.exe",
            "tabby.exe",
            "conemu64.exe",
        ] {
            assert_eq!(never_correct(app), Some("terminal"), "{app}");
            assert!(!keep_words(Some(app)));
        }
        // The default-excluded consoles stay the user's choice, but keep no words.
        for app in ["windowsterminal.exe", "cmd.exe", "pwsh.exe", "mintty.exe"] {
            assert_eq!(never_correct(app), None, "{app}");
            assert!(!keep_words(Some(app)), "{app}");
        }
        for app in ["notepad.exe", "slack.exe", "code.exe", "mstsc.exe"] {
            assert_eq!(never_correct(app), None, "{app}");
            assert!(keep_words(Some(app)), "{app}");
        }
        assert!(!keep_words(None), "unknown app keeps no words");
    }

    /// Owner live test: only a plain second Backspace keeps the hold for the undo; everything
    /// else, Ctrl+Backspace included, sends the held Backspace first.
    #[test]
    fn what_releases_a_held_backspace() {
        assert!(
            !releases_held_backspace(0x08, false, false, false),
            "second Backspace undoes"
        );
        assert!(
            releases_held_backspace(0x08, true, false, false),
            "auto-repeat"
        );
        assert!(
            releases_held_backspace(0x08, false, true, false),
            "Ctrl+Backspace"
        );
        for vk in [0xBC, 0x41, 0x25, 0x10, 0x0D, 0x20] {
            assert!(releases_held_backspace(vk, false, false, false), "{vk:#x}");
        }
        // Review blocker: replayed keys release it too, a plain Backspace included, and never
        // start a hold of their own.
        for vk in [0x08, 0xBC, 0x41] {
            assert!(releases_held_backspace(vk, false, false, true), "{vk:#x}");
        }
        assert!(may_hold_backspace(false));
        assert!(!may_hold_backspace(true));
    }

    #[test]
    fn xterm_inputs_are_terminals() {
        assert!(is_terminal_input_class("xterm-helper-textarea"));
        assert!(is_terminal_input_class("foo xterm-helper-textarea"));
        assert!(!is_terminal_input_class(
            "inputarea monaco-mouse-cursor-text"
        ));
        assert!(!is_terminal_input_class(""));
    }

    /// The list here must match the app's default exclude list (app/src-tauri/src/settings.rs).
    #[test]
    fn default_excluded_terminals_match_the_app() {
        let settings = include_str!("../../../app/src-tauri/src/settings.rs");
        for app in DEFAULT_EXCLUDED_TERMINALS {
            assert!(settings.contains(&format!("\"{app}\"")), "{app}");
        }
    }

    #[test]
    fn gate_feeds_normally() {
        let mut g = Gate::default();
        assert_eq!(g.decide(Key::Char('a'), false), GateDecision::Feed);
        assert_eq!(g.decide(Key::Boundary(' '), false), GateDecision::Feed);
    }

    #[test]
    fn gate_sends_one_reset_then_nothing_while_blocked() {
        let mut g = Gate::default();
        assert_eq!(g.decide(Key::Char('p'), true), GateDecision::ResetOnly);
        for k in [
            Key::Char('w'),
            Key::Backspace,
            Key::Boundary(' '),
            Key::Char('d'),
        ] {
            assert_eq!(g.decide(k, true), GateDecision::Drop);
        }
    }

    #[test]
    fn gate_drops_the_tail_of_a_word_after_unblocking() {
        let mut g = Gate::default();
        g.decide(Key::Char('t'), true);
        // Probe answered "not secure" mid-word: "eh" is not a word on its own.
        assert_eq!(g.decide(Key::Char('e'), false), GateDecision::Drop);
        assert_eq!(g.decide(Key::Char('h'), false), GateDecision::Drop);
        assert_eq!(g.decide(Key::Backspace, false), GateDecision::Drop);
        assert_eq!(g.decide(Key::Boundary(' '), false), GateDecision::ResetOnly);
        assert_eq!(g.decide(Key::Char('n'), false), GateDecision::Feed);
    }

    #[test]
    fn gate_resumes_on_reset() {
        let mut g = Gate::default();
        g.decide(Key::Char('t'), true);
        g.on_reset();
        assert_eq!(g.decide(Key::Char('n'), false), GateDecision::Feed);
        let mut g = Gate::default();
        g.decide(Key::Char('t'), true);
        assert_eq!(g.decide(Key::Reset, false), GateDecision::ResetOnly);
        assert_eq!(g.decide(Key::Char('n'), false), GateDecision::Feed);
    }

    const NONE: Modifiers = Modifiers {
        shift: false,
        ctrl: false,
        alt: false,
        win: false,
        caps_lock: false,
    };

    #[test]
    fn vetoes() {
        let shift = Modifiers {
            shift: true,
            ..NONE
        };
        let altgr = Modifiers {
            ctrl: true,
            alt: true,
            ..NONE
        };
        assert_eq!(injection_veto(Key::Boundary(' '), NONE, false, false), None);
        assert_eq!(injection_veto(Key::Backspace, NONE, false, false), None);
        assert_eq!(
            injection_veto(Key::Boundary('?'), shift, false, false),
            None
        );
        assert_eq!(
            injection_veto(Key::Boundary('\n'), NONE, false, false),
            None
        );
        assert_eq!(
            injection_veto(Key::Boundary('\n'), shift, false, false),
            Some("Shift+Enter")
        );
        assert_eq!(
            injection_veto(Key::Boundary('@'), altgr, false, false),
            Some("modifier held")
        );
        assert_eq!(
            injection_veto(Key::Boundary(' '), NONE, true, false),
            Some("auto-repeat")
        );
        assert!(injection_veto(Key::Reset, NONE, false, false).is_some());
    }

    #[test]
    fn dry_run_vetoes_every_action() {
        let shift = Modifiers {
            shift: true,
            ..NONE
        };
        for key in [
            Key::Char('a'),
            Key::Boundary(' '),
            Key::Boundary('\n'),
            Key::Backspace,
            Key::Reset,
        ] {
            for mods in [NONE, shift] {
                for repeat in [false, true] {
                    assert_eq!(injection_veto(key, mods, repeat, true), Some(DRY_RUN));
                }
            }
        }
    }

    #[test]
    fn ime_languages() {
        assert!(is_ime_language(0x0411)); // ja-JP
        assert!(is_ime_language(0x0804)); // zh-CN
        assert!(is_ime_language(0x0404)); // zh-TW
        assert!(is_ime_language(0x0412)); // ko-KR
        assert!(!is_ime_language(0x0409)); // en-US
        assert!(!is_ime_language(0x040C)); // fr-FR
        assert!(!is_ime_language(0x0407)); // de-DE
    }

    #[test]
    fn swallowed_keys_get_their_key_up_swallowed() {
        let mut t = SwallowTracker::default();
        assert!(!t.on_down(0x20, false));
        t.swallowed(0x20);
        assert!(!t.on_up(0x41), "other keys' ups pass");
        assert!(t.on_up(0x20));
        assert!(!t.on_up(0x20), "only once");
    }

    #[test]
    fn repeats_are_detected() {
        let mut t = SwallowTracker::default();
        // The previous down went through, so Windows already has the key down.
        assert!(t.on_down(0x20, true));
        // A repeat of a key we swallowed: Windows doesn't know it's down, we do.
        t.swallowed(0x08);
        assert!(t.on_down(0x08, false));
        // ...and its key-up now belongs to the app.
        assert!(!t.on_up(0x08));
        // A fresh press.
        assert!(!t.on_down(0x08, false));
    }
}
