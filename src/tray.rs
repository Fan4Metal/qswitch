//! The icon in the notification area: the two letters of the layout the
//! foreground window is in (`RU`, `EN`), drawn with GDI in the colour of
//! the taskbar's text; a click shows the window, the menu (right button)
//! has Show, Pause and Exit. The messages come to the window's subclass
//! (`window.rs`), which shows the menu and queues what was chosen. From
//! metalamp's `tray.rs`, with the letters instead of the exe's icon.

use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};

/// The message the icon's events come with (`WM_APP` + 0x51).
pub const WM_TRAY: u32 = 0x8000 + 0x51;

/// What the icon's click or menu asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayCmd {
    Show,
    TogglePause,
    Exit,
}

impl TrayCmd {
    /// The menu's items, in its order (a separator before Exit).
    pub const MENU: [TrayCmd; 3] = [TrayCmd::Show, TrayCmd::TogglePause, TrayCmd::Exit];

    pub fn label(self, paused: bool) -> String {
        match self {
            TrayCmd::Show => tr!(
                format!("Show {}", crate::APP_NAME),
                format!("Показать {}", crate::APP_NAME)
            ),
            TrayCmd::TogglePause if paused => tr!("Resume", "Продолжить").to_owned(),
            TrayCmd::TogglePause => tr!("Pause", "Пауза").to_owned(),
            TrayCmd::Exit => tr!("Exit", "Выход").to_owned(),
        }
    }

    /// The menu item's id (1-based), and back.
    pub fn id(self) -> u32 {
        Self::MENU
            .iter()
            .position(|&c| c == self)
            .map_or(0, |i| i as u32 + 1)
    }

    pub fn of_id(id: u32) -> Option<TrayCmd> {
        Self::MENU.get((id as usize).checked_sub(1)?).copied()
    }
}

pub struct Tray {
    hwnd: isize,
    icon: isize,
    /// What the icon shows: the letters, the taskbar light, paused.
    shown: (String, bool, bool),
    tip: String,
}

impl Tray {
    /// The icon added for `hwnd`; `None` if the shell refused it.
    pub fn new(hwnd: isize) -> Option<Tray> {
        let shown = ("··".to_owned(), taskbar_is_light(), false);
        let mut tray = Tray {
            hwnd,
            icon: letters_icon(&shown.0, shown.1, shown.2),
            shown,
            tip: crate::APP_NAME.to_owned(),
        };
        tray.add().then_some(tray)
    }

    fn data(&self) -> NOTIFYICONDATAW {
        // SAFETY: a plain struct, all zeros a valid start.
        let mut d: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        d.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        d.hWnd = self.hwnd as _;
        d.uID = 1;
        d
    }

    /// Adds the icon (again, after Explorer restarted); whether it worked.
    pub fn add(&mut self) -> bool {
        let mut d = self.data();
        d.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        d.uCallbackMessage = WM_TRAY;
        d.hIcon = self.icon as _;
        put_tip(&mut d, &self.tip);
        // SAFETY: a filled NOTIFYICONDATAW of this window.
        let added = unsafe { Shell_NotifyIconW(NIM_ADD, &d) } != 0;
        if added {
            d.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            unsafe { Shell_NotifyIconW(NIM_SETVERSION, &d) };
        }
        added
    }

    /// The letters of the layout, greyed while paused; the tooltip.
    pub fn set(&mut self, letters: &str, paused: bool, tip: &str) {
        let light = taskbar_is_light();
        let wanted = (letters.to_owned(), light, paused);
        let mut d = self.data();
        d.uFlags = NIF_TIP | NIF_SHOWTIP;
        if wanted != self.shown {
            let icon = letters_icon(letters, light, paused);
            if icon != 0 {
                if self.icon != 0 {
                    unsafe {
                        windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon(self.icon as _)
                    };
                }
                self.icon = icon;
                d.uFlags |= NIF_ICON;
                d.hIcon = icon as _;
            }
            self.shown = wanted;
        } else if tip == self.tip {
            return;
        }
        self.tip = tip.to_owned();
        put_tip(&mut d, tip);
        // SAFETY: as in `add`.
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &d) };
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let d = self.data();
        // SAFETY: the icon of this window, removed once.
        unsafe { Shell_NotifyIconW(NIM_DELETE, &d) };
        if self.icon != 0 {
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon(self.icon as _) };
        }
    }
}

/// `tip` in the struct's 128 UTF-16 units, cut and NUL-ended.
fn put_tip(d: &mut NOTIFYICONDATAW, tip: &str) {
    let wide: Vec<u16> = tip.encode_utf16().take(d.szTip.len() - 1).collect();
    d.szTip.fill(0);
    d.szTip[..wide.len()].copy_from_slice(&wide);
}

/// Whether the taskbar is light (Settings > Colors > "Choose your default
/// Windows mode"), so the letters are dark on it.
pub fn taskbar_is_light() -> bool {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let key = crate::win::wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let value = crate::win::wide("SystemUsesLightTheme");
    let mut data = 0u32;
    let mut len = 4u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut data).cast(),
            &mut len,
        )
    };
    ok == 0 && data != 0
}

/// An icon of the small-icon size with `letters` in bold Segoe UI, dark
/// on a light taskbar and white on a dark one, grey while paused. GDI
/// writes no alpha, so the text is drawn white on black and its
/// brightness becomes the alpha (premultiplied, as icons want).
fn letters_icon(letters: &str, light: bool, paused: bool) -> isize {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS,
        CreateBitmap, CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET,
        DEFAULT_PITCH, DIB_RGB_COLORS, DT_CENTER, DT_NOCLIP, DT_SINGLELINE, DT_VCENTER, DeleteDC,
        DeleteObject, DrawTextW, FF_DONTCARE, FW_BOLD, GdiFlush, OUT_DEFAULT_PRECIS, SelectObject,
        SetBkMode, SetTextColor, TRANSPARENT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateIconIndirect, GetSystemMetrics, ICONINFO, SM_CXSMICON,
    };
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
    let (r, g, b) = match (light, paused) {
        (true, false) => (0x1A, 0x1A, 0x1A),
        (true, true) => (0x90, 0x90, 0x90),
        (false, false) => (0xFF, 0xFF, 0xFF),
        (false, true) => (0x80, 0x80, 0x80),
    };
    // SAFETY: GDI objects made, used and deleted here; the DIB's bits are
    // written only between GdiFlush and the icon's creation.
    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        if dc.is_null() {
            return 0;
        }
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = size;
        info.bmiHeader.biHeight = -size; // top-down
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(
            dc,
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if bitmap.is_null() || bits.is_null() {
            DeleteDC(dc);
            return 0;
        }
        let old_bitmap = SelectObject(dc, bitmap as _);
        let face = crate::win::wide("Segoe UI");
        let font = CreateFontW(
            -(size * 11 / 16),
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
            ANTIALIASED_QUALITY as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            face.as_ptr(),
        );
        let old_font = SelectObject(dc, font as _);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, 0x00FF_FFFF);
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: size,
            bottom: size,
        };
        let text: Vec<u16> = letters.encode_utf16().collect();
        DrawTextW(
            dc,
            text.as_ptr(),
            text.len() as i32,
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOCLIP,
        );
        GdiFlush();
        let px = std::slice::from_raw_parts_mut(bits as *mut u8, (size * size * 4) as usize);
        for p in px.as_chunks_mut::<4>().0 {
            let a = p[0].max(p[1]).max(p[2]) as u32;
            p[0] = (b as u32 * a / 255) as u8;
            p[1] = (g as u32 * a / 255) as u8;
            p[2] = (r as u32 * a / 255) as u8;
            p[3] = a as u8;
        }
        SelectObject(dc, old_font);
        DeleteObject(font as _);
        SelectObject(dc, old_bitmap);
        let mask = CreateBitmap(size, size, 1, 1, std::ptr::null());
        let icon_info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: bitmap,
        };
        let icon = CreateIconIndirect(&icon_info);
        DeleteObject(mask as _);
        DeleteObject(bitmap as _);
        DeleteDC(dc);
        icon as isize
    }
}

/// The icon's menu at the pointer (`x`, `y` in screen pixels) for
/// `hwnd`, which is made the foreground window first, so the menu closes
/// when clicked away from; the command chosen.
pub fn menu(hwnd: isize, x: i32, y: i32, paused: bool) -> Option<TrayCmd> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, MF_CHECKED, MF_SEPARATOR, MF_STRING,
        PostMessageW, SetForegroundWindow, SetMenuDefaultItem, TPM_NONOTIFY, TPM_RETURNCMD,
        TPM_RIGHTBUTTON, TrackPopupMenuEx, WM_NULL,
    };
    // SAFETY: a menu made, shown and destroyed here, on the window's
    // thread (this runs in its window procedure).
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return None;
        }
        for cmd in TrayCmd::MENU {
            if cmd == TrayCmd::Exit {
                AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            }
            let label: Vec<u16> = cmd.label(paused).encode_utf16().chain(Some(0)).collect();
            let checked = if cmd == TrayCmd::TogglePause && paused {
                MF_CHECKED
            } else {
                0
            };
            AppendMenuW(menu, MF_STRING | checked, cmd.id() as usize, label.as_ptr());
        }
        SetMenuDefaultItem(menu, TrayCmd::Show.id(), 0);
        SetForegroundWindow(hwnd as _);
        let chosen = TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            x,
            y,
            hwnd as _,
            std::ptr::null(),
        );
        PostMessageW(hwnd as _, WM_NULL, 0, 0);
        DestroyMenu(menu);
        TrayCmd::of_id(chosen as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_round_trip() {
        for cmd in TrayCmd::MENU {
            assert_eq!(TrayCmd::of_id(cmd.id()), Some(cmd));
        }
        assert_eq!(TrayCmd::of_id(0), None);
        assert_eq!(TrayCmd::of_id(99), None);
    }

    #[test]
    fn letters_make_an_icon() {
        let icon = letters_icon("RU", true, false);
        assert_ne!(icon, 0);
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon(icon as _) };
    }
}
