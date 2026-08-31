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

use crate::kana::split_kana_lyric;

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
/// guessed nucleus would *constrain* the melody toward a contour nobody verified.
pub fn kana_accent_phrase(text: &str) -> Option<AccentPhrase> {
    let moras = split_kana_lyric(text)?
        .into_iter()
        .map(|(text, phonemes)| SungMora { text, phonemes })
        .collect::<Vec<_>>();
    (!moras.is_empty()).then_some(AccentPhrase {
        moras,
        accent: None,
    })
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
}
