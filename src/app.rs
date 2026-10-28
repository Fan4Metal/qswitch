//! The egui window: the eframe `App` with the pieces every app of the
//! family has (theme switch, About, update check, dark caption, settings)
//! and the program's own state: what the hook thread reports, the tray
//! icon, the pages (`ui/pages.rs`). The window is only a view: the
//! program works from the hook thread whether the window is shown or not.

use std::collections::VecDeque;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use egui::{Key, Modifiers, ThemePreference};

use crate::hook::{self, Event};
use crate::layout::{self, Hkl, Layout};
use crate::settings::{RuleMode, Settings, ThemeSetting, WindowGeometry};
use crate::tray::{Tray, TrayCmd};
use crate::ui::about::AboutDialog;
use crate::update::Updates;
use crate::window::{self, Hooked, Remote};

/// A conversion made, for the Status page.
#[derive(Clone, Debug)]
pub struct Conversion {
    pub from: String,
    pub to: String,
    pub exe: String,
    pub at: Instant,
}

/// The pages of the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Status,
    Settings,
    Programs,
}

impl Page {
    pub const ALL: [Page; 3] = [Page::Status, Page::Settings, Page::Programs];

    pub fn label(self) -> &'static str {
        match self {
            Page::Status => tr!("Status", "Состояние"),
            Page::Settings => tr!("Settings", "Настройки"),
            Page::Programs => tr!("Programs", "Программы"),
        }
    }
}

pub struct App {
    pub settings: Settings,
    /// The settings as last saved (or loaded).
    saved: Settings,
    /// Set-up last given to the hook thread.
    setup: hook::Setup,
    pub about: AboutDialog,
    pub updates: Updates,
    /// The window's handle, for the caption; `None` off Windows.
    hwnd: Option<isize>,
    /// The theme the caption was last painted in.
    shown_theme: Option<egui::Theme>,
    /// The caption must be repainted in the next frame (the theme changed).
    refresh_caption: bool,
    /// The window is cloaked until it has painted a few frames: shown
    /// before its first frame reached DWM, it flashed white. The time it is
    /// shown anyway and the frame it was cloaked in.
    cloak: Option<(Instant, u64)>,
    tray: Option<Tray>,
    events: Receiver<Event>,
    pub layouts: Vec<Layout>,
    pub pair: [Hkl; 2],
    /// The foreground window's layout and its place in the pair.
    pub current: Option<(Hkl, Option<u8>)>,
    /// The foreground program (a path) and its rule.
    pub foreground: (String, Option<RuleMode>),
    pub conversions: VecDeque<Conversion>,
    /// Whether the hooks are in place; `None` until the thread says.
    pub hooks: Option<bool>,
    pub page: Page,
    /// The window's new place, applied to the settings once it has rested.
    pending_window: Option<(WindowGeometry, Instant)>,
    /// The Run entry, as read at start and as toggled.
    pub autostart: bool,
    /// The program is ending: the close is not a hide.
    exiting: bool,
    /// Text of the "add a program" field (Programs page).
    pub new_rule: String,
    /// The programs with a window now, for the Programs page, and when
    /// they were listed.
    pub running: Option<(Instant, Vec<String>)>,
    /// The "try here" field of the Status page.
    pub try_text: String,
    /// The trace file while tracing is on (Status page).
    trace: Option<std::io::BufWriter<std::fs::File>>,
}

/// Frames the window paints, cloaked, before it is shown, and the longest
/// it stays cloaked.
const PAINTED_TO_SHOW: u64 = 3;
const UNCLOAK_TIMEOUT: Duration = Duration::from_secs(1);
/// How long the window rests after a move or resize before it is saved.
const WINDOW_REST: Duration = Duration::from_secs(1);
/// Conversions kept for the Status page.
const MAX_CONVERSIONS: usize = 20;

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, settings: Settings, hidden: bool) -> Self {
        cc.egui_ctx.set_theme(settings.theme.preference());
        cc.egui_ctx.options_mut(|o| {
            // Ctrl+plus and Ctrl+minus are the app's, not egui's zoom.
            o.zoom_with_keyboard = false;
            // A double click as slow as Windows allows (egui: 0.3 s).
            o.input_options.max_double_click_delay = crate::win::double_click_time();
            // A wheel notch scrolls 100 points instead of egui's 40, about
            // what Windows and Chromium scroll (the same in every app of
            // the family).
            o.input_options.line_scroll_speed = 100.0;
        });
        // The window is still hidden (eframe shows it after the first
        // frame), so a caption set now is shown from the start; set after
        // egui's first frame, Windows 10 would keep the caption of the
        // Windows theme. Following Windows, winit has already done it.
        let hwnd = crate::caption::hwnd(cc);
        let mut cloak = None;
        if let Some(hwnd) = hwnd {
            if !hidden {
                crate::caption::cloak(hwnd, true);
                cloak = Some((
                    Instant::now() + UNCLOAK_TIMEOUT,
                    cc.egui_ctx.cumulative_frame_nr(),
                ));
            }
            match settings.theme {
                ThemeSetting::Dark => crate::caption::set_dark(hwnd, true),
                ThemeSetting::Light => crate::caption::set_dark(hwnd, false),
                ThemeSetting::System => {}
            }
            if hidden {
                // eframe shows the window after its first frame anyway:
                // cloaked, it is not seen before it is hidden again.
                crate::caption::cloak(hwnd, true);
                window::keep_hidden();
            }
            let ctx = cc.egui_ctx.clone();
            window::hook_window(hwnd, move || ctx.request_repaint());
        }
        crate::instance::listen();
        let mut updates = Updates::new(settings.check_updates, settings.last_update_check);
        updates.start_if_due(&cc.egui_ctx);
        let mut about = AboutDialog::default();
        about.lang = settings.language;

        let layouts = layout::installed();
        let pair = pair_of(&settings, &layouts);
        let setup = hook::Setup {
            config: settings.engine_config(langs_of(pair)),
            pair,
            rules: settings.rules.clone(),
            layouts: layouts.iter().map(|l| (l.hkl, l.code.clone())).collect(),
            indicator: settings.indicator_config(),
        };
        // Else the hook sees no key typed into this window (the "try
        // here" field).
        hook::release_raw_input();
        let events = hook::start(setup.clone(), cc.egui_ctx.clone());
        let tray = hwnd.and_then(Tray::new);
        if tray.is_none() {
            log::warn!("no tray icon");
        }
        let mut app = Self {
            saved: settings.clone(),
            settings,
            setup,
            about,
            updates,
            hwnd,
            shown_theme: None,
            refresh_caption: false,
            cloak,
            tray,
            events,
            layouts,
            pair,
            current: None,
            foreground: (String::new(), None),
            conversions: VecDeque::new(),
            hooks: None,
            page: Page::Status,
            pending_window: None,
            autostart: crate::autostart::is_enabled(),
            exiting: false,
            new_rule: String::new(),
            running: None,
            try_text: String::new(),
            trace: None,
        };
        // QSWITCH_TRACE set: the trace from the first key (a check without
        // the window).
        if std::env::var_os("QSWITCH_TRACE").is_some() {
            app.set_tracing(true);
        }
        app
    }

    /// Where the trace is written: next to the settings.
    pub fn trace_path() -> Option<std::path::PathBuf> {
        crate::settings::location()
            .file
            .as_ref()
            .and_then(|f| f.parent())
            .map(|d| d.join("trace.log"))
    }

    pub fn tracing(&self) -> bool {
        self.trace.is_some()
    }

    /// Start or stop writing the trace (the file is started afresh).
    pub fn set_tracing(&mut self, on: bool) {
        use std::io::Write;
        if on && self.trace.is_none() {
            if let Some(path) = Self::trace_path()
                && let Ok(file) = std::fs::File::create(&path)
            {
                let mut w = std::io::BufWriter::new(file);
                let _ = writeln!(
                    w,
                    "{} {} pair={:08x}/{:08x}",
                    crate::APP_NAME,
                    crate::VERSION,
                    self.pair[0] as u32,
                    self.pair[1] as u32
                );
                self.trace = Some(w);
                hook::set_trace(true);
            }
        } else if !on {
            hook::set_trace(false);
            if let Some(mut w) = self.trace.take() {
                let _ = w.flush();
            }
        }
    }

    /// Show the window once it has painted `PAINTED_TO_SHOW` frames.
    fn uncloak(&mut self, ctx: &egui::Context) {
        let Some((until, frame)) = self.cloak else {
            return;
        };
        if ctx.cumulative_frame_nr() >= frame + PAINTED_TO_SHOW || Instant::now() >= until {
            if let Some(hwnd) = self.hwnd {
                crate::caption::cloak(hwnd, false);
            }
            self.cloak = None;
        } else {
            ctx.request_repaint();
        }
    }

    /// Bring the window caption in line with the theme. egui passes a new
    /// theme to the window at the end of the frame it changed in, and
    /// Windows applies it to the caption only on the next repaint of the
    /// frame, so the frame after the change asks for one.
    fn follow_theme(&mut self, ctx: &egui::Context) {
        let theme = ctx.theme();
        if std::mem::take(&mut self.refresh_caption)
            && let Some(hwnd) = self.hwnd
        {
            crate::caption::set_dark(hwnd, theme == egui::Theme::Dark);
            crate::caption::refresh(hwnd);
        }
        if self.shown_theme.is_some_and(|t| t != theme) {
            self.refresh_caption = true;
            ctx.request_repaint();
        }
        self.shown_theme = Some(theme);
    }

    /// What the hook thread reported.
    fn take_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Layout { hkl, index } => self.current = Some((hkl, index)),
                Event::Foreground { exe, mode } => self.foreground = (exe, mode),
                Event::Converted { from, to, exe } => {
                    if !self.settings.keep_conversions {
                        continue;
                    }
                    self.conversions.push_front(Conversion {
                        from,
                        to,
                        exe,
                        at: Instant::now(),
                    });
                    self.conversions.truncate(MAX_CONVERSIONS);
                }
                Event::Hooks(ok) => self.hooks = Some(ok),
                Event::Trace(line) => {
                    use std::io::Write;
                    if let Some(w) = &mut self.trace {
                        let _ = writeln!(w, "{line}");
                        let _ = w.flush();
                    }
                }
            }
        }
    }

    /// What the window's messages asked for: the tray, a close while
    /// hidden, the session's end, a later start's command.
    fn take_hooked(&mut self, ctx: &egui::Context) {
        for hooked in window::take_hooked() {
            match hooked {
                Hooked::Tray(TrayCmd::Show) | Hooked::Remote(Remote::Show) => self.show(ctx),
                Hooked::Tray(TrayCmd::TogglePause) => self.set_paused(!hook::paused()),
                Hooked::Tray(TrayCmd::ToggleAuto) => {
                    self.settings.auto_switch = !self.settings.auto_switch;
                }
                Hooked::Tray(TrayCmd::Exit) | Hooked::Remote(Remote::Quit) | Hooked::CloseNow => {
                    self.quit()
                }
                Hooked::CloseButton => {
                    if self.settings.close_to_tray && self.tray.is_some() {
                        self.hide();
                    } else {
                        self.quit();
                    }
                }
                Hooked::EndSession => self.save_now(),
                Hooked::TrayCreated => {
                    if let Some(tray) = &mut self.tray {
                        tray.add();
                    }
                }
                Hooked::Remote(Remote::Pause) => self.set_paused(true),
                Hooked::Remote(Remote::Resume) => self.set_paused(false),
            }
        }
    }

    pub fn set_paused(&mut self, paused: bool) {
        hook::set_paused(paused);
        window::set_paused(paused);
    }

    /// The window shown and in front (from the tray, a later start).
    fn show(&mut self, ctx: &egui::Context) {
        if let Some(hwnd) = self.hwnd {
            window::bring_back(hwnd);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.request_repaint();
    }

    /// The window to the tray.
    fn hide(&mut self) {
        if let Some(hwnd) = self.hwnd {
            window::hide(hwnd);
        }
    }

    fn save_now(&mut self) {
        if let Some((geometry, _)) = self.pending_window.take() {
            self.settings.window = Some(geometry);
        }
        self.saved = self.settings.clone();
        self.settings.save();
    }

    /// End the program: settings written, the hooks and the icon removed.
    fn quit(&mut self) -> ! {
        self.exiting = true;
        self.save_now();
        hook::stop();
        self.tray = None;
        std::process::exit(0);
    }

    /// Settings changed in the window: saved, and the hook thread told.
    fn follow_settings(&mut self) {
        if let Some((geometry, at)) = self.pending_window
            && at.elapsed() >= WINDOW_REST
        {
            self.settings.window = Some(geometry);
            self.pending_window = None;
        }
        if self.settings == self.saved {
            return;
        }
        let pair = pair_of(&self.settings, &self.layouts);
        let setup = hook::Setup {
            config: self.settings.engine_config(langs_of(pair)),
            pair,
            rules: self.settings.rules.clone(),
            layouts: self
                .layouts
                .iter()
                .map(|l| (l.hkl, l.code.clone()))
                .collect(),
            indicator: self.settings.indicator_config(),
        };
        if setup != self.setup {
            self.pair = pair;
            hook::reload(setup.clone());
            self.setup = setup;
        }
        self.saved = self.settings.clone();
        self.settings.save_in_background();
    }

    /// The tray icon shows the layout the foreground window is in.
    fn follow_tray(&mut self) {
        let supported = langs_of(self.pair).iter().all(Option::is_some);
        window::set_auto(supported.then_some(self.settings.auto_switch));
        let Some(tray) = &mut self.tray else { return };
        let paused = hook::paused();
        let layout = self
            .current
            .and_then(|(hkl, _)| self.layouts.iter().find(|l| l.hkl == hkl));
        let letters = layout.map_or("··", |l| l.code.as_str());
        let tip = match (layout, paused) {
            (_, true) => tr!(
                format!("{} — paused", crate::APP_NAME),
                format!("{} — пауза", crate::APP_NAME)
            ),
            (Some(l), false) => format!("{} — {}", crate::APP_NAME, l.name),
            (None, false) => crate::APP_NAME.to_owned(),
        };
        let auto = supported && self.settings.auto_switch;
        let tip = if auto && !paused {
            tr!(
                format!("{tip}, automatic switching"),
                format!("{tip}, автопереключение")
            )
        } else {
            tip
        };
        tray.set(letters, paused, auto, &tip);
    }

    /// Note where the window is, to be saved once it rests.
    fn remember_window(&mut self, ctx: &egui::Context) {
        let (outer, inner, minimized) = ctx.input(|i| {
            (
                i.viewport().outer_rect,
                i.viewport().inner_rect,
                i.viewport().minimized,
            )
        });
        if self.cloak.is_some() || minimized == Some(true) {
            return;
        }
        let (Some(outer), Some(inner)) = (outer, inner) else {
            return;
        };
        let geometry = WindowGeometry {
            x: outer.min.x.round(),
            y: outer.min.y.round(),
            width: inner.width().round(),
            height: inner.height().round(),
        };
        if geometry.width < 100.0 || geometry.height < 100.0 {
            return;
        }
        let known = self.pending_window.map(|(g, _)| g).or(self.settings.window);
        if known != Some(geometry) {
            self.pending_window = Some((geometry, Instant::now()));
            ctx.request_repaint_after(WINDOW_REST);
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let paused = hook::paused();
            let label = if paused {
                tr!("Resume", "Продолжить")
            } else {
                tr!("Pause", "Пауза")
            };
            let tip = if paused {
                tr!("Work again", "Снова работать")
            } else {
                tr!(
                    "Let every key through until resumed",
                    "Пропускать все клавиши, пока не продолжить"
                )
            };
            if ui
                .selectable_label(paused, label)
                .on_hover_text(tip)
                .clicked()
            {
                self.set_paused(!paused);
            }
            // The automatic switch, on and off with one click.
            let supported = langs_of(self.pair).iter().all(Option::is_some);
            let auto = self.settings.auto_switch;
            let tip = if !supported {
                tr!(
                    "Automatic switching: for a pair of English and Russian layouts only",
                    "Автопереключение: только для пары английской и русской раскладок"
                )
            } else if auto {
                tr!(
                    "Automatic switching is on: a word typed in the wrong layout is converted at the space after it (click: off)",
                    "Автопереключение включено: слово не в той раскладке переводится по пробелу после него (щелчок: выключить)"
                )
            } else {
                tr!(
                    "Automatic switching is off (click: on)",
                    "Автопереключение выключено (щелчок: включить)"
                )
            };
            if ui
                .add_enabled(
                    supported,
                    egui::Button::selectable(auto, tr!("Auto", "Авто")),
                )
                .on_hover_text(tip)
                .clicked()
            {
                self.settings.auto_switch = !auto;
            }
            // What keeps or writes what was typed is said in red while it
            // is on, whatever page is shown; a click goes to its check box
            // on the Status page.
            let mut warnings = Vec::new();
            if self.tracing() {
                let path = Self::trace_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                warnings.push((
                    tr!("Every key is written to a file!", "Все клавиши пишутся в файл!").to_owned(),
                    tr!(
                        format!("The trace goes to {path}; turn it off on the Status page."),
                        format!("Трассировка пишется в {path}; выключите её на странице «Состояние».")
                    ),
                ));
            }
            if self.settings.keep_conversions {
                warnings.push((
                    tr!("Conversions are kept!", "Преобразования хранятся!").to_owned(),
                    tr!(
                        "The list on the Status page keeps the converted words in memory, passwords among them; turn it off there.",
                        "Список на странице «Состояние» держит в памяти преобразованные слова, в том числе пароли; выключите его там."
                    )
                    .to_owned(),
                ));
            }
            for (text, tip) in warnings {
                ui.add_space(8.0);
                let text = egui::RichText::new(text)
                    .color(ui.visuals().error_fg_color)
                    .strong();
                if ui
                    .add(egui::Label::new(text).sense(egui::Sense::click()))
                    .on_hover_text(tip)
                    .clicked()
                {
                    self.page = Page::Status;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(tag) = self.updates.newer().map(str::to_owned)
                    && crate::ui::update_button(ui, &tag).clicked()
                {
                    crate::win::shell_open(crate::update::release_url(&tag));
                }
                if crate::ui::about_button(ui).clicked() {
                    self.about.toggle();
                }
                let theme = self.settings.theme.preference();
                if crate::ui::theme_button(ui, theme).clicked() {
                    let next = crate::ui::next_theme(theme);
                    self.settings.theme = ThemeSetting::from_preference(next);
                    ui.ctx().set_theme(next);
                }
            });
        });
    }

    /// The layout `index` of the pair, if installed.
    pub fn layout(&self, index: usize) -> Option<&Layout> {
        self.layouts.iter().find(|l| l.hkl == self.pair[index])
    }

    /// The two letters of layout `index` of the pair.
    pub fn code(&self, index: usize) -> String {
        self.layout(index)
            .map_or_else(|| "?".to_owned(), |l| l.code.clone())
    }
}

/// The pair of layouts the settings name, if both are installed; else the
/// first two installed (the same one twice when there is only one).
pub fn pair_of(settings: &Settings, layouts: &[Layout]) -> [Hkl; 2] {
    let find = |id: &str| layouts.iter().find(|l| l.id() == id).map(|l| l.hkl);
    if let [a, b] = settings.layouts.as_slice()
        && let (Some(a), Some(b)) = (find(a), find(b))
    {
        return [a, b];
    }
    let first = layouts.first().map_or(0, |l| l.hkl);
    let second = layouts.get(1).map_or(first, |l| l.hkl);
    [first, second]
}

/// The language of each layout of the pair, for the automatic switch.
pub fn langs_of(pair: [Hkl; 2]) -> [Option<crate::lang::Lang>; 2] {
    pair.map(|hkl| crate::lang::Lang::of_hkl(hkl as usize))
}

impl eframe::App for App {
    /// Before every frame, and while the window is hidden whenever
    /// something wakes the context: the hook thread's events, the tray.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.updates.poll();
        self.take_events();
        self.take_hooked(ctx);
        self.follow_settings();
        self.follow_tray();
    }

    fn ui(&mut self, root_ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root_ui.ctx().clone();
        self.uncloak(&ctx);
        self.follow_theme(&ctx);
        self.remember_window(&ctx);
        // A close that reached winit (not one the subclass took): end.
        if ctx.input(|i| i.viewport().close_requested()) && !self.exiting {
            self.quit();
        }
        // A modal takes the keys while it is open.
        if !self.about.open && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F1)) {
            self.about.toggle();
        }
        // Esc hides the window as the close button does, unless a field
        // or a list has it.
        let busy = ctx.memory(|m| m.focused().is_some()) || ctx.any_popup_open();
        if !self.about.open
            && !busy
            && self.settings.close_to_tray
            && self.tray.is_some()
            && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
        {
            self.hide();
        }
        egui::Panel::top("toolbar").show(root_ui, |ui| self.toolbar(ui));
        egui::Panel::left("pages")
            .resizable(false)
            .default_size(130.0)
            .show(root_ui, |ui| {
                ui.add_space(6.0);
                for page in Page::ALL {
                    if ui
                        .selectable_label(self.page == page, page.label())
                        .clicked()
                    {
                        self.page = page;
                    }
                }
            });
        egui::CentralPanel::default().show(root_ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| match self.page {
                    Page::Status => crate::ui::pages::status(self, ui),
                    Page::Settings => crate::ui::pages::settings(self, ui),
                    Page::Programs => crate::ui::pages::programs(self, ui),
                });
        });
        self.about.show(&ctx, &mut self.updates);
        // About changes the language and the update check in place; the
        // layouts' names follow the language.
        if self.settings.language != self.about.lang {
            self.settings.language = self.about.lang;
            self.layouts = layout::installed();
        }
        self.settings.check_updates = self.updates.enabled;
        self.settings.last_update_check = self.updates.last_check;
        let _ = ThemePreference::System;
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_now();
        hook::stop();
        self.tray = None;
    }
}
