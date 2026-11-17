//! Windows adapter: low-level keyboard hook, SendInput injection, ISpellChecker backend.
//! Implemented in the Windows adapter PR. Compiles to nothing on other platforms.

#[cfg(windows)]
pub fn placeholder() {}
