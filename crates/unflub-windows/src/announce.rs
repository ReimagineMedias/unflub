//! Screen reader announcements: what the pill shows, read out loud (#79).
//!
//! A fix, an "Undo?" prompt (a first Backspace held) and an undo each raise a UI Automation
//! notification (`UiaRaiseNotificationEvent`, Windows 10 1709 and later), which Narrator, NVDA
//! and JAWS speak. The text names only the word pair the user just typed and its fix, plus how
//! to undo it.
//!
//! The notification is raised on the host provider of the focused window
//! (`UiaHostProviderFromHwnd`), so screen readers see it come from the app being typed in. NVDA
//! drops notifications from background apps (`event_UIA_notification` in
//! `NVDAObjects/UIA/__init__.py`: "Do not announce notifications from background apps"), and a
//! window of our own is never in the foreground.
//!
//! Gating ([`should_announce`]): the user's setting, a screen reader running
//! (`SPI_GETSCREENREADER`, which Narrator, NVDA and JAWS set) and a field that may be read
//! aloud. Never for terminals or unknown apps, where a "word" may be a password typed at a
//! prompt ([`policy::keep_words`](crate::policy::keep_words)), and never when focus is in a
//! password edit by the time the announcement goes out. The hook already never fixes words in
//! secure fields.
//!
//! [`Announcer`] runs on its own `unflub-announce` thread. [`AnnouncerHandle::say`] only pushes
//! onto a channel, and the hook thread never calls it (the runner forwards the
//! [`HookEvent`](crate::HookEvent)s it already receives), so the hook never waits on a screen
//! reader.
//!
//! The pure parts (message text, gating) build and are tested on every OS.

use unflub_core::UndoMode;

use crate::overlay::{Correction, CorrectionKind};

/// What a screen reader says for a correction, following the undo setting.
pub fn message(c: &Correction, undo: UndoMode) -> String {
    match c.kind {
        CorrectionKind::Fixed => {
            let fixed = format!("Fixed {} to {}.", c.original, c.corrected);
            match (c.undoable, undo) {
                (true, UndoMode::Twice) => format!("{fixed} Backspace twice to undo."),
                (true, UndoMode::Once) => format!("{fixed} Backspace to undo."),
                _ => fixed,
            }
        }
        CorrectionKind::UndoPending => "Undo? Backspace again to undo.".into(),
        CorrectionKind::Undone => format!("Undone. Back to {}.", c.original),
    }
}

/// May an announcement go out? `enabled`: the user's setting. `screen_reader`: one is running.
/// `private`: the words may be a password (a password field, a terminal, an unknown app).
pub fn should_announce(enabled: bool, screen_reader: bool, private: bool) -> bool {
    enabled && screen_reader && !private
}

#[cfg(windows)]
pub use win::{screen_reader_running, Announcer, AnnouncerHandle};

#[cfg(windows)]
mod win {
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::thread::JoinHandle;

    use windows::core::BSTR;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
    use windows::Win32::UI::Accessibility::{
        NotificationKind_ActionCompleted, NotificationProcessing_ImportantMostRecent,
        UiaClientsAreListening, UiaHostProviderFromHwnd, UiaRaiseNotificationEvent,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETSCREENREADER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };

    use crate::focus;

    const THREAD_NAME: &str = "unflub-announce";
    /// One activity for every announcement, so a newer one replaces an older one still queued
    /// ("Undone" over "Fixed").
    const ACTIVITY: &str = "unflub-correction";

    /// Is a screen reader running? Narrator, NVDA and JAWS set this system flag while they run.
    pub fn screen_reader_running() -> bool {
        let mut on = windows::Win32::Foundation::BOOL(0);
        // SAFETY: SPI_GETSCREENREADER writes one BOOL to the pointer.
        let ok = unsafe {
            SystemParametersInfoW(
                SPI_GETSCREENREADER,
                0,
                Some(&mut on as *mut _ as *mut core::ffi::c_void),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        };
        ok.is_ok() && on.as_bool()
    }

    /// Starts the announcer thread.
    pub struct Announcer;

    impl Announcer {
        pub fn spawn() -> std::io::Result<AnnouncerHandle> {
            let (tx, rx) = mpsc::channel();
            let thread = std::thread::Builder::new()
                .name(THREAD_NAME.into())
                .spawn(move || run(rx))?;
            Ok(AnnouncerHandle {
                texts: Some(tx),
                thread: Some(thread),
            })
        }
    }

    /// Talks to the announcer thread. [`AnnouncerHandle::say`] returns immediately. Dropping it
    /// ends the thread after any announcement in progress.
    pub struct AnnouncerHandle {
        texts: Option<Sender<String>>,
        thread: Option<JoinHandle<()>>,
    }

    impl AnnouncerHandle {
        /// Announce `text`, gated already ([`super::should_announce`]). Replaces one still
        /// waiting.
        pub fn say(&self, text: String) {
            if let Some(tx) = &self.texts {
                let _ = tx.send(text);
            }
        }
    }

    impl Drop for AnnouncerHandle {
        fn drop(&mut self) {
            self.texts.take();
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn run(texts: Receiver<String>) {
        // SAFETY: balanced by CoUninitialize below; this thread never enters an STA.
        if unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_err() {
            return;
        }
        while let Ok(mut text) = texts.recv() {
            // Only the latest matters.
            while let Ok(t) = texts.try_recv() {
                text = t;
            }
            raise(&text);
        }
        // SAFETY: matches the successful CoInitializeEx on this thread.
        unsafe { CoUninitialize() };
    }

    /// Raise the notification on the focused window, unless nobody listens or focus moved into
    /// a password edit since the fix.
    fn raise(text: &str) {
        // SAFETY: no arguments; reports whether any UIA client registered for events.
        if !unsafe { UiaClientsAreListening() }.as_bool() {
            return;
        }
        let (foreground, _) = focus::foreground();
        if foreground.is_invalid() {
            return;
        }
        let target = focus::focus_window(foreground);
        if focus::inspect_window(target).1 {
            return;
        }
        // SAFETY: the host provider of a window handle; UIA validates the handle. The provider
        // is released when it drops.
        let _ = unsafe {
            UiaHostProviderFromHwnd(target).and_then(|provider| {
                UiaRaiseNotificationEvent(
                    &provider,
                    NotificationKind_ActionCompleted,
                    NotificationProcessing_ImportantMostRecent,
                    &BSTR::from(text),
                    &BSTR::from(ACTIVITY),
                )
            })
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unflub_core::Replacement;

    fn fix(insert: &str, original: &str, corrected: &str) -> Replacement {
        Replacement {
            delete: original.chars().count(),
            insert: insert.into(),
            original: original.into(),
            corrected: corrected.into(),
            source: unflub_core::Source::Bundled,
        }
    }

    #[test]
    fn a_fix_names_the_pair_and_the_undo_key() {
        let c = Correction::fixed(&fix("the ", "teh", "the"));
        assert_eq!(
            message(&c, UndoMode::Twice),
            "Fixed teh to the. Backspace twice to undo."
        );
        assert_eq!(
            message(&c, UndoMode::Once),
            "Fixed teh to the. Backspace to undo."
        );
        assert_eq!(message(&c, UndoMode::Off), "Fixed teh to the.");
    }

    #[test]
    fn no_undo_hint_after_enter_or_tab() {
        for insert in ["the\n", "the\t"] {
            let c = Correction::fixed(&fix(insert, "teh", "the"));
            assert_eq!(message(&c, UndoMode::Twice), "Fixed teh to the.");
        }
    }

    #[test]
    fn undo_prompt_and_undone() {
        let r = fix("the ", "teh", "the");
        assert_eq!(
            message(&Correction::undo_pending(&r), UndoMode::Twice),
            "Undo? Backspace again to undo."
        );
        let undone = Correction::undone(&fix("teh ", "teh", "the"));
        assert_eq!(message(&undone, UndoMode::Twice), "Undone. Back to teh.");
        assert_eq!(message(&undone, UndoMode::Once), "Undone. Back to teh.");
    }

    /// Only the word pair is read: never the boundary or anything typed around it.
    #[test]
    fn says_nothing_but_the_pair() {
        let c = Correction::fixed(&fix("I'm, ", "im", "I'm"));
        let text = message(&c, UndoMode::Twice);
        assert_eq!(text, "Fixed im to I'm. Backspace twice to undo.");
        assert!(!text.contains(','));
        assert!(!message(
            &Correction::undo_pending(&fix("the ", "teh", "the")),
            UndoMode::Twice
        )
        .contains("teh"));
    }

    /// No em or en dashes in anything read out.
    #[test]
    fn plain_punctuation() {
        let r = fix("the ", "teh", "the");
        for c in [
            Correction::fixed(&r),
            Correction::undo_pending(&r),
            Correction::undone(&r),
        ] {
            for mode in [UndoMode::Twice, UndoMode::Once, UndoMode::Off] {
                let text = message(&c, mode);
                assert!(!text.contains(['\u{2014}', '\u{2013}']), "{text}");
            }
        }
    }

    #[test]
    fn gating() {
        assert!(should_announce(true, true, false));
        assert!(!should_announce(true, false, false), "no screen reader");
        assert!(!should_announce(false, true, false), "turned off");
        assert!(!should_announce(true, true, true), "password or terminal");
        assert!(!should_announce(false, false, true));
    }

    #[cfg(windows)]
    #[test]
    fn announcer_starts_and_stops_without_speaking() {
        let announcer = Announcer::spawn().expect("announcer thread");
        drop(announcer);
        // Reading the flag never fails the caller.
        let _ = screen_reader_running();
    }
}
