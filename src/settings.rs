//! The settings, as JSON in `%APPDATA%\qswitch\config\settings.json` (or
//! [`PORTABLE_FILE`] next to the exe, see [`location`]), as metalmd keeps
//! its own. Every field has a default (`#[serde(default)]`), so a file of
//! an older or newer version reads with defaults for what it lacks; the
//! file is written through a `.tmp` and a rename, so a crash mid-write
//! leaves the previous one. The window's place is kept here too: eframe's
//! own persistence is off.
//!
//! The settings are saved whenever they change (`App::follow_settings`),
//! not at exit: the program lives in the tray and may be ended from there
//! or by the installer, without a frame of its window.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::engine::{self, Trigger};
use crate::i18n::LangChoice;

/// The number of the last save asked for, and of the last one written.
static SAVES: AtomicU64 = AtomicU64::new(0);
static WRITTEN: Mutex<u64> = Mutex::new(0);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeSetting {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeSetting {
    pub fn preference(self) -> egui::ThemePreference {
        match self {
            ThemeSetting::System => egui::ThemePreference::System,
            ThemeSetting::Light => egui::ThemePreference::Light,
            ThemeSetting::Dark => egui::ThemePreference::Dark,
        }
    }

    pub fn from_preference(p: egui::ThemePreference) -> ThemeSetting {
        match p {
            egui::ThemePreference::System => ThemeSetting::System,
            egui::ThemePreference::Light => ThemeSetting::Light,
            egui::ThemePreference::Dark => ThemeSetting::Dark,
        }
    }
}

/// The window's place, in points of the screen.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// What the program does in the windows of one program.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleMode {
    /// Nothing: no gesture, no tracking (games, remote desktops).
    #[default]
    Off,
    /// The first layout of the pair is put on when the window comes to
    /// the front; the gesture works as usual.
    First,
    /// The same with the second layout.
    Second,
}

impl RuleMode {
    pub const ALL: [RuleMode; 3] = [RuleMode::Off, RuleMode::First, RuleMode::Second];
}

/// The badge with the layout's letters near the caret (`indicator.rs`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndicatorMode {
    Off,
    /// For a moment when the layout changes.
    #[default]
    OnSwitch,
    /// Beside the caret as it moves.
    Always,
}

impl IndicatorMode {
    pub const ALL: [IndicatorMode; 3] = [
        IndicatorMode::Off,
        IndicatorMode::OnSwitch,
        IndicatorMode::Always,
    ];
}

/// The sizes the badge's letters may have, points (the Gestures page's
/// slider; a hand-edited value outside is put back to the default).
pub const INDICATOR_SIZES: std::ops::RangeInclusive<u32> = 6..=32;

/// The badge's colours for the pair until chosen: a blue and a red.
pub const DEFAULT_COLORS: [&str; 2] = ["#2F6FD6", "#C0392B"];

/// `#RRGGBB` as bytes; `None` for anything else.
pub fn color_of(hex: &str) -> Option<[u8; 3]> {
    let digits = hex.strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// Bytes as `#RRGGBB`.
pub fn hex_of(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

/// A rule for one program: by the file name of its exe, case-insensitive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub exe: String,
    pub mode: RuleMode,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeSetting,
    /// Interface language, chosen in About.
    pub language: LangChoice,
    /// Check for a newer release at start-up, at most once a day; off
    /// until turned on in About.
    pub check_updates: bool,
    /// Unix seconds of the last update check that got an answer.
    pub last_update_check: u64,
    pub window: Option<WindowGeometry>,
    /// The window's close button hides it to the tray; off, it ends the
    /// program.
    pub close_to_tray: bool,
    /// The Status page lists the last conversions (in memory only, and
    /// said in red in the toolbar: passwords are converted too); off
    /// until turned on, and then nothing of them is kept.
    pub keep_conversions: bool,
    /// The gesture that converts what was typed.
    pub trigger: Trigger,
    /// Caps Lock switches the layout (Shift+Caps Lock is the real one).
    pub caps_lock_switch: bool,
    /// The badge near the caret.
    pub indicator: IndicatorMode,
    /// How long the badge stays when shown for a moment, milliseconds.
    pub indicator_ms: u64,
    /// The size of the badge's letters, points.
    pub indicator_size: u32,
    /// The badge takes the colour of the layout (`indicator_colors`);
    /// off, it is grey as the taskbar.
    pub indicator_colored: bool,
    /// The badge's colour for each layout of the pair, `#RRGGBB`.
    pub indicator_colors: [String; 2],
    /// Shown for a moment also when the caret is placed (a click, another
    /// field or window), in the `on_switch` mode.
    pub indicator_on_caret: bool,
    /// A tap of the gesture's key: held at most this long, milliseconds.
    pub tap_ms: u64,
    /// The second tap starts within this of the first one's release.
    pub gap_ms: u64,
    /// The pair of layouts the program converts between, as the hex of
    /// their handles (`layout::id`); empty: the first two installed.
    pub layouts: Vec<String>,
    /// Programs treated specially.
    pub rules: Vec<Rule>,
}

impl Default for Settings {
    fn default() -> Self {
        let engine = engine::Config::default();
        Settings {
            theme: ThemeSetting::System,
            language: LangChoice::System,
            check_updates: false,
            last_update_check: 0,
            window: None,
            close_to_tray: true,
            keep_conversions: false,
            trigger: engine.trigger,
            caps_lock_switch: engine.caps_lock_switch,
            indicator: IndicatorMode::OnSwitch,
            indicator_ms: 700,
            indicator_size: 10,
            indicator_colored: false,
            indicator_colors: [DEFAULT_COLORS[0].to_owned(), DEFAULT_COLORS[1].to_owned()],
            indicator_on_caret: true,
            tap_ms: engine.tap_ms,
            gap_ms: engine.gap_ms,
            layouts: Vec::new(),
            rules: vec![
                Rule {
                    exe: "mstsc.exe".into(),
                    mode: RuleMode::Off,
                },
                Rule {
                    exe: "VirtualBoxVM.exe".into(),
                    mode: RuleMode::Off,
                },
                Rule {
                    exe: "vmware-vmx.exe".into(),
                    mode: RuleMode::Off,
                },
                Rule {
                    exe: "vmconnect.exe".into(),
                    mode: RuleMode::Off,
                },
            ],
        }
    }
}

/// Settings file that makes a copy portable: when it lies next to the exe
/// (even empty) and can be written, the settings are kept in it instead of
/// the user profile. The portable zip ships an empty one
/// (`PORTABLE_SETTINGS` in tools/make_release.py).
pub const PORTABLE_FILE: &str = "qswitch.json";

/// Where the settings of this run are kept.
#[derive(Debug)]
pub struct Location {
    /// The settings file, `None` if there is nowhere to keep it.
    pub file: Option<PathBuf>,
    /// `file` is [`PORTABLE_FILE`] next to the exe.
    pub portable: bool,
}

/// Where the settings are kept, decided once per run.
pub fn location() -> &'static Location {
    static LOCATION: OnceLock<Location> = OnceLock::new();
    LOCATION.get_or_init(|| {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf));
        match exe_dir.and_then(|d| portable_file_in(&d)) {
            Some(file) => Location {
                file: Some(file),
                portable: true,
            },
            None => Location {
                file: directories::ProjectDirs::from("", "", crate::APP_NAME)
                    .map(|dirs| dirs.config_dir().join("settings.json")),
                portable: false,
            },
        }
    })
}

/// [`PORTABLE_FILE`] in `dir` if it is there and can be written; a copy on
/// a read-only medium keeps its settings in the profile instead of losing
/// them.
fn portable_file_in(dir: &Path) -> Option<PathBuf> {
    let file = dir.join(PORTABLE_FILE);
    // Opening for appending checks the permission without changing the file.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .ok()
        .map(|_| file)
}

impl Settings {
    /// Reads the settings file; missing, empty or malformed files give
    /// defaults.
    pub fn load() -> Settings {
        location()
            .file
            .as_ref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .map(Settings::checked)
            .unwrap_or_default()
    }

    /// Values a hand-edited file could put out of range, put back.
    fn checked(mut self) -> Settings {
        let d = Settings::default();
        if !(50..=2000).contains(&self.tap_ms) {
            self.tap_ms = d.tap_ms;
        }
        if !(50..=2000).contains(&self.gap_ms) {
            self.gap_ms = d.gap_ms;
        }
        if !(200..=5000).contains(&self.indicator_ms) {
            self.indicator_ms = d.indicator_ms;
        }
        if !(INDICATOR_SIZES).contains(&self.indicator_size) {
            self.indicator_size = d.indicator_size;
        }
        for (color, default) in self.indicator_colors.iter_mut().zip(&d.indicator_colors) {
            if color_of(color).is_none() {
                *color = default.clone();
            }
        }
        if self.layouts.len() != 2 || self.layouts[0] == self.layouts[1] {
            self.layouts.clear();
        }
        self.rules.retain(|r| !r.exe.trim().is_empty());
        self
    }

    /// The badge's part of the settings.
    pub fn indicator_config(&self) -> crate::indicator::Config {
        let d = Settings::default();
        let color = |i: usize| {
            color_of(&self.indicator_colors[i])
                .or_else(|| color_of(&d.indicator_colors[i]))
                .unwrap_or([0, 0, 0])
        };
        crate::indicator::Config {
            mode: self.indicator,
            duration_ms: self.indicator_ms,
            points: self.indicator_size,
            colors: self.indicator_colored.then(|| [color(0), color(1)]),
            on_caret: self.indicator_on_caret,
        }
    }

    /// The machine's part of the settings.
    pub fn engine_config(&self) -> engine::Config {
        engine::Config {
            trigger: self.trigger,
            caps_lock_switch: self.caps_lock_switch,
            tap_ms: self.tap_ms,
            gap_ms: self.gap_ms,
        }
    }

    /// The rule for the program whose exe is `exe` (a path or a file
    /// name); `None` when there is none.
    #[cfg(test)]
    pub fn rule_for(&self, exe: &str) -> Option<RuleMode> {
        rule_for(&self.rules, exe)
    }

    /// Saves a copy on a background thread. Saves are numbered when asked
    /// for, so one asked for later wins even if this one finishes after it.
    pub fn save_in_background(&self) {
        let number = SAVES.fetch_add(1, Ordering::SeqCst) + 1;
        let settings = self.clone();
        std::thread::spawn(move || settings.save_numbered(number));
    }

    /// Saves now, on this thread (at exit).
    pub fn save(&self) {
        self.save_numbered(SAVES.fetch_add(1, Ordering::SeqCst) + 1);
    }

    /// Writes the file unless a later save was written already. One writer
    /// at a time: they share the temporary file.
    fn save_numbered(&self, number: u64) {
        let Some(path) = &location().file else {
            return;
        };
        let mut written = WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
        if *written > number {
            return;
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, path).is_ok() {
                *written = number;
            }
        }
    }
}

/// The rule among `rules` for the program whose exe is `exe` (a path or a
/// file name, case-insensitive); `None` when there is none.
pub fn rule_for(rules: &[Rule], exe: &str) -> Option<RuleMode> {
    let name = file_name(exe).to_lowercase();
    if name.is_empty() {
        return None;
    }
    rules
        .iter()
        .find(|r| file_name(&r.exe).to_lowercase() == name)
        .map(|r| r.mode)
}

/// The file name of a path, or the text itself when it has no separator.
pub fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_of_version_0_1_0_reads_with_defaults_for_new_fields() {
        let json = r#"{ "theme": "dark", "trigger": "pause", "rules": [ { "exe": "Game.EXE", "mode": "off" } ] }"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.theme, ThemeSetting::Dark);
        assert_eq!(settings.trigger, Trigger::Pause);
        assert!(settings.close_to_tray);
        assert!(!settings.keep_conversions);
        assert!(settings.indicator_on_caret);
        assert_eq!(settings.indicator_size, 10);
        assert_eq!(settings.rule_for(r"C:\Games\game.exe"), Some(RuleMode::Off));
        assert_eq!(settings.rule_for("other.exe"), None);
    }

    #[test]
    fn round_trip_and_checks() {
        let mut s = Settings {
            tap_ms: 5,
            indicator_size: 100,
            indicator_colors: ["#12345".into(), "#aBcDeF".into()],
            layouts: vec!["04090409".into()],
            ..Settings::default()
        };
        s.rules.push(Rule {
            exe: "  ".into(),
            mode: RuleMode::First,
        });
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str::<Settings>(&json).unwrap().checked();
        assert_eq!(back.tap_ms, Settings::default().tap_ms);
        assert_eq!(back.indicator_size, Settings::default().indicator_size);
        assert_eq!(back.indicator_colors[0], DEFAULT_COLORS[0]);
        assert_eq!(back.indicator_colors[1], "#aBcDeF");
        assert!(back.layouts.is_empty());
        assert_eq!(back.rules.len(), Settings::default().rules.len());
        let d: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(d, Settings::default());
    }

    #[test]
    fn colors_round_trip() {
        assert_eq!(color_of("#2F6FD6"), Some([0x2F, 0x6F, 0xD6]));
        assert_eq!(color_of("#2f6fd6"), Some([0x2F, 0x6F, 0xD6]));
        assert_eq!(color_of("2F6FD6"), None);
        assert_eq!(color_of("#2F6FD"), None);
        assert_eq!(color_of("#2F6FDG"), None);
        assert_eq!(hex_of([0x2F, 0x6F, 0xD6]), "#2F6FD6");
        let d = Settings::default().indicator_config();
        assert_eq!(d.colors, None);
        let colored = Settings {
            indicator_colored: true,
            ..Settings::default()
        }
        .indicator_config();
        assert_eq!(colored.colors, Some([[0x2F, 0x6F, 0xD6], [0xC0, 0x39, 0x2B]]));
    }

    #[test]
    fn portable_file_only_when_present_and_writable() {
        let dir =
            std::env::temp_dir().join(format!("qswitch_portable_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(PORTABLE_FILE);
        assert_eq!(portable_file_in(&dir), None);
        std::fs::write(&file, "").unwrap();
        assert_eq!(portable_file_in(&dir), Some(file.clone()));
        let mut perm = std::fs::metadata(&file).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&file, perm.clone()).unwrap();
        assert_eq!(portable_file_in(&dir), None);
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        std::fs::set_permissions(&file, perm).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
