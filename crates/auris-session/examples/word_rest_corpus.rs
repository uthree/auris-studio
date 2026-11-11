//! Counts rests inside words against rests between them in songs written by people, for
//! calibrating `lyric_rhythm`'s `rest_in_word` with the dictionary the tool itself uses.
//!
//! ```text
//! cargo run --release -p auris-session --example word_rest_corpus -- <songs.jsonl>
//! ```
//!
//! Each input line is one song, `{"song": "...", "moras": [{"kana": "ま", "note": 0,
//! "gap_beats": 0.0}, ...]}`: its moras in time order, the index of the note each is sung on,
//! and on a note's first mora the silence before that note in beats. A converter for a
//! MusicXML corpus writes that. Words come from the installed Japanese dictionary, read from
//! the song's kana as a whole; a song whose reading has other moras is skipped and counted.
//! The rules are `lyric_rhythm`'s: a rest is at least a sixteenth, the silence after っ is the
//! sokuon and not a rest, and a breath between the halves of a reduplicated word is not inside
//! it. One JSON line is printed for the corpus.

use std::io::BufRead;
use std::path::Path;

use auris_vocal::JapaneseDictionary;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct Song {
    moras: Vec<Mora>,
}

#[derive(Deserialize)]
struct Mora {
    kana: String,
    note: usize,
    gap_beats: f64,
}

/// A sixteenth, as `lyric_rhythm`'s `WORD_REST` is.
const WORD_REST_BEATS: f64 = 0.25;

#[derive(Default)]
struct Tally {
    steps: usize,
    rests: usize,
    sixteenth: usize,
    eighth: usize,
    beat: usize,
}

impl Tally {
    fn add(&mut self, gap: f64) {
        self.steps += 1;
        if gap >= WORD_REST_BEATS {
            self.rests += 1;
            match gap {
                g if g < 0.5 => self.sixteenth += 1,
                g if g < 1.0 => self.eighth += 1,
                _ => self.beat += 1,
            }
        }
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "steps": self.steps, "rests": self.rests,
            "rate": (self.rests as f64 / self.steps.max(1) as f64 * 10000.0).round() / 10000.0,
            "by_length": {"sixteenth": self.sixteenth, "eighth": self.eighth, "beat_or_more": self.beat},
        })
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: word_rest_corpus <songs.jsonl>")?;
    let folder = auris_session::library::installed_dictionary()
        .ok_or("no Japanese dictionary is installed")?;
    let dictionary = JapaneseDictionary::load(&folder)?;
    let (mut inside, mut between) = (Tally::default(), Tally::default());
    let (mut songs, mut read, mut spots) = (0, 0, Vec::new());
    for line in std::io::BufReader::new(std::fs::File::open(Path::new(&path))?).lines() {
        let song: Song = serde_json::from_str(&line?)?;
        songs += 1;
        let text: String = song.moras.iter().map(|m| m.kana.as_str()).collect();
        let Ok(phrases) = dictionary.accent_phrases(&text) else {
            continue;
        };
        let mut word = Vec::new();
        let mut count = 0;
        for phrase in &phrases {
            for at in 0..phrase.moras.len() {
                if at == 0 || phrase.words.contains(&at) {
                    count += 1;
                }
                word.push(count);
            }
        }
        if word.len() != song.moras.len() {
            continue;
        }
        read += 1;
        let texts: Vec<&str> = song.moras.iter().map(|m| m.kana.as_str()).collect();
        for at in 1..song.moras.len() {
            let mora = &song.moras[at];
            if mora.note == song.moras[at - 1].note || texts[at - 1] == "っ" {
                continue;
            }
            if word[at] != word[at - 1] {
                between.add(mora.gap_beats);
                continue;
            }
            let start = (0..at)
                .rev()
                .take_while(|m| word[*m] == word[at])
                .last()
                .unwrap_or(at);
            let end = (at..word.len())
                .take_while(|m| word[*m] == word[at])
                .last()
                .unwrap_or(at);
            if texts[start..at] == texts[at..=end] {
                continue;
            }
            inside.add(mora.gap_beats);
            if mora.gap_beats >= WORD_REST_BEATS && spots.len() < 20 {
                spots.push(format!(
                    "{}・{}",
                    texts[start..at].concat(),
                    texts[at..=end].concat()
                ));
            }
        }
    }
    println!(
        "{}",
        json!({
            "songs": songs, "read": read,
            "inside_words": inside.json(), "between_words": between.json(),
            "examples": spots,
        })
    );
    Ok(())
}
