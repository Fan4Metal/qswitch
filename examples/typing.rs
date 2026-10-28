//! Does an egui text field take what the hook thread injects? A window
//! with a field holding `ghbdtn`; after 1.5 s a thread sends six
//! Backspaces and `привет` as `KEYEVENTF_UNICODE` through `SendInput`
//! (as `hook::inject` does), then asks the focus window for the second
//! installed layout with `WM_INPUTLANGCHANGEREQUEST` (as
//! `layout::switch_to` does). The field's text and the window's layout
//! are printed whenever they change; the program ends after 4 s.
//!
//! `cargo run --example typing`. The window takes the focus and receives
//! simulated input.

use std::time::{Duration, Instant};

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, GetKeyboardLayoutList, INPUT, INPUT_KEYBOARD, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, SendInput, VK_BACK,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GUITHREADINFO, GetGUIThreadInfo, PostMessageW, WM_INPUTLANGCHANGEREQUEST,
};

struct Typing {
    text: String,
    shown: String,
    layout: isize,
    started: Instant,
    sent: bool,
}

impl eframe::App for Typing {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        egui::CentralPanel::default().show(root, |ui| {
            let field = ui.add(egui::TextEdit::singleline(&mut self.text));
            field.request_focus();
        });
        if self.text != self.shown {
            println!("{:6} ms text: {:?}", self.started.elapsed().as_millis(), self.text);
            self.shown = self.text.clone();
        }
        let layout = unsafe { GetKeyboardLayout(0) } as isize;
        if layout != self.layout {
            println!(
                "{:6} ms layout: {:08x}",
                self.started.elapsed().as_millis(),
                layout as u32
            );
            self.layout = layout;
        }
        if !self.sent && self.started.elapsed() > Duration::from_millis(1500) {
            self.sent = true;
            std::thread::spawn(|| {
                inject(6, "привет");
                std::thread::sleep(Duration::from_millis(300));
                let mut info: GUITHREADINFO = unsafe { std::mem::zeroed() };
                info.cbSize = size_of::<GUITHREADINFO>() as u32;
                let focus = if unsafe { GetGUIThreadInfo(0, &mut info) } != 0 {
                    info.hwndFocus
                } else {
                    std::ptr::null_mut()
                };
                let mut list = [std::ptr::null_mut(); 8];
                let n = unsafe { GetKeyboardLayoutList(8, list.as_mut_ptr()) } as usize;
                let current = unsafe { GetKeyboardLayout(0) };
                let other = list[..n].iter().copied().find(|&h| h != current);
                println!(
                    "focus {:?}, layouts {n}, other {:?}",
                    focus,
                    other.map(|h| h as u32)
                );
                if let Some(other) = other {
                    let posted = unsafe {
                        PostMessageW(focus, WM_INPUTLANGCHANGEREQUEST, 0, other as isize)
                    };
                    println!("posted: {posted}");
                }
            });
        }
        if self.started.elapsed() > Duration::from_secs(4) {
            std::process::exit(0);
        }
        ctx.request_repaint_after(Duration::from_millis(50));
    }
}

fn inject(backspaces: usize, text: &str) {
    let key = |vk: u16, scan: u16, flags: u32| {
        let mut input = INPUT {
            r#type: INPUT_KEYBOARD,
            ..unsafe { std::mem::zeroed() }
        };
        input.Anonymous.ki.wVk = vk;
        input.Anonymous.ki.wScan = scan;
        input.Anonymous.ki.dwFlags = flags;
        input
    };
    let mut inputs = Vec::new();
    for _ in 0..backspaces {
        inputs.push(key(VK_BACK, 0, 0));
        inputs.push(key(VK_BACK, 0, KEYEVENTF_KEYUP));
    }
    for unit in text.encode_utf16() {
        inputs.push(key(0, unit, KEYEVENTF_UNICODE));
        inputs.push(key(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
    }
    let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size_of::<INPUT>() as i32) };
    println!("sent {sent} of {}", inputs.len());
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([300.0, 80.0]),
        ..Default::default()
    };
    eframe::run_native(
        "typing",
        options,
        Box::new(|_cc| {
            Ok(Box::new(Typing {
                text: "ghbdtn".to_owned(),
                shown: String::new(),
                layout: 0,
                started: Instant::now(),
                sent: false,
            }))
        }),
    )
}
