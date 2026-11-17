//! Where the text caret is, so the pill can sit next to the word we just fixed.
//!
//! Two sources, tried in this order by the overlay thread:
//! 1. The Win32 system caret of the foreground thread (`GetGUIThreadInfo`). Classic Win32 apps
//!    (Notepad, WordPad, Office, most dialogs) keep it where the text caret is. It gives the line
//!    (top and bottom) and the caret x; the word's width is estimated from the line height.
//! 2. For browser engines and XAML/WPF hosts (the same window classes `focus` sends to UI
//!    Automation), UIA `TextPattern2::GetCaretRange`, or the selection of `TextPattern` when the
//!    app has no `TextPattern2`. We move the range back over the word, so its bounding rectangle
//!    is the word's exact span. UIA can block on hung apps, so it runs on its own worker thread
//!    with the same short timeouts as the password probe, and never on the hook thread.
//!
//! Anything else (terminals, games, canvas editors) has no caret we can see: the overlay skips
//! the pill rather than guess.
//!
//! All coordinates are physical screen pixels.

/// Average width of one letter as a fraction of the caret (line) height. A Segoe UI or Calibri
/// line is about 1.33 em tall and an average lowercase letter about 0.49 em wide.
pub const CHAR_WIDTH_PER_LINE_HEIGHT: f32 = 0.37;

/// Width of a boundary character typed after the word (space, `.`, `,`): about 0.27 em.
pub const BOUNDARY_WIDTH_PER_LINE_HEIGHT: f32 = 0.2;

/// Line heights outside this range are clamped before estimating a word's width (some apps
/// report a 1 px or a full-window caret).
const LINE_HEIGHT_RANGE: (i32, i32) = (8, 400);

/// The caret's line and position, plus the corrected word's span when the app told us.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaretSpot {
    /// Caret x (left edge of the system caret, right edge of the text before it).
    pub x: i32,
    /// Top of the caret's line.
    pub top: i32,
    /// Bottom of the caret's line.
    pub bottom: i32,
    /// Exact `(left, right)` of the corrected word, from UI Automation.
    pub word: Option<(i32, i32)>,
}

impl CaretSpot {
    /// The corrected word's `(left, right)`: exact when known, else estimated from the caret.
    /// `trailing_chars` are the characters typed after the word (the boundary).
    pub fn word_span(&self, word_chars: usize, trailing_chars: usize) -> (i32, i32) {
        self.word.unwrap_or_else(|| {
            estimate_word_span(self.x, self.bottom - self.top, word_chars, trailing_chars)
        })
    }
}

/// Estimate where a word of `word_chars` characters ends and starts, given that the caret sits
/// `trailing_chars` characters after it on a line `line_height` pixels tall.
pub fn estimate_word_span(
    caret_x: i32,
    line_height: i32,
    word_chars: usize,
    trailing_chars: usize,
) -> (i32, i32) {
    let h = line_height.clamp(LINE_HEIGHT_RANGE.0, LINE_HEIGHT_RANGE.1) as f32;
    let end = caret_x as f32 - h * BOUNDARY_WIDTH_PER_LINE_HEIGHT * trailing_chars as f32;
    let start = end - h * CHAR_WIDTH_PER_LINE_HEIGHT * word_chars.max(1) as f32;
    (start.round() as i32, end.round() as i32)
}

/// Build a spot from UIA bounding rectangles (`[left, top, width, height]` per line, physical
/// screen pixels). The last rectangle is the one on the caret's line. `is_word`: the range was
/// the corrected word (its span is exact); otherwise it was the character before the caret.
pub fn spot_from_rects(rects: &[f64], is_word: bool) -> Option<CaretSpot> {
    let r = rects
        .chunks_exact(4)
        .rev()
        .find(|r| r[3] > 0.0 && r.iter().all(|v| v.is_finite()))?;
    let (left, top, width, height) = (r[0], r[1], r[2].max(0.0), r[3]);
    let right = (left + width).round() as i32;
    Some(CaretSpot {
        x: right,
        top: top.round() as i32,
        bottom: (top + height).round() as i32,
        word: is_word.then_some((left.round() as i32, right)),
    })
}

#[cfg(windows)]
pub use win::{focus_needs_uia, system_caret, CaretProbe, CaretReply};

#[cfg(windows)]
mod win {
    use super::{spot_from_rects, CaretSpot};
    use crate::focus::{self, FieldKind};
    use std::sync::mpsc::{self, Receiver, Sender};
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
    use windows::Win32::System::Ole::{
        SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetLBound, SafeArrayGetUBound,
        SafeArrayUnaccessData,
    };
    use windows::Win32::UI::Accessibility::{
        IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextPattern2,
        IUIAutomationTextRange, TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start,
        TextUnit_Character, UIA_TextPattern2Id, UIA_TextPatternId,
    };
    use windows::Win32::UI::HiDpi::{
        GetWindowDpiAwarenessContext, LogicalToPhysicalPointForPerMonitorDPI,
        SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetGUIThreadInfo, GetWindowThreadProcessId, GUITHREADINFO,
    };

    /// The foreground thread's system caret, in physical screen pixels. Never sends messages.
    pub fn system_caret() -> Option<CaretSpot> {
        let (foreground, _) = focus::foreground();
        if foreground.is_invalid() {
            return None;
        }
        // SAFETY: plain Win32 queries with valid out-pointers; none of them send messages. The
        // thread DPI context is restored before returning.
        unsafe {
            let tid = GetWindowThreadProcessId(foreground, None);
            let mut info = GUITHREADINFO {
                cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
                ..Default::default()
            };
            if tid == 0 || GetGUIThreadInfo(tid, &mut info).is_err() {
                return None;
            }
            let hwnd = info.hwndCaret;
            let rc = info.rcCaret;
            if hwnd.is_invalid() || rc.bottom <= rc.top {
                return None;
            }
            // rcCaret is in the caret window's client coordinates, in that window's own DPI
            // space. Map it to screen coordinates in that space, then to physical pixels.
            let mut top = POINT {
                x: rc.left,
                y: rc.top,
            };
            let mut bottom = POINT {
                x: rc.left,
                y: rc.bottom,
            };
            let previous = SetThreadDpiAwarenessContext(GetWindowDpiAwarenessContext(hwnd));
            let mapped = ClientToScreen(hwnd, &mut top).as_bool()
                && ClientToScreen(hwnd, &mut bottom).as_bool();
            if !previous.is_invalid() {
                SetThreadDpiAwarenessContext(previous);
            }
            if !mapped {
                return None;
            }
            // No-ops for per-monitor aware windows; they fail harmlessly when already physical.
            let _ = LogicalToPhysicalPointForPerMonitorDPI(hwnd, &mut top);
            let _ = LogicalToPhysicalPointForPerMonitorDPI(hwnd, &mut bottom);
            Some(CaretSpot {
                x: top.x,
                top: top.y,
                bottom: bottom.y,
                word: None,
            })
        }
    }

    /// True when the focused window is a browser engine or XAML/WPF host, where only UI
    /// Automation knows the caret. Class reads only; never sends messages.
    pub fn focus_needs_uia() -> bool {
        let (foreground, _) = focus::foreground();
        focus::inspect_window(focus::focus_window(foreground)).0 == FieldKind::Probe
    }

    struct Request {
        id: u64,
        word_chars: usize,
        trailing_chars: usize,
    }

    /// Answer to [`CaretProbe::request`]. `spot` is `None` when UIA had no caret for the field.
    #[derive(Debug, Clone, Copy)]
    pub struct CaretReply {
        pub id: u64,
        pub spot: Option<CaretSpot>,
    }

    /// Handle to the `unflub-caret` UIA worker. Dropping it stops the worker after its current
    /// query.
    pub struct CaretProbe {
        requests: Sender<Request>,
    }

    impl CaretProbe {
        /// Start the worker. It does not wait for UI Automation to load: requests queue up
        /// meanwhile. `reply` is called on the worker thread with each answer.
        pub fn start(reply: Box<dyn Fn(CaretReply) + Send>) -> Result<Self, String> {
            let (tx, rx) = mpsc::channel();
            std::thread::Builder::new()
                .name("unflub-caret".into())
                .spawn(move || worker(rx, reply))
                .map_err(|e| format!("can't start the caret thread: {e}"))?;
            Ok(Self { requests: tx })
        }

        /// Ask for the caret and the span of the word `trailing_chars` characters before it.
        /// False if the worker is gone (UIA unavailable).
        pub fn request(&self, id: u64, word_chars: usize, trailing_chars: usize) -> bool {
            self.requests
                .send(Request {
                    id,
                    word_chars,
                    trailing_chars,
                })
                .is_ok()
        }
    }

    fn worker(rx: Receiver<Request>, reply: Box<dyn Fn(CaretReply) + Send>) {
        // SAFETY: thread-level DPI awareness only; UIA then reports physical pixels.
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        // SAFETY: balanced by CoUninitialize below; this thread never enters an STA.
        if unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_err() {
            return;
        }
        if let Ok((uia, _warnings)) = focus::create_uia() {
            while let Ok(mut req) = rx.recv() {
                // Only the latest correction matters.
                while let Ok(r) = rx.try_recv() {
                    req = r;
                }
                let spot = query(&uia, req.word_chars, req.trailing_chars);
                reply(CaretReply { id: req.id, spot });
            }
        }
        // SAFETY: matches the successful CoInitializeEx on this thread; `uia` is dropped.
        unsafe { CoUninitialize() };
    }

    fn query(uia: &IUIAutomation, word_chars: usize, trailing_chars: usize) -> Option<CaretSpot> {
        // SAFETY: COM calls on the MTA thread that created `uia`; the UIA timeouts bound them.
        unsafe {
            let element = uia.GetFocusedElement().ok()?;
            let caret = caret_range(&element)?;
            let back = |chars: usize| -(i32::try_from(chars).unwrap_or(i32::MAX));
            let word = caret.Clone().ok().and_then(|r| {
                r.MoveEndpointByUnit(
                    TextPatternRangeEndpoint_Start,
                    TextUnit_Character,
                    back(word_chars + trailing_chars),
                )
                .ok()?;
                r.MoveEndpointByUnit(
                    TextPatternRangeEndpoint_End,
                    TextUnit_Character,
                    back(trailing_chars),
                )
                .ok()?;
                spot_from_rects(&rects(&r), true)
            });
            if word.is_some() {
                return word;
            }
            // Some providers give no rectangles for a word on another line; the character
            // just before the caret still tells us the caret's line.
            let r = caret.Clone().ok()?;
            r.MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Character, -1)
                .ok()?;
            spot_from_rects(&rects(&r), false)
        }
    }

    /// The degenerate range at the caret: `TextPattern2` when the app has it, else the first
    /// selection range of `TextPattern`.
    unsafe fn caret_range(element: &IUIAutomationElement) -> Option<IUIAutomationTextRange> {
        if let Ok(p2) = element.GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id)
        {
            let mut active = Default::default();
            if let Ok(r) = p2.GetCaretRange(&mut active) {
                return Some(r);
            }
        }
        let p = element
            .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            .ok()?;
        let selection = p.GetSelection().ok()?;
        if selection.Length().ok()? < 1 {
            return None;
        }
        let r = selection.GetElement(0).ok()?;
        // Collapse to the end of the selection, where the caret is.
        r.MoveEndpointByRange(
            TextPatternRangeEndpoint_Start,
            &r,
            TextPatternRangeEndpoint_End,
        )
        .ok()?;
        Some(r)
    }

    /// `GetBoundingRectangles` as a flat `[left, top, width, height, ...]` list.
    unsafe fn rects(range: &IUIAutomationTextRange) -> Vec<f64> {
        let Ok(sa) = range.GetBoundingRectangles() else {
            return Vec::new();
        };
        if sa.is_null() {
            return Vec::new();
        }
        let mut out = Vec::new();
        if let (Ok(lo), Ok(hi)) = (SafeArrayGetLBound(sa, 1), SafeArrayGetUBound(sa, 1)) {
            let mut data = std::ptr::null_mut();
            if hi >= lo && SafeArrayAccessData(sa, &mut data).is_ok() {
                let n = (hi - lo + 1) as usize;
                // SAFETY: a VT_R8 array of n elements, locked by SafeArrayAccessData.
                out.extend_from_slice(std::slice::from_raw_parts(data as *const f64, n));
                let _ = SafeArrayUnaccessData(sa);
            }
        }
        let _ = SafeArrayDestroy(sa);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimated_span_ends_before_the_boundary() {
        // 20 px line: 7.4 px per letter, 4 px per space. "the" then one space typed after it.
        let (l, r) = estimate_word_span(500, 20, 3, 1);
        assert_eq!(r, 496);
        assert_eq!(l, 474);
        // Undo leaves the caret right after the word.
        assert_eq!(estimate_word_span(500, 20, 3, 0), (478, 500));
    }

    #[test]
    fn estimated_span_scales_with_line_height() {
        let (l1, r1) = estimate_word_span(1000, 20, 5, 0);
        let (l2, r2) = estimate_word_span(1000, 30, 5, 0);
        assert_eq!((r1, r2), (1000, 1000));
        let (w1, w2) = (r1 - l1, r2 - l2);
        assert!((w2 as f32 / w1 as f32 - 1.5).abs() < 0.05, "{w1} {w2}");
    }

    #[test]
    fn silly_line_heights_are_clamped() {
        assert_eq!(
            estimate_word_span(100, 1, 1, 0),
            estimate_word_span(100, 8, 1, 0)
        );
        assert_eq!(
            estimate_word_span(5000, 5000, 1, 0),
            estimate_word_span(5000, 400, 1, 0)
        );
        // An empty word still gets a visible span.
        let (l, r) = estimate_word_span(100, 20, 0, 0);
        assert!(r > l);
    }

    #[test]
    fn exact_word_span_wins_over_the_estimate() {
        let spot = CaretSpot {
            x: 300,
            top: 10,
            bottom: 30,
            word: Some((250, 290)),
        };
        assert_eq!(spot.word_span(3, 1), (250, 290));
        let spot = CaretSpot { word: None, ..spot };
        assert_eq!(spot.word_span(3, 0), estimate_word_span(300, 20, 3, 0));
    }

    #[test]
    fn uia_rects_use_the_last_line() {
        // Word wrapped over two lines: the caret is on the second.
        let rects = [10.0, 20.0, 50.0, 18.0, 5.0, 40.0, 30.4, 18.0];
        assert_eq!(
            spot_from_rects(&rects, true),
            Some(CaretSpot {
                x: 35,
                top: 40,
                bottom: 58,
                word: Some((5, 35)),
            })
        );
        let caret_only = spot_from_rects(&rects[..4], false).unwrap();
        assert_eq!(caret_only.word, None);
        assert_eq!(caret_only.x, 60);
    }

    #[test]
    fn empty_or_degenerate_uia_rects_mean_unknown() {
        assert_eq!(spot_from_rects(&[], true), None);
        assert_eq!(spot_from_rects(&[1.0, 2.0, 3.0], true), None);
        assert_eq!(spot_from_rects(&[1.0, 2.0, 3.0, 0.0], true), None);
        assert_eq!(spot_from_rects(&[f64::NAN, 2.0, 3.0, 4.0], true), None);
    }
}
