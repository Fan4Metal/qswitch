//! The heart of the program, with no Win32 in it: a state machine fed the
//! keys as the low-level hook sees them, which keeps what was typed since
//! the caret last moved, recognises the gesture (two taps of Shift, a
//! press of Pause) and says what to do: let the key through or swallow
//! it, switch the layout, replace the last word on the screen with the
//! same keys read in the other layout. Pure, so that every path is under
//! tests with made-up layouts.
//!
//! What is on the screen is tracked as a list of [`Typed`] keys, each
//! knowing which character it gives in either layout of the pair and which
//! of the two is shown now. The gesture flips the last word to the other
//! layout; repeated, it flips the word back (the keys know what they typed
//! and what they show, so the gesture is a plain toggle of the last word).

use std::collections::VecDeque;

/// Virtual-key codes the machine knows by number.
#[allow(dead_code)]
pub mod vk {
    pub const BACK: u16 = 0x08;
    pub const TAB: u16 = 0x09;
    pub const RETURN: u16 = 0x0D;
    pub const SHIFT: u16 = 0x10;
    pub const CONTROL: u16 = 0x11;
    pub const MENU: u16 = 0x12;
    pub const PAUSE: u16 = 0x13;
    pub const CAPITAL: u16 = 0x14;
    pub const ESCAPE: u16 = 0x1B;
    pub const SPACE: u16 = 0x20;
    pub const PRIOR: u16 = 0x21;
    pub const NEXT: u16 = 0x22;
    pub const END: u16 = 0x23;
    pub const HOME: u16 = 0x24;
    pub const LEFT: u16 = 0x25;
    pub const UP: u16 = 0x26;
    pub const RIGHT: u16 = 0x27;
    pub const DOWN: u16 = 0x28;
    pub const INSERT: u16 = 0x2D;
    pub const DELETE: u16 = 0x2E;
    pub const LWIN: u16 = 0x5B;
    pub const RWIN: u16 = 0x5C;
    pub const NUMLOCK: u16 = 0x90;
    pub const SCROLL: u16 = 0x91;
    pub const LSHIFT: u16 = 0xA0;
    pub const RSHIFT: u16 = 0xA1;
    pub const LCONTROL: u16 = 0xA2;
    pub const RCONTROL: u16 = 0xA3;
    pub const LMENU: u16 = 0xA4;
    pub const RMENU: u16 = 0xA5;
}

/// How the conversion is asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// Shift pressed and released twice in a row, nothing else between.
    #[default]
    DoubleShift,
    /// The same with Ctrl.
    DoubleCtrl,
    /// Pause/Break (Punto Switcher's key).
    Pause,
    /// No gesture: the program only shows the layout and follows the rules.
    Off,
}

impl Trigger {
    pub const ALL: [Trigger; 4] = [
        Trigger::DoubleShift,
        Trigger::DoubleCtrl,
        Trigger::Pause,
        Trigger::Off,
    ];
}

/// What the machine is set to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub trigger: Trigger,
    /// Caps Lock switches the layout; Shift+Caps Lock is the real Caps Lock.
    pub caps_lock_switch: bool,
    /// A tap: the key held no longer than this, in milliseconds.
    pub tap_ms: u64,
    /// Two taps make the gesture when the second starts within this of
    /// the first's release.
    pub gap_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            trigger: Trigger::DoubleShift,
            caps_lock_switch: false,
            tap_ms: 250,
            gap_ms: 350,
        }
    }
}

/// A key as the hook saw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub vk: u16,
    pub down: bool,
    /// Milliseconds, any monotonic origin (`KBDLLHOOKSTRUCT::time`).
    pub time: u64,
    /// The character this key puts on the screen in each layout of the
    /// pair with the modifiers as they are now; `None` for a key that
    /// types nothing (or something no single character stands for).
    pub chars: [Option<char>; 2],
    /// Which layout of the pair the window is in; `None` for a layout
    /// outside the pair (nothing is tracked then).
    pub layout: Option<u8>,
}

/// What the machine asks the hook thread to do, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Put the foreground window into this layout of the pair.
    SwitchLayout(u8),
    /// The other layout of the pair than the window is in now.
    ToggleLayout,
    /// Nothing typed: convert the selected text if there is one (the
    /// hook thread reads it), else the layout alone.
    ConvertSelection,
    /// Erase this many characters before the caret and type `text`.
    Replace { backspaces: usize, text: String },
    /// For the window's list: what was on the screen and what is now.
    Converted { from: String, to: String },
}

/// The machine's answer to a key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    /// The key is not to reach the window.
    pub swallow: bool,
    pub effects: Vec<Effect>,
}

/// A key on the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Typed {
    chars: [Option<char>; 2],
    /// The layout it was typed in.
    typed: u8,
    /// The layout its character is shown in now.
    shown: u8,
}

impl Typed {
    fn char_in(&self, layout: u8) -> char {
        // A key with no character in the other layout keeps its own.
        self.chars[layout as usize]
            .or(self.chars[self.typed as usize])
            .unwrap_or(' ')
    }

    fn on_screen(&self) -> char {
        self.char_in(self.shown)
    }

    fn is_space(&self) -> bool {
        self.chars[self.typed as usize] == Some(' ')
    }
}

/// One tap of a modifier that is being watched for the gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Tap {
    vk: u16,
    down_at: u64,
}

/// Longest line kept; older keys are forgotten first.
const MAX_LINE: usize = 200;

#[derive(Debug)]
pub struct Engine {
    config: Config,
    enabled: bool,
    /// Keys typed since the last reset, in order, spaces included.
    line: VecDeque<Typed>,
    /// Keys held now, by virtual key.
    held: Vec<u16>,
    /// A tap in progress or just finished, waiting for its twin.
    tap: Option<Tap>,
    /// The first of two taps, finished, waiting for the second.
    first_tap_up: Option<(u16, u64)>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new(Config::default())
    }
}

impl Engine {
    pub fn new(config: Config) -> Self {
        Engine {
            config,
            enabled: true,
            line: VecDeque::new(),
            held: Vec::new(),
            tap: None,
            first_tap_up: None,
        }
    }

    pub fn set_config(&mut self, config: Config) {
        if self.config != config {
            self.config = config;
            self.reset();
        }
    }

    /// Off, the machine lets every key through and keeps nothing (a
    /// window the rules exclude, a password field, the program paused).
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.reset();
        }
    }

    /// Forget the keys noted as held that are up now (`down` says which
    /// are down): their release went where the hook does not see it (the
    /// lock screen after Win+L, Ctrl+Alt+Del, an elevated window), and a
    /// key left held would keep every Shift from being a tap alone, and
    /// every letter a shortcut, until it is pressed again.
    pub fn forget_released(&mut self, down: impl Fn(u16) -> bool) {
        self.held.retain(|&v| down(v));
    }

    /// Forget the line: the caret moved (a click, the focus changed).
    pub fn reset(&mut self) {
        self.line.clear();
        self.tap = None;
        self.first_tap_up = None;
    }

    /// What is on the screen since the last reset, as the machine sees it.
    pub fn screen(&self) -> String {
        self.line.iter().map(Typed::on_screen).collect()
    }

    /// A key seen by the hook; what to do with it.
    pub fn key(&mut self, key: Key) -> Verdict {
        let mut verdict = Verdict::default();
        // A down of a key already held is auto-repeat.
        let repeat = key.down && self.held.contains(&key.vk);
        self.track_held(key);
        if !self.enabled {
            return verdict;
        }
        if self.config.caps_lock_switch && key.vk == vk::CAPITAL && !self.shift_held() {
            // Caps Lock turns into the layout switch; with Shift it stays
            // itself (the key goes on, Windows toggles the lock).
            verdict.swallow = true;
            if key.down && !repeat {
                verdict.effects.push(Effect::ToggleLayout);
            }
            return verdict;
        }
        if self.config.trigger == Trigger::Pause && key.vk == vk::PAUSE {
            verdict.swallow = true;
            if key.down && !repeat {
                self.gesture(&mut verdict);
            }
            return verdict;
        }
        if self.watch_taps(key, &mut verdict) {
            return verdict;
        }
        if key.down {
            self.typed(key);
        }
        verdict
    }

    /// Keep `held` in step.
    fn track_held(&mut self, key: Key) {
        if key.down {
            if !self.held.contains(&key.vk) {
                self.held.push(key.vk);
            }
        } else {
            self.held.retain(|&v| v != key.vk);
        }
    }

    fn shift_held(&self) -> bool {
        self.held
            .iter()
            .any(|&v| matches!(v, vk::SHIFT | vk::LSHIFT | vk::RSHIFT))
    }

    fn ctrl_held(&self) -> bool {
        self.held
            .iter()
            .any(|&v| matches!(v, vk::CONTROL | vk::LCONTROL | vk::RCONTROL))
    }

    fn alt_or_win_held(&self) -> bool {
        self.held
            .iter()
            .any(|&v| matches!(v, vk::MENU | vk::LMENU | vk::RMENU | vk::LWIN | vk::RWIN))
    }

    /// The keys the configured double tap is made of.
    fn tap_keys(&self) -> &'static [u16] {
        match self.config.trigger {
            Trigger::DoubleShift => &[vk::LSHIFT, vk::RSHIFT, vk::SHIFT],
            Trigger::DoubleCtrl => &[vk::LCONTROL, vk::RCONTROL, vk::CONTROL],
            Trigger::Pause | Trigger::Off => &[],
        }
    }

    /// Follow the taps of the gesture's key. True when the key was one of
    /// them (it goes on to the window in any case: a Shift alone does
    /// nothing there).
    fn watch_taps(&mut self, key: Key, verdict: &mut Verdict) -> bool {
        let keys = self.tap_keys();
        if !keys.contains(&key.vk) {
            // Any other key breaks a tap and the wait for the second.
            self.tap = None;
            self.first_tap_up = None;
            return false;
        }
        if key.down {
            let alone = self.held.iter().all(|&v| v == key.vk);
            match self.first_tap_up {
                // The second tap starts in time, with nothing else held.
                Some((_, up)) if alone && key.time.saturating_sub(up) <= self.config.gap_ms => {
                    self.tap = Some(Tap {
                        vk: key.vk,
                        down_at: key.time,
                    });
                }
                _ => {
                    self.first_tap_up = None;
                    self.tap = if alone {
                        Some(Tap {
                            vk: key.vk,
                            down_at: key.time,
                        })
                    } else {
                        None
                    };
                }
            }
            return true;
        }
        // Up.
        let Some(tap) = self.tap.take() else {
            self.first_tap_up = None;
            return true;
        };
        if tap.vk != key.vk || key.time.saturating_sub(tap.down_at) > self.config.tap_ms {
            self.first_tap_up = None;
            return true;
        }
        if self.first_tap_up.take().is_some() {
            // The second tap finished: the gesture.
            self.gesture(verdict);
        } else {
            self.first_tap_up = Some((key.vk, key.time));
        }
        true
    }

    /// A character key (or a control key) went down.
    fn typed(&mut self, key: Key) {
        if self.tap_keys().contains(&key.vk) {
            return;
        }
        match key.vk {
            vk::SHIFT | vk::LSHIFT | vk::RSHIFT | vk::CAPITAL | vk::NUMLOCK | vk::SCROLL => return,
            vk::CONTROL
            | vk::LCONTROL
            | vk::RCONTROL
            | vk::MENU
            | vk::LMENU
            | vk::RMENU
            | vk::LWIN
            | vk::RWIN => {
                return;
            }
            _ => {}
        }
        if self.alt_or_win_held() {
            // A shortcut: whatever it did, the line is no longer known.
            self.reset();
            return;
        }
        if self.ctrl_held() {
            if key.vk == vk::BACK {
                // Ctrl+Backspace deletes the word before the caret.
                while self.line.back().is_some_and(Typed::is_space) {
                    self.line.pop_back();
                }
                while self.line.back().is_some_and(|t| !t.is_space()) {
                    self.line.pop_back();
                }
            } else {
                self.reset();
            }
            return;
        }
        match key.vk {
            vk::BACK => {
                self.line.pop_back();
                return;
            }
            vk::RETURN
            | vk::TAB
            | vk::ESCAPE
            | vk::PRIOR
            | vk::NEXT
            | vk::END
            | vk::HOME
            | vk::LEFT
            | vk::UP
            | vk::RIGHT
            | vk::DOWN
            | vk::INSERT
            | vk::DELETE => {
                self.reset();
                return;
            }
            _ => {}
        }
        let Some(layout) = key.layout else {
            self.reset();
            return;
        };
        if key.chars[layout as usize].is_none() {
            // A function key, a dead key: the screen is no longer known.
            self.reset();
            return;
        }
        if self.line.len() >= MAX_LINE {
            self.line.pop_front();
        }
        self.line.push_back(Typed {
            chars: key.chars,
            typed: layout,
            shown: layout,
        });
    }

    /// The gesture: flip the last word to the other layout; a word flipped
    /// already goes back as typed.
    fn gesture(&mut self, verdict: &mut Verdict) {
        // The last word: the non-space keys before the trailing spaces.
        let trailing = self.line.iter().rev().take_while(|t| t.is_space()).count();
        let word_len = self
            .line
            .iter()
            .rev()
            .skip(trailing)
            .take_while(|t| !t.is_space())
            .count();
        if word_len == 0 {
            // Nothing typed: the selection, or the layout alone.
            verdict.effects.push(Effect::ConvertSelection);
            return;
        }
        let from = self.screen();
        let start = self.line.len() - trailing - word_len;
        // The word is shown in one layout throughout (typed in one, flipped
        // as a whole), so the last key says where it stands.
        let to = 1 - self.line[start + word_len - 1].shown;
        for t in self.line.iter_mut().skip(start) {
            t.shown = to;
        }
        self.emit_difference(&from, verdict);
        verdict.effects.push(Effect::SwitchLayout(to));
    }

    /// Replace what differs between `from` and the screen now: the common
    /// start stays, the rest is erased and typed again.
    fn emit_difference(&self, from: &str, verdict: &mut Verdict) {
        let to = self.screen();
        let from_chars: Vec<char> = from.chars().collect();
        let to_chars: Vec<char> = to.chars().collect();
        let common = from_chars
            .iter()
            .zip(&to_chars)
            .take_while(|(a, b)| a == b)
            .count();
        let backspaces = from_chars.len() - common;
        let text: String = to_chars[common..].iter().collect();
        if backspaces == 0 && text.is_empty() {
            return;
        }
        verdict.effects.push(Effect::Replace { backspaces, text });
        verdict.effects.push(Effect::Converted {
            from: from_chars[common..]
                .iter()
                .collect::<String>()
                .trim()
                .to_owned(),
            to: to_chars[common..]
                .iter()
                .collect::<String>()
                .trim()
                .to_owned(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up pair of layouts: layout 0 is Latin, layout 1 maps each
    /// Latin letter to the Cyrillic letter on the same key (ЙЦУКЕН).
    fn chars_of(c: char) -> [Option<char>; 2] {
        const EN: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,./";
        const RU: &str = "йцукенгшщзхъфывапролджэячсмитьбю.";
        if c == ' ' {
            return [Some(' '), Some(' ')];
        }
        let lower = c.to_ascii_lowercase();
        let i = EN
            .find(lower)
            .or_else(|| {
                RU.chars()
                    .position(|r| r == lower)
                    .and_then(|p| EN.chars().nth(p).map(|e| EN.find(e).unwrap()))
            })
            .unwrap();
        let (e, r) = (EN.chars().nth(i).unwrap(), RU.chars().nth(i).unwrap());
        if c.is_uppercase() {
            [
                Some(e.to_ascii_uppercase()),
                Some(r.to_uppercase().next().unwrap()),
            ]
        } else {
            [Some(e), Some(r)]
        }
    }

    struct Typer {
        engine: Engine,
        time: u64,
        layout: u8,
    }

    impl Typer {
        fn new(config: Config) -> Self {
            Typer {
                engine: Engine::new(config),
                time: 1000,
                layout: 0,
            }
        }

        fn press(&mut self, vk: u16, hold: u64) -> Vec<Effect> {
            let down = self.engine.key(Key {
                vk,
                down: true,
                time: self.time,
                chars: [None, None],
                layout: Some(self.layout),
            });
            self.time += hold;
            let up = self.engine.key(Key {
                vk,
                down: false,
                time: self.time,
                chars: [None, None],
                layout: Some(self.layout),
            });
            down.effects.into_iter().chain(up.effects).collect()
        }

        /// Type text with the keys of `layout` 0 (Latin letters name the
        /// keys), each key 40 ms apart.
        fn type_text(&mut self, text: &str) {
            for c in text.chars() {
                let vk = if c == ' ' {
                    vk::SPACE
                } else {
                    0x100 + c as u16
                };
                let chars = chars_of(c);
                self.engine.key(Key {
                    vk,
                    down: true,
                    time: self.time,
                    chars,
                    layout: Some(self.layout),
                });
                self.engine.key(Key {
                    vk,
                    down: false,
                    time: self.time + 20,
                    chars,
                    layout: Some(self.layout),
                });
                self.time += 40;
            }
        }

        fn wait(&mut self, ms: u64) {
            self.time += ms;
        }

        fn double_shift(&mut self) -> Vec<Effect> {
            let mut effects = self.press(vk::LSHIFT, 60);
            self.time += 80;
            effects.extend(self.press(vk::LSHIFT, 60));
            effects
        }
    }

    fn replace(effects: &[Effect]) -> Option<(usize, String)> {
        effects.iter().find_map(|e| match e {
            Effect::Replace { backspaces, text } => Some((*backspaces, text.clone())),
            _ => None,
        })
    }

    #[test]
    fn double_shift_converts_the_last_word_and_switches() {
        let mut t = Typer::new(Config::default());
        t.type_text("ghbdtn rfr ltkf");
        assert_eq!(t.engine.screen(), "ghbdtn rfr ltkf");
        let effects = t.double_shift();
        assert_eq!(replace(&effects), Some((4, "дела".to_owned())));
        assert!(effects.contains(&Effect::SwitchLayout(1)));
        assert!(effects.contains(&Effect::Converted {
            from: "ltkf".into(),
            to: "дела".into()
        }));
        assert_eq!(t.engine.screen(), "ghbdtn rfr дела");
    }

    #[test]
    fn the_gesture_repeated_puts_the_word_back_and_leaves_the_rest() {
        let mut t = Typer::new(Config::default());
        t.type_text("ghbdtn rfr ltkf");
        t.double_shift();
        let effects = t.double_shift();
        assert_eq!(replace(&effects), Some((4, "ltkf".to_owned())));
        assert!(effects.contains(&Effect::SwitchLayout(0)));
        assert_eq!(t.engine.screen(), "ghbdtn rfr ltkf");
        // And forth again.
        let effects = t.double_shift();
        assert_eq!(replace(&effects), Some((4, "дела".to_owned())));
        assert!(effects.contains(&Effect::SwitchLayout(1)));
    }

    #[test]
    fn a_word_typed_on_after_the_gesture_flips_as_a_whole() {
        let mut t = Typer::new(Config::default());
        t.type_text("ghb");
        t.double_shift();
        assert_eq!(t.engine.screen(), "при");
        // The layout is now 1: the rest of the word is typed there.
        t.layout = 1;
        t.type_text("мун");
        assert_eq!(t.engine.screen(), "примун");
        let effects = t.double_shift();
        assert_eq!(replace(&effects), Some((6, "ghbvey".to_owned())));
        assert!(effects.contains(&Effect::SwitchLayout(0)));
        t.layout = 0;
        let effects = t.double_shift();
        assert_eq!(replace(&effects), Some((6, "примун".to_owned())));
    }

    #[test]
    fn one_word_goes_back_on_the_second_press() {
        let mut t = Typer::new(Config::default());
        t.type_text("ghbdtn");
        t.double_shift();
        let effects = t.double_shift();
        assert_eq!(replace(&effects), Some((6, "ghbdtn".to_owned())));
        assert!(effects.contains(&Effect::SwitchLayout(0)));
    }

    #[test]
    fn a_late_repeat_puts_the_word_back_too() {
        let mut t = Typer::new(Config::default());
        t.type_text("ghbdtn rfr");
        t.double_shift();
        assert_eq!(t.engine.screen(), "ghbdtn как");
        t.wait(10_000);
        let effects = t.double_shift();
        assert_eq!(replace(&effects), Some((3, "rfr".to_owned())));
        assert_eq!(t.engine.screen(), "ghbdtn rfr");
    }

    #[test]
    fn trailing_spaces_belong_to_no_word() {
        let mut t = Typer::new(Config::default());
        t.type_text("ghbdtn  ");
        let effects = t.double_shift();
        assert_eq!(replace(&effects), Some((8, "привет  ".to_owned())));
        assert_eq!(t.engine.screen(), "привет  ");
    }

    #[test]
    fn nothing_typed_goes_for_the_selection() {
        let mut t = Typer::new(Config::default());
        let effects = t.double_shift();
        assert_eq!(effects, vec![Effect::ConvertSelection]);
        t.type_text("   ");
        assert_eq!(t.double_shift(), vec![Effect::ConvertSelection]);
    }

    #[test]
    fn typing_in_the_other_layout_converts_to_latin() {
        let mut t = Typer::new(Config::default());
        t.layout = 1;
        t.type_text("зкштеа");
        assert_eq!(t.engine.screen(), "зкштеа");
        let effects = t.double_shift();
        assert_eq!(replace(&effects), Some((6, "printf".to_owned())));
        assert!(effects.contains(&Effect::SwitchLayout(0)));
    }

    #[test]
    fn a_key_whose_release_was_missed_is_forgotten() {
        let mut t = Typer::new(Config::default());
        // Win+L: the lock screen takes the releases.
        for vk in [0x5B, 0x14C] {
            t.engine.key(Key {
                vk,
                down: true,
                time: t.time,
                chars: [None, None],
                layout: Some(0),
            });
        }
        t.wait(5000);
        t.type_text("ghbdtn");
        assert!(replace(&t.double_shift()).is_none());
        // The keyboard says nothing is down: the gesture works again.
        t.engine.forget_released(|_| false);
        t.type_text("ghbdtn");
        assert_eq!(replace(&t.double_shift()), Some((6, "привет".to_string())));
    }

    #[test]
    fn a_slow_tap_or_a_key_between_is_no_gesture() {
        let mut t = Typer::new(Config::default());
        t.type_text("ghbdtn");
        assert!(t.press(vk::LSHIFT, 400).is_empty());
        t.wait(50);
        assert!(t.press(vk::LSHIFT, 60).is_empty());
        // Too long between the taps.
        t.press(vk::LSHIFT, 60);
        t.wait(600);
        assert!(t.press(vk::LSHIFT, 60).is_empty());
        // A letter between them.
        t.press(vk::LSHIFT, 60);
        t.type_text("a");
        assert!(t.press(vk::LSHIFT, 60).is_empty());
        // Shift held with a letter is not a tap.
        t.engine.key(Key {
            vk: vk::LSHIFT,
            down: true,
            time: t.time,
            chars: [None, None],
            layout: Some(0),
        });
        t.time += 30;
        t.type_text("A");
        t.time += 30;
        t.engine.key(Key {
            vk: vk::LSHIFT,
            down: false,
            time: t.time,
            chars: [None, None],
            layout: Some(0),
        });
        t.wait(50);
        assert!(t.press(vk::LSHIFT, 60).is_empty());
        assert_eq!(t.engine.screen(), "ghbdtnaA");
    }

    #[test]
    fn backspace_and_navigation_keep_the_screen_right() {
        let mut t = Typer::new(Config::default());
        t.type_text("ghbdtn");
        t.press(vk::BACK, 20);
        t.press(vk::BACK, 20);
        assert_eq!(t.engine.screen(), "ghbd");
        t.press(vk::LEFT, 20);
        assert_eq!(t.engine.screen(), "");
        t.type_text("rfr ltkf");
        // Ctrl+Backspace removes the word.
        t.engine.key(Key {
            vk: vk::LCONTROL,
            down: true,
            time: t.time,
            chars: [None, None],
            layout: Some(0),
        });
        t.press(vk::BACK, 20);
        t.engine.key(Key {
            vk: vk::LCONTROL,
            down: false,
            time: t.time,
            chars: [None, None],
            layout: Some(0),
        });
        assert_eq!(t.engine.screen(), "rfr ");
        // A shortcut: nothing is known any more.
        t.engine.key(Key {
            vk: vk::LCONTROL,
            down: true,
            time: t.time,
            chars: [None, None],
            layout: Some(0),
        });
        t.type_text("v");
        t.engine.key(Key {
            vk: vk::LCONTROL,
            down: false,
            time: t.time,
            chars: [None, None],
            layout: Some(0),
        });
        assert_eq!(t.engine.screen(), "");
    }

    #[test]
    fn caps_lock_switches_and_is_swallowed() {
        let mut t = Typer::new(Config {
            caps_lock_switch: true,
            ..Config::default()
        });
        let down = t.engine.key(Key {
            vk: vk::CAPITAL,
            down: true,
            time: 0,
            chars: [None, None],
            layout: Some(0),
        });
        assert_eq!(
            down,
            Verdict {
                swallow: true,
                effects: vec![Effect::ToggleLayout]
            }
        );
        let up = t.engine.key(Key {
            vk: vk::CAPITAL,
            down: false,
            time: 10,
            chars: [None, None],
            layout: Some(0),
        });
        assert_eq!(
            up,
            Verdict {
                swallow: true,
                effects: vec![]
            }
        );
        // With Shift it is the real Caps Lock.
        t.engine.key(Key {
            vk: vk::LSHIFT,
            down: true,
            time: 20,
            chars: [None, None],
            layout: Some(0),
        });
        let down = t.engine.key(Key {
            vk: vk::CAPITAL,
            down: true,
            time: 30,
            chars: [None, None],
            layout: Some(0),
        });
        assert_eq!(down, Verdict::default());
    }

    #[test]
    fn pause_key_is_the_gesture_when_chosen() {
        let mut t = Typer::new(Config {
            trigger: Trigger::Pause,
            ..Config::default()
        });
        t.type_text("ghbdtn");
        let down = t.engine.key(Key {
            vk: vk::PAUSE,
            down: true,
            time: t.time,
            chars: [None, None],
            layout: Some(0),
        });
        assert!(down.swallow);
        assert_eq!(replace(&down.effects), Some((6, "привет".to_owned())));
        // Two Shifts do nothing now.
        assert!(t.double_shift().is_empty());
    }

    #[test]
    fn disabled_engine_passes_everything() {
        let mut t = Typer::new(Config {
            caps_lock_switch: true,
            ..Config::default()
        });
        t.engine.set_enabled(false);
        t.type_text("ghbdtn");
        assert_eq!(t.engine.screen(), "");
        assert!(t.double_shift().is_empty());
        let down = t.engine.key(Key {
            vk: vk::CAPITAL,
            down: true,
            time: t.time,
            chars: [None, None],
            layout: Some(0),
        });
        assert_eq!(down, Verdict::default());
    }

    #[test]
    fn a_layout_outside_the_pair_tracks_nothing() {
        let mut t = Typer::new(Config::default());
        t.type_text("ghb");
        t.engine.key(Key {
            vk: 0x141,
            down: true,
            time: t.time,
            chars: [Some('a'), Some('ф')],
            layout: None,
        });
        assert_eq!(t.engine.screen(), "");
    }

    #[test]
    fn the_line_is_capped() {
        let mut t = Typer::new(Config::default());
        for _ in 0..30 {
            t.type_text("ghbdtnrfr ");
        }
        assert_eq!(t.engine.line.len(), MAX_LINE);
    }
}
