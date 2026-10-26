//! Starting with Windows: a value in the user's `Run` key with the exe's
//! path and `--hidden`, so that the program starts in the tray.

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "qswitch";

/// The command the Run value holds for this exe.
fn command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\" --hidden", exe.display()))
}

/// Whether the Run value is there and names this exe.
#[cfg(windows)]
pub fn is_enabled() -> bool {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    let (key, value) = (crate::win::wide(RUN_KEY), crate::win::wide(VALUE));
    let mut buf = [0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if ok != 0 {
        return false;
    }
    let text = String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]);
    command().is_some_and(|c| c.eq_ignore_ascii_case(&text))
}

/// Set or clear the Run value; whether it worked.
#[cfg(windows)]
pub fn set(enabled: bool) -> bool {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW, RegOpenKeyExW,
        RegSetValueExW,
    };
    let (key, value) = (crate::win::wide(RUN_KEY), crate::win::wide(VALUE));
    let mut handle = std::ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut handle,
        )
    } != 0
    {
        return false;
    }
    let result = if enabled {
        let Some(command) = command() else {
            return false;
        };
        let data = crate::win::wide(command);
        unsafe {
            RegSetValueExW(
                handle,
                value.as_ptr(),
                0,
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        }
    } else {
        let r = unsafe { RegDeleteValueW(handle, value.as_ptr()) };
        // Already absent is fine.
        if r == 2 { 0 } else { r }
    };
    unsafe { RegCloseKey(handle) };
    if result != 0 {
        log::warn!("Run value not written: error {result}");
    }
    result == 0
}

#[cfg(not(windows))]
pub fn is_enabled() -> bool {
    false
}

#[cfg(not(windows))]
pub fn set(_enabled: bool) -> bool {
    false
}
