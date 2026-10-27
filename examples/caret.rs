//! Where the caret is in the window in front, as the badge finds it
//! (`src/caret.rs`): the Win32 caret and the MSAA / UI Automation answer,
//! with the time each took, every second for a while.
//!
//! cargo run --example caret

#[allow(dead_code)] // spawn: the program's
#[path = "../src/caret.rs"]
mod caret;

use windows_sys::Win32::UI::WindowsAndMessaging::{
    GUITHREADINFO, GetClassNameW, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId,
};

#[link(name = "user32")]
unsafe extern "system" {
    fn SetProcessDpiAwarenessContext(context: isize) -> i32;
}

fn main() {
    // Per-monitor aware, as the program: screen pixels.
    unsafe { SetProcessDpiAwarenessContext(-4) };
    let finder = caret::Finder::new().expect("UI Automation");
    focused();
    let rounds: u32 = std::env::args()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(5);
    for _ in 0..rounds {
        unsafe {
            let foreground = GetForegroundWindow();
            let mut buf = [0u16; 128];
            let len = GetClassNameW(foreground, buf.as_mut_ptr(), 128);
            let class = String::from_utf16_lossy(&buf[..len.max(0) as usize]);
            let thread = GetWindowThreadProcessId(foreground, std::ptr::null_mut());
            let mut info: GUITHREADINFO = std::mem::zeroed();
            info.cbSize = size_of::<GUITHREADINFO>() as u32;
            GetGUIThreadInfo(thread, &mut info);
            let start = std::time::Instant::now();
            let found = finder.caret(false);
            let took = start.elapsed();
            let strict = finder.caret(true);
            let msaa = caret::msaa_caret();
            let uia = finder.uia_caret(false);
            println!(
                "  msaa {:?} uia {:?}",
                msaa.map(|r| (r.left, r.top, r.bottom)),
                uia.map(|r| (r.left, r.top, r.bottom))
            );
            println!(
                "{class}: win32 caret {:?} {:?} | found {:?} in {:?} | strict {:?}",
                info.hwndCaret,
                (info.rcCaret.left, info.rcCaret.top, info.rcCaret.bottom),
                found.map(|r| (r.left, r.top, r.bottom)),
                took,
                strict.map(|r| (r.left, r.top, r.bottom)),
            );
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// The focused element: its control type, name, and the caret object's
/// raw MSAA state and place.
fn focused() {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::Win32::UI::Accessibility::{CUIAutomation8, IUIAutomation};
    unsafe {
        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER).unwrap();
        match automation.GetFocusedElement() {
            Ok(e) => println!(
                "focused: type {:?} name {:?} class {:?}",
                e.CurrentControlType().map(|t| t.0),
                e.CurrentName().map(|n| n.to_string()),
                e.CurrentClassName().map(|n| n.to_string())
            ),
            Err(err) => println!("focused: {err}"),
        }
    }
}
