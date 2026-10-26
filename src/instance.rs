//! One copy of the program at a time: a second start hands its command
//! to the running one (show the window, quit, pause, resume) and exits.
//! Two copies would each convert the text, so the first one owns a named
//! mutex, and once its window exists also a message-only window of a
//! class with the same name, which takes the command in a `WM_COPYDATA`.
//! Both names come from the path of the exe, so an installed copy and a
//! development build do not take each other's commands (metalamp's
//! `instance.rs`, with commands instead of files).

use std::hash::{Hash, Hasher};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, FindWindowExW, HWND_MESSAGE, RegisterClassW, SMTO_ABORTIFHUNG,
    SMTO_BLOCK, SendMessageTimeoutW, WM_COPYDATA, WNDCLASSW,
};

use crate::win::wide;
use crate::window::{Hooked, Remote};

/// Marks a `WM_COPYDATA` as the program's (`COPYDATASTRUCT::dwData`).
const MAGIC: usize = 0x7173_7769;
/// How long a second start waits for the window of the first one, which
/// appears soon after its start.
const WAIT: Duration = Duration::from_secs(5);
/// How long the first start may take to answer.
const ANSWER_MS: u32 = 3000;

/// The copy that holds the mutex (dropped never: it lives as long as the
/// process).
static MUTEX: OnceLock<isize> = OnceLock::new();
static LISTENING: OnceLock<()> = OnceLock::new();

/// Name of the mutex and of the window class, the same for every start of
/// this exe.
fn name() -> String {
    let exe = std::env::current_exe().unwrap_or_default();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    exe.to_string_lossy().to_lowercase().hash(&mut hasher);
    format!("qswitch.{:016x}", hasher.finish())
}

/// What a start found.
#[derive(Debug, PartialEq, Eq)]
pub enum Start {
    /// This is the only copy: it goes on.
    First,
    /// A copy was running and took the command: this one exits.
    Delivered,
    /// A copy was running but did not answer; this one goes on anyway
    /// (for `Quit`: the running one is gone).
    NotAnswered,
}

/// Called at start-up with the command this start carries (`Show` for a
/// plain start). Whether this process is the first.
pub fn claim(command: Remote) -> Start {
    let name = name();
    let mutex_name = wide(format!("Local\\{name}"));
    let class = wide(&name);
    let deadline = Instant::now() + WAIT;
    loop {
        let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
        if mutex.is_null() {
            log::warn!("CreateMutexW failed: {}", std::io::Error::last_os_error());
            return Start::First;
        }
        if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
            let _ = MUTEX.set(mutex as isize);
            return if command == Remote::Quit {
                Start::NotAnswered
            } else {
                Start::First
            };
        }
        unsafe { CloseHandle(mutex) };
        let window = unsafe {
            FindWindowExW(
                HWND_MESSAGE,
                std::ptr::null_mut(),
                class.as_ptr(),
                std::ptr::null(),
            )
        };
        if !window.is_null() {
            if send(window, command) {
                if command == Remote::Quit {
                    // Wait for it to go, so that an installer may replace
                    // the exe.
                    while Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(100));
                        let mutex =
                            unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
                        let mine =
                            !mutex.is_null() && unsafe { GetLastError() } != ERROR_ALREADY_EXISTS;
                        if !mutex.is_null() {
                            unsafe { CloseHandle(mutex) };
                        }
                        if mine {
                            break;
                        }
                    }
                }
                return Start::Delivered;
            }
            return Start::NotAnswered;
        }
        if Instant::now() >= deadline {
            log::warn!("the program is running but its window was not found");
            return Start::NotAnswered;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Send `command` to the message window `to`; whether it was taken.
fn send(to: HWND, command: Remote) -> bool {
    let text: Vec<u16> = command.name().encode_utf16().collect();
    let data = COPYDATASTRUCT {
        dwData: MAGIC,
        cbData: (text.len() * 2) as u32,
        lpData: text.as_ptr() as *mut _,
    };
    let mut answer = 0usize;
    let ok = unsafe {
        SendMessageTimeoutW(
            to,
            WM_COPYDATA,
            0,
            &raw const data as LPARAM,
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            ANSWER_MS,
            &mut answer,
        )
    };
    if ok == 0 {
        log::warn!("the running copy did not answer");
        return false;
    }
    answer != 0
}

/// Make this process the one later starts hand their commands to: a
/// message-only window on the UI thread, whose messages the event loop
/// dispatches; the commands are queued through `window::hook`.
pub fn listen() {
    if LISTENING.set(()).is_err() {
        return;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn ChangeWindowMessageFilterEx(
            hwnd: HWND,
            message: u32,
            action: u32,
            filter: *mut core::ffi::c_void,
        ) -> i32;
    }
    const MSGFLT_ALLOW: u32 = 1;
    let class = wide(name());
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        if RegisterClassW(&wc) == 0 {
            log::warn!("RegisterClassW failed: {}", std::io::Error::last_os_error());
            return;
        }
        let window = CreateWindowExW(
            0,
            class.as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if window.is_null() {
            log::warn!(
                "CreateWindowExW failed: {}",
                std::io::Error::last_os_error()
            );
            return;
        }
        // A copy running elevated still takes commands from a plain one
        // (the installer's `--quit`).
        if crate::window::is_elevated() {
            ChangeWindowMessageFilterEx(window, WM_COPYDATA, MSGFLT_ALLOW, std::ptr::null_mut());
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg != WM_COPYDATA || lparam == 0 {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    let data = unsafe { &*(lparam as *const COPYDATASTRUCT) };
    if data.dwData != MAGIC || data.lpData.is_null() {
        return 0;
    }
    // Read as bytes: the copy the system makes need not be aligned.
    let bytes =
        unsafe { std::slice::from_raw_parts(data.lpData as *const u8, data.cbData as usize) };
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b))
        .collect();
    let Some(command) = Remote::of_name(&String::from_utf16_lossy(&units)) else {
        return 0;
    };
    crate::window::hook(Hooked::Remote(command));
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_have_names() {
        for r in [Remote::Show, Remote::Quit, Remote::Pause, Remote::Resume] {
            assert_eq!(Remote::of_name(r.name()), Some(r));
        }
        assert_eq!(Remote::of_name("open"), None);
        assert!(name().starts_with("qswitch."));
    }
}
