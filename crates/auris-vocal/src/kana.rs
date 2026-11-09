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

/// Recover a backend-readable spelling from the pronunciation saved on a note.
/// Unknown pronunciations remain explicit instead of being replaced by a vowel.
pub fn phonemes_to_kana(phonemes: &[String]) -> Option<String> {
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static SPELLINGS: OnceLock<HashMap<Vec<String>, String>> = OnceLock::new();
    let spellings = SPELLINGS.get_or_init(|| {
        let mut spellings = HashMap::new();
        for base in "あいうえおかきくけこがぎぐげござじずぜぞさしすせそたちつてとだでどなにぬねのはひふへほばびぶべぼぱぴぷぺぽまみむめもやゆよらりるれろわゔんっ".chars() {
            if let Some(phonemes) = single(base) {
                spellings.entry(phonemes).or_insert_with(|| base.to_string());
            }
            for small in SMALL {
                if let Some(phonemes) = digraph(base, small) {
                    spellings.entry(phonemes).or_insert_with(|| format!("{base}{small}"));
                }
            }
        }
        spellings
    });
    if phonemes.is_empty() {
        return None;
    }
    let phonemes: Vec<String> = phonemes
        .iter()
        .map(|phoneme| phoneme.trim_end_matches('\u{0325}').to_string())
        .collect();
    crate::phoneme_moras(&phonemes)
        .iter()
        .map(|mora| spellings.get(mora).map(String::as_str))
        .collect()
}

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
/// across notes; a prolonged-sound mark ー is the mora of whatever vowel it stretches. A caller
/// distributing moras to *sung* notes wants [`split_kana_lyric`] instead, which keeps each
/// mora's phonemes with it.
pub fn split_kana_moras(text: &str) -> Option<Vec<String>> {
    walk(text).map(|moras| moras.into_iter().map(|(mora, _)| mora).collect())
}

/// A kana phrase split into moras, each carrying its own phonemes, or `None` where any of it
/// is not kana.
///
/// The pairing is the point: ー's phoneme is the vowel of the mora *before* it, a fact only the
/// whole phrase knows. Splitting first and asking [`kana_phonemes`] of each mora alone gets
/// `None` for every ー — and a lyric written 「こーひー」 would fall through to the open-vowel
/// placeholder and be sung こあひあ.
pub fn split_kana_lyric(text: &str) -> Option<Vec<(String, Vec<String>)>> {
    walk(text)
}

/// Small kana that glue onto the mora before them.
const SMALL: [char; 9] = ['ゃ', 'ゅ', 'ょ', 'ぁ', 'ぃ', 'ぅ', 'ぇ', 'ぉ', 'ゎ'];

/// Reads a kana string as `(mora text, phonemes)` pairs, or `None` at the first thing that is
/// not kana. Whitespace separates moras and is otherwise ignored.
fn walk(text: &str) -> Option<Vec<(String, Vec<String>)>> {
    walk_spaced(text).map(|moras| {
        moras
            .into_iter()
            .map(|mora| (mora.text, mora.phonemes))
            .collect()
    })
}

/// One mora of a walked phrase, remembering whether whitespace came before it.
///
/// The whitespace is the only word boundary a kana phrase has without a dictionary, and the
/// particle rule in [`spoken_moras`] is a rule about the ends of words.
#[derive(Clone, Debug)]
pub(crate) struct WalkedMora {
    /// The mora as written, folded onto hiragana.
    pub(crate) text: String,
    /// Its phonemes, read as written.
    pub(crate) phonemes: Vec<String>,
    /// Whether whitespace separates it from the mora before.
    pub(crate) spaced: bool,
}

/// [`walk`], keeping the word boundaries.
pub(crate) fn walk_spaced(text: &str) -> Option<Vec<WalkedMora>> {
    let normalized = normalize(text);
    let mut chars: Vec<(char, bool)> = Vec::new();
    let mut spaced = false;
    for c in normalized.chars() {
        if c.is_whitespace() {
            spaced = true;
        } else {
            chars.push((c, std::mem::take(&mut spaced)));
        }
    }
    let mut out: Vec<WalkedMora> = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let (this, spaced) = chars[at];
        if this == 'ー' {
            // The stretch of whatever vowel came before it — which is the last phoneme of the
            // previous mora whenever that mora ends in one.
            let vowel = out
                .last()
                .and_then(|mora| mora.phonemes.last())
                .filter(|last| VOWELS.contains(&last.as_str()))?
                .clone();
            out.push(WalkedMora {
                text: 'ー'.to_string(),
                phonemes: vec![vowel],
                spaced,
            });
            at += 1;
            continue;
        }
        let next = chars.get(at + 1).copied();
        // A small kana glues onto its base even across a space: き ょ is still きょ.
        if let Some((small, _)) = next.filter(|(next, _)| SMALL.contains(next))
            && let Some(phonemes) = digraph(this, small)
        {
            out.push(WalkedMora {
                text: format!("{this}{small}"),
                phonemes,
                spaced,
            });
            at += 2;
            continue;
        }
        out.push(WalkedMora {
            text: this.to_string(),
            phonemes: single(this)?,
            spaced,
        });
        at += 1;
    }
    Some(out)
}

/// The moras of a kana phrase as they are *spoken*, or `None` where any of it is not kana.
///
/// Kana spells Japanese as it is pronounced with three exceptions, all particles: は is
/// read わ, へ is read え and を is read お. を is never anything but the particle, so the table
/// already reads it お; は and へ are ordinary syllables everywhere else — はな, へや — so which
/// reading a written は takes depends on the words around it, and a table that reads one mora
/// at a time sang every topic marker as *ha*. This is the phrase-level reading that fixes it.
///
/// `particles` is a per-mora verdict from something that knows the words — the dictionary,
/// through [`spoken_kana_lyric`](crate::spoken_kana_lyric) — and is consulted where it has the
/// same length as the phrase. Without it the rule is the one whitespace allows: a は or へ that
/// ends a word of two or more moras is the particle, which is how lyrics are ordinarily spaced
/// (きみは わらう, あしたへ). Either way two guards hold, because a mistaken *wa* is as audible
/// as a mistaken *ha*: the first mora of a phrase is never a particle, and neither is a は
/// written straight after another は — はは and あはは are a mother and a laugh.
///
/// The mora text stays as written, so a note still shows the は somebody typed; only the
/// phonemes change.
pub(crate) fn spoken_moras(
    text: &str,
    particles: Option<&[bool]>,
) -> Option<Vec<(String, Vec<String>)>> {
    let mut moras = walk_spaced(text)?;
    let particles = particles.filter(|marks| marks.len() == moras.len());
    for at in 1..moras.len() {
        let spoken: &[&str] = match moras[at].text.as_str() {
            "は" => &["w", "a"],
            "へ" => &["e"],
            _ => continue,
        };
        let ends_word = |at: usize| moras.get(at + 1).is_none_or(|next| next.spaced);
        let word_final = ends_word(at);
        // へ joined by の or と — きみへの, みらいへと — is the particle doubled up, and the
        // words that end so are rare; へとへと is the one a lyric meets, and is spared.
        let doubled = moras[at].text == "へ"
            && moras
                .get(at + 1)
                .is_some_and(|next| !next.spaced && matches!(next.text.as_str(), "の" | "と"))
            && ends_word(at + 1)
            && moras[at - 1].text != "と";
        // The dictionary is trusted inside a word, never at the start of one: a particle
        // follows something, and はしる after a space is a verb.
        let marked = particles.is_some_and(|marks| marks[at]) && (!moras[at].spaced || word_final);
        let repeated = !moras[at].spaced && moras[at - 1].text == moras[at].text;
        if (marked || word_final || doubled) && !repeated {
            moras[at].phonemes = tokens(spoken);
        }
    }
    Some(
        moras
            .into_iter()
            .map(|mora| (mora.text, mora.phonemes))
            .collect(),
    )
}

/// Katakana folded onto hiragana, and the spellings people actually type folded onto the
/// ones the table knows.
///
/// A wave dash is how a lyric sheet writes a held vowel (あ〜), and the half-width ｰ is the
/// prolonged mark from a half-width keyboard; both mean ー. ヷ ヸ ヹ ヺ have no hiragana and
/// are ヴ with a small vowel.
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '〜' | '～' | 'ｰ' => out.push('ー'),
            'ヷ' => out.push_str("ゔぁ"),
            'ヸ' => out.push_str("ゔぃ"),
            'ヹ' => out.push_str("ゔぇ"),
            'ヺ' => out.push_str("ゔぉ"),
            _ => out.push(hiragana(c)),
        }
    }
    out
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
            'ぃ' => "i",
            'ゅ' => "ɯ",
            'ぇ' => "e",
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
        'く' | 'ぐ' => {
            let onset = if base == 'く' { "kʷ" } else { "gʷ" };
            if small == 'ゎ' {
                Some(tokens(&[onset, "a"]))
            } else {
                with_vowel(onset)
            }
        }
        'し' => palatal("ɕ").or_else(|| with_vowel("ɕ")),
        'じ' | 'ぢ' => palatal("dʑ").or_else(|| with_vowel("dʑ")),
        'ち' => palatal("tɕ").or_else(|| with_vowel("tɕ")),
        'に' => palatal("ɲ"),
        'ひ' => palatal("ç"),
        'び' => palatal("bʲ"),
        'ぴ' => palatal("pʲ"),
        'み' => palatal("mʲ"),
        'り' => palatal("ɾʲ"),
        'て' => match small {
            'ぃ' => Some(tokens(&["t", "i"])),
            _ => palatal("tʲ"),
        },
        'で' => match small {
            'ぃ' => Some(tokens(&["d", "i"])),
            _ => palatal("dʲ"),
        },
        'す' => (small == 'ぃ').then(|| tokens(&["s", "i"])),
        'ず' => (small == 'ぃ').then(|| tokens(&["dz", "i"])),
        'と' => (small == 'ぅ').then(|| tokens(&["t", "ɯ"])),
        'ど' => (small == 'ぅ').then(|| tokens(&["d", "ɯ"])),
        'つ' => with_vowel("ts"),
        'ふ' => with_vowel("ɸ"),
        'う' => match small {
            'ぃ' => Some(tokens(&["w", "i"])),
            'ぅ' => Some(tokens(&["w", "ɯ"])),
            'ぇ' => Some(tokens(&["w", "e"])),
            'ぉ' => Some(tokens(&["w", "o"])),
            _ => None,
        },
        'ゔ' => with_vowel("v").or_else(|| palatal("bʲ")),
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
        'け' | 'ゖ' => &["k", "e"],
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
        'ざ' => &["dz", "a"],
        'じ' | 'ぢ' => &["dʑ", "i"],
        'ず' | 'づ' => &["dz", "ɯ"],
        'ぜ' => &["dz", "e"],
        'ぞ' => &["dz", "o"],
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
    fn extended_moras_keep_their_consonant_instead_of_splitting_into_syllables() {
        for (text, onset, vowel) in [
            ("きぃ", "kʲ", "i"),
            ("きぇ", "kʲ", "e"),
            ("ぎぃ", "gʲ", "i"),
            ("ぎぇ", "gʲ", "e"),
            ("にぃ", "ɲ", "i"),
            ("にぇ", "ɲ", "e"),
            ("ひぃ", "ç", "i"),
            ("ひぇ", "ç", "e"),
            ("びぃ", "bʲ", "i"),
            ("びぇ", "bʲ", "e"),
            ("ぴぃ", "pʲ", "i"),
            ("ぴぇ", "pʲ", "e"),
            ("みぃ", "mʲ", "i"),
            ("みぇ", "mʲ", "e"),
            ("りぃ", "ɾʲ", "i"),
            ("りぇ", "ɾʲ", "e"),
            ("すぃ", "s", "i"),
            ("ずぃ", "dz", "i"),
            ("くぁ", "kʷ", "a"),
            ("くぃ", "kʷ", "i"),
            ("くぅ", "kʷ", "ɯ"),
            ("くぇ", "kʷ", "e"),
            ("くぉ", "kʷ", "o"),
            ("ぐぁ", "gʷ", "a"),
            ("ぐぃ", "gʷ", "i"),
            ("ぐぅ", "gʷ", "ɯ"),
            ("ぐぇ", "gʷ", "e"),
            ("ぐぉ", "gʷ", "o"),
            ("てゃ", "tʲ", "a"),
            ("てぇ", "tʲ", "e"),
            ("てょ", "tʲ", "o"),
            ("でゃ", "dʲ", "a"),
            ("でぇ", "dʲ", "e"),
            ("でょ", "dʲ", "o"),
            ("うぅ", "w", "ɯ"),
        ] {
            let expected = tokens(&[onset, vowel]);
            assert_eq!(kana_phonemes(text), Some(expected.clone()), "{text}");
            assert_eq!(split_kana_moras(text), Some(vec![text.into()]), "{text}");
            let recovered = phonemes_to_kana(&expected).unwrap();
            assert_eq!(kana_phonemes(&recovered), Some(expected), "{text}");
        }
    }

    #[test]
    fn engine_aliases_keep_their_single_mora_pronunciation() {
        for (text, expected) in [
            ("ヴャ", tokens(&["bʲ", "a"])),
            ("ヴュ", tokens(&["bʲ", "ɯ"])),
            ("ヴョ", tokens(&["bʲ", "o"])),
            ("ヶ", tokens(&["k", "e"])),
        ] {
            assert_eq!(kana_phonemes(text), Some(expected), "{text}");
            assert_eq!(split_kana_moras(text).unwrap().len(), 1, "{text}");
        }
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
    fn a_prolonged_mark_keeps_its_vowel_when_split_for_notes() {
        // ー alone answers nothing — its vowel lives in the mora before it — so the split has
        // to carry the phonemes along. こーひー stretches o then i, and a pipeline that asked
        // each mora separately would sing both as the placeholder a.
        let pairs = split_kana_lyric("こーひー").unwrap();
        let moras: Vec<&str> = pairs.iter().map(|(mora, _)| mora.as_str()).collect();
        assert_eq!(moras, ["こ", "ー", "ひ", "ー"]);
        assert_eq!(pairs[1].1, ["o"]);
        assert_eq!(pairs[3].1, ["i"]);
    }

    fn spoken(text: &str) -> Vec<String> {
        spoken_moras(text, None)
            .unwrap_or_else(|| panic!("{text} should read as kana"))
            .into_iter()
            .flat_map(|(_, phonemes)| phonemes)
            .collect()
    }

    #[test]
    fn particles_that_end_a_word_are_spoken_wa_and_e() {
        assert_eq!(
            spoken("きみは わらう"),
            ["k", "i", "m", "i", "w", "a", "w", "a", "ɾ", "a", "ɯ"]
        );
        assert_eq!(spoken("あしたへ"), ["a", "ɕ", "i", "t", "a", "e"]);
        assert_eq!(spoken("ぼくを"), ["b", "o", "k", "ɯ", "o"]);
        // A particle written as a word of its own is still the particle.
        assert_eq!(spoken("きみ は"), ["k", "i", "m", "i", "w", "a"]);
        assert_eq!(spoken("こんにちは")[7..], ["w", "a"]);
        // The written spelling stays on the mora; only the sound changes.
        let moras = spoken_moras("ゆめは", None).unwrap();
        assert_eq!(moras[2].0, "は");
        assert_eq!(moras[2].1, ["w", "a"]);
        // Katakana is the same kana.
        assert_eq!(spoken("ソラヘ")[4..], ["e"]);
    }

    #[test]
    fn the_same_kana_inside_a_word_keeps_its_written_sound() {
        assert_eq!(spoken("はな"), ["h", "a", "n", "a"]);
        assert_eq!(spoken("へや"), ["h", "e", "j", "a"]);
        assert_eq!(
            spoken("はれる そら"),
            ["h", "a", "ɾ", "e", "ɾ", "ɯ", "s", "o", "ɾ", "a"]
        );
        // The first mora of a phrase belongs to no word before it.
        assert_eq!(spoken("は"), ["h", "a"]);
        assert_eq!(spoken("へ"), ["h", "e"]);
        // はは is a mother, あはは a laugh; neither holds a particle.
        assert_eq!(spoken("はは"), ["h", "a", "h", "a"]);
        assert_eq!(spoken("あはは"), ["a", "h", "a", "h", "a"]);
        assert_eq!(spoken("えへへ"), ["e", "h", "e", "h", "e"]);
        // But a mother who *is* the topic: the particle stands apart from her.
        assert_eq!(spoken("はは は")[4..], ["w", "a"]);
        // Without a dictionary, an unspaced phrase has no words to end.
        assert_eq!(spoken("きみはわらう")[4..6], ["h", "a"]);
        assert_eq!(spoken("きみへの うた")[4], "e");
        assert_eq!(spoken("みらいへと")[5], "e");
        assert_eq!(spoken("へとへと"), ["h", "e", "t", "o", "h", "e", "t", "o"]);
    }

    #[test]
    fn a_verdict_from_the_words_marks_particles_inside_unspaced_text() {
        let marks = [false, false, true, false, false, false];
        let moras = spoken_moras("きみはわらう", Some(&marks)).unwrap();
        assert_eq!(moras[2].1, ["w", "a"]);
        // The guards outrank the verdict: はは is never *wawa*.
        let moras = spoken_moras("はは", Some(&[true, true])).unwrap();
        assert_eq!(moras[0].1, ["h", "a"]);
        assert_eq!(moras[1].1, ["h", "a"]);
        // Nor does it start a word: はしる after a space is a verb.
        let moras = spoken_moras(
            "わたしは はしる",
            Some(&[false, false, false, true, true, false, false]),
        )
        .unwrap();
        assert_eq!(moras[3].1, ["w", "a"]);
        assert_eq!(moras[4].1, ["h", "a"]);
        // A verdict of the wrong length is no verdict.
        let moras = spoken_moras("きみはわらう", Some(&[true])).unwrap();
        assert_eq!(moras[2].1, ["h", "a"]);
    }

    #[test]
    fn a_wave_dash_holds_the_vowel_like_the_prolonged_mark() {
        assert_eq!(phonemes("あ〜"), ["a", "a"]);
        assert_eq!(phonemes("ラ～"), ["ɾ", "a", "a"]);
        assert_eq!(phonemes("そｰ"), ["s", "o", "o"]);
        assert_eq!(split_kana_moras("きみ〜").unwrap(), ["き", "み", "ー"]);
    }

    #[test]
    fn the_katakana_va_row_reads_as_vu_with_a_small_vowel() {
        assert_eq!(phonemes("ヷ"), ["v", "a"]);
        assert_eq!(phonemes("ヸ"), ["v", "i"]);
        assert_eq!(phonemes("ヹ"), ["v", "e"]);
        assert_eq!(phonemes("ヺ"), ["v", "o"]);
        assert_eq!(split_kana_moras("ヷ").unwrap().len(), 1);
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
