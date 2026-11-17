//! Virtual key + modifier state -> [`Key`]. Pure logic, no OS calls, so it is tested on every OS.
//!
//! The Windows hook supplies the raw event, the modifier state and (lazily) the text the key
//! would type under the current layout. Asking for that text is the expensive and stateful part
//! (`ToUnicodeEx`), so it is a closure that only runs when the key could be a character.
//!
//! Dead keys: the hook asks `ToUnicodeEx` *not* to touch the kernel's dead-key buffer, so the
//! user's accents keep working in their app. The cost is that we cannot see what a dead key
//! composes with. We remember that a dead key is pending and report the next key as
//! [`Key::Reset`]: the engine forgets the word rather than guessing at `é` vs `e`. Clicks and
//! focus changes don't clear that memory: the app's dead-key state survives them too, so the
//! next key may still compose.

use unflub_core::Key;

/// Virtual-key codes we care about (values from WinUser.h).
pub mod vk {
    pub const BACK: u16 = 0x08;
    pub const TAB: u16 = 0x09;
    pub const RETURN: u16 = 0x0D;
    pub const SHIFT: u16 = 0x10;
    pub const CONTROL: u16 = 0x11;
    pub const MENU: u16 = 0x12;
    pub const CAPITAL: u16 = 0x14;
    pub const ESCAPE: u16 = 0x1B;
    pub const SPACE: u16 = 0x20;
    pub const PRIOR: u16 = 0x21;
    pub const NEXT: u16 = 0x22;
    pub const END: u16 = 0x23;
    pub const HOME: u16 = 0x24;
    pub const LEFT: u16 = 0x25;
    pub const UP: u16 = 0x26;
    pub const RIGHT: u16 = 0x27;
    pub const DOWN: u16 = 0x28;
    pub const INSERT: u16 = 0x2D;
    pub const DELETE: u16 = 0x2E;
    pub const LWIN: u16 = 0x5B;
    pub const RWIN: u16 = 0x5C;
    pub const APPS: u16 = 0x5D;
    pub const F1: u16 = 0x70;
    pub const F24: u16 = 0x87;
    pub const NUMLOCK: u16 = 0x90;
    pub const SCROLL: u16 = 0x91;
    pub const LSHIFT: u16 = 0xA0;
    pub const RMENU: u16 = 0xA5;
    pub const PROCESSKEY: u16 = 0xE5;
    pub const PACKET: u16 = 0xE7;
}

/// One low-level keyboard event, as the hook saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawKey {
    pub vk: u16,
    pub scan: u16,
    /// Key-down (including auto-repeat). Key-ups are ignored.
    pub down: bool,
    /// Injected by another program (not us). Our own injections never reach the translator.
    pub foreign_injected: bool,
}

/// Physical modifier state at the time of the event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub win: bool,
    pub caps_lock: bool,
}

impl Modifiers {
    /// Ctrl and Alt together: that is AltGr on layouts that have it (Windows treats them alike).
    pub fn altgr(&self) -> bool {
        self.ctrl && self.alt
    }

    /// Any modifier that turns a key into a shortcut.
    pub fn shortcut(&self) -> bool {
        self.ctrl || self.alt || self.win
    }
}

/// What the key types under the current layout (the result of `ToUnicodeEx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Typed {
    /// Exactly one character.
    Char(char),
    /// A dead key (accent waiting for the next key).
    Dead,
    /// Nothing, or more than one character (ligatures, uncomposable dead-key output).
    Other,
}

impl Typed {
    /// Interpret `ToUnicodeEx`'s return value and buffer.
    pub fn from_to_unicode(ret: i32, buf: &[u16]) -> Self {
        if ret < 0 {
            return Typed::Dead;
        }
        let len = (ret as usize).min(buf.len());
        let mut chars = char::decode_utf16(buf[..len].iter().copied());
        match (chars.next(), chars.next()) {
            (Some(Ok(c)), None) => Typed::Char(c),
            _ => Typed::Other,
        }
    }
}

/// Is this a word character? Letters, digits, apostrophes (straight and typographic) and hyphen.
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '\'' | '\u{2019}' | '-')
}

/// Map a typed character to a key: word characters, boundaries, or Reset for control codes.
pub fn classify_char(c: char) -> Key {
    if is_word_char(c) {
        Key::Char(c)
    } else if c == '\r' || c == '\n' {
        Key::Boundary('\n')
    } else if c == ' ' || c == '\t' || !c.is_control() {
        Key::Boundary(c)
    } else {
        Key::Reset
    }
}

fn is_modifier(vk: u16) -> bool {
    matches!(
        vk,
        vk::SHIFT
            | vk::CONTROL
            | vk::MENU
            | vk::CAPITAL
            | vk::LWIN
            | vk::RWIN
            | vk::NUMLOCK
            | vk::SCROLL
    ) || (vk::LSHIFT..=vk::RMENU).contains(&vk)
}

/// Keys that move the caret, change mode or otherwise make our view of the text stale.
fn is_reset_key(vk: u16) -> bool {
    matches!(
        vk,
        0x01..=0x07 // mouse buttons, Cancel
            | 0x0C // Clear
            | 0x13 // Pause
            | 0x15..=0x1A // IME keys (Kana/Hangul, Junja, Final, Hanja/Kanji, IME on/off)
            | vk::ESCAPE
            | 0x1C..=0x1F // IME Convert, NonConvert, Accept, ModeChange
            | vk::PRIOR..=vk::DOWN
            | 0x29..=0x2C // Select, Print, Execute, Snapshot
            | vk::INSERT
            | vk::DELETE
            | 0x2F // Help
            | vk::APPS
            | 0x5F // Sleep
            | vk::F1..=vk::F24
            | 0xA6..=0xB7 // browser, volume, media and launch keys
            | vk::PROCESSKEY
            | vk::PACKET
            | 0xF6..=0xFE // Attn, CrSel, ExSel, EraseEOF, Play, Zoom, PA1, OEM Clear
    )
}

/// Turns raw events into [`Key`]s. Holds the only state translation needs: a pending dead key.
#[derive(Debug, Default)]
pub struct KeyTranslator {
    dead_pending: bool,
}

impl KeyTranslator {
    pub fn new() -> Self {
        Self::default()
    }

    /// True after a dead key until the next non-modifier key.
    pub fn dead_pending(&self) -> bool {
        self.dead_pending
    }

    /// Translate one event. `None` means "not interesting, don't tell the engine" (key-ups,
    /// bare modifiers, dead keys). `typed` is only called when the key could type a character.
    pub fn translate(
        &mut self,
        key: RawKey,
        mods: Modifiers,
        typed: impl FnOnce() -> Typed,
    ) -> Option<Key> {
        if !key.down {
            return None;
        }
        if key.foreign_injected {
            self.dead_pending = false;
            return Some(Key::Reset);
        }
        if is_modifier(key.vk) {
            return None;
        }
        // Whatever follows a dead key composes with it in the app; we can't see the result.
        if std::mem::take(&mut self.dead_pending) || mods.win || is_reset_key(key.vk) {
            return Some(Key::Reset);
        }
        let any = mods.shift || mods.ctrl || mods.alt;
        match key.vk {
            vk::BACK => return Some(if any { Key::Reset } else { Key::Backspace }),
            vk::TAB => return Some(if any { Key::Reset } else { Key::Boundary('\t') }),
            vk::RETURN => {
                return Some(if mods.ctrl || mods.alt {
                    Key::Reset
                } else {
                    Key::Boundary('\n')
                })
            }
            vk::SPACE => {
                return Some(if mods.ctrl || mods.alt {
                    Key::Reset
                } else {
                    Key::Boundary(' ')
                })
            }
            _ => {}
        }
        // Ctrl or Alt alone is a shortcut. Both together may be AltGr typing a character.
        if (mods.ctrl || mods.alt) && !mods.altgr() {
            return Some(Key::Reset);
        }
        match typed() {
            Typed::Dead => {
                self.dead_pending = true;
                None
            }
            Typed::Char(c) if !c.is_control() => Some(classify_char(c)),
            Typed::Char(_) | Typed::Other => Some(Key::Reset),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn down(vk: u16) -> RawKey {
        RawKey {
            vk,
            scan: 0,
            down: true,
            foreign_injected: false,
        }
    }

    const NONE: Modifiers = Modifiers {
        shift: false,
        ctrl: false,
        alt: false,
        win: false,
        caps_lock: false,
    };
    const SHIFT: Modifiers = Modifiers {
        shift: true,
        ..NONE
    };
    const CTRL: Modifiers = Modifiers { ctrl: true, ..NONE };
    const ALT: Modifiers = Modifiers { alt: true, ..NONE };
    const WIN: Modifiers = Modifiers { win: true, ..NONE };
    const ALTGR: Modifiers = Modifiers {
        ctrl: true,
        alt: true,
        ..NONE
    };

    fn tr(vk: u16, mods: Modifiers, typed: Typed) -> Option<Key> {
        KeyTranslator::new().translate(down(vk), mods, || typed)
    }

    #[test]
    fn letters_digits_apostrophe_hyphen_are_chars() {
        assert_eq!(tr(0x41, NONE, Typed::Char('a')), Some(Key::Char('a')));
        assert_eq!(tr(0x41, SHIFT, Typed::Char('A')), Some(Key::Char('A')));
        assert_eq!(tr(0x35, NONE, Typed::Char('5')), Some(Key::Char('5')));
        assert_eq!(tr(0xDE, NONE, Typed::Char('\'')), Some(Key::Char('\'')));
        assert_eq!(tr(0xBD, NONE, Typed::Char('-')), Some(Key::Char('-')));
        assert_eq!(tr(0xBA, NONE, Typed::Char('ü')), Some(Key::Char('ü')));
    }

    #[test]
    fn whitespace_enter_and_punctuation_are_boundaries() {
        assert_eq!(tr(vk::SPACE, NONE, Typed::Other), Some(Key::Boundary(' ')));
        assert_eq!(tr(vk::SPACE, SHIFT, Typed::Other), Some(Key::Boundary(' ')));
        assert_eq!(tr(vk::TAB, NONE, Typed::Other), Some(Key::Boundary('\t')));
        assert_eq!(
            tr(vk::RETURN, NONE, Typed::Other),
            Some(Key::Boundary('\n'))
        );
        assert_eq!(
            tr(vk::RETURN, SHIFT, Typed::Other),
            Some(Key::Boundary('\n'))
        );
        assert_eq!(tr(0xBE, NONE, Typed::Char('.')), Some(Key::Boundary('.')));
        assert_eq!(tr(0xBF, SHIFT, Typed::Char('?')), Some(Key::Boundary('?')));
        assert_eq!(tr(0x31, SHIFT, Typed::Char('!')), Some(Key::Boundary('!')));
    }

    #[test]
    fn backspace_only_without_modifiers() {
        assert_eq!(tr(vk::BACK, NONE, Typed::Other), Some(Key::Backspace));
        assert_eq!(tr(vk::BACK, CTRL, Typed::Other), Some(Key::Reset));
        assert_eq!(tr(vk::BACK, SHIFT, Typed::Other), Some(Key::Reset));
        assert_eq!(tr(vk::TAB, SHIFT, Typed::Other), Some(Key::Reset));
    }

    #[test]
    fn navigation_editing_and_function_keys_reset() {
        for vk in [
            vk::LEFT,
            vk::RIGHT,
            vk::UP,
            vk::DOWN,
            vk::HOME,
            vk::END,
            vk::PRIOR,
            vk::NEXT,
            vk::DELETE,
            vk::INSERT,
            vk::ESCAPE,
            vk::F1,
            0x7B,
            vk::F24,
            vk::APPS,
            vk::PROCESSKEY,
        ] {
            let typed_called = std::cell::Cell::new(false);
            let got = KeyTranslator::new().translate(down(vk), NONE, || {
                typed_called.set(true);
                Typed::Other
            });
            assert_eq!(got, Some(Key::Reset), "vk {vk:#x}");
            assert!(!typed_called.get(), "vk {vk:#x} must not call ToUnicodeEx");
        }
    }

    #[test]
    fn shortcuts_reset_but_altgr_characters_type() {
        assert_eq!(tr(0x43, CTRL, Typed::Char('\u{3}')), Some(Key::Reset));
        assert_eq!(tr(0x46, ALT, Typed::Char('f')), Some(Key::Reset));
        assert_eq!(tr(0x41, WIN, Typed::Char('a')), Some(Key::Reset));
        assert_eq!(tr(vk::SPACE, CTRL, Typed::Other), Some(Key::Reset));
        assert_eq!(tr(vk::RETURN, ALT, Typed::Other), Some(Key::Reset));
        // German layout: AltGr+Q = '@', AltGr+E = '€'.
        assert_eq!(tr(0x51, ALTGR, Typed::Char('@')), Some(Key::Boundary('@')));
        assert_eq!(tr(0x45, ALTGR, Typed::Char('€')), Some(Key::Boundary('€')));
        // Polish programmer layout: AltGr+A = 'ą'.
        assert_eq!(tr(0x41, ALTGR, Typed::Char('ą')), Some(Key::Char('ą')));
        // Ctrl+Alt on a layout without AltGr types nothing.
        assert_eq!(tr(0x41, ALTGR, Typed::Other), Some(Key::Reset));
    }

    #[test]
    fn ctrl_alone_never_calls_to_unicode() {
        let got = KeyTranslator::new().translate(down(0x41), CTRL, || panic!("called"));
        assert_eq!(got, Some(Key::Reset));
    }

    #[test]
    fn key_ups_and_bare_modifiers_are_ignored() {
        let up = RawKey {
            down: false,
            ..down(0x41)
        };
        assert_eq!(
            KeyTranslator::new().translate(up, NONE, || Typed::Char('a')),
            None
        );
        for vk in [
            vk::SHIFT,
            vk::LSHIFT,
            0xA1,
            0xA2,
            0xA3,
            0xA4,
            vk::RMENU,
            vk::LWIN,
            vk::CAPITAL,
        ] {
            assert_eq!(tr(vk, NONE, Typed::Other), None, "vk {vk:#x}");
        }
    }

    #[test]
    fn foreign_injection_resets_but_its_key_ups_are_ignored() {
        let mut t = KeyTranslator::new();
        let injected = RawKey {
            foreign_injected: true,
            ..down(0x56)
        };
        assert_eq!(
            t.translate(injected, CTRL, || Typed::Other),
            Some(Key::Reset)
        );
        let up = RawKey {
            down: false,
            ..injected
        };
        assert_eq!(t.translate(up, CTRL, || Typed::Other), None);
    }

    #[test]
    fn dead_key_is_silent_and_the_composed_key_resets() {
        let mut t = KeyTranslator::new();
        assert_eq!(t.translate(down(0xDD), NONE, || Typed::Dead), None);
        assert!(t.dead_pending());
        // Shift between the accent and the letter doesn't consume the dead key.
        assert_eq!(t.translate(down(vk::LSHIFT), SHIFT, || Typed::Other), None);
        assert!(t.dead_pending());
        // The letter after it composes in the app (é); we don't guess.
        assert_eq!(
            t.translate(down(0x45), NONE, || panic!("not called")),
            Some(Key::Reset)
        );
        assert!(!t.dead_pending());
        // And typing carries on normally.
        assert_eq!(
            t.translate(down(0x45), NONE, || Typed::Char('e')),
            Some(Key::Char('e'))
        );
    }

    #[test]
    fn dead_key_then_space_resets() {
        let mut t = KeyTranslator::new();
        t.translate(down(0xDE), NONE, || Typed::Dead);
        assert_eq!(
            t.translate(down(vk::SPACE), NONE, || Typed::Other),
            Some(Key::Reset)
        );
    }

    #[test]
    fn unknown_or_multi_char_output_resets() {
        assert_eq!(tr(0xE2, NONE, Typed::Other), Some(Key::Reset));
        assert_eq!(tr(0x41, NONE, Typed::Char('\u{7f}')), Some(Key::Reset));
    }

    #[test]
    fn typed_from_to_unicode_results() {
        assert_eq!(Typed::from_to_unicode(-1, &[0xB4]), Typed::Dead);
        assert_eq!(Typed::from_to_unicode(0, &[0; 4]), Typed::Other);
        assert_eq!(
            Typed::from_to_unicode(1, &[b'a' as u16, 0]),
            Typed::Char('a')
        );
        assert_eq!(
            Typed::from_to_unicode(2, &[0xB4, b't' as u16]),
            Typed::Other
        );
        // A surrogate pair is one character.
        assert_eq!(
            Typed::from_to_unicode(2, &[0xD83D, 0xDE00]),
            Typed::Char('\u{1F600}')
        );
        // A lone surrogate is not.
        assert_eq!(Typed::from_to_unicode(1, &[0xD83D]), Typed::Other);
        // Return value larger than the buffer is clamped.
        assert_eq!(Typed::from_to_unicode(9, &[b'x' as u16]), Typed::Char('x'));
    }

    #[test]
    fn classify_chars() {
        assert_eq!(classify_char('\u{2019}'), Key::Char('\u{2019}'));
        assert_eq!(classify_char('\r'), Key::Boundary('\n'));
        assert_eq!(classify_char(','), Key::Boundary(','));
        assert_eq!(classify_char('\u{1b}'), Key::Reset);
    }
}
