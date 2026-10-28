//! The badge with the layout's letters near the caret: shown for a moment
//! when the layout changes (`IndicatorMode::OnSwitch`), or kept beside the
//! caret as it moves (`Always`). A small layered pop-up window that never
//! takes the focus and lets clicks through, owned by the hook thread, which
//! learns where the caret is from `EVENT_OBJECT_LOCATIONCHANGE` for
//! `OBJID_CARET` (`GetGUIThreadInfo` of the event's thread: no call into
//! the other process). A program that keeps no Win32 caret (Chromium and
//! Electron, WPF, UWP) is asked through accessibility on the caret thread
//! (`caret.rs`), and the badge waits for its answer a moment; without any,
//! the badge appears beside the mouse pointer. With `on_caret`, the
//! badge is shown for a moment also when the caret is placed (a left click,
//! the focus moved, another window in front), at the caret only: no caret
//! found then, no badge.

use std::cell::RefCell;
use std::sync::mpsc::Sender;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CLIP_DEFAULT_PRECIS, ClientToScreen, CreateCompatibleDC, CreateFontW,
    CreateRoundRectRgn, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER, DT_SINGLELINE,
    DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, EndPaint, FF_DONTCARE, FW_BOLD, FillRect,
    FrameRect, GetMonitorInfoW, GetTextExtentPoint32W, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint, OUT_DEFAULT_PRECIS, PAINTSTRUCT, PROOF_QUALITY, SelectObject, SetBkMode,
    SetTextColor, SetWindowRgn, TRANSPARENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GUITHREADINFO, GetCursorPos,
    GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, HWND_TOPMOST, KillTimer,
    LWA_ALPHA, RegisterClassW, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetLayeredWindowAttributes,
    SetTimer, SetWindowPos, ShowWindow, WM_PAINT, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::settings::IndicatorMode;

/// The badge's transparency (255: opaque).
const ALPHA: u8 = 235;
/// How long the badge waits for the caret thread before it goes beside
/// the pointer.
const ASK_MS: u64 = 400;
/// The timer (of the badge's window) that looks for the caret after it was
/// placed; the hook thread's loop calls [`Indicator::placed_due`] on it.
pub const PLACED_TIMER: usize = 1;
/// How long after a click or a focus change the caret is looked for: the
/// program places it first.
const PLACED_MS: u32 = 60;

/// A question to the caret thread the badge waits for.
#[derive(Clone, Copy)]
struct Asked {
    id: u64,
    /// When to stop waiting.
    until: u64,
    /// For a caret just placed: no answer, no badge (not beside the
    /// pointer).
    placed: bool,
}

/// A caret placed, to be looked for when [`PLACED_TIMER`] fires.
#[derive(Clone, Copy)]
struct Placed {
    /// By a click: the caret before it, shown only if it moved.
    click: bool,
    before: Option<RECT>,
}

/// The badge's settings (`Settings::indicator_config`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub mode: IndicatorMode,
    /// How long the badge stays when shown for a moment.
    pub duration_ms: u64,
    /// Point size of the letters.
    pub points: u32,
    /// The badge's colour for each layout of the pair; `None`: grey as
    /// the taskbar.
    pub colors: Option<[[u8; 3]; 2]>,
    /// Shown for a moment when the caret is placed (`OnSwitch` mode).
    pub on_caret: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            mode: IndicatorMode::Off,
            duration_ms: 700,
            points: 10,
            colors: None,
            on_caret: true,
        }
    }
}

/// What the window procedure paints with (the window's thread only).
struct Paint {
    letters: Vec<u16>,
    light: bool,
    /// The layout's colour (`COLORREF`), else grey after `light`.
    color: Option<u32>,
    font: isize,
}

thread_local! {
    static PAINT: RefCell<Paint> = const { RefCell::new(Paint { letters: Vec::new(), light: false, color: None, font: 0 }) };
}

pub struct Indicator {
    hwnd: isize,
    config: Config,
    letters: String,
    /// The layout's place in the pair, for its colour.
    index: Option<u8>,
    /// The caret's place on the screen, as last reported for the window in
    /// front; forgotten when another window comes to the front.
    caret: Option<RECT>,
    /// When to hide, in `OnSwitch` mode (`GetTickCount64`).
    hide_at: Option<u64>,
    visible: bool,
    /// The DPI and the point size the font was made for.
    font_for: (u32, u32),
    /// The caret thread ([`crate::caret::spawn`]).
    finder: Option<Sender<crate::caret::Ask>>,
    /// The question to it the badge waits for.
    asked: Option<Asked>,
    /// A caret placed, looked for when [`PLACED_TIMER`] fires.
    placed: Option<Placed>,
    /// Questions asked so far (their numbers).
    questions: u64,
}

impl Indicator {
    /// The window, hidden; `None` if it could not be made.
    pub fn new() -> Option<Indicator> {
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        let class = crate::win::wide("qswitch_indicator");
        // SAFETY: a window class and a window of this thread.
        let hwnd = unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let wc = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                ..std::mem::zeroed()
            };
            RegisterClassW(&wc);
            CreateWindowExW(
                WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE
                    | WS_EX_LAYERED
                    | WS_EX_TRANSPARENT,
                class.as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                0,
                0,
                10,
                10,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            log::warn!(
                "indicator window not made: {}",
                std::io::Error::last_os_error()
            );
            return None;
        }
        unsafe { SetLayeredWindowAttributes(hwnd, 0, ALPHA, LWA_ALPHA) };
        Some(Indicator {
            hwnd: hwnd as isize,
            config: Config::default(),
            letters: String::new(),
            index: None,
            caret: None,
            hide_at: None,
            visible: false,
            font_for: (0, 0),
            finder: None,
            asked: None,
            placed: None,
            questions: 0,
        })
    }

    /// The thread that finds a caret that is not a Win32 one.
    pub fn set_finder(&mut self, finder: Option<Sender<crate::caret::Ask>>) {
        self.finder = finder;
    }

    pub fn configure(&mut self, config: Config) {
        let look_changed = config.points != self.config.points
            || config.colors != self.config.colors
            || config.duration_ms != self.config.duration_ms;
        self.config = config;
        self.refresh_paint();
        if self.config.mode != IndicatorMode::Always {
            self.hide();
        }
        // A new look is shown at once, beside the pointer (on the control
        // that set it), so it can be judged while being chosen.
        if look_changed {
            self.preview();
        }
    }

    /// The badge beside the pointer for a moment (the window's "Show"
    /// button, a changed look), whatever the mode but off.
    pub fn preview(&mut self) {
        if self.config.mode == IndicatorMode::Off || self.letters.is_empty() {
            return;
        }
        self.hide_at = Some(now() + self.config.duration_ms.max(1000));
        self.show_at(None);
    }

    /// The layout changed: the badge shows its letters (for a moment, or
    /// from now on); `index` is its place in the pair; `flash` is false
    /// for the first layout seen at start.
    pub fn set_letters(&mut self, letters: &str, index: Option<u8>, flash: bool) {
        self.letters = letters.to_owned();
        self.index = index;
        self.refresh_paint();
        match self.config.mode {
            IndicatorMode::Off => {}
            IndicatorMode::OnSwitch if flash => {
                self.hide_at = Some(now() + self.config.duration_ms);
                self.show();
            }
            IndicatorMode::OnSwitch => {}
            IndicatorMode::Always => self.show(),
        }
    }

    /// What the window procedure paints: the letters, the colour of the
    /// layout (or the taskbar's shade).
    fn refresh_paint(&self) {
        let color = match (self.config.colors, self.index) {
            (Some(colors), Some(i)) => {
                let [r, g, b] = colors[i as usize];
                Some(u32::from(r) | u32::from(g) << 8 | u32::from(b) << 16)
            }
            _ => None,
        };
        PAINT.with_borrow_mut(|p| {
            p.letters = self.letters.encode_utf16().collect();
            p.light = crate::tray::taskbar_is_light();
            p.color = color;
        });
    }

    /// The caret of the window in front moved to `rect` (screen pixels).
    pub fn caret_moved(&mut self, rect: RECT) {
        let moved = !self.caret.is_some_and(|c| same(c, rect));
        self.caret = Some(rect);
        if moved && (self.config.mode == IndicatorMode::Always || self.visible) {
            self.show();
        }
    }

    /// A caret event with no Win32 caret behind it (Chromium's, once its
    /// accessibility is on): the caret thread is asked anew when the badge
    /// follows the caret.
    pub fn caret_moved_elsewhere(&mut self) {
        if self.config.mode == IndicatorMode::Always || self.visible {
            self.ask(false);
        }
    }

    /// The caret thread's answer to question `id`.
    pub fn caret_found(&mut self, id: u64, caret: Option<RECT>) {
        let Some(asked) = self.asked.filter(|a| a.id == id) else {
            return;
        };
        self.asked = None;
        if asked.placed {
            match caret {
                Some(caret) => self.placed_at(caret),
                // The focus went to something that takes no text: in the
                // `Always` mode the badge leaves with the caret, instead of
                // staying where the caret was.
                None if self.config.mode == IndicatorMode::Always => {
                    self.caret = None;
                    self.hide();
                }
                None => {}
            }
        } else {
            self.show_at(caret);
        }
    }

    /// The caret may have been placed: a left click (`click`), the focus
    /// moved, another window in front. It is looked for a moment later
    /// ([`PLACED_TIMER`]), and the badge flashes there (`OnSwitch` with
    /// `on_caret`), or settles there (`Always`: a program that reports no
    /// caret event until a key is typed, such as a browser's field after
    /// the window came back, would leave the badge hidden otherwise).
    pub fn caret_placed(&mut self, click: bool) {
        let wanted = match self.config.mode {
            IndicatorMode::Off => false,
            IndicatorMode::OnSwitch => self.config.on_caret,
            IndicatorMode::Always => true,
        };
        if !wanted || self.letters.is_empty() {
            return;
        }
        let placed = match self.placed {
            // A click followed by a focus change: the caret is new.
            Some(p) => Placed {
                click: p.click && click,
                before: p.before,
            },
            None => Placed {
                click,
                before: if click { foreground_caret() } else { None },
            },
        };
        self.placed = Some(placed);
        unsafe { SetTimer(self.hwnd as _, PLACED_TIMER, PLACED_MS, None) };
    }

    /// [`PLACED_TIMER`] fired: the badge flashes at the caret placed, if
    /// there is one (a Win32 caret that moved, or one the caret thread
    /// finds).
    pub fn placed_due(&mut self) {
        unsafe { KillTimer(self.hwnd as _, PLACED_TIMER) };
        let Some(placed) = self.placed.take() else {
            return;
        };
        match foreground_caret() {
            Some(caret) if placed.click && placed.before.is_some_and(|b| same(b, caret)) => {}
            Some(caret) => {
                self.caret = Some(caret);
                self.placed_at(caret);
            }
            None => self.ask(true),
        }
    }

    /// The badge at a caret just placed: for a moment, or from now on in
    /// the `Always` mode.
    fn placed_at(&mut self, caret: RECT) {
        if self.config.mode == IndicatorMode::Always {
            self.show_at(Some(caret));
        } else {
            self.flash_at(caret);
        }
    }

    /// The badge at `caret` for a moment.
    fn flash_at(&mut self, caret: RECT) {
        self.hide_at = Some(now() + self.config.duration_ms);
        self.show_at(Some(caret));
    }

    /// Another window is in front: its caret is not known yet.
    pub fn foreground_changed(&mut self) {
        self.caret = None;
        if self.config.mode == IndicatorMode::Always {
            self.hide();
        }
    }

    /// Every 150 ms: hide once the moment has passed.
    pub fn tick(&mut self) {
        if let Some(at) = self.hide_at
            && now() >= at
        {
            self.hide_at = None;
            self.hide();
        }
        if let Some(asked) = self.asked
            && now() >= asked.until
        {
            // No answer in time: beside the pointer (for a caret just
            // placed, nothing).
            self.asked = None;
            if !asked.placed {
                self.show_at(None);
            }
        }
    }

    /// The badge at the caret: the Win32 one, known from its events or
    /// asked of the window in front now, else (once answered) the one the
    /// caret thread finds.
    fn show(&mut self) {
        if self.letters.is_empty() {
            return;
        }
        if self.caret.is_none() {
            self.caret = foreground_caret();
        }
        match self.caret {
            Some(caret) => self.show_at(Some(caret)),
            None => self.ask(false),
        }
    }

    /// Ask the caret thread where the caret is; the badge is shown on the
    /// answer, or beside the pointer when there is no thread (for a caret
    /// just placed, `placed`, only at a caret found).
    fn ask(&mut self, placed: bool) {
        let sent = self.finder.as_ref().map(|f| {
            self.questions += 1;
            f.send(crate::caret::Ask::Caret {
                id: self.questions,
                strict: placed,
            })
            .is_ok()
        });
        if sent == Some(true) {
            self.asked = Some(Asked {
                id: self.questions,
                until: now() + ASK_MS,
                placed,
            });
            return;
        }
        self.finder = None;
        if !placed {
            self.show_at(None);
        }
    }

    /// The badge at `caret` (else beside the mouse pointer), sized for the
    /// letters and the DPI of the window in front, shown without taking
    /// the focus.
    fn show_at(&mut self, caret: Option<RECT>) {
        if self.letters.is_empty() {
            return;
        }
        // SAFETY: GDI objects of this thread; the window is this thread's.
        unsafe {
            let foreground = GetForegroundWindow();
            let dpi = if foreground.is_null() {
                96
            } else {
                GetDpiForWindow(foreground)
            }
            .max(96);
            let scale = dpi as f32 / 96.0;
            // Paddings and the corners grow with the letters: at 10 pt and
            // 96 dpi, 5 px at the sides, 2 px above and below, 6 px corners.
            let points = self.config.points;
            let em = points as f32 * scale / 10.0;
            if self.font_for != (dpi, points) {
                let old = PAINT.with_borrow(|p| p.font);
                if old != 0 {
                    DeleteObject(old as _);
                }
                let face = crate::win::wide("Segoe UI");
                let font = CreateFontW(
                    -(points as i32 * dpi as i32 / 72),
                    0,
                    0,
                    0,
                    FW_BOLD as i32,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET as u32,
                    OUT_DEFAULT_PRECIS as u32,
                    CLIP_DEFAULT_PRECIS as u32,
                    PROOF_QUALITY as u32,
                    (DEFAULT_PITCH | FF_DONTCARE) as u32,
                    face.as_ptr(),
                );
                PAINT.with_borrow_mut(|p| p.font = font as isize);
                self.font_for = (dpi, points);
            }
            // The letters' size.
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            let font = PAINT.with_borrow(|p| p.font);
            let old = SelectObject(dc, font as _);
            let text: Vec<u16> = self.letters.encode_utf16().collect();
            let mut size = SIZE { cx: 0, cy: 0 };
            GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32, &mut size);
            SelectObject(dc, old);
            DeleteDC(dc);
            let width = size.cx + (10.0 * em) as i32;
            let height = size.cy + (4.0 * em) as i32;
            // Below and right of the caret, else beside the pointer.
            let gap = (4.0 * scale) as i32;
            let (mut x, mut y, above) = match caret {
                Some(c) => (c.left, c.bottom + gap, c.top - gap - height),
                None => {
                    let mut p = POINT { x: 0, y: 0 };
                    GetCursorPos(&mut p);
                    (
                        p.x + (16.0 * scale) as i32,
                        p.y + (20.0 * scale) as i32,
                        p.y - gap - height,
                    )
                }
            };
            // Kept on the monitor's work area.
            let point = POINT { x, y };
            let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = size_of::<MONITORINFO>() as u32;
            if !monitor.is_null() && GetMonitorInfoW(monitor, &mut info) != 0 {
                let work = info.rcWork;
                if y + height > work.bottom {
                    y = above;
                }
                x = x.clamp(work.left, (work.right - width).max(work.left));
                y = y.clamp(work.top, (work.bottom - height).max(work.top));
            }
            let region = CreateRoundRectRgn(
                0,
                0,
                width + 1,
                height + 1,
                (6.0 * em) as i32,
                (6.0 * em) as i32,
            );
            SetWindowRgn(self.hwnd as _, region, 1);
            SetWindowPos(
                self.hwnd as _,
                HWND_TOPMOST,
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            // Painted anew: the letters may have changed at the same size.
            windows_sys::Win32::Graphics::Gdi::InvalidateRect(self.hwnd as _, std::ptr::null(), 1);
        }
        self.visible = true;
    }

    fn hide(&mut self) {
        self.asked = None;
        if self.visible {
            unsafe { ShowWindow(self.hwnd as _, SW_HIDE) };
            self.visible = false;
        }
    }
}

impl Drop for Indicator {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.hwnd as _) };
        let font = PAINT.with_borrow_mut(|p| std::mem::take(&mut p.font));
        if font != 0 {
            unsafe { DeleteObject(font as _) };
        }
    }
}

/// The caret of the thread `thread` (the one of a caret event) as a screen
/// rectangle, if the thread has a blinking caret in a window.
pub fn caret_of_thread(thread: u32) -> Option<RECT> {
    const GUI_CARETBLINKING: u32 = 0x0001;
    let mut info: GUITHREADINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<GUITHREADINFO>() as u32;
    if unsafe { GetGUIThreadInfo(thread, &mut info) } == 0
        || info.flags & GUI_CARETBLINKING == 0
        || info.hwndCaret.is_null()
    {
        return None;
    }
    let mut top_left = POINT {
        x: info.rcCaret.left,
        y: info.rcCaret.top,
    };
    let mut bottom_right = POINT {
        x: info.rcCaret.right,
        y: info.rcCaret.bottom,
    };
    unsafe {
        ClientToScreen(info.hwndCaret, &mut top_left);
        ClientToScreen(info.hwndCaret, &mut bottom_right);
    }
    // A caret of no height is no caret.
    if bottom_right.y <= top_left.y {
        return None;
    }
    Some(RECT {
        left: top_left.x,
        top: top_left.y,
        right: bottom_right.x,
        bottom: bottom_right.y,
    })
}

/// `color` (a `COLORREF`) at three quarters of its brightness.
fn darker(color: u32) -> u32 {
    let channel = |shift: u32| (((color >> shift) & 0xFF) * 3 / 4) << shift;
    channel(0) | channel(8) | channel(16)
}

fn same(a: RECT, b: RECT) -> bool {
    a.left == b.left && a.top == b.top && a.right == b.right && a.bottom == b.bottom
}

/// The Win32 caret of the window in front, if it has one.
fn foreground_caret() -> Option<RECT> {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return None;
    }
    caret_of_thread(unsafe { GetWindowThreadProcessId(foreground, std::ptr::null_mut()) })
}

fn now() -> u64 {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetTickCount64() -> u64;
    }
    unsafe { GetTickCount64() }
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetDpiForWindow(hwnd: HWND) -> u32;
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg != WM_PAINT {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    // SAFETY: painting this window on its own thread.
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        PAINT.with_borrow(|p| {
            let (bg, border, text) = match p.color {
                // The layout's colour, a darker border, white letters.
                Some(c) => (c, darker(c), 0x00FF_FFFFu32),
                None if p.light => (0x00F3_F3F3u32, 0x00B8_B8B8u32, 0x001A_1A1Au32),
                None => (0x002D_2D2Du32, 0x0058_5858u32, 0x00F5_F5F5u32),
            };
            let brush = CreateSolidBrush(bg);
            FillRect(dc, &ps.rcPaint, brush);
            DeleteObject(brush as _);
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rect);
            let frame = CreateSolidBrush(border);
            FrameRect(dc, &rect, frame);
            DeleteObject(frame as _);
            let old = SelectObject(dc, p.font as _);
            SetBkMode(dc, TRANSPARENT as i32);
            SetTextColor(dc, text);
            DrawTextW(
                dc,
                p.letters.as_ptr(),
                p.letters.len() as i32,
                &mut rect,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
            SelectObject(dc, old);
        });
        EndPaint(hwnd, &ps);
    }
    0
}
