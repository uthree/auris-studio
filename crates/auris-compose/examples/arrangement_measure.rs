//! Emits arrangement measurements for every shipped preset as JSON.
//!
//! Run with `cargo run -p auris-compose --example arrangement_measure` or pass an explicit seed
//! with `--seed 123`. Without `--seed`, each preset's seed from its specification is used. The
//! output is intended for before/after comparison of arrangement shape; it is not a single score
//! for musical quality. A bar's `sounding_track_count` means that at least one note's written
//! duration overlaps the bar; `note_onset_count` counts notes whose written start is in it.
//! An onset-free bar can still be sounding when a note is held across its boundary.

use std::collections::BTreeSet;

use auris_compose::{PRESETS, compose, frame};
use serde_json::{Value, json};

struct NoteSpan<'a> {
    track: &'a str,
    start: i64,
    end: i64,
}

fn requested_seed() -> Result<Option<u64>, String> {
    let mut args = std::env::args().skip(1);
    let Some(argument) = args.next() else {
        return Ok(None);
    };
    let value = if let Some(value) = argument.strip_prefix("--seed=") {
        value.to_owned()
    } else if argument == "--seed" {
        args.next()
            .ok_or_else(|| "--seed requires an unsigned integer".to_owned())?
    } else {
        return Err(format!("unknown argument: {argument}"));
    };
    let seed = value
        .parse()
        .map_err(|_| format!("invalid seed: {value}"))?;
    if let Some(argument) = args.next() {
        return Err(format!("unexpected argument: {argument}"));
    }
    Ok(Some(seed))
}

fn notes<'a>(piece: &'a auris_compose::Composition) -> Vec<NoteSpan<'a>> {
    piece
        .tracks
        .iter()
        .flat_map(|track| {
            track.clips.iter().flat_map(move |clip| {
                clip.notes.iter().map(move |note| {
                    let start = clip.start.raw() + note.start.raw();
                    NoteSpan {
                        track: &track.name,
                        start,
                        end: start + note.length.raw().max(1),
                    }
                })
            })
        })
        .collect()
}

fn track_names(spans: &[NoteSpan<'_>], start: i64, end: i64) -> BTreeSet<String> {
    spans
        .iter()
        .filter(|span| span.start < end && span.end > start)
        .map(|span| span.track.to_owned())
        .collect()
}

struct SectionMeasurements {
    sections: Vec<Value>,
    participants: Vec<BTreeSet<String>>,
    onsets: Vec<usize>,
    silent_bars: usize,
    non_coda_tracks: BTreeSet<String>,
}

fn section_measurements(
    frame: &auris_compose::frame::Frame,
    spans: &[NoteSpan<'_>],
) -> SectionMeasurements {
    let bar_ticks = frame.grid.bar_ticks().raw().max(1);
    let mut sections = Vec::new();
    let mut participants = Vec::new();
    let mut all_onsets = Vec::new();
    let mut silent_bars = 0;
    let mut non_coda_tracks = BTreeSet::new();

    for section in frame.sections.iter().filter(|section| !section.coda) {
        let section_tracks = track_names(
            spans,
            section.start.raw(),
            section.start.raw() + section.length.raw(),
        );
        let mut bars = Vec::new();
        for bar in 0..section.bars {
            let start = section.start.raw() + bar_ticks * bar as i64;
            let end = start + bar_ticks;
            let sounding_tracks = track_names(spans, start, end);
            if sounding_tracks.is_empty() {
                silent_bars += 1;
            }
            let onset_count = spans
                .iter()
                .filter(|span| start <= span.start && span.start < end)
                .count();
            all_onsets.push(onset_count);
            bars.push(json!({
                "bar": bar + 1,
                "sounding_track_count": sounding_tracks.len(),
                "sounding_tracks": sounding_tracks,
                "note_onset_count": onset_count,
            }));
        }
        participants.push(section_tracks.clone());
        non_coda_tracks.extend(section_tracks.iter().cloned());
        sections.push(json!({
            "name": section.name,
            "instance": section.instance,
            "bars": section.bars,
            "instrument_track_count": section_tracks.len(),
            "instrument_tracks": section_tracks,
            "bar_measurements": bars,
        }));
    }
    SectionMeasurements {
        sections,
        participants,
        onsets: all_onsets,
        silent_bars,
        non_coda_tracks,
    }
}

fn participation_changes(participants: &[BTreeSet<String>], sections: &[Value]) -> Vec<Value> {
    participants
        .windows(2)
        .enumerate()
        .map(|(index, pair)| {
            let added: Vec<_> = pair[1].difference(&pair[0]).cloned().collect();
            let removed: Vec<_> = pair[0].difference(&pair[1]).cloned().collect();
            json!({
                "from": sections[index]["name"],
                "to": sections[index + 1]["name"],
                "added_tracks": added,
                "removed_tracks": removed,
                "changed_track_count": pair[0].symmetric_difference(&pair[1]).count(),
            })
        })
        .collect()
}

fn measure_preset(preset: &auris_compose::SongPreset, seed: Option<u64>) -> Value {
    let mut spec = preset.spec();
    let seed_value = seed.unwrap_or(spec.seed);
    spec.seed = seed_value;
    let frame = frame::plan(&spec);
    let piece = compose(&spec);
    let spans = notes(&piece);
    let SectionMeasurements {
        sections,
        participants,
        onsets,
        silent_bars,
        non_coda_tracks,
    } = section_measurements(&frame, &spans);
    let peak = onsets.iter().copied().max().unwrap_or(0);
    let quiet_nonzero = onsets.iter().copied().filter(|count| *count > 0).min();
    let ratio = quiet_nonzero.map(|quiet| peak as f64 / quiet as f64);

    json!({
        "preset": preset.name,
        "seed": seed_value,
        "seed_source": if seed.is_some() { "argument" } else { "preset_default" },
        "declared_part_count": spec.parts.len(),
        "instrument_track_count": piece.tracks.len(),
        "non_coda_instrument_track_count": non_coda_tracks.len(),
        "note_count": piece.note_count(),
        "non_coda_section_count": sections.len(),
        "sections": sections,
        "section_participation_changes": participation_changes(&participants, &sections),
        "density": {
            "bar_count": onsets.len(),
            "onset_free_bar_count": onsets.iter().filter(|count| **count == 0).count(),
            "silent_bar_count": silent_bars,
            "peak_note_onsets_per_bar": peak,
            "quietest_nonzero_note_onsets_per_bar": quiet_nonzero,
            "peak_to_quiet_density_ratio": ratio,
        },
    })
}

fn run() -> Result<(), String> {
    let seed = requested_seed()?;
    let presets: Vec<_> = PRESETS
        .iter()
        .map(|preset| measure_preset(preset, seed))
        .collect();
    let output = json!({
        "measurement": "arrangement_structure",
        "description": "Arrangement structure measurements for before/after comparison; not a total quality score.",
        "sounding_track_definition": "A track sounds in a bar when a written note duration overlaps that bar; onset_free_bar_count only counts bars with zero note starts.",
        "seed_argument": seed,
        "presets": presets,
    });
    let rendered = serde_json::to_string_pretty(&output).map_err(|error| error.to_string())?;
    println!("{rendered}");
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("arrangement_measure: {error}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::{NoteSpan, track_names};

    #[test]
    fn a_held_note_makes_an_onset_free_bar_sounding() {
        let spans = [NoteSpan {
            track: "pad",
            start: 0,
            end: 960,
        }];
        assert!(track_names(&spans, 480, 960).contains("pad"));
        assert_eq!(
            spans
                .iter()
                .filter(|span| 480 <= span.start && span.start < 960)
                .count(),
            0
        );
    }
}
