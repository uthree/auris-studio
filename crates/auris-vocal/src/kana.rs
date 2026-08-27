//! Kana to phonemes with nothing installed: the table a sung lyric almost always goes through.
//!
//! Lyrics in a piano roll are overwhelmingly written one mora to a note — さ, く, ら — and a
//! mora in kana names its phonemes outright. This module is that table, so the ordinary case
//! works on a machine with no dictionary folder at all; [`g2p`](crate::g2p) falls back to the
//! dictionary only for text this walker cannot read (kanji, digits, Latin).
//!
//! The vocabulary is OpenJTalk's phonemic analysis written in IPA glyphs — し is `ɕ i`, き is
//! `k i` rather than a narrow `kʲ i` — so that a lyric read here and a lyric read through the
//! dictionary come out identical. A voice model is trained on these tokens, and one syllable
//! spelt two ways would be two symbols it has to learn were the same sound.

use crate::phoneme::VOWELS;

/// The phonemes of a kana lyric, or `None` where any of it is not kana.
///
/// `None` rather than a partial answer: a lyric that is half readable is a lyric for the
/// dictionary, and phonemes for half of it would be sung as if they were the whole.
pub fn kana_phonemes(text: &str) -> Option<Vec<String>> {
    walk(text).map(|moras| {
        moras
            .into_iter()
            .flat_map(|(_, phonemes)| phonemes)
            .collect()
    })
}

/// A kana phrase split into moras — the pieces distributed one to a note — or `None` where any
/// of it is not kana.
///
/// きょ stays one mora and っ is a mora of its own, which is exactly how a person lays a word
/// across notes; a prolonged-sound mark ー is the mora of whatever vowel it stretches.
pub fn split_kana_moras(text: &str) -> Option<Vec<String>> {
    walk(text).map(|moras| moras.into_iter().map(|(mora, _)| mora).collect())
}

/// Small kana that glue onto the mora before them.
const SMALL: [char; 9] = ['ゃ', 'ゅ', 'ょ', 'ぁ', 'ぃ', 'ぅ', 'ぇ', 'ぉ', 'ゎ'];

/// Reads a kana string as `(mora text, phonemes)` pairs, or `None` at the first thing that is
/// not kana. Whitespace separates moras and is otherwise ignored.
fn walk(text: &str) -> Option<Vec<(String, Vec<String>)>> {
    let normalized: String = text.chars().map(hiragana).collect();
    let chars: Vec<char> = normalized.chars().filter(|c| !c.is_whitespace()).collect();
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let this = chars[at];
        if this == 'ー' {
            // The stretch of whatever vowel came before it — which is the last phoneme of the
            // previous mora whenever that mora ends in one.
            let vowel = out
                .last()
                .and_then(|(_, phonemes)| phonemes.last())
                .filter(|last| VOWELS.contains(&last.as_str()))?
                .clone();
            out.push(('ー'.to_string(), vec![vowel]));
            at += 1;
            continue;
        }
        let next = chars.get(at + 1).copied();
        if let Some(small) = next.filter(|next| SMALL.contains(next))
            && let Some(phonemes) = digraph(this, small)
        {
            out.push((format!("{this}{small}"), phonemes));
            at += 2;
            continue;
        }
        out.push((this.to_string(), single(this)?));
        at += 1;
    }
    Some(out)
}

/// Katakana folded onto hiragana, so one table serves both spellings.
fn hiragana(c: char) -> char {
    match c {
        // ァ..ヶ sit exactly 0x60 above ぁ..ゖ; ー is shared and stays itself.
        'ァ'..='ヶ' => char::from_u32(c as u32 - 0x60).unwrap_or(c),
        _ => c,
    }
}

/// One or more IPA tokens, allocated.
fn tokens(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// A two-kana mora: base plus small ゃゅょ or a small vowel.
fn digraph(base: char, small: char) -> Option<Vec<String>> {
    // The ゃゅょ rows: a palatal onset and the small kana's vowel.
    let palatal = |onset: &str| -> Option<Vec<String>> {
        let vowel = match small {
            'ゃ' => "a",
            'ゅ' => "ɯ",
            'ょ' => "o",
            _ => return None,
        };
        Some(tokens(&[onset, vowel]))
    };
    let with_vowel = |onset: &str| -> Option<Vec<String>> {
        let vowel = match small {
            'ぁ' => "a",
            'ぃ' => "i",
            'ぅ' => "ɯ",
            'ぇ' => "e",
            'ぉ' => "o",
            _ => return None,
        };
        Some(tokens(&[onset, vowel]))
    };
    match base {
        'き' => palatal("kʲ"),
        'ぎ' => palatal("gʲ"),
        'し' => palatal("ɕ").or_else(|| with_vowel("ɕ")),
        'じ' | 'ぢ' => palatal("dʑ").or_else(|| with_vowel("dʑ")),
        'ち' => palatal("tɕ").or_else(|| with_vowel("tɕ")),
        'に' => palatal("nʲ"),
        'ひ' => palatal("ç"),
        'び' => palatal("bʲ"),
        'ぴ' => palatal("pʲ"),
        'み' => palatal("mʲ"),
        'り' => palatal("ɾʲ"),
        'て' => match small {
            'ぃ' => Some(tokens(&["t", "i"])),
            'ゅ' => Some(tokens(&["tʲ", "ɯ"])),
            _ => None,
        },
        'で' => match small {
            'ぃ' => Some(tokens(&["d", "i"])),
            'ゅ' => Some(tokens(&["dʲ", "ɯ"])),
            _ => None,
        },
        'と' => (small == 'ぅ').then(|| tokens(&["t", "ɯ"])),
        'ど' => (small == 'ぅ').then(|| tokens(&["d", "ɯ"])),
        'つ' => with_vowel("ts"),
        'ふ' => with_vowel("ɸ"),
        'う' => match small {
            'ぃ' => Some(tokens(&["w", "i"])),
            'ぇ' => Some(tokens(&["w", "e"])),
            'ぉ' => Some(tokens(&["w", "o"])),
            _ => None,
        },
        'ゔ' => with_vowel("v"),
        'い' => (small == 'ぇ').then(|| tokens(&["j", "e"])),
        _ => None,
    }
}

/// A single-kana mora.
fn single(c: char) -> Option<Vec<String>> {
    let phonemes: &[&str] = match c {
        'あ' | 'ぁ' => &["a"],
        'い' | 'ぃ' | 'ゐ' => &["i"],
        'う' | 'ぅ' => &["ɯ"],
        'え' | 'ぇ' | 'ゑ' => &["e"],
        'お' | 'ぉ' | 'を' => &["o"],
        'か' => &["k", "a"],
        'き' => &["k", "i"],
        'く' => &["k", "ɯ"],
        'け' => &["k", "e"],
        'こ' => &["k", "o"],
        'が' => &["g", "a"],
        'ぎ' => &["g", "i"],
        'ぐ' => &["g", "ɯ"],
        'げ' => &["g", "e"],
        'ご' => &["g", "o"],
        'さ' => &["s", "a"],
        'し' => &["ɕ", "i"],
        'す' => &["s", "ɯ"],
        'せ' => &["s", "e"],
        'そ' => &["s", "o"],
        'ざ' => &["z", "a"],
        'じ' | 'ぢ' => &["dʑ", "i"],
        'ず' | 'づ' => &["z", "ɯ"],
        'ぜ' => &["z", "e"],
        'ぞ' => &["z", "o"],
        'た' => &["t", "a"],
        'ち' => &["tɕ", "i"],
        'つ' => &["ts", "ɯ"],
        'て' => &["t", "e"],
        'と' => &["t", "o"],
        'だ' => &["d", "a"],
        'で' => &["d", "e"],
        'ど' => &["d", "o"],
        'な' => &["n", "a"],
        'に' => &["n", "i"],
        'ぬ' => &["n", "ɯ"],
        'ね' => &["n", "e"],
        'の' => &["n", "o"],
        'は' => &["h", "a"],
        'ひ' => &["h", "i"],
        'ふ' => &["ɸ", "ɯ"],
        'へ' => &["h", "e"],
        'ほ' => &["h", "o"],
        'ば' => &["b", "a"],
        'び' => &["b", "i"],
        'ぶ' => &["b", "ɯ"],
        'べ' => &["b", "e"],
        'ぼ' => &["b", "o"],
        'ぱ' => &["p", "a"],
        'ぴ' => &["p", "i"],
        'ぷ' => &["p", "ɯ"],
        'ぺ' => &["p", "e"],
        'ぽ' => &["p", "o"],
        'ま' => &["m", "a"],
        'み' => &["m", "i"],
        'む' => &["m", "ɯ"],
        'め' => &["m", "e"],
        'も' => &["m", "o"],
        'や' | 'ゃ' => &["j", "a"],
        'ゆ' | 'ゅ' => &["j", "ɯ"],
        'よ' | 'ょ' => &["j", "o"],
        'ら' => &["ɾ", "a"],
        'り' => &["ɾ", "i"],
        'る' => &["ɾ", "ɯ"],
        'れ' => &["ɾ", "e"],
        'ろ' => &["ɾ", "o"],
        'わ' | 'ゎ' => &["w", "a"],
        'ゔ' => &["v", "ɯ"],
        'ん' => &["ɴ"],
        'っ' => &["ʔ"],
        _ => return None,
    };
    Some(tokens(phonemes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phonemes(text: &str) -> Vec<String> {
        kana_phonemes(text).unwrap_or_else(|| panic!("{text} should read as kana"))
    }

    #[test]
    fn plain_moras_read_off_the_table() {
        assert_eq!(phonemes("さくら"), ["s", "a", "k", "ɯ", "ɾ", "a"]);
        assert_eq!(phonemes("し"), ["ɕ", "i"]);
        assert_eq!(phonemes("ふ"), ["ɸ", "ɯ"]);
        assert_eq!(phonemes("ん"), ["ɴ"]);
        assert_eq!(phonemes("っ"), ["ʔ"]);
    }

    #[test]
    fn digraphs_are_one_mora_not_two() {
        assert_eq!(phonemes("きょ"), ["kʲ", "o"]);
        assert_eq!(phonemes("しゃ"), ["ɕ", "a"]);
        assert_eq!(phonemes("ちゅ"), ["tɕ", "ɯ"]);
        assert_eq!(phonemes("ふぁ"), ["ɸ", "a"]);
        assert_eq!(phonemes("てぃ"), ["t", "i"]);
        assert_eq!(phonemes("うぉ"), ["w", "o"]);
    }

    #[test]
    fn katakana_and_the_long_mark_read_as_their_hiragana() {
        assert_eq!(phonemes("ラーメン"), ["ɾ", "a", "a", "m", "e", "ɴ"]);
        assert_eq!(phonemes("キョ"), ["kʲ", "o"]);
        // A stretch with nothing to stretch is not kana anybody sings.
        assert_eq!(kana_phonemes("ー"), None);
        assert_eq!(kana_phonemes("んー"), None);
    }

    #[test]
    fn moras_split_where_a_person_would_put_the_notes() {
        assert_eq!(
            split_kana_moras("こんにちは").unwrap(),
            ["こ", "ん", "に", "ち", "は"]
        );
        assert_eq!(split_kana_moras("きょうと").unwrap(), ["きょ", "う", "と"]);
        assert_eq!(split_kana_moras("ずっと").unwrap(), ["ず", "っ", "と"]);
        assert_eq!(split_kana_moras("ノート").unwrap(), ["の", "ー", "と"]);
    }

    #[test]
    fn anything_that_is_not_kana_refuses_rather_than_half_answers() {
        for text in ["歌", "a", "さk", "12", "さ。"] {
            assert_eq!(kana_phonemes(text), None, "{text}");
        }
        // Whitespace is a separator, not a refusal.
        assert_eq!(phonemes("さ く"), ["s", "a", "k", "ɯ"]);
    }
}
