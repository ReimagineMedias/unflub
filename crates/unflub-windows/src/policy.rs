//! Hook-side rules that decide what reaches the corrector and when an action may be injected.
//! Pure logic, tested on every OS.

use std::time::{Duration, Instant};

use unflub_core::Key;

use crate::focus::FieldCheck;
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

/// Letters of a word typed while the field's UIA check is pending, kept by the hook (never
/// given to the corrector) until the answer arrives (#110).
///
/// Every Enter, Tab or click asks UIA again, and the field is secure until it answers (some
/// 50 ms plus the query). Fast typists started the next word before that, and the [`Gate`]
/// then dropped the whole word, so the first word after Enter was never corrected. Now its
/// letters wait here, while still going to the app as usual. Cleared: they go to the corrector
/// first, in order, and the word is handled as if nothing had been pending. Secure, or
/// anything but a letter while still pending (a boundary, Backspace, a caret move): they are
/// forgotten and the field is treated as secure, as before. Password text never reaches the
/// corrector either way.
///
/// Letters that may belong to a password are overwritten before they are let go (the buffer
/// never grows past [`PendingWord::MAX`], so no copy is left behind by a reallocation either).
pub struct PendingWord {
    letters: Vec<char>,
}

/// Shows how many letters are held, never which: they may be a password.
impl std::fmt::Debug for PendingWord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingWord")
            .field("held", &self.letters.len())
            .finish()
    }
}

impl Default for PendingWord {
    fn default() -> Self {
        Self {
            letters: Vec::with_capacity(Self::MAX),
        }
    }
}

impl Drop for PendingWord {
    fn drop(&mut self) {
        wipe(&mut self.letters);
    }
}

/// Overwrite `letters` with NULs, in a way the compiler can't drop as a dead store, then empty
/// it. The allocation is kept.
pub fn wipe(letters: &mut Vec<char>) {
    for c in letters.iter_mut() {
        // SAFETY: `c` is a valid, aligned, exclusive reference.
        unsafe { std::ptr::write_volatile(c, '\0') };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    letters.clear();
}

impl PendingWord {
    /// More than a word's worth: give up and treat the field as secure.
    pub const MAX: usize = 48;

    /// A key typed where corrections are otherwise allowed (no IME, app not excluded), in a
    /// field that is `check`. `None`: the letter is kept, pass the key on and do nothing else.
    /// `Some(letters)`: feed these to the corrector first, then handle the key with the field
    /// secure unless it is cleared.
    pub fn on_key(&mut self, key: Key, check: FieldCheck) -> Option<Vec<char>> {
        match (check, key) {
            (FieldCheck::Pending, Key::Char(c)) if self.letters.len() < Self::MAX => {
                self.letters.push(c);
                None
            }
            (FieldCheck::Cleared, _) => {
                // Cleared: not a password, so the copy for the corrector needs no wiping.
                let letters = self.letters.clone();
                wipe(&mut self.letters);
                Some(letters)
            }
            _ => {
                wipe(&mut self.letters);
                Some(Vec::new())
            }
        }
    }

    /// Forget kept letters (caret moved, focus changed, IME, excluded app).
    pub fn clear(&mut self) {
        wipe(&mut self.letters);
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

/// The modifiers an injection must respect: those the key came with (`typed`) plus those held
/// on the keyboard now (`physical`). A key another program injected (an on-screen keyboard, a
/// remote tool) comes with none of its own, but Ctrl, Alt or Win held on the keyboard still
/// turn our Backspaces into Ctrl+Backspace and our letters into shortcuts. Caps Lock is the
/// typed key's.
pub fn veto_modifiers(typed: Modifiers, physical: Modifiers) -> Modifiers {
    Modifiers {
        shift: typed.shift || physical.shift,
        ctrl: typed.ctrl || physical.ctrl,
        alt: typed.alt || physical.alt,
        win: typed.win || physical.win,
        caps_lock: typed.caps_lock,
    }
}

/// Virtual machine consoles and remote viewers that forward scan codes to the guest. Fixes type
/// characters with the host layout's keys and the rest as Unicode packets, which they drop, and
/// the guest's layout may differ: the Backspaces would arrive and the fixed word might not.
/// Never corrected, whatever the exclude list says (review S3).
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
pub fn releases_held_backspace(
    vk: u16,
    auto_repeat: bool,
    modifier: bool,
    replayed: bool,
    missed_input: bool,
) -> bool {
    const BACK: u16 = 0x08;
    replayed || vk != BACK || auto_repeat || modifier || missed_input
}

/// After a key-down was handled while a first Backspace is `held`: must the held one be sent on
/// now? Yes when this was a Backspace that went to the app (`swallowed` false) instead of
/// undoing the fix: the undo was refused (auto-repeat, modifier, dry run) or never decided
/// (blocked field, broken engine). Both Backspaces then reach the app, and the "Undo?" prompt
/// ends instead of waiting for a key that would push the held one out behind it (#39).
pub fn releases_unanswered_hold(vk: u16, swallowed: bool, held: bool) -> bool {
    const BACK: u16 = 0x08;
    held && vk == BACK && !swallowed
}

/// The element a focus WinEvent names: its window, object id and child id. Chromium and other
/// apps that draw their own controls name every focused element by child id in one window.
pub type FocusId = (isize, i32, i32);

/// What to do with a held first Backspace now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldVerdict {
    /// Send it on, to the element that still has focus.
    Release,
    /// Focus just left the element it was typed in: it can't undo anything now (end the
    /// "Undo?" prompt), but keep it in case focus comes back before the next key.
    Away,
    /// Keep waiting: it is away, and nothing changed that.
    Keep,
    /// Throw it away: sending it would delete a character somewhere the user didn't type it.
    Drop,
}

/// Where a held first Backspace was typed, and whether focus is still there (#66).
///
/// Any focus or foreground event used to drop it, losing the key to a toast that flashed by,
/// an autocomplete list or a focus event fired again for the same field. Now an event naming
/// the element it was typed in, while focus never left, sends it on there. An event naming
/// anything else puts it aside, and the next key or click drops it, as before. Focus coming
/// back sends it only when the return can be trusted: the element has its own child id, it came
/// back within [`HeldFocus::RETURN_WITHIN`], and no input went missing meanwhile. Swing, Qt,
/// Flutter and games name every control by its window alone, and an elevated window can keep
/// focus for minutes, so any other return drops it. A click always drops it, since it may have
/// moved the caret. Nothing ever sends it to an element it wasn't typed in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HeldFocus {
    /// The focused element when it was held, if a focus event named one.
    at: Option<FocusId>,
    /// When focus left it.
    away_since: Option<Instant>,
    /// Input our hooks never saw arrived while it was held.
    missed: bool,
}

impl HeldFocus {
    /// How soon focus must come back for a set-aside Backspace to be sent after all.
    pub const RETURN_WITHIN: Duration = Duration::from_secs(1);

    /// A Backspace is held while `at` has focus (`None`: not known).
    pub fn new(at: Option<FocusId>) -> Self {
        Self {
            at,
            away_since: None,
            missed: false,
        }
    }

    /// Focus left the element it was typed in: it is kept aside, not sent.
    pub fn away(&self) -> bool {
        self.away_since.is_some()
    }

    /// Input went missing (the watchdog, or an event the hook was too busy for): a return of
    /// focus can't be trusted any more.
    pub fn missed_input(&mut self) {
        self.missed = true;
    }

    pub fn input_was_missed(&self) -> bool {
        self.missed
    }

    /// A focus event naming `now` (`None`: a foreground switch, which names no element), at
    /// time `at`.
    pub fn on_focus(&mut self, now: Option<FocusId>, at: Instant) -> HeldVerdict {
        let same = self.at.is_some() && now == self.at;
        match self.away_since {
            None if same => HeldVerdict::Release,
            None => {
                self.away_since = Some(at);
                HeldVerdict::Away
            }
            Some(since) if same => {
                let own_child = now.is_some_and(|(_, _, child)| child != 0);
                if own_child
                    && !self.missed
                    && at.saturating_duration_since(since) < Self::RETURN_WITHIN
                {
                    HeldVerdict::Release
                } else {
                    HeldVerdict::Drop
                }
            }
            Some(_) => HeldVerdict::Keep,
        }
    }

    /// A key-down that would send it on (anything but a plain second Backspace, while focus is
    /// still where it was typed).
    pub fn on_key(&self) -> HeldVerdict {
        if self.away() {
            HeldVerdict::Drop
        } else {
            HeldVerdict::Release
        }
    }
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

    /// Is a key-up of `vk` owed to us (its key-down was swallowed and is still held on the
    /// keyboard)? A key-down of `vk` now is then an auto-repeat that Windows' own key state
    /// can't show, since Windows never saw the press. Changes nothing.
    pub fn awaits_up(&self, vk: u16) -> bool {
        self.up_pending == Some(vk)
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
            !releases_held_backspace(0x08, false, false, false, false),
            "second Backspace undoes"
        );
        assert!(
            releases_held_backspace(0x08, true, false, false, false),
            "auto-repeat"
        );
        assert!(
            releases_held_backspace(0x08, false, true, false, false),
            "Ctrl+Backspace"
        );
        for vk in [0xBC, 0x41, 0x25, 0x10, 0x0D, 0x20] {
            assert!(
                releases_held_backspace(vk, false, false, false, false),
                "{vk:#x}"
            );
        }
        // Review blocker: replayed keys release it too, a plain Backspace included, and never
        // start a hold of their own.
        for vk in [0x08, 0xBC, 0x41] {
            assert!(
                releases_held_backspace(vk, false, false, true, false),
                "{vk:#x}"
            );
        }
        assert!(may_hold_backspace(false));
        assert!(!may_hold_backspace(true));
    }

    #[test]
    fn a_second_backspace_after_missed_input_releases_the_hold() {
        assert!(releases_held_backspace(0x08, false, false, false, true));
    }

    /// #66: every order of focus events, keys and clicks while a first Backspace is held.
    #[test]
    fn a_held_backspace_follows_focus() {
        use HeldVerdict::*;
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let field: FocusId = (0x100, -4, 7);
        let popup: FocusId = (0x200, -4, 0);
        let list: FocusId = (0x100, -4, 9);

        // The same element fires focus again while focus never left: sent there now.
        let mut h = HeldFocus::new(Some(field));
        assert_eq!(h.on_focus(Some(field), ms(5_000)), Release);

        // A toast or popup takes focus and gives it back quickly: sent on return.
        let mut h = HeldFocus::new(Some(field));
        assert_eq!(h.on_focus(Some(popup), ms(0)), Away);
        assert!(h.away());
        assert_eq!(
            h.on_focus(None, ms(300)),
            Keep,
            "the foreground switch back"
        );
        assert_eq!(h.on_focus(Some(field), ms(400)), Release);

        // Another element in the same window (an autocomplete list) keeps focus: the next key
        // drops it.
        let mut h = HeldFocus::new(Some(field));
        assert_eq!(h.on_focus(Some(list), ms(0)), Away);
        assert_eq!(h.on_key(), Drop);

        // Focus moves on and on: still kept aside, never sent elsewhere.
        let mut h = HeldFocus::new(Some(field));
        assert_eq!(h.on_focus(Some(popup), ms(0)), Away);
        assert_eq!(h.on_focus(Some(list), ms(100)), Keep);
        assert_eq!(h.on_focus(None, ms(200)), Keep);
        assert_eq!(h.on_key(), Drop);

        // A foreground switch names no element: away until the element itself comes back.
        let mut h = HeldFocus::new(Some(field));
        assert_eq!(h.on_focus(None, ms(0)), Away);
        assert_eq!(h.on_focus(Some(field), ms(999)), Release);

        // Where it was typed isn't known: any focus event puts it aside for good.
        let mut h = HeldFocus::new(None);
        assert_eq!(h.on_focus(None, ms(0)), Away);
        assert_eq!(h.on_focus(Some(field), ms(10)), Keep);
        assert_eq!(h.on_key(), Drop);

        // No focus event: the next key sends it on as before (a click drops it, as before).
        let h = HeldFocus::new(Some(field));
        assert!(!h.away());
        assert_eq!(h.on_key(), Release);
    }

    /// Review of #66: a return of focus is trusted only when it names a control of its own,
    /// comes back within a second, and no input went missing meanwhile. Otherwise dropped.
    #[test]
    fn an_untrusted_return_of_focus_drops_the_held_backspace() {
        use HeldVerdict::*;
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let field: FocusId = (0x100, -4, 7);
        // Swing, Qt, Flutter, games: every control is the window itself (CHILDID_SELF).
        let window: FocusId = (0x300, -4, 0);
        let popup: FocusId = (0x200, -4, 0);

        let mut h = HeldFocus::new(Some(window));
        assert_eq!(h.on_focus(Some(popup), ms(0)), Away);
        assert_eq!(h.on_focus(Some(window), ms(100)), Drop, "no child id");
        // ...though a re-fire while focus never left still sends it.
        let mut h = HeldFocus::new(Some(window));
        assert_eq!(h.on_focus(Some(window), ms(100)), Release);

        // An elevated window or anything else that keeps focus for a while.
        let mut h = HeldFocus::new(Some(field));
        assert_eq!(h.on_focus(None, ms(0)), Away);
        assert_eq!(
            h.on_focus(Some(field), ms(1_000)),
            Drop,
            "a second is too long"
        );
        let mut h = HeldFocus::new(Some(field));
        h.on_focus(Some(popup), ms(0));
        assert_eq!(h.on_focus(Some(field), ms(120_000)), Drop);

        // Input went missing while it was held, before or after focus left.
        for before in [true, false] {
            let mut h = HeldFocus::new(Some(field));
            if before {
                h.missed_input();
            }
            assert_eq!(h.on_focus(Some(popup), ms(0)), Away);
            if !before {
                h.missed_input();
            }
            assert_eq!(
                h.on_focus(Some(field), ms(50)),
                Drop,
                "missed before: {before}"
            );
        }
    }

    /// #39: a second Backspace that went to the app instead of undoing takes the held one with
    /// it; the one that started the hold, an undo, or another key never does.
    #[test]
    fn an_unanswered_hold_goes_out_with_the_backspace() {
        assert!(releases_unanswered_hold(0x08, false, true), "refused undo");
        assert!(
            !releases_unanswered_hold(0x08, true, true),
            "undo or a new hold"
        );
        assert!(
            !releases_unanswered_hold(0x08, false, false),
            "nothing held"
        );
        assert!(
            !releases_unanswered_hold(0x41, false, true),
            "released before"
        );
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

    /// #110: Enter, then "teh " typed before UIA answers.
    #[test]
    fn letters_typed_while_pending_wait_for_the_answer() {
        use FieldCheck::*;
        let mut w = PendingWord::default();
        assert_eq!(w.on_key(Key::Char('t'), Pending), None);
        assert_eq!(w.on_key(Key::Char('e'), Pending), None);
        // Cleared before the word ends: its letters go first, then 'h' as usual.
        assert_eq!(w.on_key(Key::Char('h'), Cleared), Some(vec!['t', 'e']));
        assert_eq!(w.on_key(Key::Boundary(' '), Cleared), Some(vec![]));

        // Still pending when the word ends: forgotten, the word is skipped as before.
        for end in [
            Key::Boundary(' '),
            Key::Backspace,
            Key::Reset,
            Key::DeleteWordLeft,
        ] {
            let mut w = PendingWord::default();
            w.on_key(Key::Char('t'), Pending);
            assert_eq!(w.on_key(end, Pending), Some(vec![]), "{end:?}");
            assert_eq!(w.on_key(Key::Char('x'), Cleared), Some(vec![]), "{end:?}");
        }

        // A password field: forgotten, never handed over.
        let mut w = PendingWord::default();
        w.on_key(Key::Char('h'), Pending);
        w.on_key(Key::Char('u'), Pending);
        assert_eq!(w.on_key(Key::Char('n'), Secure), Some(vec![]));
        assert_eq!(w.on_key(Key::Char('t'), Cleared), Some(vec![]));

        // Too long to be a word: forgotten.
        let mut w = PendingWord::default();
        for _ in 0..PendingWord::MAX {
            assert_eq!(w.on_key(Key::Char('a'), Pending), None);
        }
        assert_eq!(w.on_key(Key::Char('a'), Pending), Some(vec![]));
        assert_eq!(w.on_key(Key::Char('a'), Cleared), Some(vec![]));

        // A caret move in between.
        let mut w = PendingWord::default();
        w.on_key(Key::Char('a'), Pending);
        w.clear();
        assert_eq!(w.on_key(Key::Char('b'), Cleared), Some(vec![]));
    }

    /// Review finding 6: held letters are overwritten, not just forgotten.
    #[test]
    fn wiped_letters_leave_nothing_behind() {
        let mut letters = Vec::with_capacity(8);
        letters.extend("hunter2".chars());
        let n = letters.len();
        wipe(&mut letters);
        assert!(letters.is_empty());
        assert_eq!(letters.capacity(), 8, "same buffer");
        let old: Vec<char> = letters.spare_capacity_mut()[..n]
            .iter()
            // SAFETY: these slots held chars and were overwritten with NULs, which are chars.
            .map(|c| unsafe { c.assume_init_read() })
            .collect();
        assert_eq!(old, vec!['\0'; n]);

        let mut w = PendingWord::default();
        assert_eq!(w.letters.capacity(), PendingWord::MAX, "never reallocates");
        // Debug output never shows the letters.
        w.on_key(Key::Char('q'), FieldCheck::Pending);
        w.on_key(Key::Char('z'), FieldCheck::Pending);
        let shown = format!("{w:?}");
        assert_eq!(shown, "PendingWord { held: 2 }");
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

    /// Review finding 3: an injected key with Ctrl held on the keyboard is vetoed too.
    #[test]
    fn physical_modifiers_veto_injected_keys() {
        let ctrl = Modifiers { ctrl: true, ..NONE };
        let win = Modifiers { win: true, ..NONE };
        for physical in [ctrl, win, Modifiers { alt: true, ..NONE }] {
            let mods = veto_modifiers(NONE, physical);
            assert_eq!(
                injection_veto(Key::Boundary(' '), mods, false, false),
                Some("modifier held"),
                "{physical:?}"
            );
        }
        assert_eq!(
            injection_veto(Key::Boundary(' '), veto_modifiers(NONE, NONE), false, false),
            None
        );
        let caps = Modifiers {
            caps_lock: true,
            ..NONE
        };
        assert!(veto_modifiers(caps, NONE).caps_lock);
        assert!(
            !veto_modifiers(NONE, caps).caps_lock,
            "caps lock is the key's own"
        );
        assert_eq!(veto_modifiers(ctrl, NONE), ctrl);
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

    /// #39: a held key's repeat is known before it is handled, without using it up.
    #[test]
    fn a_swallowed_key_still_down_is_seen() {
        let mut t = SwallowTracker::default();
        assert!(!t.awaits_up(0x08));
        t.swallowed(0x08);
        assert!(t.awaits_up(0x08));
        assert!(!t.awaits_up(0x41));
        assert!(t.awaits_up(0x08), "asking changes nothing");
        assert!(t.on_up(0x08));
        assert!(!t.awaits_up(0x08), "let go");
    }
}
