//! How a lyric's moras share notes.
//!
//! A mora is a unit of the language, not of the melody. Two of them need no note of their own:
//! a held long vowel — ー, or a vowel repeating the one before it as in さあ — may be sung as
//! the end of the previous note, and a sokuon っ is the rest after a note rather than a note.
//! Everything else takes a note, so a phrase fits any note count between "every mora but っ
//! on its own note" and "every long vowel held as well". Nothing here guesses: a phrase that
//! fits neither way is reported, and a grouping is only ever written when it is asked for.

use auris_vocal::{JapaneseDictionary, split_kana_moras, spoken_kana_lyric};

use super::lyrics::read_lyrics;
use serde::Serialize;

/// Separates notes in a grouped reading: `さあ|い|こ|う` sings さあ on one note.
pub const NOTE_SEPARATOR: char = '|';

/// What a mora asks of the melody.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MoraSlot {
    /// Needs a note of its own.
    Note,
    /// A held long vowel: a note of its own, or the end of the previous note.
    Hold,
    /// A sokuon: no note, but a rest after the previous note.
    Rest,
}

const PLAIN_VOWELS: [&str; 5] = ["a", "i", "u", "e", "o"];

/// Classifies each mora. `word_starts` lists moras that begin a word; a repeated vowel there
/// is the next word's first sound (きみに いま), never a held vowel.
pub(crate) fn mora_slots(moras: &[(&str, &[String])], word_starts: &[usize]) -> Vec<MoraSlot> {
    moras
        .iter()
        .enumerate()
        .map(|(at, (text, phonemes))| {
            if at == 0 {
                return MoraSlot::Note;
            }
            if *phonemes == ["ʔ"] {
                return MoraSlot::Rest;
            }
            let repeated = matches!(phonemes, [vowel] if PLAIN_VOWELS.contains(&vowel.as_str())
                && moras[at - 1].1.last() == Some(vowel))
                && !word_starts.contains(&at);
            if *text == "ー" || repeated {
                MoraSlot::Hold
            } else {
                MoraSlot::Note
            }
        })
        .collect()
}

/// The fewest and most notes a phrase can be sung on.
pub(crate) fn note_range(slots: &[MoraSlot]) -> (usize, usize) {
    let most = slots.iter().filter(|s| **s != MoraSlot::Rest).count();
    let holds = slots.iter().filter(|s| **s == MoraSlot::Hold).count();
    (most - holds, most)
}

/// Group sizes laying the phrase on `notes` notes, or `None` outside [`note_range`]. Long
/// vowels join their notes from the end of the phrase first, where a held note is usual.
pub(crate) fn note_groups(slots: &[MoraSlot], notes: usize) -> Option<Vec<usize>> {
    let (fewest, most) = note_range(slots);
    if slots.is_empty() || notes < fewest || notes > most {
        return None;
    }
    let mut merge = most - notes;
    let mut joins: Vec<bool> = slots.iter().map(|s| *s == MoraSlot::Rest).collect();
    for at in (0..slots.len()).rev() {
        if merge == 0 {
            break;
        }
        if slots[at] == MoraSlot::Hold {
            joins[at] = true;
            merge -= 1;
        }
    }
    let mut sizes = Vec::new();
    for join in joins {
        match sizes.last_mut() {
            Some(size) if join => *size += 1,
            _ => sizes.push(1),
        }
    }
    Some(sizes)
}

/// Spells `sizes` as a grouped reading, with a space before each word that starts a note so
/// that the particle rule still knows where words end.
pub(crate) fn grouped_reading(texts: &[&str], sizes: &[usize], word_starts: &[usize]) -> String {
    let mut out = String::new();
    let mut at = 0;
    for size in sizes {
        if at > 0 {
            out.push(NOTE_SEPARATOR);
            if word_starts.contains(&at) {
                out.push(' ');
            }
        }
        for text in &texts[at..at + size] {
            out.push_str(text);
        }
        at += size;
    }
    out
}

/// A kana reading's moras, with each long vowel sung as the dictionary hears the written text.
///
/// Kana spells a long o as おう and a long e as えい — りょうて, せんせい — and the kana table
/// sings what is spelt, りょ・う・て. The dictionary knows 両手 is リョーテ. Where it reads the
/// written text to as many moras as the reading, differing only where the reading spells う
/// or い and the dictionary hears the o or e before it held, the held vowel is sung and the
/// reading's spelling is kept. Any other difference means the reading is a deliberate
/// alternative, and it is sung exactly as spelt; so is every reading without a dictionary.
pub(crate) fn spoken_reading(
    text: &str,
    reading: &str,
    dictionary: Option<&JapaneseDictionary>,
) -> Option<Vec<(String, Vec<String>)>> {
    let mut moras = spoken_kana_lyric(reading, dictionary)?;
    let Some(heard) = dictionary.and_then(|d| read_lyrics(text, Some(d)).ok()) else {
        return Some(moras);
    };
    let heard: Vec<_> = heard.into_iter().flat_map(|phrase| phrase.moras).collect();
    if heard.len() != moras.len() {
        return Some(moras);
    }
    let mut held = Vec::new();
    for (at, ((_, spelt), sung)) in moras.iter().zip(&heard).enumerate() {
        if *spelt == sung.phonemes {
            continue;
        }
        let long = matches!(
            (spelt.as_slice(), sung.phonemes.as_slice()),
            ([u], [o]) if u == "ɯ" && o == "o"
        ) || matches!(
            (spelt.as_slice(), sung.phonemes.as_slice()),
            ([i], [e]) if i == "i" && e == "e"
        );
        let after_same_vowel = at > 0 && moras[at - 1].1.last() == sung.phonemes.last();
        if !(long && after_same_vowel) {
            return Some(moras);
        }
        held.push(at);
    }
    for at in held {
        moras[at].1 = heard[at].phonemes.clone();
    }
    Some(moras)
}

/// One note of a grouped reading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NoteGroup {
    /// The kana sung on this note.
    pub(crate) text: String,
    /// Its phonemes; a final っ contributes none, because it is the rest that follows.
    pub(crate) phonemes: Vec<String>,
    /// Whether the note ends in っ and so needs a rest after it.
    pub(crate) rest_after: bool,
}

/// Reads a reading containing [`NOTE_SEPARATOR`] into one group per note.
///
/// The phrase is read whole — ー needs the vowel before it and は needs its words — and then
/// cut where the separators are. Only a held long vowel or a final っ may share a note, so a
/// grouping can never hide two sung moras on one note.
pub(crate) fn read_note_groups(
    text: &str,
    reading: &str,
    dictionary: Option<&JapaneseDictionary>,
) -> Result<Vec<NoteGroup>, String> {
    let pieces: Vec<&str> = reading.split(NOTE_SEPARATOR).collect();
    if let Some(at) = pieces.iter().position(|piece| piece.trim().is_empty()) {
        return Err(format!(
            "Note group {} of the reading is empty; put kana between every '|'",
            at + 1
        ));
    }
    let whole: String = pieces.concat();
    let moras = spoken_reading(text, whole.trim(), dictionary)
        .filter(|moras| !moras.is_empty())
        .ok_or("The grouped reading must be kana")?;
    let mut sizes = Vec::with_capacity(pieces.len());
    let mut before = 0;
    for end in 1..=pieces.len() {
        let prefix = split_kana_moras(pieces[..end].concat().trim())
            .ok_or("The grouped reading must be kana")?;
        let counted = prefix.len();
        // A cut inside きょ reads き on its own, which the whole phrase never does.
        let whole_prefix = moras.get(..counted).map(|m| m.iter().map(|(t, _)| t));
        if counted == before || whole_prefix.is_none_or(|texts| !texts.eq(prefix.iter())) {
            return Err(format!(
                "Note group {end} of the reading cuts a mora in two; keep small kana with their base"
            ));
        }
        sizes.push(counted - before);
        before = counted;
    }
    if before != moras.len() {
        return Err("The grouped reading could not be cut into notes".into());
    }
    let borrowed: Vec<(&str, &[String])> = moras
        .iter()
        .map(|(text, phonemes)| (text.as_str(), phonemes.as_slice()))
        .collect();
    let slots = mora_slots(&borrowed, &[]);
    let mut groups = Vec::with_capacity(sizes.len());
    let mut at = 0;
    for (index, size) in sizes.into_iter().enumerate() {
        let range = at..at + size;
        at += size;
        let text: String = moras[range.clone()]
            .iter()
            .map(|(t, _)| t.as_str())
            .collect();
        if slots[range.start] == MoraSlot::Rest {
            return Err(format!(
                "Note group {} \"{text}\" starts with っ; っ is the rest after a note, so write it at the end of the previous group",
                index + 1
            ));
        }
        let shared = &slots[range.start + 1..range.end];
        let rest_after = shared.last() == Some(&MoraSlot::Rest);
        let held = &shared[..shared.len() - usize::from(rest_after)];
        if held.iter().any(|slot| *slot != MoraSlot::Hold) {
            return Err(format!(
                "Note group {} \"{text}\" puts {size} moras on one note; only a held long vowel (ー, or a repeated vowel as in さあ) and a final っ may share a note",
                index + 1
            ));
        }
        groups.push(NoteGroup {
            text,
            phonemes: moras[range]
                .iter()
                .flat_map(|(_, phonemes)| phonemes.iter())
                .filter(|phoneme| *phoneme != "ʔ")
                .cloned()
                .collect(),
            rest_after,
        });
    }
    Ok(groups)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slots_of(kana: &str) -> Vec<MoraSlot> {
        let moras = auris_vocal::split_kana_lyric(kana).unwrap();
        let borrowed: Vec<(&str, &[String])> = moras
            .iter()
            .map(|(t, p)| (t.as_str(), p.as_slice()))
            .collect();
        mora_slots(&borrowed, &[])
    }

    #[test]
    fn long_vowels_hold_and_sokuon_rests() {
        use MoraSlot::*;
        assert_eq!(slots_of("さあ"), [Note, Hold]);
        assert_eq!(slots_of("こーひー"), [Note, Hold, Note, Hold]);
        assert_eq!(slots_of("まって"), [Note, Rest, Note]);
        // A different vowel is a new sound, not a held one.
        assert_eq!(slots_of("かお"), [Note, Note]);
        // ん is sung on a note of its own.
        assert_eq!(slots_of("さん"), [Note, Note]);
    }

    #[test]
    fn a_repeated_vowel_that_starts_a_word_is_not_held() {
        let moras = auris_vocal::split_kana_lyric("きみにいま").unwrap();
        let borrowed: Vec<(&str, &[String])> = moras
            .iter()
            .map(|(t, p)| (t.as_str(), p.as_slice()))
            .collect();
        assert_eq!(mora_slots(&borrowed, &[0, 3])[3], MoraSlot::Note);
        assert_eq!(mora_slots(&borrowed, &[])[3], MoraSlot::Hold);
    }

    #[test]
    fn groups_cover_exactly_the_range_and_merge_from_the_end() {
        // さあ いっしょに ゆこー: 9 moras, っ rests, two holds.
        let slots = slots_of("さあいっしょにゆこー");
        assert_eq!(note_range(&slots), (6, 8));
        assert_eq!(note_groups(&slots, 8).unwrap(), [1, 1, 2, 1, 1, 1, 1, 1]);
        assert_eq!(note_groups(&slots, 7).unwrap(), [1, 1, 2, 1, 1, 1, 2]);
        assert_eq!(note_groups(&slots, 6).unwrap(), [2, 2, 1, 1, 1, 2]);
        assert!(note_groups(&slots, 5).is_none());
        assert!(note_groups(&slots, 9).is_none());
        let moras = auris_vocal::split_kana_moras("さあいっしょにゆこー").unwrap();
        let texts: Vec<&str> = moras.iter().map(String::as_str).collect();
        assert_eq!(
            grouped_reading(&texts, &[2, 2, 1, 1, 1, 2], &[2]),
            "さあ| いっ|しょ|に|ゆ|こー"
        );
    }

    #[test]
    fn a_grouped_reading_puts_holds_on_one_note_and_drops_the_closure() {
        let groups = read_note_groups("", "さあ| いっ|しょ|に", None).unwrap();
        assert_eq!(groups.len(), 4);
        assert_eq!(groups[0].phonemes, ["s", "a", "a"]);
        assert!(!groups[0].rest_after);
        assert_eq!(groups[1].text, "いっ");
        assert_eq!(groups[1].phonemes, ["i"]);
        assert!(groups[1].rest_after);
    }

    #[test]
    fn a_reading_sings_the_long_vowels_the_dictionary_hears() {
        let Some(folder) = std::env::var_os("AURIS_JAPANESE_DICTIONARY") else {
            return;
        };
        let dictionary = JapaneseDictionary::load(std::path::Path::new(&folder)).unwrap();
        let sung = |text: &str, reading: &str| -> Vec<Vec<String>> {
            spoken_reading(text, reading, Some(&dictionary))
                .unwrap()
                .into_iter()
                .map(|(_, phonemes)| phonemes)
                .collect()
        };
        // 両手 is リョーテ and 先生 is センセー, however the reading spells them.
        assert_eq!(sung("両手で", "りょうてで")[1], ["o"]);
        assert_eq!(sung("先生", "せんせい")[3], ["e"]);
        // 思う really ends in u.
        assert_eq!(sung("思う", "おもう")[2], ["ɯ"]);
        // A reading that differs elsewhere is a deliberate one, sung as spelt.
        assert_eq!(sung("両手で", "りょうてに")[1], ["ɯ"]);
        // Without a dictionary there is nothing to ask.
        assert_eq!(
            spoken_reading("両手", "りょうて", None).unwrap()[1].1,
            ["ɯ"]
        );
        // A grouped reading may then hold that vowel on one note.
        let groups = read_note_groups("両手で", "りょう|て|で", Some(&dictionary)).unwrap();
        assert_eq!(groups[0].phonemes, ["ɾʲ", "o", "o"]);
    }

    #[test]
    fn a_grouped_reading_refuses_to_hide_sung_moras() {
        assert!(
            read_note_groups("", "さく|ら", None)
                .unwrap_err()
                .contains("2 moras")
        );
        assert!(
            read_note_groups("", "ま|って", None)
                .unwrap_err()
                .contains("starts with っ")
        );
        assert!(
            read_note_groups("", "き|ょう", None)
                .unwrap_err()
                .contains("cuts a mora")
        );
        assert!(
            read_note_groups("", "さ||く", None)
                .unwrap_err()
                .contains("empty")
        );
    }
}
