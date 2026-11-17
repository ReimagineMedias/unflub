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
            Key::Boundary(_) | Key::Reset => {
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
