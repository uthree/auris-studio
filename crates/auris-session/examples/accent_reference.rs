//! Measures the installed Japanese dictionary's readings and Tokyo pitch accent against a
//! hand-annotated reference, for comparing dictionaries without composing a song.
//!
//! ```text
//! cargo run --release -p auris-session --example accent_reference -- <jsut-label folder>
//! ```
//!
//! The folder is a checkout of `sarulab-speech/jsut-label` (CC-BY-SA 4.0; never copied into
//! this repository), or the two files used from it side by side: `text_kana/basic5000.yaml`,
//! whose `text_level2` is the sentence as spoken, and `e2e_symbol/katakana.yaml`, its kana with
//! `#` between accent phrases, `_` at a pause, `[` where pitch rises and `]` after the nucleus.
//!
//! Each sentence is read by the dictionary as `lyric_accent` reads a lyric. A sentence whose
//! moras sing other phonemes than the reference's, long vowels spelt either way, is a reading
//! miss. The accent is compared on every sentence with the reference's number of moras:
//!
//! * `levels`: the share of moras given the reference's high or low pitch.
//! * `steps`: the share of adjacent mora pairs whose step (rise, fall or level) matches, the
//!   quantity `lyric_accent` measures a melody against.
//! * `nuclei`: precision, recall and F1 of the morae carrying a nucleus.
//! * `boundaries`: the same for the morae starting an accent phrase. The reference is one
//!   speaker reading aloud, whose phrasing varies with pace, so this is the loosest of the four.
//!
//! One JSON line is printed, with a few reading misses and accent misses to inspect.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use auris_vocal::{JapaneseDictionary, split_kana_lyric};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct Texts {
    text_level2: String,
}

/// A sentence of the reference: its moras' phonemes, pitch, nuclei and phrase starts.
struct Reference {
    phonemes: Vec<Vec<String>>,
    kana: String,
    high: Vec<bool>,
    nuclei: Vec<usize>,
    starts: Vec<usize>,
}

/// Reads one `katakana.yaml` line's notation.
fn reference(notation: &str) -> Option<Reference> {
    let mut kana = String::new();
    // (mora index the mark sits before, the mark), and phrase starts by mora index.
    let mut rises = Vec::new();
    let mut nuclei = Vec::new();
    let mut starts = vec![0];
    let count = |kana: &str| split_kana_lyric(kana).map(|m| m.len());
    for c in notation.chars() {
        match c {
            '^' | '$' | '?' => {}
            '[' => rises.push(count(&kana)?),
            ']' => nuclei.push(count(&kana)?),
            '#' | '_' => {
                let at = count(&kana)?;
                if starts.last() != Some(&at) {
                    starts.push(at);
                }
            }
            c => kana.push(c),
        }
    }
    let moras = split_kana_lyric(&kana)?;
    let n = moras.len();
    let mut high = vec![false; n];
    for (index, &start) in starts.iter().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(n);
        // A nucleus after the phrase's first mora is 頭高: high from the start.
        let nucleus = nuclei.iter().copied().find(|k| *k > start && *k <= end);
        let rise = rises.iter().copied().find(|r| *r >= start && *r < end);
        let from = match (nucleus, rise) {
            (Some(k), _) if k == start + 1 => Some(start),
            (_, Some(r)) => Some(r),
            _ => None,
        };
        if let Some(from) = from {
            for level in high.iter_mut().take(nucleus.unwrap_or(end)).skip(from) {
                *level = true;
            }
        }
    }
    Some(Reference {
        phonemes: moras.into_iter().map(|(_, p)| p).collect(),
        kana,
        high,
        // The nucleus mora itself, counted from 0.
        nuclei: nuclei.into_iter().map(|k| k - 1).collect(),
        starts,
    })
}

/// The moras with a held long vowel spelt as the vowel it holds: the reference writes 悲鳴 as
/// ヒメイ and 向こう as ムコウ where the dictionary reads ヒメー and ムコー, which is one sound,
/// and the speaker's ユー for いう as the word.
fn long_vowels(moras: &[Vec<String>]) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::with_capacity(moras.len());
    for mora in moras {
        let before = out.last().and_then(|m| m.last()).map(String::as_str);
        let held = match (before, mora.as_slice()) {
            (Some("o"), [u]) if u == "ɯ" => Some("o"),
            (Some("e"), [i]) if i == "i" => Some("e"),
            _ => None,
        };
        out.push(held.map_or_else(|| mora.clone(), |v| vec![v.to_string()]));
    }
    // いう said ユー (という as トユー) is the same word in the speaker's pronunciation.
    for at in 1..out.len() {
        if out[at - 1] == ["i"] && out[at] == ["ɯ"] {
            out[at - 1] = vec!["j".into(), "ɯ".into()];
        }
    }
    out
}

#[derive(Default)]
struct Score {
    hits: usize,
    predicted: usize,
    expected: usize,
}

impl Score {
    fn add(&mut self, predicted: &[usize], expected: &[usize]) {
        self.predicted += predicted.len();
        self.expected += expected.len();
        self.hits += predicted.iter().filter(|p| expected.contains(p)).count();
    }

    fn json(&self) -> serde_json::Value {
        let p = self.hits as f64 / self.predicted.max(1) as f64;
        let r = self.hits as f64 / self.expected.max(1) as f64;
        let f = if p + r > 0.0 {
            2.0 * p * r / (p + r)
        } else {
            0.0
        };
        let round = |x: f64| (x * 1000.0).round() / 1000.0;
        json!({"precision": round(p), "recall": round(r), "f1": round(f)})
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let folder = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: accent_reference <jsut-label folder>")?,
    );
    let find = |nested: &str, flat: &str| {
        [folder.join(nested), folder.join(flat)]
            .into_iter()
            .find(|path| path.is_file())
            .ok_or(format!("{nested} not found under {}", folder.display()))
    };
    let texts: BTreeMap<String, Texts> = serde_yaml_ng::from_str(&std::fs::read_to_string(find(
        "text_kana/basic5000.yaml",
        "basic5000.yaml",
    )?)?)?;
    let notations: BTreeMap<String, String> = serde_yaml_ng::from_str(&std::fs::read_to_string(
        find("e2e_symbol/katakana.yaml", "katakana.yaml")?,
    )?)?;
    let dictionary_folder = std::env::var_os("AURIS_JAPANESE_DICTIONARY")
        .map(PathBuf::from)
        .or_else(auris_session::library::installed_dictionary)
        .ok_or("no Japanese dictionary is installed")?;
    let dictionary = JapaneseDictionary::load(Path::new(&dictionary_folder))?;

    let (mut sentences, mut unparsed, mut misread) = (0, 0, 0);
    let (mut unread, mut uncounted) = (0, 0);
    let (mut moras, mut levels, mut pairs, mut steps) = (0, 0, 0, 0);
    let (mut nuclei, mut boundaries) = (Score::default(), Score::default());
    let (mut reading_examples, mut accent_examples) = (Vec::new(), Vec::new());
    for (id, notation) in &notations {
        let Some(text) = texts.get(id) else { continue };
        sentences += 1;
        let Some(expected) = reference(notation) else {
            unparsed += 1;
            continue;
        };
        let read = dictionary.accent_phrases(&text.text_level2).ok();
        let mut phonemes = Vec::new();
        let mut high = Vec::new();
        let mut starts = Vec::new();
        let mut found = Vec::new();
        let mut known = true;
        for phrase in read.iter().flatten() {
            starts.push(phonemes.len());
            match (phrase.levels(), phrase.accent) {
                (Some(l), Some(accent)) => {
                    if accent > 0 {
                        found.push(phonemes.len() + accent - 1);
                    }
                    high.extend(l);
                }
                _ => known = false,
            }
            phonemes.extend(phrase.moras.iter().map(|m| m.phonemes.clone()));
        }
        if read.is_none() || !known {
            unread += 1;
            continue;
        }
        if long_vowels(&phonemes) != long_vowels(&expected.phonemes) {
            misread += 1;
            if reading_examples.len() < 12 {
                let spoken: String = read
                    .iter()
                    .flatten()
                    .flat_map(|p| p.moras.iter().map(|m| m.text.clone()))
                    .collect();
                reading_examples.push(format!("{id}: {} / {spoken}", expected.kana));
            }
        }
        // The accent is compared wherever the moras line up, a slightly different reading
        // included: where pitch rises and falls is still comparable mora by mora.
        if phonemes.len() != expected.phonemes.len() {
            uncounted += 1;
            continue;
        }
        moras += high.len();
        levels += high
            .iter()
            .zip(&expected.high)
            .filter(|(a, b)| a == b)
            .count();
        let step = |l: &[bool], i: usize| (l[i + 1] as i8) - (l[i] as i8);
        for i in 0..high.len().saturating_sub(1) {
            pairs += 1;
            steps += usize::from(step(&high, i) == step(&expected.high, i));
        }
        nuclei.add(&found, &expected.nuclei);
        let inner = |s: &[usize]| s.iter().copied().filter(|x| *x > 0).collect::<Vec<_>>();
        boundaries.add(&inner(&starts), &inner(&expected.starts));
        if high != expected.high && accent_examples.len() < 12 {
            let spell = |l: &[bool]| {
                l.iter()
                    .map(|h| if *h { 'H' } else { 'L' })
                    .collect::<String>()
            };
            accent_examples.push(format!(
                "{id}: {} {} (reference) {} (dictionary)",
                expected.kana,
                spell(&expected.high),
                spell(&high)
            ));
        }
    }
    let share = |a: usize, b: usize| (a as f64 / b.max(1) as f64 * 1000.0).round() / 1000.0;
    println!(
        "{}",
        json!({
            "dictionary": dictionary_folder.display().to_string(),
            "sentences": sentences, "unparsed_reference": unparsed,
            "unread": unread, "reading_misses": misread,
            "other_mora_counts": uncounted,
            "accent_compared": sentences - unparsed - unread - uncounted,
            "levels": share(levels, moras), "steps": share(steps, pairs),
            "nuclei": nuclei.json(), "boundaries": boundaries.json(),
            "reading_examples": reading_examples, "accent_examples": accent_examples,
        })
    );
    Ok(())
}
