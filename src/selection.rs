//! The text selected in the focused control, through UI Automation: the
//! gesture with nothing typed converts the selection instead. Typing then
//! replaces the selection in any editor, so no clipboard is touched and
//! no Ctrl+C is sent (which a terminal would take as an interrupt).
//!
//! UIA calls cross into the other process and may take a while, so this
//! runs on a thread of its own (`hook.rs`), never on the hook thread, with
//! short timeouts.

/// How long a call into the other program may take.
const TIMEOUT_MS: u32 = 500;
/// Longest selection converted, in UTF-16 units.
const MAX_LEN: i32 = 20_000;

/// The selected text of the focused element, `None` when there is none,
/// the element has no text pattern, or UIA fails.
#[cfg(windows)]
pub fn selected_text() -> Option<String> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation8, IUIAutomation2, IUIAutomationTextPattern, UIA_TextPatternId,
    };
    use windows::core::Interface;
    // SAFETY: COM initialised and released on this thread around the calls;
    // every interface pointer comes from a successful call and is dropped
    // before CoUninitialize.
    unsafe {
        let init = CoInitializeEx(None, COINIT_MULTITHREADED);
        let result = (|| -> windows::core::Result<Option<String>> {
            let automation: IUIAutomation2 =
                CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)?;
            automation.SetConnectionTimeout(TIMEOUT_MS)?;
            automation.SetTransactionTimeout(TIMEOUT_MS)?;
            let element = automation.GetFocusedElement()?;
            let pattern: IUIAutomationTextPattern =
                element.GetCurrentPattern(UIA_TextPatternId)?.cast()?;
            let ranges = pattern.GetSelection()?;
            let mut text = String::new();
            for i in 0..ranges.Length()? {
                let range = ranges.GetElement(i)?;
                text.push_str(&range.GetText(MAX_LEN)?.to_string());
            }
            Ok((!text.is_empty()).then_some(text))
        })();
        if init.is_ok() {
            CoUninitialize();
        }
        match result {
            Ok(text) => text,
            Err(e) => {
                log::debug!("no selection through UIA: {e}");
                None
            }
        }
    }
}

#[cfg(not(windows))]
pub fn selected_text() -> Option<String> {
    None
}

/// Programs where typing never replaces a selection and Ctrl+C means
/// something else: the selection is left alone there.
pub const TERMINALS: [&str; 10] = [
    "windowsterminal.exe",
    "conhost.exe",
    "openconsole.exe",
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "mintty.exe",
    "putty.exe",
    "alacritty.exe",
    "wezterm-gui.exe",
];

/// Whether `exe` (a path or a file name) is one of [`TERMINALS`].
pub fn is_terminal(exe: &str) -> bool {
    let name = crate::settings::file_name(exe).to_lowercase();
    TERMINALS.contains(&name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminals_are_known_by_file_name() {
        assert!(is_terminal(
            r"C:\Program Files\WindowsApps\x\WindowsTerminal.exe"
        ));
        assert!(is_terminal("CMD.EXE"));
        assert!(!is_terminal("notepad.exe"));
        assert!(!is_terminal(""));
    }
}
