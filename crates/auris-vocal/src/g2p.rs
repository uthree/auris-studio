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

use crate::kana::kana_phonemes;
use crate::openjtalk::openjtalk_phoneme;

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
            .run_frontend(text)
            .map_err(|error| VocalError::Text {
                text: text.to_string(),
                detail: error.to_string(),
            })?;
        let mut phonemes = Vec::new();
        for label in &labels {
            let name = label_phoneme(label).ok_or_else(|| VocalError::Text {
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
}

/// The current phoneme of one full-context label: the stretch between `-` and `+`.
///
/// A label reads `xx^sil-k+o=N/A:…` — five phonemes of context around the current one, then
/// the linguistic features. Only the current phoneme is wanted, and cutting it out here spares
/// the crate a dependency on a label parser that would be thrown away above this line.
fn label_phoneme(label: &str) -> Option<&str> {
    let from = label.find('-')? + 1;
    let to = from + label[from..].find('+')?;
    Some(&label[from..to])
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
    fn labels_give_up_their_phoneme() {
        assert_eq!(label_phoneme("xx^sil-k+o=N/A:-3+1+5"), Some("k"));
        assert_eq!(label_phoneme("k^o-N+n=i/A:0+2+4"), Some("N"));
        assert_eq!(label_phoneme("no separators"), None);
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
