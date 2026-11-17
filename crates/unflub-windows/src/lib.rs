//! Windows adapter: low-level keyboard hook, `SendInput` injection, `ISpellChecker` backend.
//!
//! Threads in a running adapter:
//!
//! | Thread | COM | Does |
//! |---|---|---|
//! | `unflub-hook` | MTA | `WH_KEYBOARD_LL`, `WH_MOUSE_LL`, focus WinEvents, calls the [`Corrector`], `SendInput` |
//! | `unflub-uia` | MTA | UI Automation `IsPassword` on focus change, with short UIA timeouts |
//! | `unflub-spell` | MTA | slow `ComprehensiveCheck` lookups that fill the auto-replace cache |
//!
//! The pure parts ([`translate`], [`inject::plan`], [`stats`], parts of [`focus`]) compile and
//! are tested on every OS. Everything that calls Windows is behind `cfg(windows)`, so on
//! Linux and macOS this crate builds to just those helpers.
//!
//! Requirements and limitations:
//! - Windows 10 1607 or later: key translation relies on `ToUnicodeEx`'s "don't change
//!   keyboard state" flag so dead keys keep working in the user's apps.
//! - Keys typed into elevated (administrator) windows are invisible to a non-elevated hook,
//!   and injection into them is blocked by UIPI. unflub does nothing there.

use unflub_core::{Action, Context, Engine, Key};

pub mod focus;
pub mod inject;
pub mod stats;
pub mod translate;

#[cfg(windows)]
pub mod hook;
#[cfg(windows)]
pub mod spell;

#[cfg(windows)]
pub use hook::{Hook, HookEvent, HookOptions, HookReport};
#[cfg(windows)]
pub use spell::WindowsSpellBackend;

/// Anything that turns keys into actions. The hook is generic over this so it can be driven by
/// the real [`Engine`] or by a test double.
pub trait Corrector {
    fn on_key(&mut self, key: Key, ctx: &Context<'_>) -> Action;
}

impl Corrector for Engine {
    fn on_key(&mut self, key: Key, ctx: &Context<'_>) -> Action {
        Engine::on_key(self, key, ctx)
    }
}
