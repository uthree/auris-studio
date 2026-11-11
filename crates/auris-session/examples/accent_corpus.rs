//! Measures sung melodies against their lyric's pitch accent, for calibrating the thresholds
//! of `lyric_accent` against songs written by people.
//!
//! ```text
//! cargo run --release -p auris-session --example accent_corpus -- <songs.jsonl>
//! cargo run --release -p auris-session --example accent_corpus -- --project <song.auris>...
//! ```
//!
//! Each input line is one song, `{"song": "...", "lines": [[{"pitch": 67, "kana": "ま"}, ...]]}`
//! with one kana spelling a note (`""` for a melisma, `"とー"` for a held vowel); a converter
//! for a MusicXML corpus writes that. With `--project` the lead singer of each saved song is
//! measured the way the agent's tool measures it. Either way one JSON line is printed per
//! song: the tally and the verdict. The dictionary is the installed one.

use std::io::BufRead;
use std::path::Path;

use auris_session::session::lyric_accent::{
    AccentCounts, AccentNote, accent_counts, accent_verdict, note_levels,
};
use auris_session::{Session, SessionOptions};
use auris_vocal::{JapaneseDictionary, split_kana_moras};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct Song {
    song: String,
    lines: Vec<Vec<SungNote>>,
}

#[derive(Deserialize)]
struct SungNote {
    pitch: i32,
    kana: String,
}

fn tally(dictionary: &JapaneseDictionary, line: &[SungNote]) -> Option<AccentCounts> {
    let notes: Vec<AccentNote> = line
        .iter()
        .map(|note| {
            Some(AccentNote {
                pitch: note.pitch,
                moras: match note.kana.as_str() {
                    "" => 0,
                    kana => split_kana_moras(kana)?.len(),
                },
                closure: note.kana == "っ",
            })
        })
        .collect::<Option<_>>()?;
    let text: String = line.iter().map(|note| note.kana.as_str()).collect();
    let mut levels = Vec::new();
    for phrase in dictionary.accent_phrases(&text).ok()? {
        levels.extend(phrase.levels()?);
    }
    let per_note = note_levels(&notes, &levels)?;
    let pitches: Vec<i32> = line.iter().map(|note| note.pitch).collect();
    Some(accent_counts(&per_note, &pitches).0)
}

fn summary(song: &str, counts: AccentCounts, lines: usize, unread: usize) -> serde_json::Value {
    let rate =
        |count: usize| (count as f64 / counts.changes.max(1) as f64 * 1000.0).round() / 1000.0;
    json!({
        "song": song, "lines": lines, "unread": unread,
        "changes": counts.changes, "followed": rate(counts.followed),
        "same_pitch": rate(counts.same_pitch), "opposed": rate(counts.opposed),
        "verdict": accent_verdict(&counts),
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let folder = auris_session::library::installed_dictionary()
        .ok_or("no Japanese dictionary is installed")?;
    if args.first().map(String::as_str) == Some("--project") {
        let mut session = Session::new(SessionOptions::headless())?;
        session.set_japanese_dictionary(Some(&folder))?;
        for path in &args[1..] {
            session.open(Path::new(path))?;
            let lead = session
                .project()
                .tracks
                .iter()
                .find(|track| track.kind.is_singer())
                .map(|track| track.id)
                .ok_or("the song has no singer")?;
            match session.lyric_accent(lead, &[]) {
                Ok(report) => {
                    let unread = report.lines.iter().filter(|l| l.error.is_some()).count();
                    println!("{}", summary(path, report.song, report.lines.len(), unread));
                }
                Err(error) => println!("{}", json!({"song": path, "error": error})),
            }
        }
        return Ok(());
    }
    let dictionary = JapaneseDictionary::load(&folder)?;
    let input = std::fs::File::open(args.first().ok_or("name a .jsonl file")?)?;
    for line in std::io::BufReader::new(input).lines() {
        let song: Song = serde_json::from_str(&line?)?;
        let mut counts = AccentCounts::default();
        let mut unread = 0;
        for sung in &song.lines {
            match tally(&dictionary, sung) {
                Some(line) => {
                    counts.changes += line.changes;
                    counts.followed += line.followed;
                    counts.same_pitch += line.same_pitch;
                    counts.opposed += line.opposed;
                }
                None => unread += 1,
            }
        }
        println!("{}", summary(&song.song, counts, song.lines.len(), unread));
    }
    Ok(())
}
