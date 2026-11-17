//! The overlay's [`PillConfig`] for the pill settings and the OS look: theme colours, light or
//! dark, and the OS accent for the system theme.

use crate::look::OsLook;
use crate::protocol::{self, Appearance, PillSettings, Theme};
use unflub_windows::overlay::Rgba;
use unflub_windows::{PillConfig, PillPosition, PillStyle};

/// Is the pill drawn in its dark variant?
pub fn is_dark(appearance: Appearance, os: &OsLook) -> bool {
    match appearance {
        Appearance::Light => false,
        Appearance::Dark => true,
        Appearance::Auto => os.apps_dark,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PillColours {
    pub background: Rgba,
    pub text: Rgba,
    pub keycap: Rgba,
    pub accent: Rgba,
}

const fn colours(background: Rgba, text: u32, keycap: u32, accent: u32) -> PillColours {
    PillColours {
        background,
        text: Rgba::hex(text),
        keycap: Rgba::hex(keycap),
        accent: Rgba::hex(accent),
    }
}

/// The pill's colours for a theme. `accent` is the OS accent (`#RRGGBB`), used by the system
/// theme only.
pub fn pill_colours(theme: Theme, dark: bool, accent: Option<&str>) -> PillColours {
    match (theme, dark) {
        (Theme::Unflub, false) => colours(Rgba::hex(0x2D1916), 0xF2E6E3, 0xE0A595, 0xB06A5C),
        (Theme::Unflub, true) => colours(Rgba::hex(0xF2E6E3), 0x2D1916, 0x79453A, 0xD08A7B),
        (Theme::Paper, false) => colours(Rgba::hex(0x292827), 0xE7E2DA, 0xC69C72, 0xA77A55),
        (Theme::Paper, true) => colours(Rgba::hex(0xE7E2DA), 0x292827, 0xA77A55, 0xC69C72),
        (Theme::Ink, false) => colours(
            Rgba {
                a: 0xE0,
                ..Rgba::hex(0x121214)
            },
            0xEDEDEF,
            0xC69C72,
            0xA77A55,
        ),
        (Theme::Ink, true) => colours(Rgba::hex(0x2A2B30), 0xEDEDEF, 0xC69C72, 0xC69C72),
        (Theme::System, dark) => {
            let fallback = Rgba::hex(if dark { 0x60CDFF } else { 0x005FB8 });
            let accent = accent.and_then(parse_hex).unwrap_or(fallback);
            let (background, text) = if dark {
                (0x2C2C2C, 0xFFFFFF)
            } else {
                (0xF9F9F9, 0x1B1B1B)
            };
            PillColours {
                background: Rgba::hex(background),
                text: Rgba::hex(text),
                keycap: accent,
                accent,
            }
        }
    }
}

/// `#RRGGBB` or `#RRGGBBAA`.
fn parse_hex(s: &str) -> Option<Rgba> {
    let hex = s.strip_prefix('#')?;
    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    Some(if hex.len() == 6 {
        Rgba::hex(v)
    } else {
        Rgba {
            a: v as u8,
            ..Rgba::hex(v >> 8)
        }
    })
}

/// The overlay's settings for the pill settings and the current OS look.
pub fn pill_config(s: &PillSettings, os: &OsLook) -> PillConfig {
    let c = pill_colours(s.theme, is_dark(s.appearance, os), os.accent.as_deref());
    PillConfig {
        style: match s.style {
            protocol::PillStyle::UnderlineCapsule => PillStyle::UnderlineCapsule,
            protocol::PillStyle::Capsule => PillStyle::Capsule,
            protocol::PillStyle::Underline => PillStyle::Underline,
            protocol::PillStyle::Off => PillStyle::Off,
        },
        position: match s.position {
            protocol::PillPosition::Above => PillPosition::Above,
            protocol::PillPosition::Below => PillPosition::Below,
        },
        opacity: s.opacity,
        duration_ms: s.duration_ms,
        background: c.background,
        text: c.text,
        keycap: c.keycap,
        accent: c.accent,
        uia_caret: s.uia_caret,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(hex: u32) -> Rgba {
        Rgba::hex(hex)
    }

    fn look(apps_dark: bool, accent: Option<&str>) -> OsLook {
        OsLook {
            apps_dark,
            accent: accent.map(String::from),
        }
    }

    #[test]
    fn appearance_resolves_against_the_os() {
        for os_dark in [false, true] {
            let os = look(os_dark, None);
            assert!(!is_dark(Appearance::Light, &os));
            assert!(is_dark(Appearance::Dark, &os));
            assert_eq!(is_dark(Appearance::Auto, &os), os_dark);
        }
    }

    #[test]
    fn theme_colours() {
        let c = |t, dark| {
            let c = pill_colours(t, dark, Some("#123456"));
            (c.background, c.text, c.keycap, c.accent)
        };
        let table = [
            (
                Theme::Unflub,
                false,
                [0x2D1916, 0xF2E6E3, 0xE0A595, 0xB06A5C],
            ),
            (
                Theme::Unflub,
                true,
                [0xF2E6E3, 0x2D1916, 0x79453A, 0xD08A7B],
            ),
            (
                Theme::Paper,
                false,
                [0x292827, 0xE7E2DA, 0xC69C72, 0xA77A55],
            ),
            (Theme::Paper, true, [0xE7E2DA, 0x292827, 0xA77A55, 0xC69C72]),
            (Theme::Ink, true, [0x2A2B30, 0xEDEDEF, 0xC69C72, 0xC69C72]),
            (
                Theme::System,
                false,
                [0xF9F9F9, 0x1B1B1B, 0x123456, 0x123456],
            ),
            (
                Theme::System,
                true,
                [0x2C2C2C, 0xFFFFFF, 0x123456, 0x123456],
            ),
        ];
        for (theme, dark, [bg, text, key, accent]) in table {
            assert_eq!(
                c(theme, dark),
                (rgb(bg), rgb(text), rgb(key), rgb(accent)),
                "{theme:?} dark={dark}"
            );
        }
        let ink = pill_colours(Theme::Ink, false, None);
        assert_eq!(
            ink.background,
            Rgba {
                r: 0x12,
                g: 0x12,
                b: 0x14,
                a: 0xE0
            }
        );
        assert_eq!(
            (ink.text, ink.keycap, ink.accent),
            (rgb(0xEDEDEF), rgb(0xC69C72), rgb(0xA77A55))
        );
    }

    #[test]
    fn system_theme_falls_back_to_the_default_accent() {
        for accent in [None, Some("not a colour"), Some("#12345")] {
            assert_eq!(
                pill_colours(Theme::System, false, accent).accent,
                rgb(0x005FB8)
            );
            assert_eq!(
                pill_colours(Theme::System, true, accent).keycap,
                rgb(0x60CDFF)
            );
        }
    }

    #[test]
    fn parses_hex_colours() {
        assert_eq!(parse_hex("#0078D4"), Some(rgb(0x0078D4)));
        assert_eq!(
            parse_hex("#121214E0"),
            Some(Rgba {
                r: 0x12,
                g: 0x12,
                b: 0x14,
                a: 0xE0
            })
        );
        for bad in ["0078D4", "#0078D", "#0078D4F", "#GG0000", "#+078D4", ""] {
            assert_eq!(parse_hex(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn maps_settings_to_the_pill() {
        let s = PillSettings {
            style: protocol::PillStyle::Capsule,
            position: protocol::PillPosition::Below,
            opacity: 0.5,
            duration_ms: 2000,
            theme: Theme::Paper,
            appearance: Appearance::Auto,
            uia_caret: false,
        };
        let p = pill_config(&s, &look(true, None));
        assert_eq!(p.style, PillStyle::Capsule);
        assert_eq!(p.position, PillPosition::Below);
        assert_eq!(p.opacity, 0.5);
        assert_eq!(p.duration_ms, 2000);
        assert!(!p.uia_caret);
        assert_eq!(p.background, rgb(0xE7E2DA), "paper, dark from the OS");
        assert_eq!(
            pill_config(&s, &look(false, None)).background,
            rgb(0x292827)
        );

        // Defaults match the overlay's own defaults (the unflub light pill).
        let d = pill_config(&PillSettings::default(), &look(false, None));
        let o = PillConfig::default();
        assert_eq!(d, o);
        for (from, to) in [
            (
                protocol::PillStyle::UnderlineCapsule,
                PillStyle::UnderlineCapsule,
            ),
            (protocol::PillStyle::Underline, PillStyle::Underline),
            (protocol::PillStyle::Off, PillStyle::Off),
        ] {
            let s = PillSettings {
                style: from,
                ..PillSettings::default()
            };
            assert_eq!(pill_config(&s, &OsLook::default()).style, to);
        }
    }
}
