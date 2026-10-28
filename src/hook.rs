//! The hook thread: the low-level keyboard and mouse hooks, the foreground
//! and focus events, the layout poll, and the injection of what the
//! machine (`engine.rs`) decides. Its own thread with its own message
//! loop, at high priority: a low-level hook's callback runs when the
//! thread that set it pumps messages, so on the UI thread every key would
//! wait for a frame, and a callback later than `LowLevelHooksTimeout`
//! (300 ms) makes Windows drop the hook without a word.
//!
//! The callback itself only runs the machine (microseconds) and notes what
//! to do; the doing (`SendInput`, the layout switch) is posted to the
//! thread's own queue and done once the callback has returned, so the key
//! that caused it has gone on to its window first.
//!
//! The UI talks to the thread through [`reload`], [`set_paused`] and
//! [`stop`]; the thread answers with [`Event`]s on a channel and wakes
//! the UI with `request_repaint`.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, INPUT, INPUT_KEYBOARD, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    SendInput, VK_BACK, VK_CAPITAL, VK_LSHIFT, VK_PAUSE, VK_RSHIFT, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, EVENT_OBJECT_FOCUS, EVENT_OBJECT_LOCATIONCHANGE,
    EVENT_SYSTEM_FOREGROUND, GetForegroundWindow, GetMessageW, KBDLLHOOKSTRUCT, KillTimer,
    LLKHF_INJECTED, MSG, OBJID_CARET, PostThreadMessageW, SetTimer, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS, WM_APP, WM_KEYDOWN, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_QUIT,
    WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_TIMER, WM_XBUTTONDOWN,
};

use crate::caret::{Answer, Ask};
use crate::engine::{Effect, Engine, Key};
use crate::layout::{self, Hkl, Table};
use crate::settings::{Rule, RuleMode, rule_for};

/// What the thread tells the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The foreground window's layout (changed, or first seen) and its
    /// place in the pair.
    Layout { hkl: Hkl, index: Option<u8> },
    /// Another program's window came to the front: its exe and the rule.
    Foreground { exe: String, mode: Option<RuleMode> },
    /// The gesture converted text in the foreground program.
    Converted {
        from: String,
        to: String,
        exe: String,
    },
    /// The hooks are in place, or could not be set.
    Hooks(bool),
    /// A line of the trace (while [`set_trace`] is on): a key as the hook
    /// saw it and what the machine made of it, a job done, a window change.
    Trace(String),
}

/// What the thread works with, from the settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setup {
    pub config: crate::engine::Config,
    pub pair: [Hkl; 2],
    pub rules: Vec<Rule>,
    /// The installed layouts with their letters, for the badge.
    pub layouts: Vec<(Hkl, String)>,
    pub indicator: crate::indicator::Config,
}

/// Marks the thread's own injected input (`dwExtraInfo`), which the hook
/// then lets through untouched.
const MAGIC: usize = 0x7153_7769;
/// The settings changed: read `SETUP` again.
const WM_RELOAD: u32 = WM_APP + 1;
/// Jobs are waiting.
const WM_JOB: u32 = WM_APP + 2;
/// The tray icon was clicked: toggle the layout of the window in front
/// before the taskbar.
const WM_TOGGLE: u32 = WM_APP + 3;
/// The caret thread answered (`CARET_ANSWER`).
const WM_CARET: u32 = WM_APP + 4;
/// The window asks to see the badge (beside the pointer).
const WM_PREVIEW: u32 = WM_APP + 5;
/// The caret thread answered whether the focused element takes a
/// password (`PASSWORD`).
const WM_PASSWORD: u32 = WM_APP + 6;
static PASSWORD: AtomicBool = AtomicBool::new(false);
/// How often the layout is looked at (it changes by Alt+Shift too).
const POLL_MS: u32 = 150;

static SETUP: Mutex<Option<Setup>> = Mutex::new(None);
static PAUSED: AtomicBool = AtomicBool::new(false);
static TRACE: AtomicBool = AtomicBool::new(false);
static THREAD: AtomicU32 = AtomicU32::new(0);
/// The caret thread's last answer: the question's number and the caret.
static CARET_ANSWER: Mutex<Option<(u64, Option<windows_sys::Win32::Foundation::RECT>)>> =
    Mutex::new(None);

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Something to do once the hook callback has returned.
#[derive(Debug)]
enum Job {
    Switch(Hkl),
    Replace {
        backspaces: usize,
        text: String,
    },
    /// Convert the selected text, else switch the layout; `index` is the
    /// layout of the pair the window is in.
    Selection {
        index: Option<u8>,
    },
}

struct State {
    engine: Engine,
    pair: [Hkl; 2],
    tables: std::sync::Arc<[Table; 2]>,
    rules: Vec<Rule>,
    layouts: Vec<(Hkl, String)>,
    indicator: Option<crate::indicator::Indicator>,
    /// The caret thread (`caret.rs`): the badge's caret, the password
    /// question.
    finder: Option<Sender<Ask>>,
    /// The focus control is a classic password edit (`ES_PASSWORD`).
    classic_password: bool,
    /// UI Automation says the focused element takes a password.
    uia_password: bool,
    /// Shift is held (followed here for the character tables).
    shift: bool,
    /// The foreground program's exe (a full path) and its rule.
    exe: String,
    mode: Option<RuleMode>,
    /// The last window in front that is not the taskbar's (a click on
    /// the tray icon brings the taskbar forward), when it is a program's
    /// window ([`is_app_window`]; 0 after the desktop, the Start menu,
    /// Alt+Tab): the tray's toggle switches its layout.
    app_window: isize,
    last_layout: Option<Hkl>,
    jobs: VecDeque<Job>,
    events: Sender<Event>,
    ctx: egui::Context,
}

/// Start the thread; events come on the receiver.
pub fn start(setup: Setup, ctx: egui::Context) -> Receiver<Event> {
    let (tx, rx) = channel();
    *SETUP.lock().unwrap_or_else(|e| e.into_inner()) = Some(setup);
    let spawned = std::thread::Builder::new()
        .name("hook".into())
        .spawn(move || run(tx, ctx));
    if let Err(e) = spawned {
        log::error!("hook thread not started: {e}");
    }
    rx
}

/// New settings for the thread.
pub fn reload(setup: Setup) {
    *SETUP.lock().unwrap_or_else(|e| e.into_inner()) = Some(setup);
    post(WM_RELOAD);
}

pub fn paused() -> bool {
    PAUSED.load(Ordering::Relaxed)
}

/// Paused, the hooks stay in place but let everything through.
/// Send every key and job to the UI as [`Event::Trace`] lines (the UI
/// writes them to a file), for finding out what a program does with the
/// injected keys.
pub fn set_trace(on: bool) {
    TRACE.store(on, Ordering::Relaxed);
}

fn tracing() -> bool {
    TRACE.load(Ordering::Relaxed)
}

pub fn set_paused(paused: bool) {
    PAUSED.store(paused, Ordering::Relaxed);
    post(WM_RELOAD);
}

/// Gives up the raw input of the keyboard and the mouse that winit
/// registers for its window (for its device events, which egui does not
/// use). While a process has it, Windows does not call that process's
/// low-level keyboard hook for the keys typed into its own windows: with
/// it, the gesture did nothing in the "try here" field. Called from the
/// main thread once the window exists (`App::new`), after winit's
/// registration.
pub fn release_raw_input() {
    use windows_sys::Win32::UI::Input::{RAWINPUTDEVICE, RIDEV_REMOVE, RegisterRawInputDevices};
    const GENERIC_DESKTOP: u16 = 1;
    const MOUSE: u16 = 2;
    const KEYBOARD: u16 = 6;
    let remove = |usage| RAWINPUTDEVICE {
        usUsagePage: GENERIC_DESKTOP,
        usUsage: usage,
        dwFlags: RIDEV_REMOVE,
        hwndTarget: std::ptr::null_mut(),
    };
    let devices = [remove(KEYBOARD), remove(MOUSE)];
    let ok = unsafe {
        RegisterRawInputDevices(
            devices.as_ptr(),
            devices.len() as u32,
            size_of::<RAWINPUTDEVICE>() as u32,
        )
    };
    if ok == 0 {
        log::warn!(
            "raw input not released: {}",
            std::io::Error::last_os_error()
        );
    }
}

/// The tray icon clicked: the window that was in front before the
/// taskbar takes the other layout of the pair (the focus stays put).
pub fn toggle_layout() {
    post(WM_TOGGLE);
}

/// The badge for a moment beside the pointer (the window's "Show").
pub fn preview_badge() {
    post(WM_PREVIEW);
}

/// End the thread (the hooks are removed with it).
pub fn stop() {
    post(WM_QUIT);
}

fn post(msg: u32) {
    let thread = THREAD.load(Ordering::Relaxed);
    if thread != 0 {
        unsafe { PostThreadMessageW(thread, msg, 0, 0) };
    }
}

fn run(events: Sender<Event>, ctx: egui::Context) {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, GetCurrentThreadId, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
    };
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        THREAD.store(GetCurrentThreadId(), Ordering::Relaxed);
    }
    let setup = SETUP
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or(Setup {
            config: Default::default(),
            pair: [0, 0],
            rules: Vec::new(),
            layouts: Vec::new(),
            indicator: Default::default(),
        });
    let mut state = State {
        engine: Engine::new(setup.config.clone()),
        pair: setup.pair,
        tables: std::sync::Arc::new([Table::empty(), Table::empty()]),
        rules: Vec::new(),
        layouts: Vec::new(),
        indicator: crate::indicator::Indicator::new(),
        finder: crate::caret::spawn(|answer| match answer {
            Answer::Caret { id, place } => {
                *CARET_ANSWER.lock().unwrap_or_else(|e| e.into_inner()) = Some((id, place));
                post(WM_CARET);
            }
            Answer::Password(password) => {
                PASSWORD.store(password, Ordering::Relaxed);
                post(WM_PASSWORD);
            }
        }),
        classic_password: false,
        uia_password: false,
        shift: false,
        exe: String::new(),
        mode: None,
        app_window: 0,
        last_layout: None,
        jobs: VecDeque::new(),
        events,
        ctx,
    };
    if let Some(indicator) = &mut state.indicator {
        indicator.set_finder(state.finder.clone());
    }
    state.apply(setup);
    state.foreground_changed(unsafe { GetForegroundWindow() } as isize);
    state.poll_layout();
    STATE.with(|s| *s.borrow_mut() = Some(state));

    let module =
        unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) };
    let keyboard = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0) };
    let mouse = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0) };
    let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
    let foreground = unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            std::ptr::null_mut(),
            Some(win_event_proc),
            0,
            0,
            flags,
        )
    };
    let focus = unsafe {
        SetWinEventHook(
            EVENT_OBJECT_FOCUS,
            EVENT_OBJECT_FOCUS,
            std::ptr::null_mut(),
            Some(win_event_proc),
            0,
            0,
            flags,
        )
    };
    let caret = unsafe {
        SetWinEventHook(
            EVENT_OBJECT_LOCATIONCHANGE,
            EVENT_OBJECT_LOCATIONCHANGE,
            std::ptr::null_mut(),
            Some(win_event_proc),
            0,
            0,
            flags,
        )
    };
    let ok = !keyboard.is_null();
    if !ok {
        log::error!(
            "WH_KEYBOARD_LL not set: {}",
            std::io::Error::last_os_error()
        );
    }
    with_state(|s| s.send(Event::Hooks(ok)));
    let timer = unsafe { SetTimer(std::ptr::null_mut(), 0, POLL_MS, None) };

    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
        match msg.message {
            WM_TIMER if msg.wParam == crate::indicator::PLACED_TIMER && !msg.hwnd.is_null() => {
                with_state(|s| {
                    if let Some(indicator) = &mut s.indicator {
                        indicator.placed_due();
                    }
                });
            }
            WM_TIMER => {
                with_state(|s| {
                    s.poll_layout();
                    if let Some(indicator) = &mut s.indicator {
                        indicator.tick();
                    }
                });
            }
            WM_RELOAD => {
                if let Some(setup) = SETUP.lock().unwrap_or_else(|e| e.into_inner()).clone() {
                    with_state(|s| s.apply(setup));
                }
            }
            WM_JOB => {
                with_state(State::run_jobs);
            }
            WM_TOGGLE => {
                with_state(State::toggle_from_tray);
            }
            WM_PREVIEW => {
                with_state(|s| {
                    if let Some(indicator) = &mut s.indicator {
                        indicator.preview();
                    }
                });
            }
            WM_PASSWORD => {
                let password = PASSWORD.load(Ordering::Relaxed);
                with_state(|s| {
                    s.uia_password = password;
                    s.follow_mode();
                });
            }
            WM_CARET => {
                let answer = CARET_ANSWER
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take();
                if let Some((id, caret)) = answer {
                    with_state(|s| {
                        if let Some(indicator) = &mut s.indicator {
                            indicator.caret_found(id, caret);
                        }
                    });
                }
            }
            _ => unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            },
        }
    }
    unsafe {
        KillTimer(std::ptr::null_mut(), timer);
        if !keyboard.is_null() {
            UnhookWindowsHookEx(keyboard);
        }
        if !mouse.is_null() {
            UnhookWindowsHookEx(mouse);
        }
        for hook in [foreground, focus, caret] {
            if !hook.is_null() {
                UnhookWinEvent(hook);
            }
        }
    }
    STATE.with(|s| *s.borrow_mut() = None);
    THREAD.store(0, Ordering::Relaxed);
}

/// Run `f` on the thread's state; nothing while a callback is already
/// in it (the hooks are not re-entered while a callback runs, but a
/// message loop inside one would be).
fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| {
        let mut s = s.try_borrow_mut().ok()?;
        s.as_mut().map(f)
    })
}

impl State {
    fn send(&self, event: Event) {
        if self.events.send(event).is_ok() {
            self.ctx.request_repaint();
        }
    }

    fn apply(&mut self, setup: Setup) {
        if setup.pair != self.pair || self.last_layout.is_none() {
            self.pair = setup.pair;
            self.tables = std::sync::Arc::new([Table::of(self.pair[0]), Table::of(self.pair[1])]);
        }
        self.rules = setup.rules;
        self.layouts = setup.layouts;
        if let Some(indicator) = &mut self.indicator {
            indicator.configure(setup.indicator);
        }
        self.engine.set_config(setup.config);
        self.mode = rule_for(&self.rules, &self.exe);
        self.follow_mode();
        self.last_layout = None;
        self.poll_layout();
    }

    /// Paused or in a window the rules exclude, the machine is off.
    fn follow_mode(&mut self) {
        self.engine
            .set_enabled(!paused() && self.mode != Some(RuleMode::Off));
        // No automatic conversion of a password (a classic password edit,
        // or what UI Automation says of the focused element), nor in a
        // terminal (a password typed there shows nothing).
        self.engine.set_auto_blocked(
            self.classic_password
                || self.uia_password
                || crate::selection::is_terminal(&self.exe),
        );
    }

    /// The focus moved: is it in a password field? The classic style is
    /// known at once; UI Automation answers later (`WM_PASSWORD`).
    fn check_password(&mut self) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GWL_STYLE, GetWindowLongW};
        const ES_PASSWORD: u32 = 0x0020;
        let focus = layout::target_window().unwrap_or(0);
        self.classic_password = focus != 0
            && class_of(focus) == "Edit"
            && unsafe { GetWindowLongW(focus as _, GWL_STYLE) } as u32 & ES_PASSWORD != 0;
        // Until the answer, the last one stands.
        if let Some(finder) = &self.finder {
            let _ = finder.send(Ask::Password);
        }
    }

    fn index_of(&self, hkl: Hkl) -> Option<u8> {
        self.pair.iter().position(|&h| h == hkl).map(|i| i as u8)
    }

    /// A key from the hook; whether to swallow it. `injected`: by some
    /// other program's `SendInput` (the program's own keys never get here).
    fn on_key(&mut self, vk: u16, down: bool, time: u64, injected: bool) -> bool {
        let is_down = |v: u16| unsafe { GetAsyncKeyState(i32::from(v)) } < 0;
        if vk == VK_SHIFT || vk == VK_LSHIFT || vk == VK_RSHIFT {
            self.shift = down;
        } else if down {
            self.shift = is_down(VK_SHIFT);
        }
        if down {
            // A release the hook missed (Win+L, Ctrl+Alt+Del, an elevated
            // window) is caught up with by the keyboard's own state. The
            // key going down now is not in it yet, and the keys the
            // engine may swallow never are.
            self.engine
                .forget_released(|v| v == vk || v == VK_CAPITAL || v == VK_PAUSE || is_down(v));
        }
        if paused() {
            return false;
        }
        let trace = tracing();
        let hkl = layout::current();
        let index = hkl.and_then(|h| self.index_of(h));
        let chars = if down {
            let caps = unsafe { GetKeyState(VK_CAPITAL as i32) } & 1 != 0;
            [
                self.tables[0].get(vk, self.shift, caps),
                self.tables[1].get(vk, self.shift, caps),
            ]
        } else {
            [None, None]
        };
        let verdict = self.engine.key(Key {
            vk,
            down,
            time,
            chars,
            layout: index,
        });
        if trace {
            let line = format!(
                "key vk={vk:#04x} {} t={time} inj={injected} hkl={:08x} layout={index:?} chars={chars:?} shift={} -> swallow={} effects={:?} screen={:?}",
                if down { "down" } else { "up" },
                hkl.unwrap_or(0) as u32,
                self.shift,
                verdict.swallow,
                verdict.effects,
                self.engine.screen()
            );
            self.send(Event::Trace(line));
        }
        for effect in verdict.effects {
            match effect {
                Effect::SwitchLayout(i) => self.jobs.push_back(Job::Switch(self.pair[i as usize])),
                Effect::ToggleLayout => {
                    self.jobs.push_back(Job::Switch(other_of(self.pair, index)))
                }
                Effect::ConvertSelection => self.jobs.push_back(Job::Selection { index }),
                Effect::Replace { backspaces, text } => {
                    self.jobs.push_back(Job::Replace { backspaces, text })
                }
                Effect::Converted { from, to } => {
                    let exe = self.exe.clone();
                    self.send(Event::Converted { from, to, exe });
                }
            }
        }
        if !self.jobs.is_empty() {
            post(WM_JOB);
        }
        verdict.swallow
    }

    fn run_jobs(&mut self) {
        while let Some(job) = self.jobs.pop_front() {
            if tracing() {
                self.send(Event::Trace(format!("job {job:?}")));
            }
            match job {
                Job::Switch(hkl) => {
                    layout::switch_to(hkl);
                    // Seen by the next poll; the tray follows then.
                }
                Job::Replace { backspaces, text } => inject(backspaces, &text),
                Job::Selection { index } => self.convert_selection(index),
            }
        }
    }

    /// [`toggle_layout`]: the window in front before the taskbar takes the
    /// other layout of the pair, and so does the window in front now (the
    /// taskbar): with one input method for all windows (the Windows
    /// default) that is the one that counts. No window is moved.
    fn toggle_from_tray(&mut self) {
        let Some(window) = self.tray_target() else {
            // No program's window at all: the layout of what is in front.
            let index = layout::current().and_then(|h| self.index_of(h));
            let hkl = other_of(self.pair, index);
            if tracing() {
                self.send(Event::Trace(format!(
                    "tray toggle, no window -> {:08x}",
                    hkl as u32
                )));
            }
            layout::switch_to(hkl);
            return;
        };
        let index = self.index_of(layout::layout_of(window));
        let hkl = other_of(self.pair, index);
        if tracing() {
            self.send(Event::Trace(format!(
                "tray toggle {window:#x} {} -> {:08x}",
                class_of(window),
                hkl as u32
            )));
        }
        layout::switch_window(window, hkl);
        layout::switch_to(hkl);
        // The tray's letters follow at once, not at the next poll.
        self.poll_layout();
    }

    /// The window the tray's toggle is for: the last program's window in
    /// front, else (it was closed, minimized, or its coming forward was
    /// missed) the topmost program's window on the screen.
    fn tray_target(&self) -> Option<isize> {
        use windows_sys::Win32::UI::WindowsAndMessaging::IsWindow;
        let noted = self.app_window;
        if noted != 0 && unsafe { IsWindow(noted as _) } != 0 && is_app_window(noted) {
            return Some(noted);
        }
        top_app_window()
    }

    /// The layout the tray shows: the foreground window's, or while the
    /// taskbar is in front (the icon was just clicked), that of the
    /// window the toggle is for.
    fn shown_layout(&self) -> Option<Hkl> {
        let foreground = unsafe { GetForegroundWindow() } as isize;
        if foreground != 0
            && is_taskbar(foreground)
            && let Some(window) = self.tray_target()
        {
            return Some(layout::layout_of(window));
        }
        layout::current()
    }

    /// The selected text converted on a thread of its own (UIA may take a
    /// while, and the hook callback must not wait); no selection, or a
    /// terminal in front: the layout alone.
    fn convert_selection(&mut self, index: Option<u8>) {
        let other = other_of(self.pair, index);
        if crate::selection::is_terminal(&self.exe) {
            layout::switch_to(other);
            return;
        }
        let tables = self.tables.clone();
        let pair = self.pair;
        let exe = self.exe.clone();
        let events = self.events.clone();
        let ctx = self.ctx.clone();
        let trace = tracing();
        let spawned = std::thread::Builder::new()
            .name("selection".into())
            .spawn(move || {
                let text = crate::selection::selected_text();
                if trace {
                    let _ = events.send(Event::Trace(format!("selection {text:?}")));
                }
                let converted = text
                    .as_deref()
                    .map(|t| layout::convert_text(t, &tables, index));
                match (text, converted) {
                    (Some(from), Some((to, Some(target)))) if to != from => {
                        inject(0, &to);
                        layout::switch_to(pair[target as usize]);
                        let _ = events.send(Event::Converted { from, to, exe });
                    }
                    _ => layout::switch_to(other),
                }
                ctx.request_repaint();
            });
        if let Err(e) = spawned {
            log::warn!("selection thread not started: {e}");
            layout::switch_to(other);
        }
    }

    /// The letters of a layout, for the badge.
    fn letters_of(&self, hkl: Hkl) -> String {
        self.layouts.iter().find(|(h, _)| *h == hkl).map_or_else(
            || format!("{:04X}", (hkl as u32) & 0xFFFF),
            |(_, code)| code.clone(),
        )
    }

    fn poll_layout(&mut self) {
        let Some(hkl) = self.shown_layout() else {
            return;
        };
        if self.last_layout != Some(hkl) {
            // The first layout seen (at start, after new settings) is no
            // change to announce.
            let flash = self.last_layout.is_some();
            self.last_layout = Some(hkl);
            let letters = self.letters_of(hkl);
            let index = self.index_of(hkl);
            if let Some(indicator) = &mut self.indicator {
                indicator.set_letters(&letters, index, flash);
            }
            if tracing() {
                self.send(Event::Trace(format!("layout {:08x}", hkl as u32)));
            }
            self.send(Event::Layout {
                hkl,
                index: self.index_of(hkl),
            });
        }
    }

    /// Another window is in front: its program's rule applies, and what
    /// was typed is forgotten.
    fn foreground_changed(&mut self, hwnd: isize) {
        if hwnd != 0 && !is_taskbar(hwnd) {
            self.app_window = if is_app_window(hwnd) { hwnd } else { 0 };
        }
        self.engine.reset();
        if let Some(indicator) = &mut self.indicator {
            indicator.foreground_changed();
            indicator.caret_placed(false);
        }
        self.check_password();
        let exe = process_name(hwnd).unwrap_or_default();
        if tracing() {
            self.send(Event::Trace(format!(
                "foreground {hwnd:#x} {} {exe} app_window={:#x}",
                class_of(hwnd),
                self.app_window
            )));
        }
        if exe != self.exe || self.last_layout.is_none() {
            self.exe = exe;
            self.mode = rule_for(&self.rules, &self.exe);
            self.send(Event::Foreground {
                exe: self.exe.clone(),
                mode: self.mode,
            });
        }
        self.follow_mode();
        match self.mode {
            Some(RuleMode::First) => self.jobs.push_back(Job::Switch(self.pair[0])),
            Some(RuleMode::Second) => self.jobs.push_back(Job::Switch(self.pair[1])),
            _ => {}
        }
        if !self.jobs.is_empty() {
            post(WM_JOB);
        }
        self.poll_layout();
    }
}

/// The taskbar or the tray's overflow, which come forward on a click on
/// the tray icon.
fn is_taskbar(hwnd: isize) -> bool {
    TASKBAR_CLASSES.contains(&class_of(hwnd).as_str())
}

fn class_of(hwnd: isize) -> String {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetClassNameW;
    let mut buf = [0u16; 128];
    let len = unsafe { GetClassNameW(hwnd as _, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

/// A program's window that the tray's toggle may bring back: shown, not
/// minimized, not cloaked (the Start menu and other shell surfaces when
/// closed, windows of other desktops), not a tool window, not the desktop.
fn is_app_window(hwnd: isize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongW, IsIconic, IsWindowVisible, WS_EX_TOOLWINDOW,
    };
    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmGetWindowAttribute(
            hwnd: isize,
            attribute: u32,
            value: *mut core::ffi::c_void,
            size: u32,
        ) -> i32;
    }
    const DWMWA_CLOAKED: u32 = 14;
    if hwnd == 0
        || unsafe { IsWindowVisible(hwnd as _) } == 0
        || unsafe { IsIconic(hwnd as _) } != 0
    {
        return false;
    }
    let ex_style = unsafe { GetWindowLongW(hwnd as _, GWL_EXSTYLE) } as u32;
    if ex_style & WS_EX_TOOLWINDOW != 0 {
        return false;
    }
    let mut cloaked = 0u32;
    let got = unsafe { DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&raw mut cloaked).cast(), 4) };
    if got == 0 && cloaked != 0 {
        return false;
    }
    !DESKTOP_CLASSES.contains(&class_of(hwnd).as_str())
}

const DESKTOP_CLASSES: [&str; 2] = ["Progman", "WorkerW"];

/// The topmost program's window in the z-order ([`is_app_window`], not
/// the taskbar's).
fn top_app_window() -> Option<isize> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GW_HWNDNEXT, GetTopWindow, GetWindow};
    let mut hwnd = unsafe { GetTopWindow(std::ptr::null_mut()) } as isize;
    while hwnd != 0 {
        if !is_taskbar(hwnd) && is_app_window(hwnd) {
            return Some(hwnd);
        }
        hwnd = unsafe { GetWindow(hwnd as _, GW_HWNDNEXT) } as isize;
    }
    None
}

/// The exe file names of the programs with a window on the screen now
/// ([`is_app_window`]), each once, sorted; not this program's. For the
/// Programs page; any thread.
pub fn running_programs() -> Vec<String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows;
    unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> i32 {
        // SAFETY: lparam is the Vec this call made, alive until EnumWindows returns.
        let names = unsafe { &mut *(lparam as *mut Vec<String>) };
        let hwnd = hwnd as isize;
        if !is_taskbar(hwnd)
            && is_app_window(hwnd)
            && let Some(path) = process_name(hwnd)
        {
            names.push(crate::settings::file_name(&path).to_owned());
        }
        1
    }
    let mut names: Vec<String> = Vec::new();
    unsafe { EnumWindows(Some(each), (&raw mut names) as LPARAM) };
    let own = std::env::current_exe()
        .ok()
        .and_then(|e| e.file_name().map(|n| n.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    names.retain(|n| n.to_lowercase() != own);
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup_by_key(|n| n.to_lowercase());
    names
}

const TASKBAR_CLASSES: [&str; 4] = [
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "NotifyIconOverflowWindow",
    "TopLevelWindowForOverflowXamlIsland",
];

/// The layout of the pair the window is not in (the first, when it is in
/// neither).
fn other_of(pair: [Hkl; 2], index: Option<u8>) -> Hkl {
    match index {
        Some(0) => pair[1],
        _ => pair[0],
    }
}

/// Erase `backspaces` characters before the caret and type `text`, as
/// Unicode (`KEYEVENTF_UNICODE`: the same whatever layout the window is
/// in), marked as the program's own so the hook lets it by. A line break
/// is typed as Enter, a tab as Tab.
fn inject(backspaces: usize, text: &str) {
    let mut inputs: Vec<INPUT> = Vec::with_capacity(backspaces * 2 + text.len() * 2);
    let key = |vk: u16, scan: u16, flags: u32| {
        let mut input = INPUT {
            r#type: INPUT_KEYBOARD,
            ..Default::default()
        };
        input.Anonymous.ki.wVk = vk;
        input.Anonymous.ki.wScan = scan;
        input.Anonymous.ki.dwFlags = flags;
        input.Anonymous.ki.dwExtraInfo = MAGIC;
        input
    };
    for _ in 0..backspaces {
        inputs.push(key(VK_BACK, 0, 0));
        inputs.push(key(VK_BACK, 0, KEYEVENTF_KEYUP));
    }
    for ch in text.chars() {
        let control = match ch {
            '\r' => continue,
            '\n' => Some(0x0Du16),
            '\t' => Some(0x09u16),
            _ => None,
        };
        if let Some(vk) = control {
            inputs.push(key(vk, 0, 0));
            inputs.push(key(vk, 0, KEYEVENTF_KEYUP));
            continue;
        }
        let mut units = [0u16; 2];
        for unit in ch.encode_utf16(&mut units) {
            inputs.push(key(0, *unit, KEYEVENTF_UNICODE));
            inputs.push(key(0, *unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
        }
    }
    if inputs.is_empty() {
        return;
    }
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    if sent as usize != inputs.len() {
        log::warn!(
            "SendInput sent {sent} of {} events: {}",
            inputs.len(),
            std::io::Error::last_os_error()
        );
    }
}

/// The full path of the exe of the process owning `hwnd`.
fn process_name(hwnd: isize) -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    if hwnd == 0 {
        return None;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd as _, &mut pid) };
    if pid == 0 {
        return None;
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return None;
    }
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut len) };
    unsafe { CloseHandle(process) };
    (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && lparam != 0 {
        let k = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let injected = k.flags & LLKHF_INJECTED != 0;
        let ours = injected && k.dwExtraInfo == MAGIC;
        if !ours {
            let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let swallow =
                with_state(|s| s.on_key(k.vkCode as u16, down, u64::from(k.time), injected))
                    .unwrap_or(false);
            if swallow {
                return 1;
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0
        && matches!(
            wparam as u32,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
        )
    {
        // The caret may be anywhere now.
        with_state(|s| {
            s.engine.reset();
            if wparam as u32 == WM_LBUTTONDOWN
                && let Some(indicator) = &mut s.indicator
            {
                indicator.caret_placed(true);
            }
        });
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    _child: i32,
    thread: u32,
    _time: u32,
) {
    match event {
        EVENT_OBJECT_LOCATIONCHANGE => {
            if object == OBJID_CARET {
                let rect = crate::indicator::caret_of_thread(thread);
                with_state(|s| {
                    if let Some(indicator) = &mut s.indicator {
                        match rect {
                            Some(rect) => indicator.caret_moved(rect),
                            None => indicator.caret_moved_elsewhere(),
                        }
                    }
                });
            }
        }
        EVENT_SYSTEM_FOREGROUND => {
            with_state(|s| s.foreground_changed(hwnd as isize));
        }
        EVENT_OBJECT_FOCUS => {
            with_state(|s| {
                if tracing() {
                    s.send(Event::Trace(format!("focus {:#x}", hwnd as isize)));
                }
                s.engine.reset();
                s.poll_layout();
                s.check_password();
                s.follow_mode();
                if let Some(indicator) = &mut s.indicator {
                    indicator.caret_placed(false);
                }
            });
        }
        _ => {}
    }
}
