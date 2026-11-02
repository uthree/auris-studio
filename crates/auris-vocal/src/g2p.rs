//! Lyric to phonemes: the kana table first, the Japanese dictionary where the table cannot read.
//!
//! [`lyric_phonemes`] is the one entry point a command calls when a note's lyric changes. The
//! order inside it is a policy worth stating: kana is read by the built-in table even when a
//! dictionary is loaded, so that the ordinary per-note lyric works identically on every machine
//! and the dictionary only ever *adds* text it alone can read — kanji, digits, mixed phrases.
//!
//! The dictionary is [jpreprocess](https://github.com/jpreprocess/jpreprocess) over a
//! **dictionary folder loaded at run time** (a prebuilt `naist-jdic` from that project's
//! releases). Run time rather than the crate's bundling feature because that feature downloads
//! the folder inside `build.rs`, and a build that needs the network fails on the wrong day.
//! Like a SoundFont, the folder is a library shared by every project, named in the settings and
//! left where it lies.

use std::path::{Path, PathBuf};

use jpreprocess::{DefaultTokenizer, JPreprocess, SystemDictionaryConfig};

use crate::accent::{AccentPhrase, SungMora};
use crate::kana::{kana_phonemes, split_kana_lyric, spoken_moras, walk_spaced};
use crate::openjtalk::openjtalk_phoneme;
use crate::phoneme::VOWELS;

/// What went wrong turning text into phonemes.
#[derive(Debug, thiserror::Error)]
pub enum VocalError {
    /// The dictionary folder could not be loaded.
    #[error("could not load the Japanese dictionary at {path}: {detail}")]
    Dictionary {
        /// The folder that was tried.
        path: PathBuf,
        /// The loader's own words.
        detail: String,
    },
    /// The dictionary could not read the text.
    #[error("could not read `{text}`: {detail}")]
    Text {
        /// The lyric that failed.
        text: String,
        /// The frontend's own words.
        detail: String,
    },
    /// The text needs a dictionary and none is loaded.
    ///
    /// Its own variant rather than a `Text` error so a frontend can answer with the setting
    /// that fixes it instead of with a shrug.
    #[error("`{text}` is not kana, and no Japanese dictionary is configured")]
    NeedsDictionary {
        /// The lyric that needed it.
        text: String,
    },
}

/// The Japanese text frontend, loaded from a dictionary folder and kept warm.
///
/// Loading parses a compiled dictionary from disk — work worth doing once, not per lyric — so
/// whoever owns the session owns one of these for as long as the path setting stands.
pub struct JapaneseDictionary {
    inner: JPreprocess<DefaultTokenizer>,
    path: PathBuf,
}

impl std::fmt::Debug for JapaneseDictionary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The tokenizer inside holds megabytes of trie; the path is the part that identifies it.
        f.debug_struct("JapaneseDictionary")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl JapaneseDictionary {
    /// Loads a compiled dictionary folder — a prebuilt `naist-jdic` directory.
    pub fn load(path: &Path) -> Result<Self, VocalError> {
        let dictionary = SystemDictionaryConfig::File(path.to_path_buf())
            .load()
            .map_err(|error| VocalError::Dictionary {
                path: path.to_path_buf(),
                detail: error.to_string(),
            })?;
        Ok(Self {
            inner: JPreprocess::with_dictionaries(dictionary, None),
            path: path.to_path_buf(),
        })
    }

    /// The folder this dictionary was loaded from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The IPA phonemes of an arbitrary Japanese text — kanji, kana, digits, the lot.
    pub fn phonemes(&self, text: &str) -> Result<Vec<String>, VocalError> {
        let labels = self
            .inner
            .extract_fullcontext(&held_vowels(text))
            .map_err(|error| VocalError::Text {
                text: text.to_string(),
                detail: error.to_string(),
            })?;
        let mut phonemes = Vec::new();
        for label in &labels {
            let name = label.phoneme.c.as_deref().ok_or_else(|| VocalError::Text {
                text: text.to_string(),
                detail: format!("unreadable label `{label}`"),
            })?;
            let mapped = openjtalk_phoneme(name).ok_or_else(|| VocalError::Text {
                text: text.to_string(),
                detail: format!("unknown phoneme `{name}`"),
            })?;
            phonemes.extend(mapped.iter().map(|s| s.to_string()));
        }
        Ok(phonemes)
    }

    /// The accent phrases of a Japanese text — moras with phonemes, and each phrase's nucleus.
    ///
    /// The same dictionary run as [`Self::phonemes`], read one level higher: nodes are
    /// grouped into accent phrases the way OpenJTalk's own label stage groups them (a node
    /// whose chain flag is set continues the phrase before it), and a phrase's nucleus is its
    /// first node's, which is where the chaining rules leave the recomputed accent.
    /// Punctuation contributes no moras — splitting a lyric at 、 and 。 is the caller's
    /// business, who is cutting *musical* phrases, not accentual ones.
    pub fn accent_phrases(&self, text: &str) -> Result<Vec<AccentPhrase>, VocalError> {
        let refused = |detail: String| VocalError::Text {
            text: text.to_string(),
            detail,
        };
        let mut njd = self
            .inner
            .text_to_njd(&held_vowels(text))
            .map_err(|error| refused(error.to_string()))?;
        njd.preprocess();

        let mut phrases: Vec<AccentPhrase> = Vec::new();
        let mut previous_vowel = None;
        // The last kana mora as *written*, and whether any mora has been read yet: the two
        // facts the particle guards of `spoken_moras` need, kept across node boundaries.
        let mut previous_written: Option<String> = None;
        let mut first = true;
        for node in &njd.nodes {
            let pron = node.get_pron();
            // Katakana per mora; a devoiced mora prints a trailing ’ the kana table has no
            // interest in, and punctuation moras are not sung at all.
            let kana: String = pron
                .moras()
                .iter()
                .map(|mora| mora.to_string().replace('’', ""))
                .filter(|text| !matches!(text.as_str(), "、" | "？"))
                .collect();
            if kana.is_empty() {
                previous_vowel = None;
                continue;
            }
            let mut moras = dictionary_moras(&kana, previous_vowel.as_deref())
                .ok_or_else(|| refused(format!("unreadable moras `{kana}`")))?;
            // The dictionary reads は and へ as particles wherever it cannot place them in a
            // word, and a phrase of bare kana is exactly where it cannot: はは comes back
            // *wawa*. Hold its verdicts to the same two guards the table keeps.
            let written = written_moras(node.get_string());
            if let Some(written) = written.as_ref().filter(|w| w.len() == moras.len()) {
                for (at, (mora, (spelling, as_written))) in
                    moras.iter_mut().zip(written).enumerate()
                {
                    let leading = first && at == 0;
                    let previous = match at {
                        0 => previous_written.as_deref(),
                        _ => Some(written[at - 1].0.as_str()),
                    };
                    let repeated = previous == Some(spelling.as_str());
                    if matches!(spelling.as_str(), "は" | "へ")
                        && mora.phonemes != *as_written
                        && (leading || repeated)
                    {
                        mora.phonemes = as_written.clone();
                    }
                }
            }
            previous_written = written.and_then(|w| w.last().map(|(spelling, _)| spelling.clone()));
            first = false;
            previous_vowel = moras
                .last()
                .and_then(|mora| mora.phonemes.last())
                .filter(|phoneme| VOWELS.contains(&phoneme.as_str()))
                .cloned();
            match (node.get_chain_flag(), phrases.last_mut()) {
                (Some(true), Some(last)) => last.moras.extend(moras),
                _ => phrases.push(AccentPhrase {
                    moras,
                    accent: Some(pron.accent()),
                }),
            }
        }
        // The dictionary's words cannot see the spaces a lyric marks them with: it reads はしる
        // after わたしは as *washiru*. Where the phrase is all kana, its written は and へ take
        // the phrase-level reading, which can — everything else stays the dictionary's.
        if let Some(spoken) = spoken_kana_lyric(text, Some(self))
            && spoken.len() == phrases.iter().map(|p| p.moras.len()).sum::<usize>()
        {
            let moras = phrases
                .iter_mut()
                .flat_map(|phrase| phrase.moras.iter_mut());
            for (mora, (written, phonemes)) in moras.zip(spoken) {
                if matches!(written.as_str(), "は" | "へ") {
                    mora.phonemes = phonemes;
                }
            }
        }
        Ok(phrases)
    }
}

/// A node's written surface as `(hiragana mora, phonemes as written)`, or `None` where it is
/// not all kana.
///
/// A leading ー — NJD gives the auxiliary in ゆこう a node of its own — is a mora whose vowel
/// lives in the node before, and is spelt here with no phonemes.
fn written_moras(surface: &str) -> Option<Vec<(String, Vec<String>)>> {
    let held = held_vowels(surface);
    let rest = held.trim_start_matches('ー');
    let mut moras: Vec<(String, Vec<String>)> = (0..held.chars().count() - rest.chars().count())
        .map(|_| ("ー".to_string(), Vec::new()))
        .collect();
    if !rest.is_empty() {
        moras.extend(
            walk_spaced(rest)?
                .into_iter()
                .map(|mora| (mora.text, mora.phonemes)),
        );
    }
    (!moras.is_empty()).then_some(moras)
}

/// A wave dash or half-width prolonged mark spelt as the ー the dictionary reads.
///
/// The dictionary takes 〜 for punctuation and drops it, so あ〜 would lose its held mora.
fn held_vowels(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '〜' | '～' | 'ｰ' => 'ー',
            c => c,
        })
        .collect()
}

/// Reads one dictionary node while retaining the vowel a preceding node can prolong.
fn dictionary_moras(kana: &str, previous_vowel: Option<&str>) -> Option<Vec<SungMora>> {
    let mut remaining = kana;
    let mut moras = Vec::new();
    // NJD turns the auxiliary in ゆこう into a separate ー node. Its vowel lives in
    // ゆこ, but its mora and accent-group membership still belong to the current node.
    while let Some(rest) = remaining.strip_prefix('ー') {
        let vowel = previous_vowel.filter(|vowel| VOWELS.contains(vowel))?;
        moras.push(SungMora {
            text: "ー".into(),
            phonemes: vec![vowel.into()],
        });
        remaining = rest;
    }
    moras.extend(
        split_kana_lyric(remaining)?
            .into_iter()
            .map(|(text, phonemes)| SungMora { text, phonemes }),
    );
    Some(moras)
}

impl JapaneseDictionary {
    /// Which moras of a kana phrase the dictionary reads as the particles は (*wa*) and
    /// へ (*e*), or `None` where its words do not line up with the phrase's moras.
    fn particle_marks(&self, text: &str) -> Option<Vec<bool>> {
        let mut njd = self.inner.text_to_njd(&held_vowels(text)).ok()?;
        njd.preprocess();
        let mut marks = Vec::new();
        for node in &njd.nodes {
            let surface = node.get_string();
            if surface.chars().all(char::is_whitespace) {
                continue;
            }
            let written = written_moras(surface).or_else(|| {
                // A node that is all punctuation is no mora at all; anything else unread
                // means the alignment is lost.
                surface.chars().all(|c| !c.is_alphanumeric()).then(Vec::new)
            })?;
            let spoken: Vec<String> = node
                .get_pron()
                .moras()
                .iter()
                .map(|mora| mora.to_string().replace('’', ""))
                .collect();
            let aligned = spoken.len() == written.len();
            for (at, (spelling, _)) in written.iter().enumerate() {
                let said = aligned.then(|| spoken[at].as_str());
                marks.push(matches!(
                    (spelling.as_str(), said),
                    ("は", Some("ワ")) | ("へ", Some("エ"))
                ));
            }
        }
        Some(marks)
    }
}

/// The moras of a kana phrase as they are spoken, or `None` where any of it is not kana.
///
/// A lyric laid across several notes is a phrase, and a phrase is where は and へ have a
/// reading: the particles sing *wa* and *e*, the same kana inside a word sing *ha* and *he*.
/// With a dictionary its word boundaries decide — きみはわらう needs no spaces — and a word
/// that ends at a space counts as well; without one, only the space does. The text of each
/// mora stays as written. A single mora on its own is read as written, because nothing says
/// which word it belongs to.
pub fn spoken_kana_lyric(
    text: &str,
    dictionary: Option<&JapaneseDictionary>,
) -> Option<Vec<(String, Vec<String>)>> {
    let marks = dictionary.and_then(|dictionary| dictionary.particle_marks(text));
    spoken_moras(text, marks.as_deref())
}

/// The phonemes of one note's lyric: the kana table, then the dictionary, then an honest error.
///
/// An empty (or all-whitespace) lyric is an empty answer, not an error — it is what every note
/// starts as. `dictionary` is optional because it is genuinely optional: a machine with no
/// dictionary folder sings every kana lyric exactly as well as one with it.
pub fn lyric_phonemes(
    lyric: &str,
    dictionary: Option<&JapaneseDictionary>,
) -> Result<Vec<String>, VocalError> {
    let lyric = lyric.trim();
    if lyric.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(phonemes) = kana_phonemes(lyric) {
        return Ok(phonemes);
    }
    match dictionary {
        Some(dictionary) => dictionary.phonemes(lyric),
        None => Err(VocalError::NeedsDictionary {
            text: lyric.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_split_dictionary_long_vowel_keeps_its_mora_and_previous_vowel() {
        let mut moras = dictionary_moras("ユコ", None).unwrap();
        let vowel = moras.last().unwrap().phonemes.last().unwrap();
        let continuation = dictionary_moras("ー", Some(vowel)).unwrap();
        assert_eq!(continuation.len(), 1);
        assert_eq!(continuation[0].text, "ー");
        assert_eq!(continuation[0].phonemes, ["o"]);
        moras.extend(continuation);
        assert_eq!(
            moras,
            dictionary_moras("ユコー", None).unwrap(),
            "a dictionary node boundary must not change sung moras"
        );
        assert_eq!(
            dictionary_moras("ーート", Some("o")).unwrap(),
            dictionary_moras("オーート", None).unwrap()[1..]
        );
    }

    #[test]
    fn a_dictionary_long_vowel_needs_immediately_preceding_vowel_context() {
        for previous in [None, Some("ʔ"), Some("ɴ")] {
            assert!(dictionary_moras("ー", previous).is_none());
        }
        assert!(dictionary_moras("ー漢字", Some("o")).is_none());
        assert_eq!(
            dictionary_moras("ッ", Some("o")).unwrap()[0].phonemes,
            ["ʔ"]
        );
    }

    #[test]
    fn the_dictionary_reads_volitional_lyrics_without_losing_a_section() {
        let Some(folder) = std::env::var_os("AURIS_JAPANESE_DICTIONARY") else {
            return;
        };
        let dictionary = JapaneseDictionary::load(Path::new(&folder)).expect("a loadable folder");
        for (text, expected_moras) in [("ずっと きみと", 6), ("あるいて ゆこう", 7)] {
            let phrases = dictionary.accent_phrases(text).expect("valid lyrics read");
            assert_eq!(
                phrases.iter().map(|p| p.moras.len()).sum::<usize>(),
                expected_moras
            );
            assert!(phrases.iter().all(|p| p.accent.unwrap() <= p.moras.len()));
            let moras: Vec<_> = phrases.iter().flat_map(|phrase| &phrase.moras).collect();
            assert!(moras.iter().all(|mora| !mora.phonemes.is_empty()));
            if text.ends_with("ゆこう") {
                assert_eq!(moras.last().unwrap().text, "ー");
                assert_eq!(moras.last().unwrap().phonemes, ["o"]);
            } else {
                assert_eq!(moras[1].text, "っ");
                assert_eq!(moras[1].phonemes, ["ʔ"]);
            }
        }
    }

    /// Runs only where `AURIS_JAPANESE_DICTIONARY` points at a compiled dictionary folder —
    /// the same silent-skip contract the singer's model tests keep, and for the same reason:
    /// CI has no dictionary, and a test that fails for that would teach people to ignore it.
    #[test]
    fn the_dictionary_reads_accent_phrases() {
        let Some(folder) = std::env::var_os("AURIS_JAPANESE_DICTIONARY") else {
            return;
        };
        let dictionary = JapaneseDictionary::load(Path::new(&folder)).expect("a loadable folder");
        assert_eq!(
            dictionary.phonemes("歌").expect("kanji reads"),
            ["ɯ", "t", "a"]
        );
        let phrases = dictionary
            .accent_phrases("端を渡る")
            .expect("plain text reads");
        assert!(!phrases.is_empty());
        for phrase in &phrases {
            assert!(phrase.accent.is_some(), "the dictionary knows its accents");
            assert!(!phrase.moras.is_empty());
            assert!(
                phrase.moras.iter().all(|mora| !mora.phonemes.is_empty()),
                "every mora sings something"
            );
            assert!(
                phrase.accent.unwrap_or(0) <= phrase.moras.len(),
                "a nucleus falls inside its own phrase"
            );
        }
    }

    fn sounds(moras: &[(String, Vec<String>)]) -> Vec<&str> {
        moras
            .iter()
            .flat_map(|(_, phonemes)| phonemes.iter().map(String::as_str))
            .collect()
    }

    #[test]
    fn without_a_dictionary_a_spaced_particle_is_still_spoken() {
        let moras = spoken_kana_lyric("きみは そらへ", None).unwrap();
        assert_eq!(sounds(&moras)[4..6], ["w", "a"]);
        assert_eq!(*sounds(&moras).last().unwrap(), "e");
        assert_eq!(moras[2].0, "は", "the note keeps the kana as written");
    }

    /// The particles inside unspaced kana, read by the dictionary's words — and its
    /// over-eager *wa* held back where the phrase plainly holds none.
    #[test]
    fn the_dictionary_finds_particles_and_keeps_the_guards() {
        let Some(folder) = std::env::var_os("AURIS_JAPANESE_DICTIONARY") else {
            return;
        };
        let dictionary = JapaneseDictionary::load(Path::new(&folder)).expect("a loadable folder");
        let read = |text: &str| spoken_kana_lyric(text, Some(&dictionary)).unwrap();
        assert_eq!(sounds(&read("きみはわらう"))[4..6], ["w", "a"]);
        assert_eq!(*sounds(&read("とおくへいこう")).get(5).unwrap(), "e");
        assert_eq!(sounds(&read("はなが さいた"))[..2], ["h", "a"]);
        assert_eq!(sounds(&read("はは")), ["h", "a", "h", "a"]);
        assert_eq!(sounds(&read("こんにちは"))[7..], ["w", "a"]);

        let flat = |text: &str| -> Vec<String> {
            dictionary
                .accent_phrases(text)
                .unwrap()
                .into_iter()
                .flat_map(|phrase| phrase.moras)
                .flat_map(|mora| mora.phonemes)
                .collect()
        };
        assert_eq!(flat("はは"), ["h", "a", "h", "a"]);
        assert_eq!(flat("母は")[4..], ["w", "a"]);
        assert_eq!(flat("空へ")[4..], ["e"]);
        assert_eq!(flat("あ〜"), ["a", "a"], "a wave dash is a held vowel");
        assert_eq!(
            flat("わたしは はしる"),
            [
                "w", "a", "t", "a", "ɕ", "i", "w", "a", "h", "a", "ɕ", "i", "ɾ", "ɯ"
            ]
        );
    }

    #[test]
    fn kana_needs_no_dictionary_and_kanji_says_which_setting_is_missing() {
        assert_eq!(lyric_phonemes("さ", None).unwrap(), ["s", "a"]);
        assert_eq!(lyric_phonemes("  ", None).unwrap(), Vec::<String>::new());
        match lyric_phonemes("歌", None) {
            Err(VocalError::NeedsDictionary { text }) => assert_eq!(text, "歌"),
            other => panic!("expected NeedsDictionary, got {other:?}"),
        }
    }

    #[test]
    fn a_folder_that_is_not_a_dictionary_is_an_error_not_a_panic() {
        let error = JapaneseDictionary::load(Path::new("Z:/no/such/folder")).unwrap_err();
        assert!(matches!(error, VocalError::Dictionary { .. }));
    }
}
