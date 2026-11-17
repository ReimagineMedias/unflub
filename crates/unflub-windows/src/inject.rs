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

#[cfg(windows)]
pub use win::{send, to_inputs};

#[cfg(windows)]
mod win {
    use super::{Stroke, INJECT_TAG};
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

    /// One `SendInput` call. Returns how many events Windows accepted (0 if blocked, e.g. by
    /// UIPI when the foreground window is elevated).
    pub fn send(inputs: &[INPUT]) -> u32 {
        if inputs.is_empty() {
            return 0;
        }
        // SAFETY: `inputs` is a valid slice of initialised INPUT structs and cbsize matches.
        unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) }
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
