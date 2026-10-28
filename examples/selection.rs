//! What UI Automation says about the focused element and its selection,
//! as the gesture with nothing typed reads it (`src/selection.rs`):
//! the program in front, the element's type, name, class and framework,
//! the patterns it offers, and the text each of them gives for the
//! selection. Printed once a second while anything changed, for a while:
//! to see why the conversion of a selection does nothing in a program.
//!
//! cargo run --example selection [seconds]

#[allow(dead_code)]
#[path = "../src/selection.rs"]
mod selection;

/// `selection.rs` names the program's `settings::file_name`.
mod settings {
    pub fn file_name(path: &str) -> &str {
        path.rsplit(['\\', '/']).next().unwrap_or(path)
    }
}

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation, IUIAutomationElement, IUIAutomationLegacyIAccessiblePattern,
    IUIAutomationTextPattern, IUIAutomationTextPattern2, IUIAutomationValuePattern,
    UIA_LegacyIAccessiblePatternId, UIA_TextPattern2Id, UIA_TextPatternId, UIA_ValuePatternId,
};
use windows::core::Interface;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

fn main() {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(30);
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }
            .expect("UI Automation");
    let mut last = String::new();
    for _ in 0..seconds {
        let report = snapshot(&automation);
        if report != last {
            println!("{report}");
            println!("---");
            last = report;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

fn snapshot(automation: &IUIAutomation) -> String {
    let mut out = format!("front: {}\n", foreground_exe());
    let element = match unsafe { automation.GetFocusedElement() } {
        Ok(e) => e,
        Err(e) => return out + &format!("focused: {e}"),
    };
    out += &describe(&element);
    out += &format!("selected_text(): {:?}\n", selection::selected_text());
    out
}

fn describe(element: &IUIAutomationElement) -> String {
    let mut out = String::new();
    unsafe {
        out += &format!(
            "focused: type {:?} name {:?} class {:?} framework {:?} automation id {:?}\n",
            element.CurrentControlType().map(|t| t.0),
            element.CurrentName().map(|n| n.to_string()),
            element.CurrentClassName().map(|n| n.to_string()),
            element.CurrentFrameworkId().map(|n| n.to_string()),
            element.CurrentAutomationId().map(|n| n.to_string()),
        );
        match element
            .GetCurrentPattern(UIA_TextPatternId)
            .and_then(|p| p.cast::<IUIAutomationTextPattern>())
        {
            Ok(text) => {
                let selection = text.GetSelection().and_then(|ranges| {
                    let mut parts = Vec::new();
                    for i in 0..ranges.Length()? {
                        parts.push(ranges.GetElement(i)?.GetText(2000)?.to_string());
                    }
                    Ok(parts)
                });
                let whole = text
                    .DocumentRange()
                    .and_then(|r| r.GetText(200))
                    .map(|t| t.to_string());
                out += &format!(
                    "  TextPattern: selection {selection:?}\n  document starts {whole:?}\n"
                );
            }
            Err(e) => out += &format!("  TextPattern: none ({e})\n"),
        }
        match element
            .GetCurrentPattern(UIA_TextPattern2Id)
            .and_then(|p| p.cast::<IUIAutomationTextPattern2>())
        {
            Ok(text) => {
                let mut active = windows::core::BOOL::default();
                let caret = text
                    .GetCaretRange(&mut active)
                    .and_then(|r| r.GetText(200))
                    .map(|t| t.to_string());
                out += &format!("  TextPattern2: caret range {caret:?} active {active:?}\n");
            }
            Err(e) => out += &format!("  TextPattern2: none ({e})\n"),
        }
        match element
            .GetCurrentPattern(UIA_ValuePatternId)
            .and_then(|p| p.cast::<IUIAutomationValuePattern>())
        {
            Ok(value) => {
                out += &format!(
                    "  ValuePattern: {:?} read-only {:?}\n",
                    value.CurrentValue().map(|v| {
                        let v = v.to_string();
                        v.chars().take(200).collect::<String>()
                    }),
                    value.CurrentIsReadOnly()
                )
            }
            Err(e) => out += &format!("  ValuePattern: none ({e})\n"),
        }
        match element
            .GetCurrentPattern(UIA_LegacyIAccessiblePatternId)
            .and_then(|p| p.cast::<IUIAutomationLegacyIAccessiblePattern>())
        {
            Ok(legacy) => {
                out += &format!(
                    "  LegacyIAccessible: role {:?} state {:?} value {:?}\n",
                    legacy.CurrentRole(),
                    legacy.CurrentState().map(|s| format!("{s:#x}")),
                    legacy.CurrentValue().map(|v| v.to_string())
                )
            }
            Err(e) => out += &format!("  LegacyIAccessible: none ({e})\n"),
        }
    }
    out
}

fn foreground_exe() -> String {
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return format!("pid {pid}");
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut len);
        windows_sys::Win32::Foundation::CloseHandle(process);
        if ok == 0 {
            return format!("pid {pid}");
        }
        String::from_utf16_lossy(&buf[..len as usize])
    }
}
