//! Text injection: turn an [`Action`](unflub_core::Action)'s `delete` + `insert` into keystrokes.
//!
//! [`plan`] is pure and tested on every OS. On Windows, [`to_inputs`] turns the plan into one
//! `INPUT` array (key-down then key-up per stroke) for a single `SendInput` call, tagged with
//! [`INJECT_TAG`] so our own hook recognises and ignores it.

/// `dwExtraInfo` on every event we inject ("UNFL").
pub const INJECT_TAG: usize = 0x554E_464C;

/// One keystroke to send (down then up).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stroke {
    /// A real virtual key with its scan code (Backspace, Enter, Tab).
    Vk { vk: u16, scan: u16 },
    /// One UTF-16 code unit via `KEYEVENTF_UNICODE`. Characters outside the BMP become two.
    Unit(u16),
}

pub const BACKSPACE: Stroke = Stroke::Vk {
    vk: 0x08,
    scan: 0x0E,
};
pub const ENTER: Stroke = Stroke::Vk {
    vk: 0x0D,
    scan: 0x1C,
};
pub const TAB: Stroke = Stroke::Vk {
    vk: 0x09,
    scan: 0x0F,
};

/// `delete` backspaces, then `insert`. `\n` (or a lone `\r`, or `\r\n`) is sent as Enter and
/// `\t` as Tab, since many apps ignore those characters when they arrive as Unicode packets.
pub fn plan(delete: usize, insert: &str) -> Vec<Stroke> {
    let mut out = Vec::with_capacity(delete + insert.len() + 2);
    out.extend(std::iter::repeat_n(BACKSPACE, delete));
    let mut chars = insert.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push(ENTER);
            }
            '\n' => out.push(ENTER),
            '\t' => out.push(TAB),
            _ => {
                let mut buf = [0u16; 2];
                out.extend(c.encode_utf16(&mut buf).iter().map(|&u| Stroke::Unit(u)));
            }
        }
    }
    out
}

/// What happened to one action's `SendInput`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOutcome {
    /// Every event went out.
    Complete,
    /// Only `sent` of `total` events went out (0 when blocked, e.g. by UIPI). `released` is
    /// true when a key-down went out without its key-up and we sent the key-up separately.
    Partial {
        sent: usize,
        total: usize,
        released: bool,
    },
}

/// Inputs come in down/up pairs. If `SendInput` stopped after a key-down (odd count), returns
/// the index of the key-up that must still be sent so no key is left held down.
pub fn missing_key_up(sent: usize, total: usize) -> Option<usize> {
    (sent < total && sent % 2 == 1).then_some(sent)
}

#[cfg(windows)]
pub use win::{send, to_inputs};

#[cfg(windows)]
mod win {
    use super::{SendOutcome, Stroke, INJECT_TAG};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        KEYEVENTF_UNICODE, VIRTUAL_KEY,
    };

    fn key(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: scan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: INJECT_TAG,
                },
            },
        }
    }

    /// Down + up for every stroke, in order.
    pub fn to_inputs(strokes: &[Stroke]) -> Vec<INPUT> {
        let mut out = Vec::with_capacity(strokes.len() * 2);
        for s in strokes {
            let (vk, scan, flags) = match *s {
                Stroke::Vk { vk, scan } => (vk, scan, KEYBD_EVENT_FLAGS(0)),
                Stroke::Unit(u) => (0, u, KEYEVENTF_UNICODE),
            };
            out.push(key(vk, scan, flags));
            out.push(key(vk, scan, flags | KEYEVENTF_KEYUP));
        }
        out
    }

    fn send_raw(inputs: &[INPUT]) -> usize {
        if inputs.is_empty() {
            return 0;
        }
        // SAFETY: `inputs` is a valid slice of initialised INPUT structs and cbsize matches.
        unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) as usize }
    }

    /// One `SendInput` call for the whole action. Anything short of every event is a failure;
    /// a key-down left without its key-up is released with one more call.
    pub fn send(inputs: &[INPUT]) -> SendOutcome {
        let total = inputs.len();
        let sent = send_raw(inputs);
        if sent == total {
            return SendOutcome::Complete;
        }
        let released = super::missing_key_up(sent, total)
            .is_some_and(|i| send_raw(std::slice::from_ref(&inputs[i])) == 1);
        SendOutcome::Partial {
            sent,
            total,
            released,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deletes_then_types() {
        assert_eq!(
            plan(3, "the "),
            vec![
                BACKSPACE,
                BACKSPACE,
                BACKSPACE,
                Stroke::Unit('t' as u16),
                Stroke::Unit('h' as u16),
                Stroke::Unit('e' as u16),
                Stroke::Unit(' ' as u16),
            ]
        );
    }

    #[test]
    fn newline_and_tab_are_real_keys() {
        assert_eq!(plan(0, "a\n"), vec![Stroke::Unit('a' as u16), ENTER]);
        assert_eq!(plan(0, "a\t"), vec![Stroke::Unit('a' as u16), TAB]);
        assert_eq!(plan(0, "\r\n"), vec![ENTER]);
        assert_eq!(plan(0, "\r"), vec![ENTER]);
    }

    #[test]
    fn astral_characters_become_surrogate_pairs() {
        assert_eq!(
            plan(1, "\u{1F600}é"),
            vec![
                BACKSPACE,
                Stroke::Unit(0xD83D),
                Stroke::Unit(0xDE00),
                Stroke::Unit(0x00E9),
            ]
        );
    }

    #[test]
    fn partial_send_releases_a_held_key() {
        // 3 strokes = 6 events.
        assert_eq!(missing_key_up(6, 6), None, "complete");
        assert_eq!(missing_key_up(0, 6), None, "nothing sent");
        assert_eq!(missing_key_up(2, 6), None, "stopped between strokes");
        assert_eq!(
            missing_key_up(3, 6),
            Some(3),
            "stroke 2 is down: send event 3 (its up)"
        );
        assert_eq!(missing_key_up(5, 6), Some(5));
    }

    #[test]
    fn empty_action_plans_nothing() {
        assert!(plan(0, "").is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn inputs_are_tagged_down_up_pairs() {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            INPUT_KEYBOARD, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
        };
        let inputs = to_inputs(&plan(1, "\u{1F600}\n"));
        assert_eq!(inputs.len(), 8);
        let ki: Vec<_> = inputs
            .iter()
            .map(|i| {
                assert_eq!(i.r#type, INPUT_KEYBOARD);
                // SAFETY: every INPUT we build is a keyboard input.
                unsafe { i.Anonymous.ki }
            })
            .collect();
        assert!(ki.iter().all(|k| k.dwExtraInfo == INJECT_TAG));
        // Backspace down, up.
        assert_eq!((ki[0].wVk.0, ki[0].dwFlags.0), (0x08, 0));
        assert_eq!((ki[1].wVk.0, ki[1].dwFlags), (0x08, KEYEVENTF_KEYUP));
        // High surrogate down, up; low surrogate down, up.
        assert_eq!(
            (ki[2].wVk.0, ki[2].wScan, ki[2].dwFlags),
            (0, 0xD83D, KEYEVENTF_UNICODE)
        );
        assert_eq!(ki[3].dwFlags, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP);
        assert_eq!((ki[4].wScan, ki[5].wScan), (0xDE00, 0xDE00));
        // Enter as VK_RETURN.
        assert_eq!((ki[6].wVk.0, ki[6].wScan, ki[6].dwFlags.0), (0x0D, 0x1C, 0));
        assert_eq!(ki[7].dwFlags, KEYEVENTF_KEYUP);
    }
}
