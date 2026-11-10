//! Whether a melody's lines differ: the form of each section, where lines enter, how they
//! move in time, where they end, and how predictable each one is from the song so far.
//!
//! A melody can pass every register check and still be one four-bar line pasted over every
//! lyric line of the song. Friends who heard such a song said it all sounded the same, that
//! every line started on the barline with no syncopation, that the rhythm never changed and
//! that it never settled; each of those is a fact in the notes. The numbers here read them,
//! and the warnings fire only at the extremes no song chooses by accident: a choice the
//! composer means keeps its reason in the production record like any lint finding.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use auris_core::{Project, Ticks, TrackId};
use serde_json::{Value, json};

/// Lines a section must hold before all of them being the same is reported.
pub const IDENTICAL_SECTION_LINES: usize = 3;
/// Lines a song must hold before a property shared by all of them is reported.
pub const UNIFORM_LINES: usize = 8;
/// Share of onset spacings one value may take before the rhythm is reported as one value.
pub const UNIFORM_SPACING_SHARE: f64 = 0.7;
/// Onset spacings needed before that share is judged.
pub const UNIFORM_SPACING_COUNT: usize = 50;
/// Bits a note under which a line counts as predictable from the song so far.
pub const PREDICTABLE_BITS: f64 = 1.0;
/// Lines a section other than a chorus must hold before singing an earlier section's lines
/// again, note for note, is reported: a second verse copied from the first. One is enough, since
/// a verse sung without a beat of rest is one line. A chorus repeats by design.
pub const RESTATED_SECTION_LINES: usize = 1;
/// Longest context the short-term model predicts from, in notes.
const PPM_ORDER: usize = 4;
/// Distinct (interval, spacing) symbols the model can name: ±12 semitones by 1..=16 sixteenths.
const ALPHABET: usize = 25 * 16;

/// One phrase of the foreground: notes on the timeline, as `(start, length, pitch)`.
struct Line {
    start: Ticks,
    notes: Vec<(Ticks, Ticks, i32)>,
}

/// The foreground's notes as one line of melody: the highest note at each onset.
fn melody_notes(project: &Project, track: TrackId) -> Vec<(Ticks, Ticks, i32)> {
    let Some(clips) = project.track(track).and_then(|t| t.kind.note_clips()) else {
        return Vec::new();
    };
    let mut by_onset: BTreeMap<Ticks, (Ticks, i32)> = BTreeMap::new();
    for clip in clips.iter().filter(|clip| !clip.muted) {
        for note in clip.playable_notes() {
            let at = clip.start + note.start;
            let pitch = i32::from(note.pitch);
            let entry = by_onset.entry(at).or_insert((note.length, pitch));
            if pitch > entry.1 {
                *entry = (note.length, pitch);
            }
        }
    }
    by_onset
        .into_iter()
        .map(|(at, (length, pitch))| (at, length, pitch))
        .collect()
}

/// The foreground cut into lines: at each line of the song-wide lyric annotation, and at every
/// rest of a beat or more. An annotation laid over a whole clip names a section, not a line, so
/// the rests inside it still cut.
fn lines_of(project: &Project, track: TrackId) -> Vec<Line> {
    let notes = melody_notes(project, track);
    let Some(last) = notes.last() else {
        return Vec::new();
    };
    let end = last.0 + last.1 + Ticks(1);
    let annotated: Vec<Ticks> = project
        .lyrics
        .spans_in(Ticks::ZERO, end)
        .iter()
        .map(|span| span.start)
        .collect();
    let mut lines = Vec::new();
    let mut current: Vec<(Ticks, Ticks, i32)> = Vec::new();
    for note in notes {
        if let Some(previous) = current.last() {
            let rested = note.0 - (previous.0 + previous.1) >= Ticks::QUARTER;
            let next_line = annotated
                .iter()
                .any(|start| *start > previous.0 && *start <= note.0);
            if rested || next_line {
                lines.push(Line {
                    start: current[0].0,
                    notes: std::mem::take(&mut current),
                });
            }
        }
        current.push(note);
    }
    if !current.is_empty() {
        lines.push(Line {
            start: current[0].0,
            notes: current,
        });
    }
    lines
}

/// One note as the model sees it: the interval from the note before and the spacing.
type Symbol = (i32, i32);

/// A PPM model with escape method C and exclusion, learning from the song as it predicts it.
#[derive(Default)]
struct Ppm {
    counts: HashMap<Vec<Symbol>, BTreeMap<Symbol, u32>>,
}

impl Ppm {
    fn information(&self, history: &[(i32, i32)], symbol: (i32, i32)) -> f64 {
        let mut escape = 1.0;
        let mut excluded: BTreeSet<(i32, i32)> = BTreeSet::new();
        for length in (0..=PPM_ORDER.min(history.len())).rev() {
            let context = &history[history.len() - length..];
            let Some(table) = self.counts.get(context) else {
                continue;
            };
            let seen: Vec<(&(i32, i32), &u32)> = table
                .iter()
                .filter(|(s, _)| !excluded.contains(*s))
                .collect();
            let total: u32 = seen.iter().map(|(_, c)| **c).sum();
            if total == 0 {
                continue;
            }
            let distinct = seen.len() as f64;
            if let Some((_, count)) = seen.iter().find(|(s, _)| **s == symbol) {
                return -(escape * f64::from(**count) / (f64::from(total) + distinct)).log2();
            }
            escape *= distinct / (f64::from(total) + distinct);
            excluded.extend(seen.iter().map(|(s, _)| **s));
        }
        -(escape / (ALPHABET - excluded.len()).max(1) as f64).log2()
    }

    fn learn(&mut self, history: &[(i32, i32)], symbol: (i32, i32)) {
        for length in 0..=PPM_ORDER.min(history.len()) {
            let context = history[history.len() - length..].to_vec();
            *self
                .counts
                .entry(context)
                .or_default()
                .entry(symbol)
                .or_insert(0) += 1;
        }
    }
}

/// Mean short-term information content of each line, in bits a note.
fn predictability(lines: &[Line]) -> Vec<Option<f64>> {
    let mut model = Ppm::default();
    let mut history = Vec::new();
    let mut previous: Option<(Ticks, i32)> = None;
    lines
        .iter()
        .map(|line| {
            let mut bits = Vec::new();
            for &(at, _, pitch) in &line.notes {
                if let Some((before, before_pitch)) = previous {
                    let interval = (pitch - before_pitch).clamp(-12, 12);
                    let spacing = ((at - before).raw() as f64 / (Ticks::QUARTER.raw() / 4) as f64)
                        .round()
                        .clamp(1.0, 16.0) as i32;
                    let symbol = (interval, spacing);
                    bits.push(model.information(&history, symbol));
                    model.learn(&history, symbol);
                    history.push(symbol);
                }
                previous = Some((at, pitch));
            }
            (!bits.is_empty()).then(|| bits.iter().sum::<f64>() / bits.len() as f64)
        })
        .collect()
}

/// Lines with the same onsets and pitches share a letter; the same onsets under other pitches
/// add a prime.
fn letters(lines: &[Line]) -> Vec<String> {
    let mut rhythms: HashMap<Vec<i64>, char> = HashMap::new();
    let mut variants: HashMap<(Vec<i64>, Vec<i32>), String> = HashMap::new();
    let mut primes: HashMap<char, usize> = HashMap::new();
    let mut next = 'A';
    lines
        .iter()
        .map(|line| {
            let first = line.notes[0].0;
            let rhythm: Vec<i64> = line.notes.iter().map(|n| (n.0 - first).raw()).collect();
            let pitches: Vec<i32> = line.notes.iter().map(|n| n.2).collect();
            let key = (rhythm.clone(), pitches);
            if let Some(letter) = variants.get(&key) {
                return letter.clone();
            }
            let letter = match rhythms.get(&rhythm) {
                Some(base) => {
                    let count = primes.entry(*base).or_insert(0);
                    *count += 1;
                    format!("{base}{}", "'".repeat(*count))
                }
                None => {
                    let base = next;
                    next = char::from_u32(next as u32 + 1).unwrap_or('?');
                    rhythms.insert(rhythm, base);
                    base.to_string()
                }
            };
            variants.insert(key, letter.clone());
            letter
        })
        .collect()
}

/// The line-by-line shape of a track's melody, with warnings only at the extremes.
pub fn melody_form(project: &Project, track: TrackId) -> (Value, Vec<String>) {
    let (report, warnings, _) = form(project, track);
    (report, warnings)
}

/// The warnings of [`melody_form`] a score phase is not finished with. The rhythm ones are
/// every line entering at one point of the bar, no syncopation, one note count for every line,
/// and one spacing between most notes: a melody sung to one rhythm throughout sounds like a
/// drill whatever its pitches do. The last is a section other than a chorus sung again to notes
/// the song already had, such as a second verse copied from the first: new words want lines of
/// their own.
pub fn development_warnings(project: &Project, track: TrackId) -> Vec<String> {
    form(project, track).2
}

/// [`melody_form`]'s report and warnings, with the development warnings among them also listed
/// apart.
fn form(project: &Project, track: TrackId) -> (Value, Vec<String>, Vec<String>) {
    let lines = lines_of(project, track);
    let mut warnings = Vec::new();
    let mut development = Vec::new();
    if lines.is_empty() {
        return (json!(null), warnings, development);
    }
    let letters = letters(&lines);
    let bits = predictability(&lines);
    let mut rows = Vec::new();
    let mut sections: Vec<(String, Vec<String>)> = Vec::new();
    let mut entries = BTreeSet::new();
    let mut syncopated = 0;
    let mut spacings: BTreeMap<i64, usize> = BTreeMap::new();
    let mut tonic_endings = 0;
    let mut endings = 0;
    let mut note_counts = BTreeSet::new();
    for (index, line) in lines.iter().enumerate() {
        let signature = project.signatures.signature_at(line.start);
        let beat = signature.beat_ticks().raw().max(1);
        let bar = project.signatures.bar_floor(line.start);
        let entry = (line.start - bar).raw() as f64 / beat as f64 + 1.0;
        entries.insert((entry * 1000.0).round() as i64);
        let mut line_syncopated = 0;
        for &(at, length, _) in &line.notes {
            let bar = project.signatures.bar_floor(at);
            let offset = (at - bar).raw();
            let next_beat = (offset / beat + 1) * beat;
            if offset % beat != 0 && offset + length.raw() > next_beat {
                line_syncopated += 1;
            }
        }
        syncopated += line_syncopated;
        for pair in line.notes.windows(2) {
            *spacings.entry((pair[1].0 - pair[0].0).raw()).or_insert(0) += 1;
        }
        note_counts.insert(line.notes.len());
        let last = line.notes[line.notes.len() - 1];
        let key = project.harmony.key_at(last.0);
        let degree = (last.2 - key.tonic.semitones()).rem_euclid(12);
        endings += 1;
        if degree == 0 {
            tonic_endings += 1;
        }
        let label = project
            .sections
            .label_at(line.start)
            .unwrap_or_default()
            .to_string();
        match sections.last_mut() {
            Some((name, form)) if *name == label => form.push(letters[index].clone()),
            _ => sections.push((label.clone(), vec![letters[index].clone()])),
        }
        rows.push(json!({
            "bar": project.signatures.bar_of(line.start),
            "section": label,
            "letter": letters[index],
            "entry_beat": (entry * 100.0).round() / 100.0,
            "notes": line.notes.len(),
            "syncopated": line_syncopated,
            "ends_on_semitones_above_tonic": degree,
            "bits_per_note": bits[index].map(|b| (b * 100.0).round() / 100.0),
        }));
    }
    for (name, form) in &sections {
        if form.len() >= IDENTICAL_SECTION_LINES && form.iter().all(|letter| *letter == form[0]) {
            warnings.push(format!(
                "All {} lines of {name} sing the same onsets and pitches; keep the motif's head and give the lines different jobs (state, answer, turn, land)",
                form.len()
            ));
        }
    }
    if lines.len() >= UNIFORM_LINES {
        if entries.len() == 1 {
            development.push(format!(
                "All {} lines enter at the same point of the bar; let some enter with a pickup (弱起) or late",
                lines.len()
            ));
        }
        if syncopated == 0 {
            development.push(format!(
                "No note in {} lines starts off the beat and holds across it; anticipate a strong beat (食い) where the drive helps",
                lines.len()
            ));
        }
        if note_counts.len() == 1 {
            development.push(format!(
                "Every line has {} notes; let line lengths differ where no hook repeats",
                lines[0].notes.len()
            ));
        }
    }
    let total: usize = spacings.values().sum();
    if total >= UNIFORM_SPACING_COUNT
        && let Some((value, count)) = spacings.iter().max_by_key(|(_, count)| **count)
        && *count as f64 / total as f64 >= UNIFORM_SPACING_SHARE
    {
        development.push(format!(
            "{:.0}% of the melody's notes start {:.2} beats after the one before; mix shorter and longer values",
            *count as f64 * 100.0 / total as f64,
            *value as f64 / Ticks::QUARTER.raw() as f64
        ));
    }
    warnings.extend(development.iter().cloned());
    if endings >= 4 && tonic_endings == 0 {
        warnings.push(format!(
            "None of {endings} lines ends on the tonic; land at least the last chorus there unless the song should stay open"
        ));
    }
    for (index, (name, form)) in sections.iter().enumerate() {
        if form.len() < RESTATED_SECTION_LINES || super::melody_shape::is_chorus(name) {
            continue;
        }
        if let Some((earlier, _)) = sections[..index].iter().find(|(_, lines)| lines == form) {
            let warning = format!(
                "{name} sings {earlier}'s {} lines again note for note; keep each line's head and let the new words reshape the tails, by a pickup, a split or a merge",
                form.len()
            );
            warnings.push(warning.clone());
            development.push(warning);
        }
    }
    let form: Vec<Value> = sections
        .iter()
        .map(|(name, form)| json!({"section": name, "lines": form.join(" ")}))
        .collect();
    let distinct: BTreeSet<&String> = letters.iter().collect();
    (
        json!({
            "lines": lines.len(),
            "distinct_lines": distinct.len(),
            "form": form,
            "syncopated_notes": syncopated,
            "tonic_endings": tonic_endings,
            "per_line": rows,
            "note": "Lines are cut at the lines of the song-wide lyric annotation and at rests of a beat. Letters: same onsets and pitches share one; same onsets under other pitches add a prime. bits_per_note is a short-term model that learns only from this song: an exact repeat falls towards zero.",
        }),
        warnings,
        development,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::Note;

    const BAR: i64 = 3840;

    /// A singer singing `lines` four-bar lines in one verse, each from `line(index)`.
    fn song(lines: i64, line: impl Fn(i64) -> Vec<Note>) -> (Project, TrackId) {
        let mut project = Project::new("Song", 48_000.0);
        let track = project.add_singer_track("Lead", "auris.synth.vocal");
        project
            .sections
            .set_point(Ticks::ZERO, Some("Verse".into()));
        project.sections.set_point(Ticks(BAR * 4 * lines), None);
        for index in 0..lines {
            let clip = project
                .add_midi_clip(track, "Line", Ticks(BAR * 4 * index), Ticks(BAR * 4))
                .unwrap();
            project.midi_clip_mut(clip).unwrap().notes = line(index);
        }
        (project, track)
    }

    /// Twelve quarter notes from the downbeat, ending on D in C major: the pasted template.
    fn template(_: i64) -> Vec<Note> {
        [60, 62, 64, 65, 64, 62, 60, 62, 64, 65, 64, 62]
            .iter()
            .enumerate()
            .map(|(i, &p)| Note::new(p, Ticks(960 * i as i64), Ticks(768)))
            .collect()
    }

    #[test]
    fn one_line_pasted_over_a_verse_raises_every_extreme() {
        let (project, track) = song(8, template);
        let (report, warnings) = melody_form(&project, track);
        assert_eq!(report["distinct_lines"], 1);
        assert_eq!(report["form"][0]["lines"], "A A A A A A A A");
        let text = warnings.join("\n");
        for needle in [
            "same onsets and pitches",
            "same point of the bar",
            "off the beat",
            "Every line has 12 notes",
            "100% of the melody's notes start 1.00 beats",
            "ends on the tonic",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in {text}");
        }
        let bits: Vec<f64> = report["per_line"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| line["bits_per_note"].as_f64().unwrap())
            .collect();
        assert!(bits[0] > 4.0 && bits[7] < 0.5, "{bits:?}");
    }

    #[test]
    fn varied_entries_rhythms_and_a_tonic_close_raise_nothing() {
        let (project, track) = song(8, |index| {
            // Each line enters somewhere else, holds a syncopated note and ends on C or E.
            let entry = [0, 480, 1440, 960][index as usize % 4];
            let mut notes = vec![
                Note::new(67, Ticks(entry), Ticks(480)),
                Note::new(69, Ticks(entry + 480), Ticks(960)),
            ];
            for i in 0..(4 + index % 3) {
                let at = entry + 1920 + i * (480 + 240 * (i % 2));
                notes.push(Note::new(64 + (i as u8 % 3), Ticks(at), Ticks(400)));
            }
            let close = if index % 2 == 0 { 60 } else { 64 };
            notes.push(Note::new(
                close,
                Ticks(entry + BAR * 2 + 960 * (index % 2)),
                Ticks(1920),
            ));
            notes
        });
        let (_, warnings) = melody_form(&project, track);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn the_development_warnings_are_the_rhythm_part_of_the_form_warnings() {
        let (project, track) = song(8, template);
        let rhythm = development_warnings(&project, track);
        let (_, warnings) = melody_form(&project, track);
        assert_eq!(rhythm.len(), 4, "{rhythm:?}");
        assert!(rhythm.iter().all(|w| warnings.contains(w)));
        // Pitch and form warnings stay out: the tonic ending, the repeated line.
        assert!(
            rhythm
                .iter()
                .all(|w| !w.contains("tonic") && !w.contains("pitches"))
        );
    }

    #[test]
    fn an_annotation_laid_over_a_whole_verse_does_not_hide_its_lines() {
        let (mut project, track) = song(8, template);
        // One annotation for all eight lines, as a composer annotating clip by clip writes it.
        project
            .lyrics
            .write_line(Ticks::ZERO, Ticks(BAR * 32), "eight lines of words");
        let (form, warnings) = melody_form(&project, track);
        assert_eq!(form["lines"], 8);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("All 8 lines enter at the same point")),
            "{warnings:?}"
        );
    }

    #[test]
    fn annotated_lines_are_cut_apart_where_no_rest_separates_them() {
        // Sixteen quarter notes sung through four bars without a rest.
        let (mut project, track) = song(1, |_| {
            (0..16)
                .map(|i| Note::new(60 + (i % 5) as u8, Ticks(960 * i), Ticks(960)))
                .collect()
        });
        assert_eq!(melody_form(&project, track).0["lines"], 1);
        project
            .lyrics
            .write_line(Ticks::ZERO, Ticks(BAR * 2), "first line");
        project
            .lyrics
            .write_line(Ticks(BAR * 2), Ticks(BAR * 4), "second line");
        assert_eq!(melody_form(&project, track).0["lines"], 2);
    }

    #[test]
    fn a_second_verse_copied_from_the_first_is_reported_and_a_repeated_chorus_is_not() {
        // Verse, Chorus, Verse 2, Chorus 2: four lines each, the second pair a copy of the first.
        let (mut project, track) = song(16, |index| {
            let pitches: &[u8] = if (index / 4) % 2 == 0 {
                &[60, 62, 64, 67, 64, 62]
            } else {
                &[67, 69, 72, 71, 69, 67]
            };
            let shift = (index % 4) as u8;
            pitches
                .iter()
                .enumerate()
                .map(|(i, &p)| {
                    Note::new(
                        p + shift,
                        Ticks(480 * i as i64 + 240 * (index % 2)),
                        Ticks(400),
                    )
                })
                .collect()
        });
        for (bar, name) in [(4, "Chorus"), (8, "Verse 2"), (12, "Chorus 2")] {
            project
                .sections
                .set_point(Ticks(BAR * 4 * bar), Some(name.into()));
        }
        let warnings = development_warnings(&project, track);
        let restated: Vec<&String> = warnings
            .iter()
            .filter(|w| w.contains("note for note"))
            .collect();
        assert_eq!(restated.len(), 1, "{warnings:?}");
        assert!(restated[0].contains("Verse 2"), "{restated:?}");
    }
}
