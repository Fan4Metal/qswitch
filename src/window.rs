//! The main window's subclass: the tray icon's clicks and menu, a close
//! while the window is hidden (`taskkill`, `build.py`, the installer: no
//! frame would act on it), the end of the session, Explorer starting
//! again (the icon is to be added anew), and commands handed over by a
//! later start (`instance.rs`). Everything is queued for the app, which
//! takes it in `App::logic` (called even while the window is hidden,
//! once woken with `request_repaint`). From metalamp's `win.rs`.

use std::sync::{Mutex, OnceLock};

use crate::tray::TrayCmd;

/// What a later start of the program asks for (its command line).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Remote {
    Show,
    Quit,
    Pause,
    Resume,
}

impl Remote {
    pub fn name(self) -> &'static str {
        match self {
            Remote::Show => "show",
            Remote::Quit => "quit",
            Remote::Pause => "pause",
            Remote::Resume => "resume",
        }
    }

    pub fn of_name(name: &str) -> Option<Remote> {
        [Remote::Show, Remote::Quit, Remote::Pause, Remote::Resume]
            .into_iter()
            .find(|r| r.name() == name)
    }
}

/// What the window's messages asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hooked {
    /// The tray icon clicked, or its menu's choice.
    Tray(TrayCmd),
    /// `WM_CLOSE` not from the window's own close button (`taskkill`,
    /// `build.py`, the installer), or while the window is hidden or
    /// minimized: the app ends itself (its settings are saved as they
    /// change). The close button and Alt+F4 come as `WM_SYSCOMMAND`
    /// `SC_CLOSE` first and are the app's to hide the window on.
    CloseNow,
    /// The window's close button or Alt+F4.
    CloseButton,
    /// Windows is ending the session or the Restart Manager (an installer)
    /// asks the program to close.
    EndSession,
    /// Explorer started (again): the tray icon is to be added anew.
    TrayCreated,
    /// A later start's command.
    Remote(Remote),
}

static HOOKED: Mutex<Vec<Hooked>> = Mutex::new(Vec::new());
static WAKE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
/// `TaskbarCreated`, registered by [`hook_window`].
static EXPLORER_STARTED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// A `--hidden` start: eframe shows the window after its first frame
/// whatever the builder said (and Windows sets `WS_VISIBLE` even when
/// `WM_WINDOWPOSCHANGING` drops `SWP_SHOWWINDOW`), so that show is made
/// without activation on a cloaked window and undone right after.
/// [`HIDDEN_WAIT`] until that show, [`HIDDEN_UNDO`] until it is undone
/// (`ShowWindow` activates with a second `SetWindowPos`, refused too).
static KEEP_HIDDEN: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
const HIDDEN_WAIT: u8 = 1;
const HIDDEN_UNDO: u8 = 2;
/// Posted by that first show: hide the window and uncloak it.
const WM_HIDE_AGAIN: u32 = 0x8000 + 0x52;

/// Keep the window, cloaked by the caller, hidden through eframe's show
/// after the first frame (before [`hook_window`]).
pub fn keep_hidden() {
    KEEP_HIDDEN.store(HIDDEN_WAIT, std::sync::atomic::Ordering::Relaxed);
}

/// Whether the program is paused, for the menu's label.
static PAUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The automatic switch, for the menu's check mark: 0 not available for
/// the pair, 1 off, 2 on.
static AUTO: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Subclass the window once; `wake` is called when something is queued.
pub fn hook_window(hwnd: isize, wake: impl Fn() + Send + Sync + 'static) {
    use std::sync::Once;
    use windows_sys::Win32::UI::Shell::SetWindowSubclass;
    use windows_sys::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW;
    static SUBCLASSED: Once = Once::new();
    SUBCLASSED.call_once(|| {
        let started = crate::win::wide("TaskbarCreated");
        let started = unsafe { RegisterWindowMessageW(started.as_ptr()) };
        EXPLORER_STARTED.store(started, std::sync::atomic::Ordering::Relaxed);
        let _ = WAKE.set(Box::new(wake));
        // A copy running elevated still hears Explorer (UIPI).
        #[link(name = "user32")]
        unsafe extern "system" {
            fn ChangeWindowMessageFilterEx(
                hwnd: windows_sys::Win32::Foundation::HWND,
                message: u32,
                action: u32,
                filter: *mut core::ffi::c_void,
            ) -> i32;
        }
        const MSGFLT_ALLOW: u32 = 1;
        if is_elevated() {
            for message in [started, crate::tray::WM_TRAY] {
                unsafe {
                    ChangeWindowMessageFilterEx(
                        hwnd as _,
                        message,
                        MSGFLT_ALLOW,
                        std::ptr::null_mut(),
                    )
                };
            }
        }
        unsafe { SetWindowSubclass(hwnd as _, Some(window_proc), 1, 0) };
    });
}

/// What the window's messages asked for since the last call.
pub fn take_hooked() -> Vec<Hooked> {
    HOOKED
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

/// Queue `what` for the app and wake it (from any thread).
pub fn hook(what: Hooked) {
    if let Ok(mut q) = HOOKED.lock() {
        q.push(what);
    }
    if let Some(wake) = WAKE.get() {
        wake();
    }
}

/// Tell the menu whether the program is paused.
pub fn set_paused(paused: bool) {
    PAUSED.store(paused, std::sync::atomic::Ordering::Relaxed);
}

/// Tell the menu whether the automatic switch is on (`None`: not
/// available for the pair of layouts).
pub fn set_auto(auto: Option<bool>) {
    let value = match auto {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    };
    AUTO.store(value, std::sync::atomic::Ordering::Relaxed);
}

/// Whether this process runs elevated (as administrator).
pub fn is_elevated() -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    static ELEVATED: OnceLock<bool> = OnceLock::new();
    *ELEVATED.get_or_init(|| unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            (&raw mut elevation).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    })
}

pub fn is_visible(hwnd: isize) -> bool {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd as _) != 0 }
}

pub fn is_minimized(hwnd: isize) -> bool {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsIconic(hwnd as _) != 0 }
}

/// The window shown, restored if minimized, and in front.
pub fn bring_back(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow,
    };
    unsafe {
        if is_minimized(hwnd) {
            ShowWindow(hwnd as _, SW_RESTORE);
        } else if !is_visible(hwnd) {
            ShowWindow(hwnd as _, SW_SHOW);
        }
        SetForegroundWindow(hwnd as _);
    }
}

/// The window hidden (to the tray).
pub fn hide(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};
    unsafe { ShowWindow(hwnd as _, SW_HIDE) };
}

unsafe extern "system" fn window_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
    _id: usize,
    _data: usize,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::Shell::DefSubclassProc;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        ENDSESSION_CLOSEAPP, PostMessageW, SC_CLOSE, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW,
        WINDOWPOS, WM_CLOSE, WM_ENDSESSION, WM_QUERYENDSESSION, WM_SYSCOMMAND,
        WM_WINDOWPOSCHANGING,
    };
    if msg != 0 && msg == EXPLORER_STARTED.load(std::sync::atomic::Ordering::Relaxed) {
        hook(Hooked::TrayCreated);
    }
    let keep_hidden = KEEP_HIDDEN.load(std::sync::atomic::Ordering::Relaxed);
    if msg == WM_WINDOWPOSCHANGING && keep_hidden != 0 && lparam != 0 {
        let pos = unsafe { &mut *(lparam as *mut WINDOWPOS) };
        if keep_hidden == HIDDEN_WAIT && pos.flags & SWP_SHOWWINDOW != 0 {
            // eframe's first show: hidden again at once.
            KEEP_HIDDEN.store(HIDDEN_UNDO, std::sync::atomic::Ordering::Relaxed);
            unsafe { PostMessageW(hwnd, WM_HIDE_AGAIN, 0, 0) };
        }
        if KEEP_HIDDEN.load(std::sync::atomic::Ordering::Relaxed) == HIDDEN_UNDO {
            // Neither the show nor the activation after it takes the
            // focus from the window in front.
            pos.flags |= SWP_NOACTIVATE | SWP_NOZORDER;
        }
    }
    if msg == WM_HIDE_AGAIN {
        hide(hwnd as isize);
        crate::caption::cloak(hwnd as isize, false);
        KEEP_HIDDEN.store(0, std::sync::atomic::Ordering::Relaxed);
        return 0;
    }
    if msg == crate::tray::WM_TRAY {
        const NIN_SELECT: u32 = 0x400;
        const NIN_KEYSELECT: u32 = 0x401;
        const WM_CONTEXTMENU: u32 = 0x007B;
        match lparam as u32 & 0xFFFF {
            // A click (or Enter on the icon): the layout, nothing else;
            // the window opens from the menu only.
            NIN_SELECT | NIN_KEYSELECT => crate::hook::toggle_layout(),
            WM_CONTEXTMENU => {
                let x = (wparam & 0xFFFF) as u16 as i16 as i32;
                let y = ((wparam >> 16) & 0xFFFF) as u16 as i16 as i32;
                let paused = PAUSED.load(std::sync::atomic::Ordering::Relaxed);
                let auto = match AUTO.load(std::sync::atomic::Ordering::Relaxed) {
                    0 => None,
                    1 => Some(false),
                    _ => Some(true),
                };
                match crate::tray::menu(hwnd as isize, x, y, paused, auto) {
                    Some(TrayCmd::Show) => {
                        bring_back(hwnd as isize);
                        hook(Hooked::Tray(TrayCmd::Show));
                    }
                    Some(cmd) => hook(Hooked::Tray(cmd)),
                    None => {}
                }
            }
            _ => {}
        }
        return 0;
    }
    match msg {
        // The close button and Alt+F4: the app decides (hide, or end).
        WM_SYSCOMMAND if wparam as u32 & 0xFFF0 == SC_CLOSE => {
            hook(Hooked::CloseButton);
            return 0;
        }
        // Anything else that closes the window asks the program to end.
        WM_CLOSE => {
            hook(Hooked::CloseNow);
            return 0;
        }
        WM_QUERYENDSESSION => {
            hook(Hooked::EndSession);
            if lparam as u32 & ENDSESSION_CLOSEAPP != 0 {
                return 1;
            }
        }
        WM_ENDSESSION if wparam != 0 => {
            hook(Hooked::EndSession);
        }
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}
