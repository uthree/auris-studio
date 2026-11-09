//! Structured chord and instrument references, independent of edits to the song.

use std::io::Write;
use std::path::{Path, PathBuf};

use auris_core::theory::{key::Key, numeral::Numeral};
use auris_core::time::{TICKS_PER_QUARTER, TimeSignature};
use serde_json::{Value, json};

use crate::{Session, TimbreSound};

/// Chord and timbre references are a few kilobytes; this only refuses something that is not one.
const MAX_MUSIC_REFERENCE_BYTES: usize = 256 * 1024;

/// Parse bar-delimited chords in a captured key and meter without changing the song.
/// Chords within a bar divide its duration equally; both absolute names and numerals work.
pub fn chord_reference(text: &str, key: Key, meter: TimeSignature) -> Result<Value, String> {
    if text.len() > 16 * 1024
        || !TimeSignature::NUMERATORS.contains(&meter.numerator)
        || !TimeSignature::DENOMINATORS.contains(&meter.denominator)
    {
        return Err("Invalid or oversized chord reference".into());
    }
    let text = text.trim();
    let text = text.strip_prefix('|').unwrap_or(text);
    let text = text.strip_suffix('|').unwrap_or(text).trim();
    let bars: Vec<_> = text.split('|').collect();
    if bars.len() > 128 {
        return Err("A chord reference supports up to 128 bars".into());
    }
    let bar_ticks = meter.ticks_per_bar().0;
    let mut chords = Vec::new();
    for (bar, input) in bars.iter().enumerate() {
        let tokens: Vec<_> = input.split_whitespace().collect();
        if tokens.is_empty() || tokens.len() > 16 {
            return Err(format!("Bar {} must contain 1–16 chords", bar + 1));
        }
        for (slot, token) in tokens.iter().enumerate() {
            let numeral = Numeral::parse_in_key(token, key)
                .ok_or_else(|| format!("Invalid chord: {token}"))?;
            let start = bar_ticks * slot as i64 / tokens.len() as i64;
            let end = bar_ticks * (slot + 1) as i64 / tokens.len() as i64;
            chords.push(json!({
                "bar": bar + 1, "symbol": token,
                "chord": numeral.chord_in(key).name_in(key),
                "start_ticks": bar as i64 * bar_ticks + start,
                "length_ticks": end - start,
            }));
        }
    }
    Ok(json!({"reference_type": "chord_progression", "version": 1,
        "key": key.to_text(), "meter": meter, "ticks_per_quarter": TICKS_PER_QUARTER,
        "notation": text, "chords": chords}))
}

/// Capture the selected map sound's stable instrument and preset addresses.
/// SoundFont metadata helps resolve a project-local font ID in another project.
pub fn sound_reference(session: &Session, sound: &TimbreSound) -> Value {
    let font = sound.preset.as_ref().and_then(|preset| {
        session
            .soundfonts()
            .find(|font| font.id == preset.font)
            .map(|font| {
                json!({"id": font.id, "name": font.name, "byte_size": font.byte_size,
                "path": font.path.resolve(session.project_folder())})
            })
    });
    json!({"reference_type": "instrument_choice", "version": 1,
        "name": sound.name, "instrument_id": sound.instrument_id,
        "preset": sound.preset, "soundfont": font})
}

/// Save an immutable, bounded JSON attachment. No source project is modified.
pub fn save_snapshot(value: &Value, directory: &Path, label: &str) -> Result<PathBuf, String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_MUSIC_REFERENCE_BYTES {
        return Err("Music reference is too large".into());
    }
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let label: String = label
        .chars()
        .take(40)
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut file = tempfile::Builder::new()
        .prefix(&format!("{label}-"))
        .suffix(".json")
        .tempfile_in(directory)
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.keep().map(|(_, path)| path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chord_reference_preserves_bars_inversions_and_captured_key() {
        let value = chord_reference(
            "| C/E G7 | vi |",
            Key::parse("C major").unwrap(),
            TimeSignature::default(),
        )
        .unwrap();
        assert_eq!(value["chords"][0]["chord"], "C/E");
        assert_eq!(value["chords"][0]["length_ticks"], 1920);
        assert_eq!(value["chords"][1]["start_ticks"], 1920);
        assert_eq!(value["chords"][2]["chord"], "Am");
        assert_eq!(value["chords"][2]["start_ticks"], 3840);
        for invalid in ["", "| C || G |", "| nonsense |"] {
            assert!(
                chord_reference(
                    invalid,
                    Key::parse("C major").unwrap(),
                    TimeSignature::default()
                )
                .is_err()
            );
        }
    }
}
