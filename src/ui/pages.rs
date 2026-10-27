//! The window's pages: Status (what the program sees and did), Gestures
//! (the settings of the gesture, the pair, the program's behaviour) and
//! Programs (the rules per exe).

use egui::Ui;

use crate::app::App;
use crate::engine::Trigger;
use crate::settings::{IndicatorMode, Rule, RuleMode, file_name};

/// Width of the combo boxes.
const COMBO: f32 = 260.0;

pub fn status(app: &mut App, ui: &mut Ui) {
    ui.add_space(4.0);
    match app.hooks {
        Some(false) => {
            ui.colored_label(
                ui.visuals().error_fg_color,
                tr!(
                    "The keyboard hook could not be set: the program sees no keys.",
                    "Не удалось установить перехват клавиатуры: программа не видит клавиш."
                ),
            );
        }
        _ if crate::hook::paused() => {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                tr!(
                    "Paused: every key goes through untouched.",
                    "Пауза: все клавиши проходят как есть."
                ),
            );
        }
        _ => {
            ui.label(tr!("Working.", "Работает."));
        }
    }
    if app.layouts.len() < 2 {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            tr!(
                "Only one keyboard layout is installed: there is nothing to switch between.",
                "Установлена только одна раскладка: переключать не между чем."
            ),
        );
    }
    ui.add_space(8.0);
    egui::Grid::new("status")
        .num_columns(2)
        .spacing([16.0, 6.0])
        .show(ui, |ui| {
            ui.weak(tr!("Layout", "Раскладка"));
            let text = match app.current {
                Some((hkl, _)) => match app.layouts.iter().find(|l| l.hkl == hkl) {
                    Some(l) => format!("{} — {}", l.code, l.name),
                    None => format!("{:08x}", hkl as u32),
                },
                None => "—".to_owned(),
            };
            let outside = app.current.is_some_and(|(_, index)| index.is_none());
            ui.horizontal(|ui| {
                ui.label(text);
                if outside {
                    ui.weak(tr!("(outside the pair)", "(вне пары)"));
                }
            });
            ui.end_row();
            ui.weak(tr!("Program", "Программа"));
            let (exe, mode) = &app.foreground;
            ui.horizontal(|ui| {
                if exe.is_empty() {
                    ui.label("—");
                } else {
                    ui.label(file_name(exe)).on_hover_text(exe);
                }
                if let Some(mode) = mode {
                    ui.weak(format!("({})", mode_label(app, *mode)));
                }
            });
            ui.end_row();
            ui.weak(tr!("Pair", "Пара"));
            ui.label(format!("{} / {}", app.code(0), app.code(1)));
            ui.end_row();
        });
    ui.add_space(12.0);
    ui.strong(tr!("Recent conversions", "Последние преобразования"));
    ui.weak(tr!(
        "Kept in memory while the program runs; nothing typed is written to disk.",
        "Хранятся в памяти, пока программа работает; набранное на диск не пишется."
    ));
    ui.add_space(4.0);
    if app.conversions.is_empty() {
        ui.label(tr!(
            "None yet. Type a word in the wrong layout and tap Shift twice.",
            "Пока нет. Наберите слово не в той раскладке и дважды нажмите Shift."
        ));
    }
    egui::Grid::new("conversions")
        .num_columns(3)
        .spacing([16.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            for c in &app.conversions {
                let ago = c.at.elapsed().as_secs();
                ui.weak(if ago < 60 {
                    tr!(format!("{ago} s ago"), format!("{ago} с назад"))
                } else {
                    tr!(
                        format!("{} min ago", ago / 60),
                        format!("{} мин назад", ago / 60)
                    )
                });
                ui.horizontal(|ui| {
                    ui.label(shortened(&c.from));
                    arrow(ui);
                    ui.label(shortened(&c.to));
                });
                ui.weak(file_name(&c.exe));
                ui.end_row();
            }
        });
    if !app.conversions.is_empty() {
        // The "ago" column ticks while the page is open.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));
    }
    ui.add_space(16.0);
    let mut tracing = app.tracing();
    let label = tr!(
        "Write a trace of the keys to a file (for finding a problem)",
        "Писать трассировку клавиш в файл (для поиска проблемы)"
    );
    let path = crate::app::App::trace_path()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    if ui
        .checkbox(&mut tracing, label)
        .on_hover_text(&path)
        .changed()
    {
        app.set_tracing(tracing);
    }
    if tracing {
        ui.weak(tr!(
            format!("Every key goes to {path} while this is on; turn it off afterwards."),
            format!("Пока включено, каждая клавиша пишется в {path}; после проверки выключите.")
        ));
    }
}

pub fn gestures(app: &mut App, ui: &mut Ui) {
    ui.add_space(4.0);
    ui.strong(tr!("Conversion", "Преобразование"));
    ui.add_space(4.0);
    egui::Grid::new("gestures")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label(tr!("Gesture", "Жест"));
            egui::ComboBox::from_id_salt("trigger")
                .width(COMBO)
                .selected_text(trigger_label(app.settings.trigger))
                .show_ui(ui, |ui| {
                    for t in Trigger::ALL {
                        ui.selectable_value(&mut app.settings.trigger, t, trigger_label(t));
                    }
                });
            ui.end_row();
            ui.label(tr!("Layouts", "Раскладки"));
            ui.horizontal(|ui| {
                layout_combo(app, ui, 0);
                ui.label("/");
                layout_combo(app, ui, 1);
            });
            ui.end_row();
        });
    ui.add_space(6.0);
    ui.label(tr!(
        "The gesture converts the last word into the other layout and switches to it. \
         Repeated, it puts the word back, and the layout too.",
        "Жест переводит последнее слово в другую раскладку и переключает её. \
         Повторённый, возвращает слово как было, и раскладку тоже."
    ));
    ui.add_space(12.0);
    ui.strong(tr!("Switching", "Переключение"));
    ui.add_space(4.0);
    ui.checkbox(
        &mut app.settings.caps_lock_switch,
        tr!(
            "Caps Lock switches the layout (Shift+Caps Lock is Caps Lock)",
            "Caps Lock переключает раскладку (Shift+Caps Lock — обычный Caps Lock)"
        ),
    );
    ui.add_space(12.0);
    ui.strong(tr!("Indication", "Индикация"));
    ui.add_space(4.0);
    egui::Grid::new("indication")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label(tr!("Badge near the caret", "Метка у курсора ввода"));
            egui::ComboBox::from_id_salt("indicator")
                .width(COMBO)
                .selected_text(indicator_label(app.settings.indicator))
                .show_ui(ui, |ui| {
                    for m in IndicatorMode::ALL {
                        ui.selectable_value(&mut app.settings.indicator, m, indicator_label(m));
                    }
                });
            ui.end_row();
            if app.settings.indicator == IndicatorMode::OnSwitch {
                ui.label(tr!("Shown for", "Показывается"));
                ui.add(
                    egui::Slider::new(&mut app.settings.indicator_ms, 200..=3000)
                        .suffix(tr!(" ms", " мс")),
                );
                ui.end_row();
            }
        });
    ui.add_space(12.0);
    ui.strong(tr!("Program", "Программа"));
    ui.add_space(4.0);
    if ui
        .checkbox(
            &mut app.autostart,
            tr!(
                "Start with Windows (in the tray)",
                "Запускать вместе с Windows (в трее)"
            ),
        )
        .changed()
        && !crate::autostart::set(app.autostart)
    {
        app.autostart = crate::autostart::is_enabled();
    }
    ui.checkbox(
        &mut app.settings.close_to_tray,
        tr!(
            "The close button hides the window to the tray",
            "Кнопка закрытия прячет окно в трей"
        ),
    );
    ui.add_space(12.0);
    ui.collapsing(tr!("Timing", "Тайминги"), |ui| {
        egui::Grid::new("timing")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label(tr!("A tap, at most", "Нажатие, не дольше"));
                ui.add(
                    egui::Slider::new(&mut app.settings.tap_ms, 50..=1000)
                        .suffix(tr!(" ms", " мс")),
                );
                ui.end_row();
                ui.label(tr!(
                    "Between the taps, at most",
                    "Между нажатиями, не дольше"
                ));
                ui.add(
                    egui::Slider::new(&mut app.settings.gap_ms, 50..=1000)
                        .suffix(tr!(" ms", " мс")),
                );
                ui.end_row();
            });
    });
}

fn layout_combo(app: &mut App, ui: &mut Ui, index: usize) {
    let current = app.code(index);
    let mut chosen: Option<crate::layout::Hkl> = None;
    egui::ComboBox::from_id_salt(("pair", index))
        .width(COMBO / 2.0 + 20.0)
        .selected_text(current)
        .show_ui(ui, |ui| {
            for l in &app.layouts {
                if ui
                    .selectable_label(l.hkl == app.pair[index], format!("{} — {}", l.code, l.name))
                    .clicked()
                {
                    chosen = Some(l.hkl);
                }
            }
        });
    if let Some(hkl) = chosen {
        let mut pair = app.pair;
        pair[index] = hkl;
        if pair[0] != pair[1] {
            app.settings.layouts = vec![crate::layout::id(pair[0]), crate::layout::id(pair[1])];
        }
    }
}

pub fn programs(app: &mut App, ui: &mut Ui) {
    ui.add_space(4.0);
    ui.label(tr!(
        "What the program does in the windows of each program, by the name of its exe.",
        "Что программа делает в окнах каждой программы, по имени её exe."
    ));
    ui.add_space(8.0);
    let mut remove = None;
    egui::Grid::new("rules")
        .num_columns(3)
        .spacing([16.0, 6.0])
        .striped(true)
        .show(ui, |ui| {
            ui.weak(tr!("Program", "Программа"));
            ui.weak(tr!("Behaviour", "Поведение"));
            ui.weak("");
            ui.end_row();
            let n = app.settings.rules.len();
            for i in 0..n {
                let exe = app.settings.rules[i].exe.clone();
                ui.label(&exe);
                let mut mode = app.settings.rules[i].mode;
                egui::ComboBox::from_id_salt(("rule", i))
                    .width(COMBO)
                    .selected_text(mode_label(app, mode))
                    .show_ui(ui, |ui| {
                        for m in RuleMode::ALL {
                            ui.selectable_value(&mut mode, m, mode_label(app, m));
                        }
                    });
                app.settings.rules[i].mode = mode;
                if crate::ui::icon_button(
                    ui,
                    crate::ui::icons::Icon::Delete,
                    false,
                    tr!("Remove", "Удалить"),
                )
                .clicked()
                {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
    if let Some(i) = remove {
        app.settings.rules.remove(i);
    }
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        let field = egui::TextEdit::singleline(&mut app.new_rule)
            .hint_text("program.exe")
            .desired_width(200.0);
        let response = ui.add(field);
        let entered = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (ui.button(tr!("Add", "Добавить")).clicked() || entered)
            && !app.new_rule.trim().is_empty()
        {
            add_rule(app, app.new_rule.trim().to_owned());
            app.new_rule.clear();
        }
        let last = file_name(&app.foreground.0).to_owned();
        if !last.is_empty()
            && ui
                .button(tr!(format!("Add {last}"), format!("Добавить {last}")))
                .clicked()
        {
            add_rule(app, last);
        }
    });
    ui.weak(tr!(
        "The second button adds the program that was in front before this window.",
        "Вторая кнопка добавляет программу, которая была впереди до этого окна."
    ));
}

/// A rule for `exe` with the Off mode, unless there is one already.
fn add_rule(app: &mut App, exe: String) {
    let name = file_name(&exe).to_lowercase();
    if app
        .settings
        .rules
        .iter()
        .any(|r| file_name(&r.exe).to_lowercase() == name)
    {
        return;
    }
    app.settings.rules.push(Rule {
        exe: file_name(&exe).to_owned(),
        mode: RuleMode::Off,
    });
}

/// A painted arrow (the default fonts lack "→").
fn arrow(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
    let color = ui.visuals().weak_text_color();
    crate::ui::icons::paint(
        ui.painter(),
        crate::ui::icons::Icon::Next,
        rect.center(),
        12.0,
        color,
        ui.visuals().panel_fill,
    );
}

/// A selection can be long: the first line, at most 40 characters.
fn shortened(text: &str) -> String {
    let line = text.lines().next().unwrap_or("");
    let mut out: String = line.chars().take(40).collect();
    if out.len() < text.len() {
        out.push('…');
    }
    out
}

fn indicator_label(m: IndicatorMode) -> String {
    match m {
        IndicatorMode::Off => tr!("Off", "Нет").to_owned(),
        IndicatorMode::OnSwitch => tr!(
            "For a moment when the layout changes",
            "На миг при смене раскладки"
        )
        .to_owned(),
        IndicatorMode::Always => {
            tr!("Always, following the caret", "Всегда, рядом с курсором").to_owned()
        }
    }
}

fn trigger_label(t: Trigger) -> String {
    match t {
        Trigger::DoubleShift => tr!("Shift, twice", "Shift дважды").to_owned(),
        Trigger::DoubleCtrl => tr!("Ctrl, twice", "Ctrl дважды").to_owned(),
        Trigger::Pause => "Pause/Break".to_owned(),
        Trigger::Off => tr!("None", "Нет").to_owned(),
    }
}

fn mode_label(app: &App, mode: RuleMode) -> String {
    match mode {
        RuleMode::Off => tr!("Do nothing", "Не вмешиваться").to_owned(),
        RuleMode::First => tr!(
            format!("Start in {}", app.code(0)),
            format!("Начинать в {}", app.code(0))
        ),
        RuleMode::Second => tr!(
            format!("Start in {}", app.code(1)),
            format!("Начинать в {}", app.code(1))
        ),
    }
}
