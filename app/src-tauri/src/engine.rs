//! Runs the autocorrect stack inside the app: keyboard hook, engine, OS spell checker and the
//! correction pill, with the same threads as the `unflub` CLI.
//!
//! [`EngineHost`] owns one `unflub-engine` thread. It builds everything, applies settings and
//! reads the hook's events (pill, learned words). Every change applies live:
//!
//! - Engine settings go to the hook thread through [`unflub_windows::LiveConfig`], picked up at
//!   the next key without blocking the hook. Excluded apps travel in the same `Config`; the
//!   hook's own exclude list holds only unflub's own exe, so its windows are never corrected.
//! - Pausing removes the keyboard hook (and the pill) entirely; resuming installs a new one.
//! - A language change restarts the hook with a spell checker for the new language.
//! - The pill gets new options and theme colours through `OverlayHandle::set_config`. The OS
//!   look (dark mode, accent) is read again before each pill, so "match system" follows it.
//! - Remote flags ([`RemoteFlags`]) can turn the pill or its UI Automation caret lookup off.
//! - Fixes and undos are reported to the app as [`Observed`] (local stats, opt-in usage counts
//!   and shared corrections decide what to keep), never from the hook thread itself.
//!
//! On macOS and Linux this is a no-op until their adapters exist.

#![cfg_attr(not(windows), allow(dead_code))]

use crate::remote::RemoteFlags;
use crate::settings::{self, Appearance, Settings, Theme};
use std::sync::Arc;
use std::time::Duration;
use unflub_windows::overlay::Rgba;
use unflub_windows::{PillConfig, PillPosition, PillStyle};

pub use host::EngineHost;

/// What happened on the engine thread that the app may count. Words stay on this machine
/// unless the user turned on "Help improve corrections" (`corrections.rs` filters them).
#[derive(Debug, Clone, PartialEq)]
pub enum Observed {
    Corrected {
        original: String,
        corrected: String,
        /// The user's own shortcut, not a typo fix.
        user_rule: bool,
        decide: Duration,
        inject: Duration,
        /// The pill was asked to show it.
        pill: bool,
    },
    Undone {
        original: String,
        corrected: String,
        user_rule: bool,
    },
    CorrectorPanicked,
    /// The adapter's watchdog installed the hooks again (Windows dropped them, or the computer
    /// resumed or was unlocked).
    HookReinstalled,
}

pub type Observer = Arc<dyn Fn(Observed) + Send + Sync>;

/// What the OS look contributes to the pill.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OsLook {
    /// Apps are in dark mode.
    pub apps_dark: bool,
    /// Accent colour as `#RRGGBB`.
    pub accent: Option<String>,
}

impl OsLook {
    pub fn read() -> Self {
        Self {
            apps_dark: crate::system::apps_use_dark(),
            accent: crate::system::accent_color(),
        }
    }
}

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

/// The overlay's settings for the user's settings, the remote flags and the current OS look.
pub fn pill_config(s: &Settings, flags: &RemoteFlags, os: &OsLook) -> PillConfig {
    let c = pill_colours(s.theme, is_dark(s.appearance, os), os.accent.as_deref());
    PillConfig {
        style: match s.pill_style {
            _ if !flags.pill_enabled => PillStyle::Off,
            settings::PillStyle::UnderlineCapsule => PillStyle::UnderlineCapsule,
            settings::PillStyle::Capsule => PillStyle::Capsule,
            settings::PillStyle::Underline => PillStyle::Underline,
            settings::PillStyle::Off => PillStyle::Off,
        },
        position: match s.pill_position {
            settings::PillPosition::Above => PillPosition::Above,
            settings::PillPosition::Below => PillPosition::Below,
        },
        opacity: s.pill_opacity,
        duration_ms: s.pill_duration_ms,
        background: c.background,
        text: c.text,
        keycap: c.keycap,
        accent: c.accent,
        uia_caret: flags.uia_caret,
    }
}

#[cfg(windows)]
mod host {
    use std::sync::atomic::AtomicU64;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::sync::Arc;
    use std::thread::JoinHandle;

    use unflub_core::{Engine, Source, SpellBackend};
    use unflub_windows::{
        Correction, Hook, HookEvent, HookOptions, LiveConfig, LiveEngine, Overlay, OverlayHandle,
        PillConfig, PillStyle, SpellLearner, WindowsSpellBackend,
    };

    use super::{pill_config, Observed, Observer, OsLook};
    use crate::remote::RemoteFlags;
    use crate::settings::Settings;

    enum Msg {
        Apply(Box<Settings>),
        Flags(RemoteFlags),
        Hook(HookEvent),
        /// A word the user taught the engine by undoing its fix.
        Learned(String),
        Shutdown,
    }

    /// The running autocorrect stack. Every method returns at once except
    /// [`shutdown`](Self::shutdown).
    pub struct EngineHost {
        tx: Sender<Msg>,
        thread: Option<JoinHandle<()>>,
    }

    impl EngineHost {
        /// Start the engine thread, which installs the hook if `settings.enabled`. Returns at
        /// once: building the dictionary index takes a moment. `on_learned` runs on the engine
        /// thread for each word learned by undo, after the OS spell checker was told;
        /// `observer` for each fix, undo and health event. Every engine adds the words it
        /// checks to `words_checked`.
        pub fn start(
            settings: Settings,
            flags: RemoteFlags,
            words_checked: Arc<AtomicU64>,
            observer: Observer,
            on_learned: impl FnMut(String) + Send + 'static,
        ) -> Self {
            let (tx, rx) = mpsc::channel();
            let worker_tx = tx.clone();
            let thread = std::thread::Builder::new()
                .name("unflub-engine".into())
                .spawn(move || {
                    let state = State {
                        settings,
                        flags,
                        tx: worker_tx,
                        running: None,
                        pill: None,
                        words_checked,
                        observer,
                    };
                    run(state, rx, on_learned)
                })
                .map_err(|e| eprintln!("autocorrect: can't start the engine thread: {e}"))
                .ok();
            Self { tx, thread }
        }

        /// Apply new settings (already sanitised).
        pub fn apply(&self, settings: &Settings) {
            let _ = self.tx.send(Msg::Apply(Box::new(settings.clone())));
        }

        /// Apply new remote flags.
        pub fn set_flags(&self, flags: RemoteFlags) {
            let _ = self.tx.send(Msg::Flags(flags));
        }

        /// Unhook, close the pill and end the engine thread.
        pub fn shutdown(mut self) {
            self.stop();
        }

        fn stop(&mut self) {
            if let Some(thread) = self.thread.take() {
                let _ = self.tx.send(Msg::Shutdown);
                let _ = thread.join();
            }
        }
    }

    impl Drop for EngineHost {
        fn drop(&mut self) {
            self.stop();
        }
    }

    /// An installed hook and what talks to it.
    struct Running {
        hook: Hook,
        config: LiveConfig,
        learner: Option<SpellLearner>,
        /// The language the user asked for (the checker may have fallen back to another).
        language: String,
    }

    struct State {
        settings: Settings,
        flags: RemoteFlags,
        tx: Sender<Msg>,
        running: Option<Running>,
        pill: Option<(OverlayHandle, PillConfig)>,
        words_checked: Arc<AtomicU64>,
        observer: Observer,
    }

    fn run(mut state: State, rx: Receiver<Msg>, mut on_learned: impl FnMut(String)) {
        state.sync();
        while let Ok(msg) = rx.recv() {
            match msg {
                Msg::Apply(next) => {
                    state.settings = *next;
                    state.sync();
                }
                Msg::Flags(flags) => {
                    state.flags = flags;
                    state.sync_pill();
                }
                Msg::Hook(event) => state.on_hook_event(event),
                Msg::Learned(word) => {
                    if let Some(learner) = state.running.as_ref().and_then(|r| r.learner.as_ref()) {
                        learner.learn(&word);
                    }
                    on_learned(word);
                }
                Msg::Shutdown => break,
            }
        }
        state.stop();
    }

    impl State {
        /// Make the running stack match `self.settings`.
        fn sync(&mut self) {
            let s = &self.settings;
            let restart = self
                .running
                .as_ref()
                .is_some_and(|r| !s.enabled || r.language != s.language);
            if restart {
                self.stop();
            }
            if let Some(r) = &self.running {
                r.config.set(self.settings.to_core());
            } else if self.settings.enabled {
                self.running = start(&self.settings, &self.tx, &self.words_checked);
            }
            self.sync_pill();
        }

        /// Spawn, update or close the pill to match the settings and the OS look right now.
        fn sync_pill(&mut self) {
            let want = pill_config(&self.settings, &self.flags, &OsLook::read());
            if self.running.is_none() || want.style == PillStyle::Off {
                if let Some((overlay, _)) = self.pill.take() {
                    overlay.shutdown();
                }
                return;
            }
            match &mut self.pill {
                Some((overlay, current)) => {
                    if *current != want {
                        overlay.set_config(want.clone());
                        *current = want;
                    }
                }
                None => match Overlay::spawn(want.clone()) {
                    Ok(overlay) => self.pill = Some((overlay, want)),
                    Err(e) => eprintln!("autocorrect: correction pill unavailable ({e})"),
                },
            }
        }

        fn on_hook_event(&mut self, event: HookEvent) {
            if let Some(correction) = Correction::from_hook_event(&event) {
                // The OS may have switched dark mode or accent since the last pill.
                self.sync_pill();
                let pill = self.pill.is_some();
                if let Some((overlay, _)) = &self.pill {
                    overlay.show(correction);
                }
                match event {
                    HookEvent::Corrected {
                        replacement,
                        timing,
                        ..
                    } => (self.observer)(Observed::Corrected {
                        user_rule: replacement.source == Source::UserRule,
                        original: replacement.original,
                        corrected: replacement.corrected,
                        decide: timing.decide,
                        inject: timing.inject,
                        pill,
                    }),
                    HookEvent::Undone { replacement, .. } => (self.observer)(Observed::Undone {
                        user_rule: replacement.source == Source::UserRule,
                        original: replacement.original,
                        corrected: replacement.corrected,
                    }),
                    _ => {}
                }
                return;
            }
            match event {
                HookEvent::Warning(w) => eprintln!("autocorrect: {w}"),
                HookEvent::HooksReinstalled { reason } => {
                    eprintln!("autocorrect: hooks installed again ({reason})");
                    (self.observer)(Observed::HookReinstalled);
                }
                HookEvent::CorrectorPanicked(msg) => {
                    eprintln!("autocorrect: the engine panicked ({msg}); keys pass through");
                    (self.observer)(Observed::CorrectorPanicked);
                }
                HookEvent::InjectFailed { sent, total, .. } => {
                    eprintln!("autocorrect: SendInput sent {sent}/{total} events")
                }
                _ => {}
            }
        }

        /// Unhook and close the pill.
        fn stop(&mut self) {
            if let Some(running) = self.running.take() {
                running.hook.stop();
            }
            if let Some((overlay, _)) = self.pill.take() {
                overlay.shutdown();
            }
        }
    }

    /// Build the spell checker and engine, and install the hook. `None` if the hook can't be
    /// installed.
    fn start(s: &Settings, tx: &Sender<Msg>, words_checked: &Arc<AtomicU64>) -> Option<Running> {
        let backend = spell_backend(&s.language);
        let learner = backend.as_ref().map(WindowsSpellBackend::learner);
        let mut engine = Engine::new(
            s.to_core(),
            backend.map(|b| Box::new(b) as Box<dyn SpellBackend>),
        );
        engine.count_words_into(words_checked.clone());
        let learned = tx.clone();
        let (corrector, config) = LiveEngine::new(engine, move |word| {
            let _ = learned.send(Msg::Learned(word));
        });
        // Excluded apps are in the engine's Config, which changes live; see the module docs. The
        // hook's own list holds only unflub itself: words typed in Settings (your dictionary,
        // app names) are never "fixed", and the tour's practice box has its own engine.
        let options = HookOptions {
            excluded_apps: own_exe().into_iter().collect(),
            ..HookOptions::default()
        };
        let (hook, events) = match Hook::start(corrector, options) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("autocorrect: can't install the keyboard hook: {e}");
                return None;
            }
        };
        // The hook's channel ends when the hook stops, which ends this thread.
        let forward = tx.clone();
        let forwarder = std::thread::Builder::new()
            .name("unflub-events".into())
            .spawn(move || {
                for event in events {
                    if forward.send(Msg::Hook(event)).is_err() {
                        break;
                    }
                }
            });
        if let Err(e) = forwarder {
            // Nobody would read the hook's events; don't run without them.
            eprintln!("autocorrect: can't start the event thread: {e}");
            hook.stop();
            return None;
        }
        Some(Running {
            hook,
            config,
            learner,
            language: s.language.clone(),
        })
    }

    /// unflub's own executable name, lowercased, as the hook names the focused app.
    fn own_exe() -> Option<String> {
        let exe = std::env::current_exe().ok()?;
        Some(exe.file_name()?.to_str()?.to_lowercase())
    }

    /// The Windows spell checker for `language`, or for the user's UI language if Windows has
    /// no dictionary for it. Called on the engine thread, which may join the COM MTA (the
    /// Tauri main thread is an STA and can't).
    fn spell_backend(language: &str) -> Option<WindowsSpellBackend> {
        WindowsSpellBackend::new(Some(language))
            .or_else(|e| {
                eprintln!("autocorrect: no {language} spell checker ({e}); using the default");
                WindowsSpellBackend::new(None)
            })
            .map_err(|e| eprintln!("autocorrect: Windows spell checker unavailable ({e})"))
            .ok()
    }
}

#[cfg(not(windows))]
mod host {
    use super::Observer;
    use crate::remote::RemoteFlags;
    use crate::settings::Settings;
    use std::sync::atomic::AtomicU64;
    use std::sync::Arc;

    /// No adapter on this OS yet: settings are stored, nothing is hooked.
    pub struct EngineHost;

    impl EngineHost {
        pub fn start(
            _settings: Settings,
            _flags: RemoteFlags,
            _words_checked: Arc<AtomicU64>,
            _observer: Observer,
            _on_learned: impl FnMut(String) + Send + 'static,
        ) -> Self {
            Self
        }

        pub fn apply(&self, _settings: &Settings) {}

        pub fn set_flags(&self, _flags: RemoteFlags) {}

        pub fn shutdown(self) {}
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
        let s = Settings {
            pill_style: settings::PillStyle::Capsule,
            pill_position: settings::PillPosition::Below,
            pill_opacity: 0.5,
            pill_duration_ms: 2000,
            theme: Theme::Paper,
            appearance: Appearance::Auto,
            ..Settings::default()
        };
        let on = RemoteFlags::default();
        let p = pill_config(&s, &on, &look(true, None));
        assert_eq!(p.style, PillStyle::Capsule);
        assert_eq!(p.position, PillPosition::Below);
        assert_eq!(p.opacity, 0.5);
        assert_eq!(p.duration_ms, 2000);
        assert_eq!(p.background, rgb(0xE7E2DA), "paper, dark from the OS");
        assert_eq!(
            pill_config(&s, &on, &look(false, None)).background,
            rgb(0x292827)
        );

        // Defaults match the overlay's own defaults (the unflub light pill).
        let d = pill_config(&Settings::default(), &on, &look(false, None));
        let o = PillConfig::default();
        assert_eq!(
            (
                d.style,
                d.position,
                d.background,
                d.text,
                d.keycap,
                d.accent
            ),
            (
                o.style,
                o.position,
                o.background,
                o.text,
                o.keycap,
                o.accent
            )
        );
        for (from, to) in [
            (
                settings::PillStyle::UnderlineCapsule,
                PillStyle::UnderlineCapsule,
            ),
            (settings::PillStyle::Underline, PillStyle::Underline),
            (settings::PillStyle::Off, PillStyle::Off),
        ] {
            let s = Settings {
                pill_style: from,
                ..Settings::default()
            };
            assert_eq!(pill_config(&s, &on, &OsLook::default()).style, to);
        }
        assert!(d.uia_caret);
    }

    #[test]
    fn remote_flags_turn_the_pill_and_uia_off() {
        let flags = RemoteFlags {
            pill_enabled: false,
            uia_caret: false,
            ..RemoteFlags::default()
        };
        let p = pill_config(&Settings::default(), &flags, &OsLook::default());
        assert_eq!(p.style, PillStyle::Off);
        assert!(!p.uia_caret);
    }
}
