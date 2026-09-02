//! What a phoneme is here: a token, its class, and the one token that is not a sound.
//!
//! A phoneme is a `String` holding an IPA symbol — possibly more than one codepoint, `tɕ` and
//! `kʲ` are single tokens — rather than an enum, because the vocabulary is open: a voice model
//! that learns Cantonese brings tokens this crate has never heard of, and every function here
//! must keep working when it does. The class query below errs the same way: a token it does not
//! know is treated as a consonant, which costs a mistimed frame rather than a crash.

/// The token frames carry where nothing is sung.
///
/// `sil` rather than an empty string or an IPA pause mark, because it is the spelling every
/// acoustic-feature pipeline descended from HTS already uses, and the exported file is read by
/// exactly such a pipeline.
pub const SILENCE: &str = "sil";

/// The vowels of the vocabulary's Japanese core.
///
/// [`is_syllabic`] is the query everything asks; this list is public only so a test can say
/// "every vowel" without copying it.
pub const VOWELS: [&str; 5] = ["a", "i", "ɯ", "e", "o"];

/// `true` for a phoneme that can be stretched to fill a note.
///
/// The timing rule in [`frames`](crate::frames) gives consonants a fixed few milliseconds and
/// divides the rest of the note among these: the vowels, the moraic nasal `ɴ`, and the glottal
/// stop `ʔ` standing for a sokuon sung on a note of its own. An unknown token answers `false` —
/// a consonant-sized slot — because guessing "vowel" would swallow the whole note.
pub fn is_syllabic(phoneme: &str) -> bool {
    VOWELS.contains(&phoneme) || phoneme == "ɴ" || phoneme == "ʔ"
}

/// The phonemes produced without vocal-fold vibration.
///
/// This is what decides a frame's *voiced* flag when frames are handed to a voice model: the
/// renderer writes f0 as a contour — consonant frames carry the pitch of the vowel they lead
/// into — so voicing can never be read off `f0 > 0`, which would hum through every /k/ and /s/.
/// The list is the voiceless obstruents of the vocabulary plus the devoiced vowels, and it
/// matches the `VOICELESS` table in auris-singer's training pipeline symbol for symbol.
pub const VOICELESS: [&str; 26] = [
    // Devoiced vowels — a vowel whispered between voiceless neighbours, as in きし.
    "ḁ", "i̥", "ɯ̥", "e̥", "o̥", // Plosives, including the glottal stop the sokuon becomes.
    "p", "pʲ", "t", "tʲ", "k", "kʲ", "kʷ", "ʔ", // Affricates.
    "ts", "tɕ", "tʃ", // Fricatives.
    "ɸ", "ɸʲ", "f", "θ", "s", "ɕ", "ʃ", "ç", "x", "h",
];

/// `true` for a phoneme produced without vocal-fold vibration, and for [`SILENCE`].
///
/// An unknown token answers `false` — voiced — because the two mistakes are not symmetric: a
/// wrongly-voiced frame keeps the pitch contour intact, while a wrongly-unvoiced one silences
/// a sung frame outright.
pub fn is_voiceless(phoneme: &str) -> bool {
    phoneme == SILENCE || VOICELESS.contains(&phoneme)
}

/// Splits a phoneme sequence into moras: each syllabic phoneme ends one.
///
/// This is what lets a phrase be distributed across notes without asking the dictionary a
/// second question — `[k o ɴ nʲ i tɕ i w a]` becomes `[k o][ɴ][nʲ i][tɕ i][w a]`, one group
/// per note. Trailing consonants with no syllabic to lean on come out as a final group of
/// their own rather than vanishing.
pub fn phoneme_moras(phonemes: &[String]) -> Vec<Vec<String>> {
    let mut moras = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for phoneme in phonemes {
        current.push(phoneme.clone());
        if is_syllabic(phoneme) {
            moras.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        moras.push(current);
    }
    moras
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn every_vowel_is_syllabic_and_the_nasal_and_stop_are_too() {
        for vowel in VOWELS {
            assert!(is_syllabic(vowel), "{vowel}");
        }
        assert!(is_syllabic("ɴ") && is_syllabic("ʔ"));
        for consonant in ["k", "ɕ", "tɕ", "kʲ", "zzz"] {
            assert!(!is_syllabic(consonant), "{consonant}");
        }
    }

    #[test]
    fn moras_break_after_each_syllabic_phoneme() {
        // こんにちは — the five notes a person would write it across.
        let phonemes = strings(&["k", "o", "ɴ", "n", "i", "tɕ", "i", "w", "a"]);
        let moras = phoneme_moras(&phonemes);
        assert_eq!(
            moras,
            vec![
                strings(&["k", "o"]),
                strings(&["ɴ"]),
                strings(&["n", "i"]),
                strings(&["tɕ", "i"]),
                strings(&["w", "a"]),
            ]
        );
    }

    #[test]
    fn voicing_follows_the_phoneme_class_and_errs_voiced() {
        for voiceless in ["k", "s", "tɕ", "ʔ", "i̥", "h", SILENCE] {
            assert!(is_voiceless(voiceless), "{voiceless}");
        }
        for voiced in ["a", "ɴ", "g", "dʑ", "m", "w"] {
            assert!(!is_voiceless(voiced), "{voiced}");
        }
        // The open vocabulary rule: a token from a language this crate has never heard of is
        // sung voiced, keeping its pitch contour, rather than silenced.
        assert!(!is_voiceless("ʈʂ"));
    }

    #[test]
    fn trailing_consonants_survive_as_a_group_of_their_own() {
        let moras = phoneme_moras(&strings(&["k", "a", "t"]));
        assert_eq!(moras, vec![strings(&["k", "a"]), strings(&["t"])]);
        assert!(phoneme_moras(&[]).is_empty());
    }
}
