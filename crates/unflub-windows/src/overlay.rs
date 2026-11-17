//! The correction pill: a tiny overlay at the caret for about a second after a fix or an undo.
//!
//! The capsule shows the original word struck through, `→ corrected` and a `⌫ undo` keycap; the
//! underline style draws a short accent bar under the corrected word. Both fade and rise in,
//! hold, and fade out.
//!
//! It runs on its own `unflub-overlay` thread with its own message loop. [`OverlayHandle::show`]
//! only pushes onto a channel and posts a message, so it never blocks; the hook thread doesn't
//! call it at all (the runner forwards [`HookEvent`](crate::HookEvent)s it already receives).
//!
//! The window is a `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW |
//! WS_EX_TOPMOST` popup: it never takes focus, clicks go through it, and it is not in Alt+Tab
//! or the taskbar. Each pill is drawn once with Direct2D (software, so no GPU device stays
//! resident) and DirectWrite into a premultiplied BGRA DIB, handed to `UpdateLayeredWindow`,
//! and the DIB is freed straight away. Animation frames only move the window and change its
//! constant alpha. While hidden there is no timer, no bitmap and no Direct2D object.
//!
//! The pure parts (config, layout, animation curve) build and are tested on every OS.

use std::fmt;
use std::str::FromStr;

use unflub_core::{Event, Replacement};

use crate::caret::CaretSpot;

/// Which parts of the pill to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PillStyle {
    /// A short accent underline under the fixed word plus the capsule. Default.
    #[default]
    UnderlineCapsule,
    /// The capsule only.
    Capsule,
    /// The underline only.
    Underline,
    /// Nothing.
    Off,
}

impl PillStyle {
    pub const ALL: [PillStyle; 4] = [
        PillStyle::UnderlineCapsule,
        PillStyle::Capsule,
        PillStyle::Underline,
        PillStyle::Off,
    ];

    pub fn name(self) -> &'static str {
        match self {
            PillStyle::UnderlineCapsule => "underline-capsule",
            PillStyle::Capsule => "capsule",
            PillStyle::Underline => "underline",
            PillStyle::Off => "off",
        }
    }

    pub fn has_capsule(self) -> bool {
        matches!(self, PillStyle::UnderlineCapsule | PillStyle::Capsule)
    }

    pub fn has_underline(self) -> bool {
        matches!(self, PillStyle::UnderlineCapsule | PillStyle::Underline)
    }
}

impl fmt::Display for PillStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for PillStyle {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.to_ascii_lowercase();
        PillStyle::ALL
            .into_iter()
            .find(|p| p.name() == s)
            .ok_or_else(|| {
                format!("unknown pill style {s:?} (underline-capsule, capsule, underline, off)")
            })
    }
}

/// Where the capsule sits relative to the caret's line. It flips when it would leave the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PillPosition {
    #[default]
    Above,
    Below,
}

/// A colour with straight (not premultiplied) alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    /// Opaque colour from `0xRRGGBB`.
    pub const fn hex(rgb: u32) -> Self {
        Self {
            r: (rgb >> 16) as u8,
            g: (rgb >> 8) as u8,
            b: rgb as u8,
            a: 255,
        }
    }
}

/// Pill settings. Plain data; apply changes with [`OverlayHandle::set_config`].
#[derive(Debug, Clone, PartialEq)]
pub struct PillConfig {
    pub style: PillStyle,
    pub position: PillPosition,
    /// Overall opacity, 0.3..=1.0.
    pub opacity: f32,
    /// Time on screen including the fades.
    pub duration_ms: u32,
    pub background: Rgba,
    pub text: Rgba,
    pub keycap: Rgba,
    /// The underline.
    pub accent: Rgba,
    /// The "Undo?" pill while a first Backspace is held: its background and underline.
    pub alert: Rgba,
    /// Text on [`PillConfig::alert`], picked for contrast.
    pub alert_text: Rgba,
    /// Ask UI Automation for the caret in browser and XAML fields. Off: the pill shows only
    /// where the system caret is known.
    pub uia_caret: bool,
}

pub const MIN_OPACITY: f32 = 0.3;
pub const DEFAULT_DURATION_MS: u32 = 1100;
pub const MAX_DURATION_MS: u32 = 10_000;
pub const FADE_IN_MS: u32 = 120;
pub const FADE_OUT_MS: u32 = 160;
/// Shortest allowed duration: both fades plus a short hold.
pub const MIN_DURATION_MS: u32 = FADE_IN_MS + FADE_OUT_MS + 100;
/// Frame interval during fades (`SetTimer` rounds to the ~15.6 ms system tick).
pub const FRAME_MS: u32 = 16;

impl Default for PillConfig {
    fn default() -> Self {
        Self {
            style: PillStyle::default(),
            position: PillPosition::default(),
            opacity: 1.0,
            duration_ms: DEFAULT_DURATION_MS,
            background: Rgba::hex(0x2D1916),
            text: Rgba::hex(0xF2E6E3),
            keycap: Rgba::hex(0xE0A595),
            accent: Rgba::hex(0xB06A5C),
            alert: Rgba::hex(0xC0392B),
            alert_text: Rgba::hex(0xFFFFFF),
            uia_caret: true,
        }
    }
}

/// The colours one pill is drawn with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrawColours {
    pub background: Rgba,
    pub text: Rgba,
    pub keycap: Rgba,
    pub accent: Rgba,
}

impl PillConfig {
    /// The theme's colours, or the alert colours while an undo is pending.
    pub fn colours_for(&self, correction: &Correction) -> DrawColours {
        match correction.kind {
            CorrectionKind::UndoPending => DrawColours {
                background: self.alert,
                text: self.alert_text,
                keycap: self.alert_text,
                accent: self.alert,
            },
            _ => DrawColours {
                background: self.background,
                text: self.text,
                keycap: self.keycap,
                accent: self.accent,
            },
        }
    }

    /// Bring every number into its allowed range.
    pub fn clamped(mut self) -> Self {
        self.opacity = if self.opacity.is_nan() {
            1.0
        } else {
            self.opacity.clamp(MIN_OPACITY, 1.0)
        };
        self.duration_ms = self.duration_ms.clamp(MIN_DURATION_MS, MAX_DURATION_MS);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorrectionKind {
    /// A word was replaced. The pill offers Backspace to undo.
    Fixed,
    /// A first Backspace is held: one more undoes the fix. Drawn in the alert colour.
    UndoPending,
    /// The user undid a fix. The pill confirms it.
    Undone,
}

/// What the pill shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Correction {
    pub kind: CorrectionKind,
    /// As the user typed it.
    pub original: String,
    /// As the engine fixed it.
    pub corrected: String,
    /// Characters typed after the word in this action (the boundary), so the caret sits that far
    /// past the word.
    pub trailing: usize,
    /// Backspace can undo it: the pill shows the hint. False after Enter or Tab (see
    /// [`unflub_core::undoable_boundary`]) and for an undo.
    pub undoable: bool,
}

impl Correction {
    pub fn fixed(r: &Replacement) -> Self {
        let trailing = r
            .insert
            .chars()
            .count()
            .saturating_sub(r.corrected.chars().count());
        Self {
            kind: CorrectionKind::Fixed,
            original: r.original.clone(),
            corrected: r.corrected.clone(),
            trailing,
            undoable: r
                .insert
                .chars()
                .last()
                .is_none_or(unflub_core::undoable_boundary),
        }
    }

    pub fn undone(r: &Replacement) -> Self {
        let trailing = r
            .insert
            .chars()
            .count()
            .saturating_sub(r.original.chars().count());
        Self {
            kind: CorrectionKind::Undone,
            original: r.original.clone(),
            corrected: r.corrected.clone(),
            trailing,
            undoable: false,
        }
    }

    /// A first Backspace is held after `r` (the fix): the pill asks "Undo?".
    pub fn undo_pending(r: &Replacement) -> Self {
        Self {
            kind: CorrectionKind::UndoPending,
            undoable: false,
            ..Self::fixed(r)
        }
    }

    /// From an engine event ([`Engine::drain_events`](unflub_core::Engine::drain_events)).
    pub fn from_event(event: &Event) -> Option<Self> {
        match event {
            Event::Corrected(r) => Some(Self::fixed(r)),
            Event::Undone(r) => Some(Self::undone(r)),
            Event::Learned(_) => None,
        }
    }

    /// From what the hook applied.
    #[cfg(windows)]
    pub fn from_hook_event(event: &crate::HookEvent) -> Option<Self> {
        match event {
            crate::HookEvent::Corrected { replacement, .. } => Some(Self::fixed(replacement)),
            crate::HookEvent::Undone { replacement, .. } => Some(Self::undone(replacement)),
            crate::HookEvent::UndoPending { replacement } => Some(Self::undo_pending(replacement)),
            _ => None,
        }
    }

    /// The word now in the field, which the underline marks.
    pub fn word(&self) -> &str {
        match self.kind {
            CorrectionKind::Fixed | CorrectionKind::UndoPending => &self.corrected,
            CorrectionKind::Undone => &self.original,
        }
    }

    /// Struck through in the capsule.
    pub fn before(&self) -> &str {
        match self.kind {
            CorrectionKind::Fixed | CorrectionKind::UndoPending => &self.original,
            CorrectionKind::Undone => &self.corrected,
        }
    }

    /// The keycap (or, after an undo, plain) label at the end of the capsule. Empty for a fix
    /// that can't be undone.
    pub fn label(&self) -> &'static str {
        match self.kind {
            CorrectionKind::Fixed if self.undoable => "\u{232B} undo",
            CorrectionKind::Fixed => "",
            CorrectionKind::UndoPending => "Undo?",
            CorrectionKind::Undone => "Undone",
        }
    }

    /// The label is drawn as a keycap (the undo hint).
    pub fn keycap(&self) -> bool {
        self.kind == CorrectionKind::Fixed && self.undoable
    }
}

/// Sizes in DIPs (1/96 inch). Multiply by `dpi / 96` for pixels.
pub mod dip {
    pub const PAD_X: f32 = 10.0;
    pub const PAD_Y: f32 = 6.0;
    /// Between the struck word, the arrow and the corrected word.
    pub const GAP: f32 = 5.0;
    /// Before the keycap.
    pub const KEY_GAP: f32 = 8.0;
    pub const KEY_PAD_X: f32 = 5.0;
    pub const KEY_PAD_Y: f32 = 1.0;
    pub const KEY_RADIUS: f32 = 4.0;
    pub const TEXT_SIZE: f32 = 12.0;
    pub const KEY_TEXT_SIZE: f32 = 11.0;
    /// Room around the capsule for its soft shadow.
    pub const SHADOW: f32 = 4.0;
    /// Shadow drop.
    pub const SHADOW_DY: f32 = 1.0;
    /// Between the caret's line and the capsule.
    pub const CARET_GAP: f32 = 4.0;
    pub const UNDERLINE: f32 = 2.0;
    /// Between the caret's line bottom and the underline.
    pub const UNDERLINE_GAP: f32 = 1.0;
    /// Shortest underline.
    pub const UNDERLINE_MIN: f32 = 6.0;
    /// How far the pill rises while fading in.
    pub const RISE: f32 = 2.0;
}

/// Measured text, in DIPs.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TextMetrics {
    pub before: f32,
    pub arrow: f32,
    pub word: f32,
    pub label: f32,
    /// Line height of the 12 px text.
    pub line: f32,
    /// Line height of the keycap text.
    pub label_line: f32,
}

/// Axis-aligned box. Units depend on use (DIPs in [`CapsuleLayout`], pixels in [`Placement`]).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BoxF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl BoxF {
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn scaled(&self, k: f32) -> BoxF {
        BoxF {
            x: self.x * k,
            y: self.y * k,
            w: self.w * k,
            h: self.h * k,
        }
    }
}

/// Where each piece of the capsule goes, in DIPs from the capsule's top-left.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CapsuleLayout {
    pub width: f32,
    pub height: f32,
    pub before_x: f32,
    pub arrow_x: f32,
    pub word_x: f32,
    pub text_y: f32,
    /// The keycap's background (only drawn for a fix).
    pub key: BoxF,
    pub label_x: f32,
    pub label_y: f32,
}

/// Lay the capsule out from text metrics. `keycap`: draw the label in a keycap box.
pub fn capsule_layout(m: &TextMetrics, keycap: bool) -> CapsuleLayout {
    use dip::*;
    let (key_pad_x, key_pad_y) = if keycap {
        (KEY_PAD_X, KEY_PAD_Y)
    } else {
        (0.0, 0.0)
    };
    let key_h = m.label_line + 2.0 * key_pad_y;
    let inner_h = m.line.max(key_h);
    let height = inner_h + 2.0 * PAD_Y;
    let before_x = PAD_X;
    let arrow_x = before_x + m.before + GAP;
    let word_x = arrow_x + m.arrow + GAP;
    let key_x = word_x + m.word + KEY_GAP;
    let key = BoxF {
        x: key_x,
        y: (height - key_h) / 2.0,
        w: m.label + 2.0 * key_pad_x,
        h: key_h,
    };
    CapsuleLayout {
        width: key.right() + PAD_X,
        height,
        before_x,
        arrow_x,
        word_x,
        text_y: (height - m.line) / 2.0,
        key,
        label_x: key_x + key_pad_x,
        label_y: (height - m.label_line) / 2.0,
    }
}

/// A rectangle in physical screen pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

/// Where the overlay window goes and what it holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// The window at rest (fully risen), physical screen pixels.
    pub window: Rect,
    /// Capsule, physical pixels from the window's top-left.
    pub capsule: Option<BoxF>,
    /// Underline, physical pixels from the window's top-left.
    pub underline: Option<BoxF>,
    /// The capsule ended up below the line (asked for, or flipped).
    pub below: bool,
}

/// Position the pill for a caret line `line_top..line_bottom` and the fixed word's horizontal
/// span `word`, inside the monitor work area `work`. `capsule` is the capsule's size in DIPs;
/// `scale` is the monitor DPI / 96. `None` when the style shows nothing.
#[allow(clippy::too_many_arguments)]
pub fn place(
    style: PillStyle,
    position: PillPosition,
    line_top: i32,
    line_bottom: i32,
    word: (i32, i32),
    capsule: (f32, f32),
    scale: f32,
    work: Rect,
) -> Option<Placement> {
    use dip::*;
    if style == PillStyle::Off {
        return None;
    }
    let s = scale.max(0.5);
    let px = |d: f32| (d * s).round();
    let (wl, wr) = (word.0.min(word.1) as f32, word.0.max(word.1) as f32);

    let underline = style.has_underline().then(|| {
        let min_w = px(UNDERLINE_MIN);
        let x0 = wl.clamp(
            work.left as f32,
            (work.right as f32 - min_w).max(work.left as f32),
        );
        let x1 = wr.min(work.right as f32).max(x0 + min_w);
        BoxF {
            x: x0,
            y: line_bottom as f32 + px(UNDERLINE_GAP),
            w: x1 - x0,
            h: px(UNDERLINE).max(1.0),
        }
    });

    let mut below = false;
    let capsule = style.has_capsule().then(|| {
        let (w, h) = ((capsule.0 * s).ceil(), (capsule.1 * s).ceil());
        let centre = (wl + wr) / 2.0;
        let max_x = (work.right as f32 - w).max(work.left as f32);
        let x = (centre - w / 2.0).round().clamp(work.left as f32, max_x);
        let under = underline.map_or(0.0, |u| u.bottom() - line_bottom as f32);
        let above_y = line_top as f32 - px(CARET_GAP) - h;
        let below_y = line_bottom as f32 + under + px(CARET_GAP);
        let above_fits = above_y >= work.top as f32;
        let below_fits = below_y + h <= work.bottom as f32;
        below = match position {
            PillPosition::Above => !above_fits && below_fits,
            PillPosition::Below => below_fits || !above_fits,
        };
        let y = if below {
            below_y.min(work.bottom as f32 - h)
        } else {
            above_y.max(work.top as f32)
        };
        BoxF { x, y, w, h }
    });

    // Union of the capsule with its shadow and the underline.
    let mut bounds: Option<(f32, f32, f32, f32)> = None;
    let mut add = |l: f32, t: f32, r: f32, b: f32| {
        bounds = Some(match bounds {
            None => (l, t, r, b),
            Some((l0, t0, r0, b0)) => (l0.min(l), t0.min(t), r0.max(r), b0.max(b)),
        });
    };
    if let Some(c) = capsule {
        let m = px(SHADOW);
        add(
            c.x - m,
            c.y - m,
            c.right() + m,
            c.bottom() + m + px(SHADOW_DY),
        );
    }
    if let Some(u) = underline {
        add(u.x, u.y, u.right(), u.bottom());
    }
    let (l, t, r, b) = bounds?;
    let window = Rect {
        left: l.floor() as i32,
        top: t.floor() as i32,
        right: r.ceil() as i32,
        bottom: b.ceil() as i32,
    };
    let local = |bx: BoxF| BoxF {
        x: bx.x - window.left as f32,
        y: bx.y - window.top as f32,
        ..bx
    };
    Some(Placement {
        window,
        capsule: capsule.map(local),
        underline: underline.map(local),
        below,
    })
}

/// One animation frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    /// 0..=1, before the configured opacity.
    pub alpha: f32,
    /// DIPs the pill still sits below its rest position.
    pub rise: f32,
}

/// The frame `elapsed_ms` after the pill appeared, or `None` once it is gone.
/// Fade and rise in over [`FADE_IN_MS`] (ease-out), hold, fade out over [`FADE_OUT_MS`].
pub fn frame_at(elapsed_ms: u32, duration_ms: u32) -> Option<Frame> {
    let total = duration_ms.max(MIN_DURATION_MS);
    let fade_out_at = total - FADE_OUT_MS;
    if elapsed_ms >= total {
        None
    } else if elapsed_ms < FADE_IN_MS {
        let t = elapsed_ms as f32 / FADE_IN_MS as f32;
        let k = 1.0 - (1.0 - t).powi(3);
        Some(Frame {
            alpha: k,
            rise: dip::RISE * (1.0 - k),
        })
    } else if elapsed_ms < fade_out_at {
        Some(Frame {
            alpha: 1.0,
            rise: 0.0,
        })
    } else {
        let t = (elapsed_ms - fade_out_at) as f32 / FADE_OUT_MS as f32;
        Some(Frame {
            alpha: 1.0 - t * t * (3.0 - 2.0 * t),
            rise: 0.0,
        })
    }
}

/// Milliseconds until the next frame worth drawing, or `None` once the pill is gone. One timer
/// tick covers the whole hold.
pub fn next_tick_ms(elapsed_ms: u32, duration_ms: u32) -> Option<u32> {
    let total = duration_ms.max(MIN_DURATION_MS);
    let fade_out_at = total - FADE_OUT_MS;
    if elapsed_ms >= total {
        None
    } else if elapsed_ms < FADE_IN_MS || elapsed_ms >= fade_out_at {
        Some(FRAME_MS.min(total - elapsed_ms))
    } else {
        Some(fade_out_at - elapsed_ms)
    }
}

/// Spot for a caret, used to place the pill: the line and the word's span.
pub fn anchor(spot: &CaretSpot, correction: &Correction) -> (i32, i32, (i32, i32)) {
    let word = spot.word_span(correction.word().chars().count(), correction.trailing);
    (spot.top, spot.bottom, word)
}

#[cfg(windows)]
pub use win::{render_preview, Overlay, OverlayHandle, Preview};

#[cfg(windows)]
mod win {
    use super::*;
    use crate::caret::{self, CaretProbe, CaretReply};
    use std::cell::RefCell;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use windows::core::{w, Result as WinResult, PCWSTR};
    use windows::Win32::Foundation::{
        GetLastError, BOOL, COLORREF, ERROR_CLASS_ALREADY_EXISTS, HANDLE, HWND, LPARAM, LRESULT,
        POINT, RECT, SIZE, WPARAM,
    };
    use windows::Win32::Graphics::Direct2D::Common::{
        D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_POINT_2F, D2D_RECT_F,
    };
    use windows::Win32::Graphics::Direct2D::{
        D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory, ID2D1RenderTarget,
        ID2D1SolidColorBrush, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
        D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES,
        D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
        D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
    };
    use windows::Win32::Graphics::DirectWrite::{
        DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
        DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
        DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
        DWRITE_TEXT_METRICS, DWRITE_TEXT_RANGE, DWRITE_WORD_WRAPPING_NO_WRAP,
    };
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, GetMonitorInfoW,
        MonitorFromPoint, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER,
        BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, MONITORINFO,
        MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::{
        GetDpiForMonitor, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        MDT_EFFECTIVE_DPI,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer,
        PostMessageW, PostQuitMessage, RegisterClassExW, SetTimer, SetWindowPos, ShowWindow,
        UpdateLayeredWindow, HTTRANSPARENT, HWND_TOPMOST, MA_NOACTIVATE, MSG, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE, ULW_ALPHA, WM_APP, WM_DESTROY,
        WM_DISPLAYCHANGE, WM_DPICHANGED, WM_MOUSEACTIVATE, WM_NCHITTEST, WM_TIMER, WNDCLASSEXW,
        WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
        WS_POPUP,
    };

    const THREAD_NAME: &str = "unflub-overlay";
    const CLASS_NAME: PCWSTR = w!("unflub-pill");
    /// Commands are waiting in the channel.
    const WM_APP_WAKE: u32 = WM_APP + 1;
    /// A caret reply is waiting from the UIA worker.
    const WM_APP_CARET: u32 = WM_APP + 2;
    const TIMER_LOCATE: usize = 1;
    const TIMER_ANIM: usize = 2;
    /// Give the app time to process the injected text before reading the caret.
    const LOCATE_DELAY_MS: u32 = 16;
    const LOCATE_RETRY_MS: u32 = 10;
    /// No caret within this long after the first try: skip the pill.
    const LOCATE_BUDGET: Duration = Duration::from_millis(64);

    const UI_FONTS: &[PCWSTR] = &[w!("Segoe UI Variable Text"), w!("Segoe UI")];
    const MONO_FONTS: &[PCWSTR] = &[w!("Cascadia Mono"), w!("Consolas")];

    enum Command {
        Show(Correction),
        Hide,
        Config(PillConfig),
        Shutdown,
    }

    /// Starts the overlay thread.
    pub struct Overlay;

    impl Overlay {
        /// Create the overlay thread and its hidden window. Returns once the window exists.
        pub fn spawn(config: PillConfig) -> std::io::Result<OverlayHandle> {
            let (tx, rx) = mpsc::channel();
            let (ready_tx, ready_rx) = mpsc::sync_channel(1);
            let config = config.clamped();
            let thread = std::thread::Builder::new()
                .name(THREAD_NAME.into())
                .spawn(move || run(config, rx, ready_tx))?;
            match ready_rx.recv() {
                Ok(Ok(hwnd)) => Ok(OverlayHandle {
                    commands: tx,
                    hwnd,
                    thread: Some(thread),
                }),
                Ok(Err(e)) => {
                    let _ = thread.join();
                    Err(std::io::Error::other(e))
                }
                Err(_) => {
                    let _ = thread.join();
                    Err(std::io::Error::other(
                        "overlay thread exited during startup",
                    ))
                }
            }
        }
    }

    /// Talks to the overlay thread. Every method returns immediately. Dropping it shuts the
    /// overlay down.
    pub struct OverlayHandle {
        commands: Sender<Command>,
        /// The overlay window, as an integer so the handle is `Send`.
        hwnd: isize,
        thread: Option<JoinHandle<()>>,
    }

    impl OverlayHandle {
        /// Show the pill for a correction. Replaces whatever is showing.
        pub fn show(&self, correction: Correction) {
            self.send(Command::Show(correction));
        }

        /// Hide whatever is showing (a held Backspace was sent on or dropped).
        pub fn hide(&self) {
            self.send(Command::Hide);
        }

        /// Apply new settings (from the next frame on).
        pub fn set_config(&self, config: PillConfig) {
            self.send(Command::Config(config.clamped()));
        }

        /// Hide the pill, destroy the window and end the thread.
        pub fn shutdown(mut self) {
            self.stop();
        }

        fn send(&self, command: Command) {
            if self.commands.send(command).is_ok() {
                post(self.hwnd, WM_APP_WAKE);
            }
        }

        fn stop(&mut self) {
            let Some(thread) = self.thread.take() else {
                return;
            };
            self.send(Command::Shutdown);
            let _ = thread.join();
        }
    }

    impl Drop for OverlayHandle {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn hwnd_of(raw: isize) -> HWND {
        HWND(raw as *mut core::ffi::c_void)
    }

    fn post(hwnd: isize, msg: u32) {
        // SAFETY: posting to a window; fails harmlessly if it is gone.
        let _ = unsafe { PostMessageW(hwnd_of(hwnd), msg, WPARAM(0), LPARAM(0)) };
    }

    thread_local! {
        static PILL: RefCell<Option<Pill>> = const { RefCell::new(None) };
    }

    /// Run `f` on the overlay state, unless it is already borrowed (a message sent to our window
    /// from inside one of our own calls), in which case the message is ignored.
    fn with_pill<R>(f: impl FnOnce(&mut Pill) -> R) -> Option<R> {
        PILL.try_with(|cell| cell.try_borrow_mut().ok()?.as_mut().map(f))
            .ok()
            .flatten()
    }

    fn run(
        config: PillConfig,
        commands: Receiver<Command>,
        ready: mpsc::SyncSender<Result<isize, String>>,
    ) {
        // SAFETY: thread-level awareness only: our window and coordinates are physical pixels
        // on every monitor, whatever the host process declares.
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        // SAFETY: creates a window owned by this thread; destroyed before the loop ends.
        let hwnd = match unsafe { create_window() } {
            Ok(h) => h,
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        };
        let raw = hwnd.0 as isize;
        let (caret_tx, caret_rx) = mpsc::channel();
        let probe = CaretProbe::start(Box::new(move |reply: CaretReply| {
            if caret_tx.send(reply).is_ok() {
                post(raw, WM_APP_CARET);
            }
        }))
        .ok();
        PILL.with(|p| {
            *p.borrow_mut() = Some(Pill {
                hwnd,
                config,
                commands,
                carets: caret_rx,
                probe,
                renderer: None,
                locating: None,
                visible: None,
                next_id: 0,
            })
        });
        let _ = ready.send(Ok(raw));
        let mut msg = MSG::default();
        // SAFETY: standard message loop for this thread's window.
        unsafe {
            while GetMessageW(&mut msg, HWND::default(), 0, 0).0 > 0 {
                DispatchMessageW(&msg);
            }
        }
        // Drops the renderer and the caret probe (its worker ends after any current query).
        PILL.with(|p| p.borrow_mut().take());
    }

    unsafe fn create_window() -> std::result::Result<HWND, String> {
        let instance = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e}"))?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            return Err(format!("RegisterClassExW: {:?}", GetLastError()));
        }
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            CLASS_NAME,
            w!("unflub"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            HWND::default(),
            None,
            instance,
            None,
        )
        .map_err(|e| format!("CreateWindowExW: {e}"))
    }

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            WM_APP_WAKE => {
                if with_pill(Pill::drain_commands) == Some(true) {
                    let _ = DestroyWindow(hwnd);
                }
                LRESULT(0)
            }
            WM_APP_CARET => {
                with_pill(Pill::drain_carets);
                LRESULT(0)
            }
            WM_TIMER => {
                match wparam.0 {
                    TIMER_LOCATE => with_pill(Pill::locate),
                    TIMER_ANIM => with_pill(Pill::animate),
                    _ => None,
                };
                LRESULT(0)
            }
            // Monitors moved or changed resolution: drop the pill rather than leave it adrift.
            WM_DISPLAYCHANGE => {
                with_pill(Pill::cancel);
                LRESULT(0)
            }
            // Each pill is rendered for its monitor's DPI; nothing to resize.
            WM_DPICHANGED => LRESULT(0),
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    struct Locating {
        id: u64,
        correction: Correction,
        /// Set at the first attempt.
        deadline: Option<Instant>,
        uia_asked: bool,
    }

    struct Visible {
        /// Window top-left at rest.
        origin: POINT,
        scale: f32,
        shown: Instant,
    }

    struct Pill {
        hwnd: HWND,
        config: PillConfig,
        commands: Receiver<Command>,
        carets: Receiver<CaretReply>,
        probe: Option<CaretProbe>,
        /// Created when a pill is drawn, dropped when it hides.
        renderer: Option<Renderer>,
        locating: Option<Locating>,
        visible: Option<Visible>,
        next_id: u64,
    }

    impl Pill {
        /// Returns true when asked to shut down.
        fn drain_commands(&mut self) -> bool {
            while let Ok(command) = self.commands.try_recv() {
                match command {
                    Command::Show(correction) => self.start_locating(correction),
                    Command::Hide => self.cancel(),
                    Command::Config(config) => {
                        self.config = config;
                        if config_is_off(&self.config) {
                            self.cancel();
                        }
                    }
                    Command::Shutdown => {
                        self.cancel();
                        return true;
                    }
                }
            }
            false
        }

        fn start_locating(&mut self, correction: Correction) {
            if config_is_off(&self.config) {
                return;
            }
            self.next_id += 1;
            self.locating = Some(Locating {
                id: self.next_id,
                correction,
                deadline: None,
                uia_asked: false,
            });
            self.set_timer(TIMER_LOCATE, LOCATE_DELAY_MS);
        }

        /// One attempt at finding the caret: the system caret, then UIA for browser/XAML hosts.
        fn locate(&mut self) {
            let Some(loc) = self.locating.as_mut() else {
                self.kill_timer(TIMER_LOCATE);
                return;
            };
            let now = Instant::now();
            let deadline = *loc.deadline.get_or_insert(now + LOCATE_BUDGET);
            if let Some(spot) = caret::system_caret() {
                self.found(spot);
                return;
            }
            if !loc.uia_asked && self.config.uia_caret && caret::focus_needs_uia() {
                loc.uia_asked = true;
                let asked = self.probe.as_ref().is_some_and(|p| {
                    p.request(
                        loc.id,
                        loc.correction.word().chars().count(),
                        loc.correction.trailing,
                    )
                });
                if !asked {
                    self.probe = None;
                }
            }
            if now >= deadline {
                self.locating = None;
                self.kill_timer(TIMER_LOCATE);
            } else {
                self.set_timer(TIMER_LOCATE, LOCATE_RETRY_MS);
            }
        }

        fn drain_carets(&mut self) {
            while let Ok(reply) = self.carets.try_recv() {
                let Some(loc) = &self.locating else {
                    continue;
                };
                let in_time = loc.deadline.is_some_and(|d| Instant::now() <= d);
                if let (true, true, Some(spot)) = (reply.id == loc.id, in_time, reply.spot) {
                    self.found(spot);
                }
            }
        }

        fn found(&mut self, spot: CaretSpot) {
            self.kill_timer(TIMER_LOCATE);
            let Some(loc) = self.locating.take() else {
                return;
            };
            // On a failed GDI or Direct2D call, skip this pill; the next one starts fresh.
            if self.present(&loc.correction, &spot).is_err() {
                self.hide();
            }
        }

        fn present(&mut self, correction: &Correction, spot: &CaretSpot) -> WinResult<()> {
            let (work, dpi) = monitor_at(POINT {
                x: spot.x,
                y: (spot.top + spot.bottom) / 2,
            });
            if self.renderer.is_none() {
                self.renderer = Some(Renderer::new()?);
            }
            let renderer = self.renderer.as_ref().expect("renderer just created");
            let scale = dpi / 96.0;
            let texts = renderer.texts(correction)?;
            let layout = capsule_layout(&texts.metrics, correction.keycap());
            let (top, bottom, word) = anchor(spot, correction);
            let Some(placement) = place(
                self.config.style,
                self.config.position,
                top,
                bottom,
                word,
                (layout.width, layout.height),
                scale,
                work,
            ) else {
                return Ok(());
            };
            let bitmap =
                renderer.render(&placement, dpi, &self.config, &texts, &layout, correction)?;
            let origin = POINT {
                x: placement.window.left,
                y: placement.window.top,
            };
            // A pill already on screen is replaced in place, without fading in again.
            let now = Instant::now();
            let shown = match &self.visible {
                Some(_) => now
                    .checked_sub(Duration::from_millis(u64::from(FADE_IN_MS)))
                    .unwrap_or(now),
                None => now,
            };
            let elapsed = elapsed_ms(shown, now);
            let frame = frame_at(elapsed, self.config.duration_ms).unwrap_or(Frame {
                alpha: 0.0,
                rise: 0.0,
            });
            let size = SIZE {
                cx: placement.window.width(),
                cy: placement.window.height(),
            };
            // SAFETY: our window and a DC holding a 32 bpp premultiplied DIB of `size`.
            unsafe {
                UpdateLayeredWindow(
                    self.hwnd,
                    HDC::default(),
                    Some(&risen(origin, frame, scale)),
                    Some(&size),
                    bitmap.dc,
                    Some(&POINT::default()),
                    COLORREF(0),
                    Some(&self.blend(frame)),
                    ULW_ALPHA,
                )?;
            }
            // The window keeps its own copy of the pixels; free ours now.
            drop(bitmap);
            if self.visible.is_none() {
                // SAFETY: shows our window without activating it, above other topmost windows.
                unsafe {
                    SetWindowPos(
                        self.hwnd,
                        HWND_TOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    )?;
                }
            }
            self.visible = Some(Visible {
                origin,
                scale,
                shown,
            });
            match next_tick_ms(elapsed, self.config.duration_ms) {
                Some(ms) => self.set_timer(TIMER_ANIM, ms),
                None => self.hide(),
            }
            Ok(())
        }

        fn animate(&mut self) {
            let Some(v) = &self.visible else {
                self.kill_timer(TIMER_ANIM);
                return;
            };
            let elapsed = elapsed_ms(v.shown, Instant::now());
            let duration = self.config.duration_ms;
            let Some(frame) = frame_at(elapsed, duration) else {
                self.hide();
                return;
            };
            let pos = risen(v.origin, frame, v.scale);
            // SAFETY: position and alpha only; the window keeps its last bitmap.
            let moved = unsafe {
                UpdateLayeredWindow(
                    self.hwnd,
                    HDC::default(),
                    Some(&pos),
                    None,
                    HDC::default(),
                    None,
                    COLORREF(0),
                    Some(&self.blend(frame)),
                    ULW_ALPHA,
                )
            };
            match (moved, next_tick_ms(elapsed, duration)) {
                (Ok(()), Some(ms)) => self.set_timer(TIMER_ANIM, ms),
                _ => self.hide(),
            }
        }

        fn blend(&self, frame: Frame) -> BLENDFUNCTION {
            BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: (frame.alpha * self.config.opacity * 255.0)
                    .round()
                    .clamp(0.0, 255.0) as u8,
                AlphaFormat: AC_SRC_ALPHA as u8,
            }
        }

        /// Hide the window and release everything drawing needs: hidden, the pill holds no
        /// timer, bitmap or Direct2D object. Recreating them costs ~2 ms per pill.
        fn hide(&mut self) {
            self.kill_timer(TIMER_ANIM);
            self.renderer = None;
            if self.visible.take().is_some() {
                // SAFETY: hides our own window.
                unsafe {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                }
            }
        }

        fn cancel(&mut self) {
            self.locating = None;
            self.kill_timer(TIMER_LOCATE);
            self.hide();
        }

        fn set_timer(&self, id: usize, ms: u32) {
            // SAFETY: a timer on our own window; re-arming an id replaces it.
            unsafe { SetTimer(self.hwnd, id, ms.max(1), None) };
        }

        fn kill_timer(&self, id: usize) {
            // SAFETY: fails harmlessly when the timer isn't set.
            let _ = unsafe { KillTimer(self.hwnd, id) };
        }
    }

    fn config_is_off(c: &PillConfig) -> bool {
        c.style == PillStyle::Off
    }

    fn elapsed_ms(since: Instant, now: Instant) -> u32 {
        u32::try_from(now.saturating_duration_since(since).as_millis()).unwrap_or(u32::MAX)
    }

    fn risen(origin: POINT, frame: Frame, scale: f32) -> POINT {
        POINT {
            x: origin.x,
            y: origin.y + (frame.rise * scale).round() as i32,
        }
    }

    /// Work area and effective DPI of the monitor at `pt`.
    fn monitor_at(pt: POINT) -> (Rect, f32) {
        // SAFETY: plain queries with valid out-pointers.
        unsafe {
            let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let work = if GetMonitorInfoW(monitor, &mut info).as_bool() {
                let r = info.rcWork;
                Rect {
                    left: r.left,
                    top: r.top,
                    right: r.right,
                    bottom: r.bottom,
                }
            } else {
                Rect {
                    left: i32::MIN / 2,
                    top: i32::MIN / 2,
                    right: i32::MAX / 2,
                    bottom: i32::MAX / 2,
                }
            };
            let (mut x, mut y) = (96u32, 96u32);
            let dpi = match GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) {
                Ok(()) if x > 0 => x as f32,
                _ => 96.0,
            };
            (work, dpi)
        }
    }

    /// A 32 bpp top-down DIB selected into its own memory DC.
    struct Bitmap {
        dc: HDC,
        bitmap: HBITMAP,
        previous: HGDIOBJ,
        bits: *mut u8,
        width: i32,
        height: i32,
    }

    impl Bitmap {
        fn new(width: i32, height: i32) -> WinResult<Self> {
            let (width, height) = (width.max(1), height.max(1));
            // SAFETY: the DC and bitmap are released in Drop; `bits` lives as long as the bitmap.
            unsafe {
                let dc = CreateCompatibleDC(HDC::default());
                if dc.is_invalid() {
                    return Err(windows::core::Error::from_win32());
                }
                let info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: width,
                        biHeight: -height,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut bits = std::ptr::null_mut();
                let bitmap = match CreateDIBSection(
                    dc,
                    &info,
                    DIB_RGB_COLORS,
                    &mut bits,
                    HANDLE::default(),
                    0,
                ) {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = DeleteDC(dc);
                        return Err(e);
                    }
                };
                let previous = SelectObject(dc, bitmap);
                Ok(Self {
                    dc,
                    bitmap,
                    previous,
                    bits: bits.cast(),
                    width,
                    height,
                })
            }
        }

        /// Premultiplied BGRA, row by row.
        fn pixels(&self) -> &[u8] {
            // SAFETY: GDI has finished drawing (GdiFlush); the DIB holds width*height*4 bytes.
            unsafe {
                let _ = GdiFlush();
                std::slice::from_raw_parts(self.bits, (self.width * self.height * 4) as usize)
            }
        }

        fn pixels_mut(&mut self) -> &mut [u8] {
            // SAFETY: as above, with exclusive access through &mut self.
            unsafe {
                let _ = GdiFlush();
                std::slice::from_raw_parts_mut(self.bits, (self.width * self.height * 4) as usize)
            }
        }
    }

    impl Drop for Bitmap {
        fn drop(&mut self) {
            // SAFETY: undo the selection, then free what `new` created.
            unsafe {
                SelectObject(self.dc, self.previous);
                let _ = DeleteObject(self.bitmap);
                let _ = DeleteDC(self.dc);
            }
        }
    }

    /// Text laid out for one correction.
    struct Texts {
        before: IDWriteTextLayout,
        arrow: IDWriteTextLayout,
        word: IDWriteTextLayout,
        label: IDWriteTextLayout,
        metrics: TextMetrics,
    }

    /// Direct2D and DirectWrite objects. All device-independent or software: nothing on the GPU.
    struct Renderer {
        target: ID2D1DCRenderTarget,
        dwrite: IDWriteFactory,
        regular: IDWriteTextFormat,
        strong: IDWriteTextFormat,
        mono: IDWriteTextFormat,
    }

    impl Renderer {
        fn new() -> WinResult<Self> {
            // SAFETY: factory creation and plain COM calls on this thread.
            unsafe {
                let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
                let props = D2D1_RENDER_TARGET_PROPERTIES {
                    r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: 96.0,
                    dpiY: 96.0,
                    usage: D2D1_RENDER_TARGET_USAGE_NONE,
                    minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
                };
                let target = d2d.CreateDCRenderTarget(&props)?;
                let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
                let ui = first_font(&dwrite, UI_FONTS);
                let mono = first_font(&dwrite, MONO_FONTS);
                Ok(Self {
                    regular: text_format(&dwrite, ui, DWRITE_FONT_WEIGHT_NORMAL, dip::TEXT_SIZE)?,
                    strong: text_format(&dwrite, ui, DWRITE_FONT_WEIGHT_SEMI_BOLD, dip::TEXT_SIZE)?,
                    mono: text_format(
                        &dwrite,
                        mono,
                        DWRITE_FONT_WEIGHT_NORMAL,
                        dip::KEY_TEXT_SIZE,
                    )?,
                    target,
                    dwrite,
                })
            }
        }

        fn texts(&self, c: &Correction) -> WinResult<Texts> {
            let before = layout(&self.dwrite, &self.regular, c.before())?;
            // SAFETY: valid layout; the range covers the whole text.
            unsafe {
                before.SetStrikethrough(
                    true,
                    DWRITE_TEXT_RANGE {
                        startPosition: 0,
                        length: u32::MAX,
                    },
                )?;
            }
            let arrow = layout(&self.dwrite, &self.regular, "\u{2192}")?;
            let word = layout(&self.dwrite, &self.strong, c.word())?;
            let label = layout(&self.dwrite, &self.mono, c.label())?;
            let (b, a, w, l) = (size(&before)?, size(&arrow)?, size(&word)?, size(&label)?);
            Ok(Texts {
                metrics: TextMetrics {
                    before: b.0,
                    arrow: a.0,
                    word: w.0,
                    label: l.0,
                    line: b.1.max(w.1),
                    label_line: l.1,
                },
                before,
                arrow,
                word,
                label,
            })
        }

        /// Draw the pill for `placement` into a new bitmap the size of its window.
        fn render(
            &self,
            placement: &Placement,
            dpi: f32,
            config: &PillConfig,
            texts: &Texts,
            layout: &CapsuleLayout,
            correction: &Correction,
        ) -> WinResult<Bitmap> {
            let bitmap = Bitmap::new(placement.window.width(), placement.window.height())?;
            let to_dip = 96.0 / dpi;
            let rt: ID2D1RenderTarget = self.begin(&bitmap, dpi)?;
            let colours = config.colours_for(correction);
            // SAFETY: drawing between BeginDraw (in `begin`) and EndDraw on a bound target.
            unsafe {
                if let Some(u) = placement.underline {
                    let u = u.scaled(to_dip);
                    let brush = brush(&rt, colours.accent, 1.0)?;
                    rt.FillRoundedRectangle(&rounded(u, u.h / 2.0), &brush);
                }
                if let Some(cap) = placement.capsule {
                    let cap = cap.scaled(to_dip);
                    let radius = cap.h / 2.0;
                    // Soft shadow: a few widening, nearly clear layers under the capsule.
                    let shadow = brush(&rt, Rgba::hex(0), 0.045)?;
                    for i in 1..=dip::SHADOW as i32 {
                        let g = i as f32;
                        let r = BoxF {
                            x: cap.x - g,
                            y: cap.y - g + dip::SHADOW_DY,
                            w: cap.w + 2.0 * g,
                            h: cap.h + 2.0 * g,
                        };
                        rt.FillRoundedRectangle(&rounded(r, radius + g), &shadow);
                    }
                    rt.FillRoundedRectangle(
                        &rounded(cap, radius),
                        &brush(&rt, colours.background, 1.0)?,
                    );
                    let at = |x: f32, y: f32| D2D_POINT_2F {
                        x: cap.x + x,
                        y: cap.y + y,
                    };
                    let faint = brush(&rt, colours.text, 0.55)?;
                    let text = brush(&rt, colours.text, 1.0)?;
                    let keycap = brush(&rt, colours.keycap, 1.0)?;
                    let opts = D2D1_DRAW_TEXT_OPTIONS_NONE;
                    rt.DrawTextLayout(
                        at(layout.before_x, layout.text_y),
                        &texts.before,
                        &faint,
                        opts,
                    );
                    rt.DrawTextLayout(
                        at(layout.arrow_x, layout.text_y),
                        &texts.arrow,
                        &faint,
                        opts,
                    );
                    rt.DrawTextLayout(at(layout.word_x, layout.text_y), &texts.word, &text, opts);
                    if correction.keycap() {
                        let key = BoxF {
                            x: cap.x + layout.key.x,
                            y: cap.y + layout.key.y,
                            ..layout.key
                        };
                        let fill = brush(&rt, colours.keycap, 0.16)?;
                        rt.FillRoundedRectangle(&rounded(key, dip::KEY_RADIUS), &fill);
                    }
                    rt.DrawTextLayout(
                        at(layout.label_x, layout.label_y),
                        &texts.label,
                        &keycap,
                        opts,
                    );
                }
                rt.EndDraw(None, None)?;
            }
            Ok(bitmap)
        }

        /// Bind the target to `bitmap`, clear it and start drawing at `dpi`.
        fn begin(&self, bitmap: &Bitmap, dpi: f32) -> WinResult<ID2D1RenderTarget> {
            let rect = RECT {
                left: 0,
                top: 0,
                right: bitmap.width,
                bottom: bitmap.height,
            };
            // SAFETY: the DC stays valid until the caller's EndDraw (it owns `bitmap`).
            unsafe {
                self.target.BindDC(bitmap.dc, &rect)?;
                let rt: ID2D1RenderTarget = windows::core::Interface::cast(&self.target)?;
                rt.SetDpi(dpi, dpi);
                rt.BeginDraw();
                rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
                rt.Clear(Some(&D2D1_COLOR_F::default()));
                Ok(rt)
            }
        }
    }

    unsafe fn first_font(dwrite: &IDWriteFactory, names: &[PCWSTR]) -> PCWSTR {
        let mut collection = None;
        if dwrite
            .GetSystemFontCollection(&mut collection, false)
            .is_ok()
        {
            if let Some(collection) = collection {
                for &name in names {
                    let (mut index, mut exists) = (0u32, BOOL(0));
                    if collection
                        .FindFamilyName(name, &mut index, &mut exists)
                        .is_ok()
                        && exists.as_bool()
                    {
                        return name;
                    }
                }
            }
        }
        names[names.len() - 1]
    }

    unsafe fn text_format(
        dwrite: &IDWriteFactory,
        family: PCWSTR,
        weight: DWRITE_FONT_WEIGHT,
        size: f32,
    ) -> WinResult<IDWriteTextFormat> {
        let format = dwrite.CreateTextFormat(
            family,
            None,
            weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!("en-us"),
        )?;
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        Ok(format)
    }

    fn layout(
        dwrite: &IDWriteFactory,
        format: &IDWriteTextFormat,
        text: &str,
    ) -> WinResult<IDWriteTextLayout> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: valid factory and format; DirectWrite copies the text.
        unsafe { dwrite.CreateTextLayout(&wide, format, 10_000.0, 1_000.0) }
    }

    /// Width (including trailing spaces) and line height, in DIPs.
    fn size(layout: &IDWriteTextLayout) -> WinResult<(f32, f32)> {
        let mut m = DWRITE_TEXT_METRICS::default();
        // SAFETY: valid layout and out-pointer.
        unsafe { layout.GetMetrics(&mut m)? };
        Ok((m.widthIncludingTrailingWhitespace, m.height))
    }

    unsafe fn brush(
        rt: &ID2D1RenderTarget,
        c: Rgba,
        alpha: f32,
    ) -> WinResult<ID2D1SolidColorBrush> {
        let colour = D2D1_COLOR_F {
            r: f32::from(c.r) / 255.0,
            g: f32::from(c.g) / 255.0,
            b: f32::from(c.b) / 255.0,
            a: f32::from(c.a) / 255.0 * alpha,
        };
        rt.CreateSolidColorBrush(&colour, None)
    }

    fn rounded(b: BoxF, radius: f32) -> D2D1_ROUNDED_RECT {
        D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: b.x,
                top: b.y,
                right: b.right(),
                bottom: b.bottom(),
            },
            radiusX: radius,
            radiusY: radius,
        }
    }

    /// An offscreen rendering of the pill over a mock text field, for checking the visuals.
    pub struct Preview {
        pub width: u32,
        pub height: u32,
        /// Opaque BGRA, row by row.
        pub bgra: Vec<u8>,
    }

    /// Render the pill as it would appear after typing `prefix` and then the correction, on a
    /// light text field at `dpi`. Uses the same layout and drawing as the live overlay, the
    /// estimated word span (as for a Win32 caret) and the final, fully faded-in frame. No window
    /// is created.
    pub fn render_preview(
        config: &PillConfig,
        correction: &Correction,
        dpi: u32,
        prefix: &str,
    ) -> std::result::Result<Preview, String> {
        let config = config.clone().clamped();
        let dpi = dpi.max(48) as f32;
        let scale = dpi / 96.0;
        let r = Renderer::new().map_err(|e| e.to_string())?;
        let (w, h) = ((360.0 * scale).ceil() as i32, (130.0 * scale).ceil() as i32);
        let mut canvas = Bitmap::new(w, h).map_err(|e| e.to_string())?;
        // The mock field: 11 pt Segoe UI, line top at 64 DIP, caret after the trailing boundary.
        let typed = format!(
            "{prefix}{}{}",
            correction.word(),
            " ".repeat(correction.trailing)
        );
        let (x0, y0) = (96.0f32, 64.0f32);
        let (caret_x, line_h) = (|| -> WinResult<(f32, f32)> {
            // SAFETY: drawing between BeginDraw and EndDraw on a bound target.
            unsafe {
                let ui = first_font(&r.dwrite, UI_FONTS);
                let field = text_format(&r.dwrite, ui, DWRITE_FONT_WEIGHT_NORMAL, 14.667)?;
                let text = layout(&r.dwrite, &field, &typed)?;
                let (tw, th) = size(&text)?;
                let rt = r.begin(&canvas, dpi)?;
                rt.Clear(Some(&D2D1_COLOR_F {
                    r: 0.98,
                    g: 0.973,
                    b: 0.969,
                    a: 1.0,
                }));
                let ink = brush(&rt, Rgba::hex(0x1F1F1F), 1.0)?;
                rt.DrawTextLayout(
                    D2D_POINT_2F { x: x0, y: y0 },
                    &text,
                    &ink,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
                let caret = BoxF {
                    x: x0 + tw,
                    y: y0,
                    w: 1.0,
                    h: th,
                };
                rt.FillRoundedRectangle(&rounded(caret, 0.0), &ink);
                rt.EndDraw(None, None)?;
                Ok((x0 + tw, th))
            }
        })()
        .map_err(|e| e.to_string())?;
        let spot = CaretSpot {
            x: (caret_x * scale).round() as i32,
            top: (y0 * scale).round() as i32,
            bottom: ((y0 + line_h) * scale).round() as i32,
            word: None,
        };
        let texts = r.texts(correction).map_err(|e| e.to_string())?;
        let layout = capsule_layout(&texts.metrics, correction.keycap());
        let (top, bottom, word) = anchor(&spot, correction);
        let work = Rect {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        if let Some(p) = place(
            config.style,
            config.position,
            top,
            bottom,
            word,
            (layout.width, layout.height),
            scale,
            work,
        ) {
            let pill = r
                .render(&p, dpi, &config, &texts, &layout, correction)
                .map_err(|e| e.to_string())?;
            composite(
                &mut canvas,
                &pill,
                p.window.left,
                p.window.top,
                config.opacity,
            );
        }
        let mut bgra = canvas.pixels().to_vec();
        for px in bgra.chunks_exact_mut(4) {
            px[3] = 255;
        }
        Ok(Preview {
            width: w as u32,
            height: h as u32,
            bgra,
        })
    }

    /// Blend premultiplied `src` over `dst` at `(x, y)` with a constant alpha, as
    /// `UpdateLayeredWindow` does on screen.
    fn composite(dst: &mut Bitmap, src: &Bitmap, x: i32, y: i32, alpha: f32) {
        let (dw, dh) = (dst.width, dst.height);
        let (sw, sh) = (src.width, src.height);
        let s = src.pixels().to_vec();
        let d = dst.pixels_mut();
        for row in 0..sh {
            let ty = y + row;
            if ty < 0 || ty >= dh {
                continue;
            }
            for col in 0..sw {
                let tx = x + col;
                if tx < 0 || tx >= dw {
                    continue;
                }
                let si = ((row * sw + col) * 4) as usize;
                let di = ((ty * dw + tx) * 4) as usize;
                let a = f32::from(s[si + 3]) / 255.0 * alpha;
                for c in 0..3 {
                    let v = f32::from(s[si + c]) * alpha + f32::from(d[di + c]) * (1.0 - a);
                    d[di + c] = v.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unflub_core::Source;

    fn work() -> Rect {
        Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        }
    }

    fn metrics() -> TextMetrics {
        TextMetrics {
            before: 20.0,
            arrow: 12.0,
            word: 22.0,
            label: 40.0,
            line: 16.0,
            label_line: 15.0,
        }
    }

    #[test]
    fn styles_parse_and_print() {
        for s in PillStyle::ALL {
            assert_eq!(s.name().parse::<PillStyle>(), Ok(s));
            assert_eq!(s.to_string(), s.name());
        }
        assert_eq!("Capsule".parse::<PillStyle>(), Ok(PillStyle::Capsule));
        assert!("pill".parse::<PillStyle>().is_err());
        assert_eq!(PillStyle::default(), PillStyle::UnderlineCapsule);
        assert!(
            PillStyle::UnderlineCapsule.has_capsule()
                && PillStyle::UnderlineCapsule.has_underline()
        );
        assert!(!PillStyle::Capsule.has_underline() && !PillStyle::Underline.has_capsule());
        assert!(!PillStyle::Off.has_capsule() && !PillStyle::Off.has_underline());
    }

    #[test]
    fn config_defaults_are_the_brand_palette() {
        let c = PillConfig::default();
        assert_eq!(
            c.background,
            Rgba {
                r: 0x2D,
                g: 0x19,
                b: 0x16,
                a: 255
            }
        );
        assert_eq!(c.text, Rgba::hex(0xF2E6E3));
        assert_eq!(c.keycap, Rgba::hex(0xE0A595));
        assert_eq!(c.accent, Rgba::hex(0xB06A5C));
        assert_eq!(c.clone().clamped(), c);
    }

    #[test]
    fn config_is_clamped() {
        let c = PillConfig {
            opacity: 0.0,
            duration_ms: 10,
            ..PillConfig::default()
        }
        .clamped();
        assert_eq!((c.opacity, c.duration_ms), (MIN_OPACITY, MIN_DURATION_MS));
        let c = PillConfig {
            opacity: 7.0,
            duration_ms: u32::MAX,
            ..PillConfig::default()
        }
        .clamped();
        assert_eq!((c.opacity, c.duration_ms), (1.0, MAX_DURATION_MS));
        let c = PillConfig {
            opacity: f32::NAN,
            ..PillConfig::default()
        }
        .clamped();
        assert_eq!(c.opacity, 1.0);
    }

    fn replacement(delete: usize, insert: &str, original: &str, corrected: &str) -> Replacement {
        Replacement {
            delete,
            insert: insert.into(),
            original: original.into(),
            corrected: corrected.into(),
            source: Source::UserRule,
        }
    }

    /// The held-Backspace pill: the same words as the fix, "Undo?", in the alert colours.
    #[test]
    fn undo_pending_pill_asks_in_the_alert_colour() {
        let r = replacement(3, "the ", "teh", "the");
        let pending = Correction::undo_pending(&r);
        assert_eq!(pending.kind, CorrectionKind::UndoPending);
        assert_eq!((pending.word(), pending.before()), ("the", "teh"));
        assert_eq!((pending.label(), pending.keycap()), ("Undo?", false));
        assert_eq!(pending.trailing, 1);
        let config = PillConfig::default();
        let c = config.colours_for(&pending);
        assert_eq!(
            (c.background, c.accent, c.text),
            (config.alert, config.alert, config.alert_text)
        );
        let fixed = config.colours_for(&Correction::fixed(&r));
        assert_eq!(
            (fixed.background, fixed.text),
            (config.background, config.text)
        );
        assert_eq!(Correction::undone(&r).label(), "Undone");
    }

    /// Review blocker B2: no undo hint after Enter or Tab, where Backspace doesn't undo.
    #[test]
    fn no_undo_hint_after_enter_or_tab() {
        for insert in ["the\n", "the\t"] {
            let fix = Correction::fixed(&replacement(3, insert, "teh", "the"));
            assert!(!fix.undoable && !fix.keycap(), "{insert:?}");
            assert_eq!(fix.label(), "");
        }
        let fix = Correction::fixed(&replacement(3, "the.", "teh", "the"));
        assert!(fix.undoable && fix.keycap());
        let undo = Correction::undone(&replacement(4, "teh.", "teh", "the"));
        assert!(!undo.keycap());
    }

    #[test]
    fn corrections_know_what_follows_the_word() {
        let fix = Correction::from_event(&Event::Corrected(replacement(3, "the ", "teh", "the")))
            .unwrap();
        assert_eq!((fix.word(), fix.before(), fix.trailing), ("the", "teh", 1));
        assert_eq!(fix.label(), "\u{232B} undo");
        let undo =
            Correction::from_event(&Event::Undone(replacement(4, "teh", "teh", "the"))).unwrap();
        assert_eq!(
            (undo.word(), undo.before(), undo.trailing),
            ("teh", "the", 0)
        );
        assert_eq!(undo.label(), "Undone");
        assert_eq!(Correction::from_event(&Event::Learned("x".into())), None);
        // Non-ASCII counts characters, not bytes.
        let fix = Correction::fixed(&replacement(4, "café.", "cafe", "café"));
        assert_eq!(fix.trailing, 1);
    }

    #[test]
    fn capsule_layout_adds_padding_and_gaps() {
        let m = metrics();
        let l = capsule_layout(&m, true);
        let expected_w = dip::PAD_X
            + m.before
            + dip::GAP
            + m.arrow
            + dip::GAP
            + m.word
            + dip::KEY_GAP
            + m.label
            + 2.0 * dip::KEY_PAD_X
            + dip::PAD_X;
        assert!((l.width - expected_w).abs() < 1e-4, "{}", l.width);
        // The keycap box (15 + 2) is taller than the text line (16) here.
        assert_eq!(
            l.height,
            m.label_line + 2.0 * dip::KEY_PAD_Y + 2.0 * dip::PAD_Y
        );
        assert_eq!(l.text_y, (l.height - m.line) / 2.0);
        let tall_text = capsule_layout(&TextMetrics { line: 20.0, ..m }, true);
        assert_eq!(tall_text.height, 20.0 + 2.0 * dip::PAD_Y);
        assert_eq!(tall_text.text_y, dip::PAD_Y);
        assert!(l.before_x < l.arrow_x && l.arrow_x < l.word_x && l.word_x < l.key.x);
        assert_eq!(l.label_x, l.key.x + dip::KEY_PAD_X);
        // Keycap centred vertically.
        assert!((l.key.y + l.key.h / 2.0 - l.height / 2.0).abs() < 1e-4);
        // Without the keycap box (undo), the label sits flush.
        let plain = capsule_layout(&m, false);
        assert!((l.width - plain.width - 2.0 * dip::KEY_PAD_X).abs() < 1e-4);
        assert_eq!(plain.label_x, plain.key.x);
    }

    fn place_default(style: PillStyle, position: PillPosition, scale: f32) -> Placement {
        // Caret line 500..520, word 400..440.
        place(
            style,
            position,
            500,
            520,
            (400, 440),
            (120.0, 28.0),
            scale,
            work(),
        )
        .unwrap()
    }

    fn screen(p: &Placement, b: BoxF) -> BoxF {
        BoxF {
            x: b.x + p.window.left as f32,
            y: b.y + p.window.top as f32,
            ..b
        }
    }

    #[test]
    fn capsule_sits_above_the_line_centred_on_the_word() {
        let p = place_default(PillStyle::Capsule, PillPosition::Above, 1.0);
        let c = screen(&p, p.capsule.unwrap());
        assert!(!p.below);
        assert_eq!((c.w, c.h), (120.0, 28.0));
        assert_eq!(c.bottom(), 500.0 - dip::CARET_GAP);
        assert_eq!(c.x + c.w / 2.0, 420.0);
        assert_eq!(p.underline, None);
        // The window leaves room for the shadow.
        assert_eq!(p.window.left as f32, c.x - dip::SHADOW);
        assert_eq!(
            p.window.bottom as f32,
            c.bottom() + dip::SHADOW + dip::SHADOW_DY
        );
    }

    #[test]
    fn underline_spans_the_word_just_under_the_line() {
        let p = place_default(PillStyle::Underline, PillPosition::Above, 1.0);
        let u = screen(&p, p.underline.unwrap());
        assert_eq!((u.x, u.w, u.y, u.h), (400.0, 40.0, 521.0, 2.0));
        assert_eq!(p.capsule, None);
        assert_eq!(
            p.window,
            Rect {
                left: 400,
                top: 521,
                right: 440,
                bottom: 523
            }
        );
    }

    #[test]
    fn below_clears_the_underline() {
        let p = place_default(PillStyle::UnderlineCapsule, PillPosition::Below, 1.0);
        assert!(p.below);
        let c = screen(&p, p.capsule.unwrap());
        let u = screen(&p, p.underline.unwrap());
        assert_eq!(c.y, u.bottom() + dip::CARET_GAP);
    }

    #[test]
    fn everything_scales_with_dpi() {
        let p1 = place_default(PillStyle::UnderlineCapsule, PillPosition::Above, 1.0);
        let p15 = place_default(PillStyle::UnderlineCapsule, PillPosition::Above, 1.5);
        let (c1, c15) = (
            screen(&p1, p1.capsule.unwrap()),
            screen(&p15, p15.capsule.unwrap()),
        );
        assert_eq!((c15.w, c15.h), (180.0, 42.0));
        assert_eq!(500.0 - c1.bottom(), dip::CARET_GAP);
        assert_eq!(500.0 - c15.bottom(), dip::CARET_GAP * 1.5);
        assert_eq!(p15.underline.unwrap().h, 3.0);
        assert_eq!(screen(&p15, p15.underline.unwrap()).y, 522.0);
        // The underline follows the word, not the DPI.
        assert_eq!(p15.underline.unwrap().w, 40.0);
    }

    #[test]
    fn flips_below_at_the_top_of_the_screen_and_above_at_the_bottom() {
        let top = place(
            PillStyle::Capsule,
            PillPosition::Above,
            10,
            30,
            (100, 140),
            (120.0, 28.0),
            1.0,
            work(),
        )
        .unwrap();
        assert!(top.below, "no room above");
        assert!(screen(&top, top.capsule.unwrap()).y >= 30.0);
        let bottom = place(
            PillStyle::Capsule,
            PillPosition::Below,
            1000,
            1020,
            (100, 140),
            (120.0, 28.0),
            1.0,
            work(),
        )
        .unwrap();
        assert!(!bottom.below, "no room below");
        assert_eq!(
            screen(&bottom, bottom.capsule.unwrap()).bottom(),
            1000.0 - dip::CARET_GAP
        );
    }

    #[test]
    fn capsule_is_clamped_to_the_work_area_sideways() {
        let left = place(
            PillStyle::Capsule,
            PillPosition::Above,
            500,
            520,
            (0, 10),
            (120.0, 28.0),
            1.0,
            work(),
        )
        .unwrap();
        assert_eq!(screen(&left, left.capsule.unwrap()).x, 0.0);
        let right = place(
            PillStyle::Capsule,
            PillPosition::Above,
            500,
            520,
            (1910, 1920),
            (120.0, 28.0),
            1.0,
            work(),
        )
        .unwrap();
        assert_eq!(screen(&right, right.capsule.unwrap()).right(), 1920.0);
        // A second monitor left of the primary has negative coordinates.
        let neg = Rect {
            left: -1280,
            top: 0,
            right: 0,
            bottom: 1000,
        };
        let p = place(
            PillStyle::Capsule,
            PillPosition::Above,
            500,
            520,
            (-1275, -1260),
            (120.0, 28.0),
            1.0,
            neg,
        )
        .unwrap();
        assert_eq!(screen(&p, p.capsule.unwrap()).x, -1280.0);
    }

    #[test]
    fn tiny_or_offscreen_words_still_get_a_visible_underline() {
        let p = place(
            PillStyle::Underline,
            PillPosition::Above,
            500,
            520,
            (300, 301),
            (0.0, 0.0),
            1.0,
            work(),
        )
        .unwrap();
        assert_eq!(p.underline.unwrap().w, dip::UNDERLINE_MIN);
        let p = place(
            PillStyle::Underline,
            PillPosition::Above,
            500,
            520,
            (1950, 1990),
            (0.0, 0.0),
            1.0,
            work(),
        )
        .unwrap();
        let u = screen(&p, p.underline.unwrap());
        assert!(u.right() <= 1920.0 && u.w >= dip::UNDERLINE_MIN);
    }

    #[test]
    fn off_places_nothing() {
        assert_eq!(
            place(
                PillStyle::Off,
                PillPosition::Above,
                500,
                520,
                (400, 440),
                (120.0, 28.0),
                1.0,
                work()
            ),
            None
        );
    }

    #[test]
    fn animation_fades_and_rises_in_holds_and_fades_out() {
        let d = DEFAULT_DURATION_MS;
        let f0 = frame_at(0, d).unwrap();
        assert_eq!((f0.alpha, f0.rise), (0.0, dip::RISE));
        let mid = frame_at(FADE_IN_MS / 2, d).unwrap();
        assert!(
            mid.alpha > 0.5 && mid.rise > 0.0 && mid.rise < dip::RISE,
            "{mid:?}"
        );
        assert_eq!(
            frame_at(FADE_IN_MS, d),
            Some(Frame {
                alpha: 1.0,
                rise: 0.0
            })
        );
        assert_eq!(frame_at(d - FADE_OUT_MS - 1, d).unwrap().alpha, 1.0);
        let out = frame_at(d - FADE_OUT_MS / 2, d).unwrap();
        assert!((out.alpha - 0.5).abs() < 0.01, "{out:?}");
        assert!(frame_at(d - 1, d).unwrap().alpha < 0.01);
        assert_eq!(frame_at(d, d), None);
        // Alpha never goes up after the fade-in.
        let mut last = 1.0;
        for t in FADE_IN_MS..d {
            let a = frame_at(t, d).unwrap().alpha;
            assert!(a <= last + 1e-6, "t={t}");
            last = a;
        }
    }

    #[test]
    fn ticks_are_frames_during_fades_and_one_timer_for_the_hold() {
        let d = DEFAULT_DURATION_MS;
        assert_eq!(next_tick_ms(0, d), Some(FRAME_MS));
        assert_eq!(
            next_tick_ms(FADE_IN_MS, d),
            Some(d - FADE_OUT_MS - FADE_IN_MS)
        );
        assert_eq!(next_tick_ms(d - FADE_OUT_MS, d), Some(FRAME_MS));
        assert_eq!(next_tick_ms(d - 5, d), Some(5));
        assert_eq!(next_tick_ms(d, d), None);
        // Too-short durations behave like the minimum.
        assert_eq!(
            frame_at(MIN_DURATION_MS - 1, 0).map(|f| f.alpha > 0.0),
            Some(true)
        );
    }

    #[test]
    fn anchor_uses_the_estimate_for_win32_carets() {
        let fix = Correction::fixed(&replacement(3, "the ", "teh", "the"));
        let spot = CaretSpot {
            x: 500,
            top: 100,
            bottom: 120,
            word: None,
        };
        assert_eq!(
            anchor(&spot, &fix),
            (100, 120, crate::caret::estimate_word_span(500, 20, 3, 1))
        );
    }

    /// Renders offscreen with Direct2D (software) and DirectWrite; no window is created.
    #[cfg(windows)]
    #[test]
    fn preview_renders_every_style() {
        let fix = Correction::fixed(&replacement(7, "receive ", "recieve", "receive"));
        let blank = render_preview(
            &PillConfig {
                style: PillStyle::Off,
                ..PillConfig::default()
            },
            &fix,
            96,
            "I ",
        )
        .unwrap();
        for style in [
            PillStyle::UnderlineCapsule,
            PillStyle::Capsule,
            PillStyle::Underline,
        ] {
            for dpi in [96, 144] {
                let p = render_preview(
                    &PillConfig {
                        style,
                        ..PillConfig::default()
                    },
                    &fix,
                    dpi,
                    "I ",
                )
                .unwrap();
                assert_eq!(p.width, (360 * dpi).div_ceil(96));
                assert_eq!(p.bgra.len(), (p.width * p.height * 4) as usize);
                if dpi == 96 {
                    let changed = p
                        .bgra
                        .iter()
                        .zip(&blank.bgra)
                        .filter(|(a, b)| a != b)
                        .count();
                    assert!(
                        changed > 100,
                        "{style}: only {changed} bytes differ from no pill"
                    );
                }
            }
        }
    }

    /// Creates the hidden overlay window and its thread, changes settings, and shuts down.
    /// Never calls `show`, so nothing appears on screen.
    #[cfg(windows)]
    #[test]
    fn overlay_starts_and_stops_without_showing() {
        let overlay = Overlay::spawn(PillConfig::default()).expect("overlay window");
        overlay.set_config(PillConfig {
            style: PillStyle::Capsule,
            opacity: 0.8,
            ..PillConfig::default()
        });
        overlay.shutdown();
        let again = Overlay::spawn(PillConfig::default()).expect("class registers twice");
        drop(again);
    }
}
