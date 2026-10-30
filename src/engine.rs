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
    /// The automatic switch; `None`: off, or a pair with a language the
    /// program has no table for.
    pub auto: Option<Auto>,
    /// Signs typed at either end of a word (`"ghbdtn"`: the quotes are
    /// `Э` in the other layout) were meant as typed and are not flipped;
    /// the word's letters are. A word that starts or ends with a letter on
    /// such a key (`,eltn` for `будет`) then keeps the sign.
    pub signs_as_typed: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            trigger: Trigger::DoubleShift,
            caps_lock_switch: false,
            tap_ms: 250,
            gap_ms: 350,
            auto: None,
            signs_as_typed: false,
        }
    }
}

/// The automatic switch: a word typed in the wrong layout is converted at
/// the space after it, as the gesture would (`lang::wrong_layout` decides).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Auto {
    /// The language of each layout of the pair.
    pub langs: [crate::lang::Lang; 2],
    pub confidence: crate::lang::Confidence,
}

/// Characters a word is left alone for: a path, an address, code.
const NOT_A_WORD: &str = "/\\@:#&=%+_~^|<>{}()[]*$`\"";
/// Words shorter than this are not judged: too little to go by.
const MIN_AUTO_LEN: usize = 3;
/// Words the user put back after an automatic conversion are not
/// converted again; this many are remembered (in memory only).
const MAX_EXCEPTIONS: usize = 1000;

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
    /// Flipped by the automatic switch (so the gesture putting it back
    /// is a correction to remember).
    auto: bool,
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

    /// A sign in both layouts of the pair (a slash, a bracket, a colon),
    /// so that it was meant as typed whatever the layout. A hyphen and an
    /// underscore join the parts of a word and are not counted; nor is a
    /// key that is a letter in one layout (`[` → `х`, `'` → `э`).
    fn is_sign(&self) -> bool {
        self.chars.iter().all(|c| {
            c.is_some_and(|c| {
                !c.is_alphanumeric() && !c.is_whitespace() && c != '-' && c != '_'
            })
        })
    }

    /// A sign in the layout it was typed in, whatever it is in the other
    /// (`"` typed in the Latin layout, `Э` in the Cyrillic one).
    fn typed_as_sign(&self) -> bool {
        self.chars[self.typed as usize].is_some_and(|c| !c.is_alphanumeric() && !c.is_whitespace())
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
    /// The automatic switch holds off: a password field, a terminal.
    auto_blocked: bool,
    /// Words (as typed, lower case) the user put back after an automatic
    /// conversion: not converted again.
    exceptions: Vec<String>,
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
            auto_blocked: false,
            exceptions: Vec::new(),
        }
    }

    /// The automatic switch holds off while this is set (a password
    /// field, a terminal); the gesture still works.
    pub fn set_auto_blocked(&mut self, blocked: bool) {
        self.auto_blocked = blocked;
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
            self.typed(key, &mut verdict);
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
    fn typed(&mut self, key: Key, verdict: &mut Verdict) {
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
            auto: false,
        });
        if key.chars[layout as usize] == Some(' ') {
            self.auto_check(layout, verdict);
        }
    }

    /// A space was typed: the word before it, if it reads as one typed in
    /// the wrong layout, is converted as the gesture would convert it,
    /// the space with it (it reaches the window before the replacement).
    fn auto_check(&mut self, layout: u8, verdict: &mut Verdict) {
        let Some(auto) = self.config.auto else {
            return;
        };
        if self.auto_blocked {
            return;
        }
        let other = 1 - layout;
        let end = self.line.len() - 1;
        let word_len = self
            .line
            .iter()
            .rev()
            .skip(1)
            .take_while(|t| !t.is_space())
            .count();
        let start = end - word_len;
        // With the signs at the ends kept, the letters are judged first
        // (`"ghbdtn` → `"привет`), then the whole word (`,eltn` → `будет`).
        let (first, last) = self.letters(start, end);
        // Letters put back once are left alone whatever signs stand by.
        let letters: String = self
            .line
            .range(first..last)
            .map(|t| t.char_in(layout))
            .collect();
        if self.exceptions.contains(&letters.to_lowercase()) {
            return;
        }
        let range = if (first, last) != (start, end) && self.wrong_layout(first, last, layout, auto)
        {
            first..last
        } else if self.wrong_layout(start, end, layout, auto) {
            start..end
        } else {
            return;
        };
        let from = self.screen();
        for t in self.line.range_mut(range) {
            t.shown = other;
            t.auto = true;
        }
        self.emit_difference(&from, verdict);
        verdict.effects.push(Effect::SwitchLayout(other));
    }

    /// The keys `start..end`, typed in `layout`, read as a word typed in
    /// the wrong layout.
    fn wrong_layout(&self, start: usize, end: usize, layout: u8, auto: Auto) -> bool {
        if end - start < MIN_AUTO_LEN {
            return false;
        }
        let other = 1 - layout;
        let word = self.line.range(start..end);
        // Typed in this layout as it stands, nothing flipped, every key
        // with a character in the other layout too.
        if word.clone().any(|t| {
            t.typed != layout || t.shown != layout || t.auto || t.chars[other as usize].is_none()
        }) {
            return false;
        }
        let current: String = word.clone().map(|t| t.char_in(layout)).collect();
        let other_text: String = word.map(|t| t.char_in(other)).collect();
        if current
            .chars()
            .any(|c| c.is_ascii_digit() || NOT_A_WORD.contains(c))
        {
            return false;
        }
        if self.exceptions.contains(&current.to_lowercase()) {
            return false;
        }
        crate::lang::wrong_layout(
            &current,
            &other_text,
            auto.langs[layout as usize],
            auto.langs[other as usize],
            auto.confidence,
        )
    }

    /// The keys of the word `start..end` that flip: all of them, or with
    /// `signs_as_typed` those between the signs typed at either end (an
    /// empty range at `end` when there are signs alone).
    fn letters(&self, start: usize, end: usize) -> (usize, usize) {
        if !self.config.signs_as_typed {
            return (start, end);
        }
        let word = self.line.range(start..end);
        let lead = word.clone().take_while(|t| t.typed_as_sign()).count();
        if start + lead == end {
            return (end, end);
        }
        let tail = word.rev().take_while(|t| t.typed_as_sign()).count();
        (start + lead, end - tail)
    }

    /// The gesture: flip the last word to the other layout; a word flipped
    /// already goes back as typed.
    fn gesture(&mut self, verdict: &mut Verdict) {
        // The last word: the non-space keys before the trailing spaces.
        let trailing = self.line.iter().rev().take_while(|t| t.is_space()).count();
        let run_len = self
            .line
            .iter()
            .rev()
            .skip(trailing)
            .take_while(|t| !t.is_space())
            .count();
        if run_len == 0 {
            // Nothing typed: the selection, or the layout alone.
            verdict.effects.push(Effect::ConvertSelection);
            return;
        }
        // A sign before the letters (`word/ghbdtn`, `/ghbdtn`) was meant
        // as typed and ends the word: what stands before it is left alone.
        // Signs after the letters (`ghbdtn/`) were typed blind with the
        // word and flip with it (`привет.`).
        let run_start = self.line.len() - trailing - run_len;
        let run = self.line.range(run_start..run_start + run_len);
        let trailing_signs = run.clone().rev().take_while(|t| t.is_sign()).count();
        let last_sign = run
            .clone()
            .rev()
            .skip(trailing_signs)
            .position(|t| t.is_sign())
            .map(|p| run_len - trailing_signs - p);
        let start = run_start + last_sign.unwrap_or(0);
        let end = self.line.len() - trailing;
        // With `signs_as_typed`, the signs at either end stay as typed.
        let (first, last) = self.letters(start, end);
        let from = self.screen();
        if first == last {
            // Signs alone: nothing to flip, the layout alone.
            for t in self.line.range_mut(start..end) {
                t.shown = t.typed;
                t.auto = false;
            }
            self.emit_difference(&from, verdict);
            verdict.effects.push(Effect::ToggleLayout);
            return;
        }
        // The letters are shown in one layout throughout (typed in one,
        // flipped as a whole), so the last of them says where they stand.
        let to = 1 - self.line[last - 1].shown;
        // An automatic conversion put back by hand: the word was right as
        // typed, and is not to be converted again.
        if self.line.range(first..last).all(|t| t.auto && t.typed == to) {
            let as_typed: String = self
                .line
                .range(start..end)
                .filter(|t| t.auto)
                .map(|t| t.char_in(to))
                .collect::<String>()
                .to_lowercase();
            if !self.exceptions.contains(&as_typed) {
                if self.exceptions.len() >= MAX_EXCEPTIONS {
                    self.exceptions.remove(0);
                }
                self.exceptions.push(as_typed);
            }
        }
        for (i, t) in self.line.iter_mut().enumerate().skip(start) {
            t.shown = if (first..last).contains(&i) || i >= end {
                to
            } else {
                t.typed
            };
            t.auto = false;
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
        let Some(i) = EN.find(lower).or_else(|| {
            RU.chars()
                .position(|r| r == lower)
                .and_then(|p| EN.chars().nth(p).map(|e| EN.find(e).unwrap()))
        }) else {
            // A digit, a sign: the same in both layouts.
            return [Some(c), Some(c)];
        };
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
        /// keys), each key 40 ms apart; the effects, in order.
        fn type_text(&mut self, text: &str) -> Vec<Effect> {
            let mut effects = Vec::new();
            for c in text.chars() {
                let vk = if c == ' ' {
                    vk::SPACE
                } else {
                    0x100 + c as u16
                };
                let chars = chars_of(c);
                let down = self.engine.key(Key {
                    vk,
                    down: true,
                    time: self.time,
                    chars,
                    layout: Some(self.layout),
                });
                effects.extend(down.effects);
                self.engine.key(Key {
                    vk,
                    down: false,
                    time: self.time + 20,
                    chars,
                    layout: Some(self.layout),
                });
                self.time += 40;
            }
            effects
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
    fn a_sign_before_the_letters_ends_the_word() {
        // The slash was meant: what stands before it stays Latin.
        let mut t = Typer::new(Config::default());
        t.type_text("word/ghbdtn");
        let effects = t.double_shift();
        assert_eq!(replace(&effects), Some((6, "привет".to_owned())));
        assert_eq!(t.engine.screen(), "word/привет");
        let effects = t.double_shift();
        assert_eq!(replace(&effects), Some((6, "ghbdtn".to_owned())));
        assert_eq!(t.engine.screen(), "word/ghbdtn");
        // A sign at the start, a bracket, a chain of signs.
        for (typed, shown) in [
            ("/ghbdtn", "/привет"),
            ("(ghbdtn", "(привет"),
            ("@user:ghbdtn", "@user:привет"),
            ("v2/ghbdtn", "v2/привет"),
        ] {
            let mut t = Typer::new(Config::default());
            t.type_text(typed);
            t.double_shift();
            assert_eq!(t.engine.screen(), shown, "{typed}");
        }
    }

    #[test]
    fn signs_after_the_letters_flip_with_the_word() {
        // `ghbdtn/` was typed blind as `привет.`.
        let mut t = Typer::new(Config::default());
        t.type_text("ghbdtn/");
        let effects = t.double_shift();
        assert_eq!(replace(&effects), Some((7, "привет.".to_owned())));
        assert_eq!(t.engine.screen(), "привет.");
        // Signs alone flip too, as any word.
        let mut t = Typer::new(Config::default());
        t.type_text("//");
        t.double_shift();
        assert_eq!(t.engine.screen(), "..");
    }

    fn signs_as_typed() -> Config {
        Config {
            signs_as_typed: true,
            ..Config::default()
        }
    }

    #[test]
    fn signs_at_the_ends_stay_as_typed_when_asked() {
        // `'` and `[` are э and х in the other layout, but were typed as
        // signs; a letter key inside the word still flips.
        for (typed, shown) in [
            ("'ghbdtn'", "'привет'"),
            ("([ghbdtn]", "([привет]"),
            ("ghbdtn.", "привет."),
            ("ghbdtn/", "привет/"),
            ("vj;tn", "может"),
            ("word/'ghbdtn", "word/'привет"),
        ] {
            let mut t = Typer::new(signs_as_typed());
            t.type_text(typed);
            t.double_shift();
            assert_eq!(t.engine.screen(), shown, "{typed}");
            t.double_shift();
            assert_eq!(t.engine.screen(), typed, "{typed}");
        }
        // Off, they flip with the word.
        let mut t = Typer::new(Config::default());
        t.type_text("'ghbdtn'");
        t.double_shift();
        assert_eq!(t.engine.screen(), "эприветэ");
        // Signs alone: the layout alone.
        let mut t = Typer::new(signs_as_typed());
        t.type_text("''");
        let effects = t.double_shift();
        assert_eq!(effects, vec![Effect::ToggleLayout]);
        assert_eq!(t.engine.screen(), "''");
    }

    #[test]
    fn a_hyphen_an_underscore_and_a_letter_key_join_a_word() {
        let mut t = Typer::new(Config::default());
        t.type_text("rnj-yb,elm");
        t.double_shift();
        assert_eq!(t.engine.screen(), "кто-нибудь");
        // `'` is э in the other layout: a word may start with it.
        let mut t = Typer::new(Config::default());
        t.type_text("'nj");
        t.double_shift();
        assert_eq!(t.engine.screen(), "это");
        // An identifier typed in the Cyrillic layout.
        let mut t = Typer::new(Config::default());
        t.layout = 1;
        t.type_text("ьн_мфк");
        assert_eq!(t.engine.screen(), "ьн_мфк");
        t.double_shift();
        assert_eq!(t.engine.screen(), "my_var");
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

    fn auto_config() -> Config {
        Config {
            auto: Some(Auto {
                langs: [crate::lang::Lang::En, crate::lang::Lang::Ru],
                confidence: crate::lang::Confidence::Normal,
            }),
            ..Config::default()
        }
    }

    #[test]
    fn a_word_typed_in_the_wrong_layout_is_converted_at_the_space() {
        let mut t = Typer::new(auto_config());
        assert!(replace(&t.type_text("ghbdtn")).is_none());
        let effects = t.type_text(" ");
        assert_eq!(replace(&effects), Some((7, "привет ".to_owned())));
        assert!(effects.contains(&Effect::SwitchLayout(1)));
        assert!(effects.contains(&Effect::Converted {
            from: "ghbdtn".into(),
            to: "привет".into()
        }));
        assert_eq!(t.engine.screen(), "привет ");
        // And the other way round.
        t.layout = 1;
        let effects = t.type_text("зкштеа ");
        assert_eq!(replace(&effects), Some((7, "printf ".to_owned())));
        assert!(effects.contains(&Effect::SwitchLayout(0)));
    }

    #[test]
    fn words_typed_right_short_ones_and_code_are_left_alone() {
        let mut t = Typer::new(auto_config());
        assert!(replace(&t.type_text("hello world ")).is_none());
        assert!(replace(&t.type_text("gh ")).is_none());
        assert!(replace(&t.type_text("ghb1 ")).is_none());
        assert!(replace(&t.type_text("c/ghbdtn ")).is_none());
        assert_eq!(t.engine.screen(), "hello world gh ghb1 c/ghbdtn ");
        // Off: nothing is converted.
        let mut t = Typer::new(Config::default());
        assert!(replace(&t.type_text("ghbdtn ")).is_none());
        // Blocked (a password field): nothing either.
        let mut t = Typer::new(auto_config());
        t.engine.set_auto_blocked(true);
        assert!(replace(&t.type_text("ghbdtn ")).is_none());
        t.engine.set_auto_blocked(false);
        assert!(replace(&t.type_text("ghbdtn ")).is_some());
    }

    #[test]
    fn the_gesture_puts_an_automatic_conversion_back_and_remembers_it() {
        let mut t = Typer::new(auto_config());
        t.type_text("ghbdtn ");
        assert_eq!(t.engine.screen(), "привет ");
        let effects = t.double_shift();
        assert_eq!(replace(&effects), Some((7, "ghbdtn ".to_owned())));
        assert!(effects.contains(&Effect::SwitchLayout(0)));
        // The word is now an exception: typed again, it stays.
        t.layout = 0;
        assert!(replace(&t.type_text("Ghbdtn ")).is_none());
        // Another word is still converted.
        assert!(replace(&t.type_text("rfr ")).is_some());
    }

    #[test]
    fn the_automatic_switch_keeps_the_signs_at_the_ends_when_asked() {
        let mut t = Typer::new(Config {
            signs_as_typed: true,
            ..auto_config()
        });
        let effects = t.type_text("'ghbdtn' ");
        assert_eq!(replace(&effects), Some((8, "привет' ".to_owned())));
        assert_eq!(t.engine.screen(), "'привет' ");
        // Put back by the gesture, and remembered.
        t.double_shift();
        assert_eq!(t.engine.screen(), "'ghbdtn' ");
        assert!(replace(&t.type_text("'ghbdtn' ")).is_none());
        // A word whose first letter is on a sign's key is converted whole.
        t.engine.reset();
        t.type_text(",eltn ");
        assert_eq!(t.engine.screen(), "будет ");
    }

    #[test]
    fn a_word_flipped_by_the_gesture_is_not_judged_again() {
        let mut t = Typer::new(auto_config());
        t.type_text("hello");
        t.double_shift();
        assert_eq!(t.engine.screen(), "руддщ");
        t.layout = 1;
        assert!(replace(&t.type_text(" ")).is_none());
    }
}
