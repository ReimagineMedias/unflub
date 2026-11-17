//! Windows adapter: low-level keyboard hook, `SendInput` injection, `ISpellChecker` backend.
//!
//! Threads in a running adapter:
//!
//! | Thread | COM | Does |
//! |---|---|---|
//! | `unflub-hook` | MTA | `WH_KEYBOARD_LL`, `WH_MOUSE_LL`, focus WinEvents, calls the [`Corrector`], `SendInput` |
//! | `unflub-uia` | MTA | UI Automation `IsPassword` on focus change, with short UIA timeouts |
//! | `unflub-spell` | MTA | slow `ComprehensiveCheck` lookups that fill the auto-replace cache |
//! | `unflub-overlay` | none | the correction pill window ([`overlay`]): caret lookup, Direct2D drawing, fades |
//! | `unflub-caret` | MTA | UI Automation caret position for browser and XAML fields, for the pill |
//!
//! Settings change while the hook runs through [`LiveEngine`]: the hook thread picks up a new
//! config at the start of the next key event, without blocking.
//!
//! The pure parts ([`translate`], [`policy`], [`inject::plan`], [`stats`], [`live`], parts of
//! [`focus`], [`overlay`] and [`caret`]) compile and are tested on every OS. Everything that
//! calls Windows is behind `cfg(windows)`, so on Linux and macOS this crate builds to just
//! those helpers.
//!
//! Requirements and limitations:
//! - Windows 10 1607 or later: key translation relies on `ToUnicodeEx`'s "don't change
//!   keyboard state" flag so dead keys keep working in the user's apps.
//! - Keys typed into elevated (administrator) windows are invisible to a non-elevated hook,
//!   and injection into them is blocked by UIPI. unflub does nothing there.

/// Re-exported so callers can name the hook's trait bound without depending on core directly.
pub use unflub_core::Corrector;

pub mod caret;
pub mod focus;
pub mod inject;
pub mod live;
pub mod overlay;
pub mod policy;
pub mod stats;
pub mod translate;

#[cfg(windows)]
pub mod hook;
#[cfg(windows)]
pub mod spell;

#[cfg(windows)]
pub use hook::{Hook, HookEvent, HookOptions, HookReport, InjectTiming};
pub use live::{LiveConfig, LiveEngine};
pub use overlay::{Correction, PillConfig, PillPosition, PillStyle};
#[cfg(windows)]
pub use overlay::{Overlay, OverlayHandle};
#[cfg(windows)]
pub use spell::{SpellLearner, WindowsSpellBackend};
