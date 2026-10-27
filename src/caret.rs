//! Where the caret is in a program that keeps no Win32 caret of its own
//! (Chromium and Electron: browsers, VS Code; WPF, UWP), for the badge:
//! the caret object of the focus window through MSAA (`OBJID_CARET`,
//! which Chromium exposes once accessibility is on), else UI Automation:
//! the caret range of the focused element (`TextPattern2`), else the end
//! of its selection (`TextPattern`). Asked for a caret just placed
//! (`strict`), only an element that takes text counts (an edit control, or
//! a value that is not read-only), so a click on a page or a list shows
//! nothing.
//!
//! These calls cross into the other process and may take a while, so they
//! run on a thread of their own ([`spawn`]), never on the hook thread,
//! with short timeouts; the answer is handed back to it.

use std::sync::mpsc::{Sender, channel};

use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::UI::Accessibility::{
    IUIAutomation2, IUIAutomationTextRange, TextPatternRangeEndpoint_End,
    TextPatternRangeEndpoint_Start, TextUnit_Character,
};
use windows_sys::Win32::Foundation::RECT;

/// How long a call into the other program may take.
const TIMEOUT_MS: u32 = 300;

/// Start the thread that answers where the caret is: each question sent
/// (a number, and whether it is `strict`) is answered with
/// `answer(number, place)` (only the latest of several sent at once). UI Automation is made at the first question, so a badge
/// that is off costs nothing. `None` when the thread could not be started.
pub fn spawn(answer: impl Fn(u64, Option<RECT>) + Send + 'static) -> Option<Sender<(u64, bool)>> {
    let (requests, received) = channel::<(u64, bool)>();
    std::thread::Builder::new()
        .name("caret".into())
        .spawn(move || {
            let mut finder = None;
            while let Ok(mut question) = received.recv() {
                while let Ok(later) = received.try_recv() {
                    question = later;
                }
                let (id, strict) = question;
                let finder = finder.get_or_insert_with(Finder::new);
                answer(id, finder.as_ref().and_then(|f| f.caret(strict)));
            }
        })
        .ok()?;
    Some(requests)
}

/// UI Automation, kept for the thread's life (COM initialised on it).
pub struct Finder {
    automation: IUIAutomation2,
}

impl Finder {
    pub fn new() -> Option<Finder> {
        use windows::Win32::System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        };
        use windows::Win32::UI::Accessibility::CUIAutomation8;
        // SAFETY: COM stays initialised on this thread for its life.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let automation: IUIAutomation2 =
                CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER).ok()?;
            automation.SetConnectionTimeout(TIMEOUT_MS).ok()?;
            automation.SetTransactionTimeout(TIMEOUT_MS).ok()?;
            Some(Finder { automation })
        }
    }

    /// The caret of the window in front, in screen pixels: a rectangle
    /// one pixel wide, as tall as the line. `strict`: only in an element
    /// that takes text.
    pub fn caret(&self, strict: bool) -> Option<RECT> {
        msaa_caret().or_else(|| self.uia_caret(strict))
    }

    pub fn uia_caret(&self, strict: bool) -> Option<RECT> {
        use windows::Win32::UI::Accessibility::{
            IUIAutomationTextPattern, IUIAutomationTextPattern2, UIA_TextPattern2Id,
            UIA_TextPatternId,
        };
        use windows::core::Interface;
        // SAFETY: every interface comes from a successful call.
        unsafe {
            let element = self.automation.GetFocusedElement().ok()?;
            if strict && !takes_text(&element) {
                return None;
            }
            if let Ok(pattern) = element.GetCurrentPattern(UIA_TextPattern2Id)
                && let Ok(pattern) = pattern.cast::<IUIAutomationTextPattern2>()
            {
                let mut active = windows::core::BOOL(0);
                if let Ok(range) = pattern.GetCaretRange(&mut active)
                    && let Some(rect) = caret_of_range(&range)
                {
                    return Some(rect);
                }
            }
            let pattern = element
                .GetCurrentPattern(UIA_TextPatternId)
                .ok()?
                .cast::<IUIAutomationTextPattern>()
                .ok()?;
            let ranges = pattern.GetSelection().ok()?;
            if ranges.Length().ok()? < 1 {
                return None;
            }
            caret_of_range(&ranges.GetElement(0).ok()?)
        }
    }
}

/// Whether `element` takes text: its value is not read-only, or (with no
/// value) it is an edit control.
fn takes_text(element: &windows::Win32::UI::Accessibility::IUIAutomationElement) -> bool {
    use windows::Win32::UI::Accessibility::{
        IUIAutomationValuePattern, UIA_EditControlTypeId, UIA_ValuePatternId,
    };
    use windows::core::Interface;
    // SAFETY: every interface comes from a successful call.
    unsafe {
        let read_only = element
            .GetCurrentPattern(UIA_ValuePatternId)
            .ok()
            .and_then(|p| p.cast::<IUIAutomationValuePattern>().ok())
            .and_then(|p| p.CurrentIsReadOnly().ok());
        match read_only {
            Some(read_only) => !read_only.as_bool(),
            None => element
                .CurrentControlType()
                .is_ok_and(|t| t == UIA_EditControlTypeId),
        }
    }
}

/// The caret object of the focus window (`OBJID_CARET`), as MSAA has it;
/// not when it says it is invisible.
pub fn msaa_caret() -> Option<RECT> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Accessibility::{AccessibleObjectFromWindow, IAccessible};
    use windows::core::Interface;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GUITHREADINFO, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId,
    };
    const OBJID_CARET: i32 = -8;
    // SAFETY: plain Win32 calls; the interface comes from a successful call.
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.is_null() {
            return None;
        }
        let thread = GetWindowThreadProcessId(foreground, std::ptr::null_mut());
        let mut info: GUITHREADINFO = std::mem::zeroed();
        info.cbSize = size_of::<GUITHREADINFO>() as u32;
        let focus = if GetGUIThreadInfo(thread, &mut info) != 0 && !info.hwndFocus.is_null() {
            info.hwndFocus
        } else {
            foreground
        };
        let mut object = std::ptr::null_mut();
        AccessibleObjectFromWindow(
            HWND(focus),
            OBJID_CARET as u32,
            &IAccessible::IID,
            &mut object,
        )
        .ok()?;
        if object.is_null() {
            return None;
        }
        let caret = IAccessible::from_raw(object);
        const STATE_SYSTEM_INVISIBLE: i32 = 0x8000;
        let state = caret
            .get_accState(&VARIANT::from(0i32))
            .ok()
            .and_then(|s| i32::try_from(&s).ok())
            .unwrap_or(0);
        if state & STATE_SYSTEM_INVISIBLE != 0 {
            return None;
        }
        let (mut x, mut y, mut w, mut h) = (0, 0, 0, 0);
        caret
            .accLocation(&mut x, &mut y, &mut w, &mut h, &VARIANT::from(0i32))
            .ok()?;
        (h > 0 && (x != 0 || y != 0)).then_some(RECT {
            left: x,
            top: y,
            right: x + w.max(1),
            bottom: y + h,
        })
    }
}

/// The place of the caret at the end of `range` (a caret range is empty):
/// the range's own rectangle when it has one, else the right edge of the
/// character before it, else the left edge of the one after it.
fn caret_of_range(range: &IUIAutomationTextRange) -> Option<RECT> {
    // SAFETY: every interface comes from a successful call.
    unsafe {
        let caret = range.Clone().ok()?;
        caret
            .MoveEndpointByRange(
                TextPatternRangeEndpoint_Start,
                range,
                TextPatternRangeEndpoint_End,
            )
            .ok()?;
        if let Some(r) = rects(&caret).and_then(|r| r.last().copied()) {
            return Some(line_at(r.left, r));
        }
        let before = caret.Clone().ok()?;
        if before
            .MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Character, -1)
            .is_ok_and(|moved| moved != 0)
            && let Some(r) = rects(&before).and_then(|r| r.last().copied())
        {
            return Some(line_at(r.right, r));
        }
        let after = caret.Clone().ok()?;
        if after
            .MoveEndpointByUnit(TextPatternRangeEndpoint_End, TextUnit_Character, 1)
            .is_ok_and(|moved| moved != 0)
            && let Some(r) = rects(&after).and_then(|r| r.first().copied())
        {
            return Some(line_at(r.left, r));
        }
        None
    }
}

fn line_at(x: i32, r: RECT) -> RECT {
    RECT {
        left: x,
        top: r.top,
        right: x + 1,
        bottom: r.bottom,
    }
}

/// The rectangles a range covers on the screen (one per line), those of
/// no height left out.
fn rects(range: &IUIAutomationTextRange) -> Option<Vec<RECT>> {
    // SAFETY: the array comes from the call, is read within its bounds and
    // destroyed once.
    unsafe {
        let array: *mut SAFEARRAY = range.GetBoundingRectangles().ok()?;
        if array.is_null() {
            return None;
        }
        let data = (*array).pvData as *const f64;
        let count = (*array).rgsabound[0].cElements as usize;
        let mut found = Vec::new();
        if !data.is_null() {
            for v in std::slice::from_raw_parts(data, count).as_chunks::<4>().0 {
                if v[3] >= 1.0 {
                    found.push(RECT {
                        left: v[0].round() as i32,
                        top: v[1].round() as i32,
                        right: (v[0] + v[2]).round() as i32,
                        bottom: (v[1] + v[3]).round() as i32,
                    });
                }
            }
        }
        let _ = windows::Win32::System::Ole::SafeArrayDestroy(array);
        (!found.is_empty()).then_some(found)
    }
}
