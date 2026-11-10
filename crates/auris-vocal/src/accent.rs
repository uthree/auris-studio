//! Accent phrases: the spoken shape of a lyric, for a melody not to contradict.
//!
//! Tokyo-dialect Japanese gives every phrase a pitch accent: a phrase of *m* moras has an
//! accent type in `0..=m`, where type 0 (平板) never falls and type *k* falls immediately
//! after mora *k* — the nucleus. 端・箸・橋 are the same phonemes wearing types 0, 1 and 2,
//! told apart in speech by nothing but that contour, which is why a melody that contradicts
//! it can sing one word and mean another. Orpheus (Fukayama & Sagayama et al., IPSJ Journal
//! 54(5), 2013) turned that observation into a per-mora melodic constraint and reported 94%
//! of its accent phrases keeping the spoken direction; [`accent_contour`] is that table.
//!
//! The output vocabulary is [`Contour`], which names no language — this module is the
//! Japanese *producer* of it, and a melody writer is a consumer. Another language's prosody
//! would be another producer beside this one, not a change to either end.

use auris_core::theory::contour::Contour;

use crate::kana::spoken_moras;

/// One mora of a sung phrase: the text a note shows, and the phonemes it sings.
#[derive(Clone, Debug, PartialEq)]
pub struct SungMora {
    /// The mora as written — きょ, っ, ー.
    pub text: String,
    /// Its IPA phonemes, the same tokens a note's phoneme list carries.
    pub phonemes: Vec<String>,
}

/// One accent phrase: the moras said in one pitch gesture, and where that gesture falls.
#[derive(Clone, Debug, PartialEq)]
pub struct AccentPhrase {
    /// The moras, in order.
    pub moras: Vec<SungMora>,
    /// The accent nucleus — `Some(0)` is 平板 (no fall), `Some(k)` falls right after mora
    /// `k` (1-based), `None` means nobody analysed it and the prosody constrains nothing.
    pub accent: Option<usize>,
}

impl AccentPhrase {
    /// What each mora asks of the melodic step arriving at it.
    pub fn contour(&self) -> Vec<Contour> {
        match self.accent {
            Some(accent) => accent_contour(self.moras.len(), accent),
            None => vec![Contour::Free; self.moras.len()],
        }
    }
}

/// The per-mora melodic constraint of an accent phrase of `moras` moras with nucleus
/// `accent` — Orpheus's reading of the Tokyo dialect, as a pure function.
///
/// Entry `i` constrains the step from mora `i - 1` to mora `i`, so entry 0 — a step from
/// outside the phrase — is always free. Within the phrase: the voice rises onto the second
/// mora (unless the fall is already due there, 頭高), falls exactly once, immediately after
/// the nucleus, and otherwise must not fall — a second fall would be heard as a second
/// accent no word has. A nucleus on the final mora (尾高) falls onto the *next* word in
/// speech, so nothing inside the phrase carries its fall.
pub fn accent_contour(moras: usize, accent: usize) -> Vec<Contour> {
    (0..moras)
        .map(|at| {
            if at == 0 {
                Contour::Free
            } else if accent >= 1 && at == accent {
                Contour::Fall
            } else if at == 1 {
                Contour::Rise
            } else {
                Contour::NoFall
            }
        })
        .collect()
}

/// A kana phrase read with no dictionary: real moras, honest ignorance about the accent.
///
/// `None` where the text is not all kana — that text needs the dictionary for its phonemes,
/// never mind its accent. The phrase's `accent` is `None` rather than a guess, because a
/// guessed nucleus would *constrain* the melody toward a contour nobody verified. The phonemes
/// are the phrase's spoken ones — a word-final は sings *wa* — by the same rule as
/// [`spoken_kana_lyric`](crate::spoken_kana_lyric) without a dictionary.
pub fn kana_accent_phrase(text: &str) -> Option<AccentPhrase> {
    let moras = spoken_moras(text, None)?
        .into_iter()
        .map(|(text, phonemes)| SungMora { text, phonemes })
        .collect::<Vec<_>>();
    (!moras.is_empty()).then_some(AccentPhrase {
        moras,
        accent: None,
    })
}

impl AccentPhrase {
    /// The spoken pitch level of each mora, `true` for high, or `None` when the accent is
    /// unknown; see [`accent_levels`].
    pub fn levels(&self) -> Option<Vec<bool>> {
        self.accent
            .map(|accent| accent_levels(self.moras.len(), accent))
    }
}

/// The high/low pitch of each mora of an accent phrase of `moras` moras with nucleus
/// `accent`, in the binary form the singing literature measures melodies against.
///
/// 頭高 (type 1) starts high and drops after the first mora; every other type starts low,
/// is high from the second mora, and drops after the nucleus — or, for 平板 (type 0),
/// never. This is the level view of the same rule [`accent_contour`] states as steps: where
/// consecutive levels differ, speech rises or falls.
pub fn accent_levels(moras: usize, accent: usize) -> Vec<bool> {
    (0..moras)
        .map(|at| match accent {
            1 => at == 0,
            0 => at > 0,
            nucleus => at > 0 && at < nucleus,
        })
        .collect()
}

/// The mark written after an accent nucleus: `は＼し` is 箸, `はし＼` is 橋, `はし` is 端.
///
/// The convention of the accent dictionaries, in which a phrase without a mark is 平板. The
/// ASCII backslash is read as the same mark, since that is what a keyboard offers.
pub const ACCENT_MARK: char = '＼';

/// Accent phrases written as kana with [`ACCENT_MARK`] after each nucleus, phrases separated
/// by spaces — the form a person or a model reads and corrects.
///
/// `None` when any phrase's accent is unknown: an unmarked phrase would claim 平板.
pub fn accent_notation(phrases: &[AccentPhrase]) -> Option<String> {
    let mut words = Vec::with_capacity(phrases.len());
    for phrase in phrases {
        let accent = phrase.accent?;
        let mut word = String::new();
        for (at, mora) in phrase.moras.iter().enumerate() {
            word.push_str(&mora.text);
            if at + 1 == accent {
                word.push(ACCENT_MARK);
            }
        }
        words.push(word);
    }
    Some(words.join(" "))
}

/// Reads [`accent_notation`] back: kana accent phrases separated by whitespace, each with at
/// most one [`ACCENT_MARK`] (or `\`) after its nucleus and none for 平板.
///
/// `None` for anything that is not kana, an empty text, a mark before any mora, or two marks
/// in one phrase — a second fall is a second accent phrase, written with a space.
pub fn parse_accent_notation(text: &str) -> Option<Vec<AccentPhrase>> {
    let mut phrases = Vec::new();
    for word in text.split_whitespace() {
        let marks: Vec<usize> = word
            .char_indices()
            .filter(|(_, c)| *c == ACCENT_MARK || *c == '\\')
            .map(|(at, _)| at)
            .collect();
        let accent = match marks.as_slice() {
            [] => 0,
            [at] => {
                let before = &word[..*at];
                let moras = crate::kana::split_kana_moras(before)?.len();
                if moras == 0 {
                    return None;
                }
                moras
            }
            _ => return None,
        };
        let plain: String = word
            .chars()
            .filter(|c| *c != ACCENT_MARK && *c != '\\')
            .collect();
        let mut phrase = kana_accent_phrase(&plain)?;
        phrase.accent = Some(accent);
        phrases.push(phrase);
    }
    (!phrases.is_empty()).then_some(phrases)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The textbook triple: 端 (0) / 箸 (1) / 橋 (2), two moras each.
    #[test]
    fn hashi_three_ways_is_the_whole_table() {
        // 平板 — rises and never falls.
        assert_eq!(accent_contour(2, 0), [Contour::Free, Contour::Rise]);
        // 頭高 — falls right after the first mora.
        assert_eq!(accent_contour(2, 1), [Contour::Free, Contour::Fall]);
        // 尾高 — rises, and its fall lands beyond the phrase.
        assert_eq!(accent_contour(2, 2), [Contour::Free, Contour::Rise]);
    }

    #[test]
    fn a_nakadaka_phrase_falls_once_and_only_once() {
        // Five moras, nucleus on the third: free, rise, no-fall, fall, no-fall.
        assert_eq!(
            accent_contour(5, 3),
            [
                Contour::Free,
                Contour::Rise,
                Contour::NoFall,
                Contour::Fall,
                Contour::NoFall
            ]
        );
        let falls = accent_contour(9, 4)
            .iter()
            .filter(|c| **c == Contour::Fall)
            .count();
        assert_eq!(falls, 1, "one nucleus, one fall");
    }

    #[test]
    fn a_kana_phrase_carries_its_moras_and_admits_ignorance() {
        let phrase = kana_accent_phrase("きょうも").expect("all kana");
        let texts: Vec<&str> = phrase.moras.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, ["きょ", "う", "も"]);
        assert_eq!(phrase.moras[0].phonemes, ["kʲ", "o"]);
        assert_eq!(phrase.accent, None);
        assert_eq!(
            phrase.contour(),
            vec![Contour::Free; 3],
            "no guessed accent"
        );

        assert_eq!(
            kana_accent_phrase("漢字"),
            None,
            "kanji is the dictionary's"
        );
        assert_eq!(kana_accent_phrase(""), None, "nothing sings nothing");
    }

    #[test]
    fn levels_are_the_textbook_high_low_patterns() {
        let pattern = |moras, accent| -> String {
            accent_levels(moras, accent)
                .iter()
                .map(|high| if *high { 'H' } else { 'L' })
                .collect()
        };
        assert_eq!(pattern(3, 0), "LHH", "平板");
        assert_eq!(pattern(3, 1), "HLL", "頭高");
        assert_eq!(pattern(4, 2), "LHLL", "中高");
        assert_eq!(pattern(3, 3), "LHH", "尾高 falls beyond the phrase");
        assert_eq!(pattern(1, 0), "L");
        assert_eq!(pattern(1, 1), "H");
        // The levels and the contour state one rule: a step changes level exactly where the
        // contour asks for a rise or a fall.
        for moras in 1..7 {
            for accent in 0..=moras {
                let levels = accent_levels(moras, accent);
                let contour = accent_contour(moras, accent);
                for at in 1..moras {
                    match contour[at] {
                        Contour::Rise => assert!(!levels[at - 1] && levels[at]),
                        Contour::Fall => assert!(levels[at - 1] && !levels[at]),
                        _ => assert_eq!(levels[at - 1], levels[at]),
                    }
                }
            }
        }
    }

    #[test]
    fn notation_round_trips_the_three_hashi() {
        for (written, accent) in [("はし", 0), ("は＼し", 1), ("はし＼", 2), ("は\\し", 1)]
        {
            let phrases = parse_accent_notation(written).expect("valid notation");
            assert_eq!(phrases.len(), 1);
            assert_eq!(phrases[0].accent, Some(accent), "{written}");
            assert_eq!(phrases[0].moras.len(), 2);
        }
        let phrases = parse_accent_notation("きょ＼うは  いい てん＼き").unwrap();
        assert_eq!(
            phrases.iter().map(|p| p.accent).collect::<Vec<_>>(),
            [Some(1), Some(0), Some(2)]
        );
        assert_eq!(
            accent_notation(&phrases).as_deref(),
            Some("きょ＼うは いい てん＼き")
        );
        assert_eq!(parse_accent_notation("＼はし"), None, "a mark needs a mora");
        assert_eq!(
            parse_accent_notation("は＼し＼"),
            None,
            "one nucleus a phrase"
        );
        assert_eq!(parse_accent_notation("橋＼"), None, "kana only");
        assert_eq!(parse_accent_notation("  "), None);
        let unknown = kana_accent_phrase("はし").unwrap();
        assert_eq!(accent_notation(&[unknown]), None, "no guessed 平板");
    }
}
