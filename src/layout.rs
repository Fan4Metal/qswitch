//! Keyboard layouts as Windows has them: the installed ones with their
//! names, which one the foreground window is in, switching it, and the
//! table of what each key types in a layout (`ToUnicodeEx`), from which
//! the machine (`engine.rs`) converts text between the two of the pair.

use std::ffi::c_void;

/// A keyboard layout handle (`HKL`): the language in the low word, the
/// layout (a device handle for a custom one) in the high word.
pub type Hkl = isize;

/// An installed layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub hkl: Hkl,
    /// Two letters for the tray: `EN`, `RU`.
    pub code: String,
    /// The language in the interface's language: `English (United States)`.
    pub name: String,
}

impl Layout {
    /// As kept in the settings: the handle in hex.
    pub fn id(&self) -> String {
        id(self.hkl)
    }
}

/// The handle as kept in the settings.
pub fn id(hkl: Hkl) -> String {
    format!("{:08x}", hkl as u32)
}

/// The installed layouts in Windows' order (the language bar's).
#[cfg(windows)]
pub fn installed() -> Vec<Layout> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayoutList;
    let n = unsafe { GetKeyboardLayoutList(0, std::ptr::null_mut()) };
    let mut list = vec![std::ptr::null_mut::<c_void>(); n.max(0) as usize];
    let got = unsafe { GetKeyboardLayoutList(list.len() as i32, list.as_mut_ptr()) };
    list.truncate(got.max(0) as usize);
    list.into_iter()
        .map(|h| {
            let hkl = h as Hkl;
            let (code, name) = describe(hkl);
            Layout { hkl, code, name }
        })
        .collect()
}

/// The two-letter code and the name of a layout's language, the name in
/// the interface's language: English for the English interface, else as
/// Windows shows it (`i18n::lang`, so `installed` is called again when
/// the language is changed).
#[cfg(windows)]
fn describe(hkl: Hkl) -> (String, String) {
    use windows_sys::Win32::Globalization::{GetLocaleInfoEx, LCIDToLocaleName};
    const LOCALE_SLOCALIZEDDISPLAYNAME: u32 = 0x0000_0002;
    const LOCALE_SENGLISHDISPLAYNAME: u32 = 0x0000_0072;
    let name_kind = if crate::i18n::lang() == crate::i18n::Lang::En {
        LOCALE_SENGLISHDISPLAYNAME
    } else {
        LOCALE_SLOCALIZEDDISPLAYNAME
    };
    let lang = (hkl as u32) & 0xFFFF;
    let mut locale = [0u16; 85];
    let n = unsafe { LCIDToLocaleName(lang, locale.as_mut_ptr(), locale.len() as i32, 0) };
    if n <= 1 {
        return (format!("{lang:04X}"), format!("Language {lang:04X}"));
    }
    let locale_name = String::from_utf16_lossy(&locale[..n as usize - 1]);
    let code = locale_name.split('-').next().unwrap_or("").to_uppercase();
    let mut name = [0u16; 128];
    let n = unsafe {
        GetLocaleInfoEx(
            locale.as_ptr(),
            name_kind,
            name.as_mut_ptr(),
            name.len() as i32,
        )
    };
    let name = if n > 1 {
        String::from_utf16_lossy(&name[..n as usize - 1])
    } else {
        locale_name.clone()
    };
    (code, name)
}

/// The foreground window, and the one with the keyboard focus in it when
/// the foreground thread tells (a control inside, or the real window of a
/// UWP app behind its frame); `None` without a foreground window.
#[cfg(windows)]
pub fn target_window() -> Option<isize> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GUITHREADINFO, GetForegroundWindow, GetGUIThreadInfo,
    };
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return None;
    }
    let mut info: GUITHREADINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<GUITHREADINFO>() as u32;
    if unsafe { GetGUIThreadInfo(0, &mut info) } != 0 && !info.hwndFocus.is_null() {
        return Some(info.hwndFocus as isize);
    }
    Some(foreground as isize)
}

/// The layout of the thread that owns `hwnd`.
#[cfg(windows)]
pub fn layout_of(hwnd: isize) -> Hkl {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    let thread = unsafe { GetWindowThreadProcessId(hwnd as _, std::ptr::null_mut()) };
    unsafe { GetKeyboardLayout(thread) as Hkl }
}

/// The layout the foreground window is in, `None` without one.
#[cfg(windows)]
pub fn current() -> Option<Hkl> {
    target_window().map(layout_of)
}

/// Ask the window typed in to take layout `hkl` (`WM_INPUTLANGCHANGEREQUEST`,
/// which the window's default procedure answers by activating it).
#[cfg(windows)]
pub fn switch_to(hkl: Hkl) {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    let foreground = unsafe { GetForegroundWindow() } as isize;
    if let Some(target) = target_window() {
        request(target, foreground, hkl);
    }
}

/// [`switch_to`] for `window`, in front or not: the control with the focus
/// in its thread is asked, and the window itself.
#[cfg(windows)]
pub fn switch_window(window: isize, hkl: Hkl) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GUITHREADINFO, GetGUIThreadInfo, GetWindowThreadProcessId,
    };
    let thread = unsafe { GetWindowThreadProcessId(window as _, std::ptr::null_mut()) };
    let mut info: GUITHREADINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<GUITHREADINFO>() as u32;
    let focus = if thread != 0
        && unsafe { GetGUIThreadInfo(thread, &mut info) } != 0
        && !info.hwndFocus.is_null()
    {
        info.hwndFocus as isize
    } else {
        window
    };
    request(focus, window, hkl);
}

#[cfg(windows)]
fn request(target: isize, frame: isize, hkl: Hkl) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_INPUTLANGCHANGEREQUEST};
    unsafe {
        PostMessageW(target as _, WM_INPUTLANGCHANGEREQUEST, 0, hkl);
        // A control inside answers for its thread, but a frame window of
        // another thread (a UWP app's) is asked too.
        if frame != target && frame != 0 {
            PostMessageW(frame as _, WM_INPUTLANGCHANGEREQUEST, 0, hkl);
        }
    }
}

/// What a key types in one layout, by virtual key and the state of Shift
/// and Caps Lock.
#[derive(Clone, Debug)]
pub struct Table {
    /// `[vk][shift | caps << 1]`.
    chars: Vec<[Option<char>; 4]>,
    /// The key (vk, modifier index) that types each character, the first
    /// found with the fewest modifiers.
    keys: std::collections::HashMap<char, (u16, usize)>,
}

impl Table {
    /// An empty table (a layout outside the pair, tests).
    pub fn empty() -> Table {
        Table {
            chars: vec![[None; 4]; 256],
            keys: std::collections::HashMap::new(),
        }
    }

    pub fn get(&self, vk: u16, shift: bool, caps: bool) -> Option<char> {
        self.get_mod(vk, usize::from(shift) | usize::from(caps) << 1)
    }

    /// By the modifier index (`shift | caps << 1`).
    pub fn get_mod(&self, vk: u16, modifiers: usize) -> Option<char> {
        self.chars.get(vk as usize).and_then(|c| c[modifiers])
    }

    /// The key and modifiers that type `ch`, if any.
    pub fn key_of(&self, ch: char) -> Option<(u16, usize)> {
        self.keys.get(&ch).copied()
    }

    /// Fills `keys` from `chars`: the plain key first, then with Shift,
    /// then with Caps Lock.
    fn index(mut self) -> Table {
        self.keys.clear();
        for modifiers in 0..4 {
            for (vk, slot) in self.chars.iter().enumerate() {
                if let Some(ch) = slot[modifiers] {
                    self.keys.entry(ch).or_insert((vk as u16, modifiers));
                }
            }
        }
        self
    }

    /// A table from what each key types, for tests.
    #[cfg(test)]
    pub fn from_keys(keys: &[(u16, [Option<char>; 4])]) -> Table {
        let mut table = Table::empty();
        for &(vk, chars) in keys {
            table.chars[vk as usize] = chars;
        }
        table.index()
    }

    /// Builds a table for a layout with `ToUnicodeEx`, which is asked not
    /// to touch the keyboard's dead-key state (bit 2 of the flags, Windows
    /// 10 1607 and later). Control characters (Enter, Tab, Backspace) and
    /// dead keys are `None`: the machine does not track them.
    #[cfg(windows)]
    pub fn of(hkl: Hkl) -> Table {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            MapVirtualKeyExW, ToUnicodeEx, VK_CAPITAL, VK_SHIFT,
        };
        const MAPVK_VK_TO_VSC: u32 = 0;
        const DO_NOT_CHANGE_STATE: u32 = 1 << 2;
        let mut chars = vec![[None; 4]; 256];
        for (vk, slot) in chars.iter_mut().enumerate() {
            let scan = unsafe { MapVirtualKeyExW(vk as u32, MAPVK_VK_TO_VSC, hkl as _) };
            if scan == 0 {
                continue;
            }
            for (i, c) in slot.iter_mut().enumerate() {
                let mut state = [0u8; 256];
                if i & 1 != 0 {
                    state[VK_SHIFT as usize] = 0x80;
                }
                if i & 2 != 0 {
                    state[VK_CAPITAL as usize] = 1;
                }
                let mut buf = [0u16; 8];
                let n = unsafe {
                    ToUnicodeEx(
                        vk as u32,
                        scan,
                        state.as_ptr(),
                        buf.as_mut_ptr(),
                        buf.len() as i32,
                        DO_NOT_CHANGE_STATE,
                        hkl as _,
                    )
                };
                if n <= 0 {
                    continue;
                }
                let text = String::from_utf16_lossy(&buf[..n as usize]);
                let mut it = text.chars();
                if let (Some(ch), None) = (it.next(), it.next())
                    && !ch.is_control()
                {
                    *c = Some(ch);
                }
            }
        }
        Table {
            chars,
            keys: std::collections::HashMap::new(),
        }
        .index()
    }
}

/// `text` with every character replaced by what its key types in the
/// other layout of the pair: a character of the layout `current` (else of
/// either, the first of the pair tried first) is looked up by key and read
/// in the other table; one found in neither stays. Also the layout the
/// last converted character went to, to switch the window to.
pub fn convert_text(text: &str, tables: &[Table; 2], current: Option<u8>) -> (String, Option<u8>) {
    let order: [usize; 2] = if current == Some(1) { [1, 0] } else { [0, 1] };
    let mut out = String::with_capacity(text.len());
    let mut target = None;
    for ch in text.chars() {
        let mut converted = None;
        for src in order {
            if let Some((vk, modifiers)) = tables[src].key_of(ch)
                && let Some(other) = tables[1 - src].get_mod(vk, modifiers)
            {
                converted = Some((other, 1 - src));
                break;
            }
        }
        match converted {
            Some((other, to)) => {
                out.push(other);
                if other != ch {
                    target = Some(to as u8);
                }
            }
            None => out.push(ch),
        }
    }
    (out, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two keys of a made-up pair: Q/Й and the period/Ю key, with Shift.
    fn pair() -> [Table; 2] {
        let en = Table::from_keys(&[
            (0x51, [Some('q'), Some('Q'), Some('Q'), Some('q')]),
            (0xBE, [Some('.'), Some('>'), Some('.'), Some('>')]),
            (0x20, [Some(' '); 4]),
        ]);
        let ru = Table::from_keys(&[
            (0x51, [Some('й'), Some('Й'), Some('Й'), Some('й')]),
            (0xBE, [Some('ю'), Some('Ю'), Some('Ю'), Some('ю')]),
            (0x20, [Some(' '); 4]),
        ]);
        [en, ru]
    }

    #[test]
    fn text_is_converted_by_keys_both_ways() {
        let tables = pair();
        assert_eq!(
            convert_text("qQ. 7", &tables, Some(0)),
            ("йЙю 7".to_owned(), Some(1))
        );
        assert_eq!(
            convert_text("йЙю", &tables, Some(1)),
            ("qQ.".to_owned(), Some(0))
        );
        // Mixed text: each character goes to the other layout.
        assert_eq!(
            convert_text("qй", &tables, None),
            ("йq".to_owned(), Some(0))
        );
        // Nothing known: unchanged, no layout to switch to.
        assert_eq!(
            convert_text("7 ", &tables, Some(0)),
            ("7 ".to_owned(), None)
        );
        // Caps Lock on: the plain key is preferred, so 'Q' maps by Shift.
        assert_eq!(tables[0].key_of('Q'), Some((0x51, 1)));
    }

    #[test]
    fn ids_are_hex_of_the_handle() {
        assert_eq!(id(0x0419_0419), "04190419");
        assert_eq!(id(0xF008_0409u32 as i32 as isize), "f0080409");
    }

    #[test]
    #[cfg(windows)]
    fn installed_layouts_have_tables() {
        // Needs at least one layout, which any Windows has.
        let layouts = installed();
        assert!(!layouts.is_empty());
        for l in &layouts {
            assert_eq!(l.code.len(), 2, "{l:?}");
            let table = Table::of(l.hkl);
            // Space is a space everywhere, A is a letter.
            assert_eq!(table.get(0x20, false, false), Some(' '));
            assert!(
                table
                    .get(0x41, false, false)
                    .is_some_and(char::is_alphabetic)
            );
            assert!(table.get(0x41, true, false).is_some_and(char::is_uppercase));
            assert!(table.get(0x41, false, true).is_some_and(char::is_uppercase));
            assert_eq!(table.get(0x0D, false, false), None);
        }
        if let Some(en) = layouts.iter().find(|l| l.code == "EN") {
            let t = Table::of(en.hkl);
            assert_eq!(t.get(0x51, false, false), Some('q'));
            assert_eq!(t.get(0xBF, true, false), Some('?'));
        }
        if let Some(ru) = layouts.iter().find(|l| l.code == "RU") {
            let t = Table::of(ru.hkl);
            assert_eq!(t.get(0x51, false, false), Some('й'));
            assert_eq!(t.get(0xBF, true, false), Some(','));
        }
    }
}
