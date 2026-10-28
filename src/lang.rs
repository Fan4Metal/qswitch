//! Which language a word is in, for the automatic switch: a word typed
//! in the wrong layout, read in the layout it was typed in, is a string of
//! letter combinations the language does not have (`ghbdtn` as English,
//! `руддщ` as Russian), while the same keys read in the other layout make
//! a word. Each language has a table of letter trigrams with the cost of
//! each (`-log10` of its probability, times 16, in a byte), built by
//! `tools/make_ngrams.py` from a word frequency list and embedded here.
//! A word's cost is the mean cost of its trigrams, the boundaries
//! included; a word is taken for one typed in the wrong layout when its
//! reading in the other layout costs much less and is plausible by
//! itself. Pure, under tests with real words.

/// A language the program has a table for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    Ru,
}

impl Lang {
    /// The language of a keyboard layout, by the primary language of its
    /// handle's low word; `None` for a language without a table.
    pub fn of_hkl(hkl: usize) -> Option<Lang> {
        match (hkl as u32) & 0x3FF {
            0x09 => Some(Lang::En),
            0x19 => Some(Lang::Ru),
            _ => None,
        }
    }

    /// The letters of the table, in its order; index 0 is the boundary.
    fn alphabet(self) -> &'static str {
        match self {
            Lang::En => "abcdefghijklmnopqrstuvwxyz'",
            Lang::Ru => "абвгдежзийклмнопрстуфхцчшщъыьэюяё",
        }
    }

    fn table(self) -> &'static [u8] {
        match self {
            Lang::En => include_bytes!("../data/ngrams_en.bin"),
            Lang::Ru => include_bytes!("../data/ngrams_ru.bin"),
        }
    }

    /// The index of a letter in the table (lower case), `None` for a
    /// character the language does not write with.
    fn index(self, c: char) -> Option<usize> {
        let c = c.to_lowercase().next().unwrap_or(c);
        self.alphabet().chars().position(|a| a == c).map(|i| i + 1)
    }

    /// The mean cost of a word's trigrams in this language (`-log10` of
    /// their probability), `None` when a character is not a letter of it.
    pub fn cost(self, word: &str) -> Option<f32> {
        let n = self.alphabet().chars().count() + 1;
        let table = self.table();
        let mut seq = vec![0usize, 0];
        for c in word.chars() {
            seq.push(self.index(c)?);
        }
        seq.push(0);
        if seq.len() < 3 {
            return None;
        }
        let total: u32 = seq
            .windows(3)
            .map(|w| u32::from(table[(w[0] * n + w[1]) * n + w[2]]))
            .sum();
        Some(total as f32 / (seq.len() - 2) as f32 / 16.0)
    }
}

/// How sure the automatic switch must be before it converts a word.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    /// Only words the language plainly cannot have.
    Cautious,
    #[default]
    Normal,
    /// Most words typed in the wrong layout, at the price of a rare
    /// wrong guess (a name, slang, an abbreviation).
    Bold,
}

impl Confidence {
    pub const ALL: [Confidence; 3] = [
        Confidence::Cautious,
        Confidence::Normal,
        Confidence::Bold,
    ];

    /// The least margin (the current reading's cost over the other's) and
    /// the greatest cost the other reading may have, per trigram. Chosen
    /// with `tools/make_ngrams.py`: on words the tables have not seen,
    /// the bold pair catches 95 % (English) and 96 % (Russian) of the
    /// words typed in the other layout and misfires on 0.04 % of the
    /// words typed right; the cautious pair catches 68 % and 75 % with
    /// no misfire.
    fn thresholds(self) -> (f32, f32) {
        match self {
            Confidence::Cautious => (2.0, 2.5),
            Confidence::Normal => (1.3, 2.5),
            Confidence::Bold => (1.0, 3.0),
        }
    }
}

/// Whether a word on the screen, `current` as read in the layout it was
/// typed in (a language `cur`), is one typed in the wrong layout: `other`
/// is the same keys read in the other layout (a language `oth`). A word
/// with a character outside its language's letters cannot be that
/// language's word; one whose reading in the other layout has such a
/// character is left alone.
pub fn wrong_layout(
    current: &str,
    other: &str,
    cur: Lang,
    oth: Lang,
    confidence: Confidence,
) -> bool {
    let (margin, plausible) = confidence.thresholds();
    let Some(other_cost) = oth.cost(other) else {
        return false;
    };
    if other_cost > plausible {
        return false;
    }
    match cur.cost(current) {
        // Not a word of the current language at all.
        None => true,
        Some(current_cost) => current_cost - other_cost > margin,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same keys read in the other layout of the QWERTY/ЙЦУКЕН pair.
    fn other_reading(word: &str) -> String {
        const EN: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,.";
        const RU: &str = "йцукенгшщзхъфывапролджэячсмитьбю";
        word.chars()
            .map(|c| {
                let lower = c.to_lowercase().next().unwrap();
                let mapped = if let Some(i) = EN.find(lower) {
                    RU.chars().nth(EN[..i].chars().count()).unwrap()
                } else if let Some(i) = RU.chars().position(|r| r == lower) {
                    EN.chars().nth(i).unwrap()
                } else {
                    lower
                };
                if c.is_uppercase() {
                    mapped.to_uppercase().next().unwrap()
                } else {
                    mapped
                }
            })
            .collect()
    }

    fn wrong_in_en(word: &str, confidence: Confidence) -> bool {
        wrong_layout(word, &other_reading(word), Lang::En, Lang::Ru, confidence)
    }

    fn wrong_in_ru(word: &str, confidence: Confidence) -> bool {
        wrong_layout(word, &other_reading(word), Lang::Ru, Lang::En, confidence)
    }

    #[test]
    fn russian_typed_in_the_latin_layout_is_caught() {
        for word in ["ghbdtn", "rfr", "ltkf", "cgfcb,j", "ghjdthrf", "hfcrkflrf", "Ghbdtn"] {
            assert!(wrong_in_en(word, Confidence::Normal), "{word}");
        }
        assert!(wrong_in_en("hfcrkflrf", Confidence::Cautious));
        assert!(wrong_in_ru("ыщьуерштп", Confidence::Cautious));
    }

    #[test]
    fn english_typed_in_the_russian_layout_is_caught() {
        for word in ["руддщ", "зкштеа", "цщкв", "ыщьуерштп", "пщщв", "Руддщ"] {
            assert!(wrong_in_ru(word, Confidence::Normal), "{word}");
        }
    }

    #[test]
    fn words_typed_right_are_left_alone() {
        for word in ["hello", "world", "something", "the", "printf", "window", "keyboard"] {
            assert!(!wrong_in_en(word, Confidence::Bold), "{word}");
        }
        for word in ["привет", "как", "дела", "спасибо", "проверка", "раскладка", "окно"] {
            assert!(!wrong_in_ru(word, Confidence::Bold), "{word}");
        }
    }

    #[test]
    fn a_reading_with_a_foreign_character_is_no_word() {
        // "zero" in the Russian layout gives "яукщ", fine; but "qwerty"
        // read in Russian is "йцукен", a keyboard row no language has.
        assert!(!wrong_in_en("qwerty", Confidence::Bold));
        assert!(!wrong_in_ru("йцукен", Confidence::Bold));
        // A character outside both alphabets: left alone.
        assert!(!wrong_layout("abc1", "фис1", Lang::En, Lang::Ru, Confidence::Bold));
    }

    #[test]
    fn the_language_of_a_layout() {
        assert_eq!(Lang::of_hkl(0x0409_0409), Some(Lang::En));
        assert_eq!(Lang::of_hkl(0x0419_0419), Some(Lang::Ru));
        assert_eq!(Lang::of_hkl(0xF0C0_0809), Some(Lang::En));
        assert_eq!(Lang::of_hkl(0x0407_0407), None);
    }
}
