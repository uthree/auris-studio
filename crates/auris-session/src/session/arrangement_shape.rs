//! Whether an arrangement changes with the song: which parts play where, whether the drums mark
//! the way into each chorus, and how long the song takes to start and to end.
//!
//! In blinded listening the backing a composer wrote was the weaker half of its songs, and the
//! scores showed why. Every part played in nearly every section, so the chorus arrived no fuller
//! than the verse; drum tracks of a hundred bars held six to eight distinct bars, so no fill
//! announced a chorus; and intros and outros lasted four bars, half what a listener wanted. These
//! are facts a composer can read off its own notes before calling the arrangement done.

use std::collections::{BTreeMap, BTreeSet};

use auris_core::{Project, Ticks, loop_passes};
use serde_json::{Value, json};

use super::melody_shape::is_chorus;

/// Bars of intro or outro below which the song is reported as starting or ending abruptly.
///
/// Eight bars is two four-bar phrases: room to state the groove and the hook before the voice,
/// and to resolve after it. Composers wrote four, and a listener asked for twice that.
pub const MIN_FRAME_BARS: f64 = 8.0;

/// How much busier a chorus's backing must be than the section before it, in notes a bar, to
/// count as fuller with the same parts.
const FULLER: f64 = 1.15;

/// Sections with drums that must sound before one strength throughout is reported.
pub const DYNAMIC_SECTIONS: usize = 3;
/// The spread of the drums' mean velocity across sections under which they are one strength:
/// about what a performance stage's random jitter alone moves a section's mean.
pub const FLAT_DYNAMICS: f64 = 0.04;

/// One part's notes, each as its position within its bar, pitch and length, grouped by bar.
type Bars = BTreeMap<i64, Vec<(i64, u8, i64)>>;

/// The arrangement's shape: parts per section, each part's variety, fills into choruses, and
/// the intro and outro around the voice.
pub fn arrangement_shape(project: &Project) -> Value {
    let bar_of = |tick: Ticks| project.signatures.bar_floor(tick);
    let mut parts: Vec<(String, bool, Bars)> = Vec::new();
    let mut sung: Option<(Ticks, Ticks)> = None;
    let mut end = Ticks::ZERO;
    for track in &project.tracks {
        let Some(clips) = track.kind.note_clips() else {
            continue;
        };
        let singer = track.kind.as_singer().is_some();
        let mut bars = Bars::new();
        for clip in clips.iter().filter(|clip| !clip.muted) {
            for (offset, span) in loop_passes(clip.length, clip.loop_end) {
                for note in clip.playable_notes().filter(|note| note.start < span) {
                    let at = clip.start + offset + note.start;
                    let stop = at + note.length.min(span - note.start);
                    end = end.max(stop);
                    if singer {
                        sung =
                            Some(sung.map_or((at, stop), |(first, last)| {
                                (first.min(at), last.max(stop))
                            }));
                        continue;
                    }
                    let bar = bar_of(at);
                    bars.entry(bar.raw()).or_default().push((
                        (at - bar).raw(),
                        note.pitch,
                        note.length.raw(),
                    ));
                }
            }
        }
        if !singer && !bars.is_empty() {
            for notes in bars.values_mut() {
                notes.sort_unstable();
            }
            parts.push((track.name.clone(), track.kind.is_drum(), bars));
        }
    }
    let spans = project.sections.spans_in(Ticks::ZERO, end.max(Ticks(1)));
    let mut warnings = Vec::new();

    // Which parts play in each section.
    let mut sections = Vec::new();
    let mut sets: Vec<BTreeSet<&str>> = Vec::new();
    let mut densities: Vec<f64> = Vec::new();
    for span in &spans {
        let playing: BTreeSet<&str> = parts
            .iter()
            .filter(|(_, _, bars)| {
                bars.range(span.start.raw()..span.end.raw())
                    .next()
                    .is_some()
            })
            .map(|(name, _, _)| name.as_str())
            .collect();
        // Backing notes per bar: the same parts playing more is a fuller chorus too.
        let notes: usize = parts
            .iter()
            .flat_map(|(_, _, bars)| bars.range(span.start.raw()..span.end.raw()))
            .map(|(_, notes)| notes.len())
            .sum();
        let bars = (span.end - span.start).raw() as f64
            / project
                .signatures
                .signature_at(span.start)
                .ticks_per_bar()
                .raw()
                .max(1) as f64;
        let density = notes as f64 / bars.max(1.0);
        sections.push(json!({
            "section": span.label, "parts": playing.iter().collect::<Vec<_>>(),
            "part_count": playing.len(), "notes_per_bar": (density * 10.0).round() / 10.0,
        }));
        sets.push(playing);
        densities.push(density);
    }
    let sounding: Vec<&BTreeSet<&str>> = sets.iter().filter(|set| !set.is_empty()).collect();
    if sounding.len() >= 3 && sounding.windows(2).all(|pair| pair[0] == pair[1]) {
        warnings.push(
            "Every section has the same parts playing; thin the verses or a bridge so the choruses arrive fuller".to_owned(),
        );
    } else {
        for (index, span) in spans.iter().enumerate() {
            if !is_chorus(&span.label) || index == 0 || is_chorus(&spans[index - 1].label) {
                continue;
            }
            let fuller = sets[index].len() > sets[index - 1].len()
                || densities[index] >= densities[index - 1] * FULLER;
            if !fuller && !sets[index].is_empty() {
                warnings.push(format!(
                    "{} is no fuller than {} before it ({} parts, {:.1} notes a bar, against {}, {:.1}); add a part or play more",
                    span.label,
                    spans[index - 1].label,
                    sets[index].len(),
                    densities[index],
                    sets[index - 1].len(),
                    densities[index - 1]
                ));
            }
        }
    }

    // How varied each part is, and whether the drums change in the bar before each chorus.
    let mut report_parts = Vec::new();
    for (name, drum, bars) in &parts {
        let distinct: BTreeSet<&Vec<(i64, u8, i64)>> = bars.values().collect();
        report_parts.push(json!({
            "part": name, "bars": bars.len(), "distinct_bars": distinct.len(), "drums": drum,
        }));
        if !drum {
            continue;
        }
        for (index, span) in spans.iter().enumerate() {
            if !is_chorus(&span.label) || index == 0 || is_chorus(&spans[index - 1].label) {
                continue;
            }
            let last = bar_of(span.start - Ticks(1));
            let before = bar_of(last - Ticks(1));
            let (Some(last_bar), Some(before_bar)) =
                (bars.get(&last.raw()), bars.get(&before.raw()))
            else {
                continue;
            };
            if last_bar == before_bar {
                warnings.push(format!(
                    "{name} plays the same bar into {} as the bar before it; mark the way into the chorus with a fill",
                    span.label
                ));
            }
        }
    }

    // Sections that share one main drum bar, and choruses that loop the verse's progression.
    warnings.extend(shared_grooves(project, &spans, &parts));
    warnings.extend(borrowed_progressions(project, &spans));
    warnings.extend(flat_drums(project, &spans));

    // The intro and outro around the voice. The song ends with the bar its last sound is in: a
    // final hit on the downbeat of the last bar still owns that bar.
    let bar_length = |at: Ticks| project.signatures.signature_at(at).ticks_per_bar().raw() as f64;
    let last_bar = bar_of(end - Ticks(1));
    let end = last_bar + project.signatures.signature_at(last_bar).ticks_per_bar();
    let frame = sung.map(|(first, last)| {
        let intro = first.raw() as f64 / bar_length(Ticks::ZERO);
        let outro = (end - last).raw().max(0) as f64 / bar_length(last);
        (intro, outro)
    });
    if let Some((intro, outro)) = frame {
        if intro < MIN_FRAME_BARS {
            warnings.push(format!(
                "The voice enters after {intro:.1} bars; give the song an intro of at least {MIN_FRAME_BARS} bars"
            ));
        }
        if outro < MIN_FRAME_BARS {
            warnings.push(format!(
                "The song ends {outro:.1} bars after the voice; give it an outro of at least {MIN_FRAME_BARS} bars"
            ));
        }
    }
    json!({
        "sections": sections, "parts": report_parts,
        "intro_bars": frame.map(|f| (f.0 * 10.0).round() / 10.0),
        "outro_bars": frame.map(|f| (f.1 * 10.0).round() / 10.0),
        "warnings": warnings,
        "note": "Measured on written notes. distinct_bars counts different one-bar patterns; a groove may repeat, but a part that never changes does not follow the song.",
    })
}

/// Sections at or above which one shared main drum bar is reported.
pub const SHARED_GROOVE_SECTIONS: usize = 3;

/// Groups of three or more sections whose most-played drum bar is the same bar.
///
/// A groove may repeat; an intro, verse, interlude and outro that all ride on one bar, with
/// only a cymbal changed elsewhere, is a drum part that does not follow the form.
fn shared_grooves(
    project: &Project,
    spans: &[auris_core::SectionSpan],
    parts: &[(String, bool, Bars)],
) -> Vec<String> {
    let bar_of = |tick: Ticks| project.signatures.bar_floor(tick).raw();
    let mut owners: BTreeMap<Vec<(i64, u8)>, Vec<String>> = BTreeMap::new();
    for span in spans {
        let mut counts: BTreeMap<Vec<(i64, u8)>, usize> = BTreeMap::new();
        let mut bar = bar_of(span.start);
        while bar < span.end.raw() {
            let mut hits: Vec<(i64, u8)> = parts
                .iter()
                .filter(|(_, drum, _)| *drum)
                .filter_map(|(_, _, bars)| bars.get(&bar))
                .flatten()
                .map(|(at, pitch, _)| (*at, *pitch))
                .collect();
            hits.sort_unstable();
            hits.dedup();
            if !hits.is_empty() {
                *counts.entry(hits).or_insert(0) += 1;
            }
            bar += project
                .signatures
                .signature_at(Ticks(bar))
                .ticks_per_bar()
                .raw()
                .max(1);
        }
        if let Some((main, _)) = counts.into_iter().max_by_key(|(_, count)| *count) {
            owners.entry(main).or_default().push(span.label.clone());
        }
    }
    owners
        .into_values()
        .filter(|names| {
            // Choruses sharing the chorus groove is a form, not a loop: count kinds of section.
            let kinds: BTreeSet<String> = names.iter().map(|name| section_kind(name)).collect();
            kinds.len() >= SHARED_GROOVE_SECTIONS
        })
        .map(|names| {
            format!(
                "{} all play one main drum bar; give each section its own groove (kick, hats, half time) rather than one bar with the cymbals changed",
                names.join(", ")
            )
        })
        .collect()
}

/// A section label without its number or a final/last marker: サビ2 and 最終サビ are both サビ.
/// A warning when the drums play every section at one written strength: a verse and the last
/// chorus struck alike is heard as a machine, however the notes differ.
fn flat_drums(project: &Project, spans: &[auris_core::SectionSpan]) -> Option<String> {
    let mut sums = vec![(0.0_f64, 0_usize); spans.len()];
    for track in project.tracks.iter().filter(|track| track.kind.is_drum()) {
        for clip in track.kind.note_clips().into_iter().flatten() {
            if clip.muted {
                continue;
            }
            for (offset, span) in loop_passes(clip.length, clip.loop_end) {
                for note in clip.playable_notes().filter(|note| note.start < span) {
                    let at = clip.start + offset + note.start;
                    if let Some(index) = spans.iter().position(|s| s.start <= at && at < s.end) {
                        sums[index].0 += f64::from(note.velocity);
                        sums[index].1 += 1;
                    }
                }
            }
        }
    }
    let means: Vec<f64> = sums
        .iter()
        .filter(|(_, count)| *count > 0)
        .map(|(sum, count)| sum / *count as f64)
        .collect();
    let (low, high) = means
        .iter()
        .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    (means.len() >= DYNAMIC_SECTIONS && high - low < FLAT_DYNAMICS).then(|| {
        format!(
            "The drums play all {} sections at one strength (mean velocity {low:.2} to {high:.2}); strike the verses and a bridge softer and the choruses harder",
            means.len()
        )
    })
}

fn section_kind(label: &str) -> String {
    let mut kind = label.to_lowercase();
    for word in ["final", "last", "最終", "ラスト", "ラス"] {
        kind = kind.replace(word, "");
    }
    kind.chars()
        .filter(|c| {
            !c.is_ascii_digit() && !c.is_whitespace() && !"０１２３４５６７８９".contains(*c)
        })
        .collect()
}

/// The shortest repeating unit of a per-bar progression.
fn loop_of(bars: &[String]) -> &[String] {
    (1..=bars.len())
        .find(|size| (0..bars.len()).all(|i| bars[i] == bars[i % size]))
        .map_or(bars, |size| &bars[..size])
}

/// Choruses whose chord loop is a verse's loop, possibly started on another chord.
fn borrowed_progressions(project: &Project, spans: &[auris_core::SectionSpan]) -> Vec<String> {
    let progression = |span: &auris_core::SectionSpan| -> Vec<String> {
        let mut bars = Vec::new();
        let mut at = project.signatures.bar_floor(span.start);
        while at < span.end {
            bars.push(
                project
                    .harmony
                    .numeral_at(at)
                    .map_or_else(|| "-".to_owned(), |numeral| numeral.to_text()),
            );
            at += project.signatures.signature_at(at).ticks_per_bar();
        }
        bars
    };
    let verse = |label: &str| {
        let label = label.to_lowercase();
        label.contains("verse") || label.contains("aメロ")
    };
    let verses: Vec<(String, Vec<String>)> = spans
        .iter()
        .filter(|span| verse(&span.label))
        .map(|span| (span.label.clone(), loop_of(&progression(span)).to_vec()))
        .filter(|(_, cycle)| cycle.iter().any(|chord| chord != "-"))
        .collect();
    let mut warnings = Vec::new();
    for span in spans.iter().filter(|span| is_chorus(&span.label)) {
        let bars = progression(span);
        let cycle = loop_of(&bars);
        let doubled: Vec<&String> = cycle.iter().chain(cycle.iter()).collect();
        if let Some((name, _)) = verses.iter().find(|(_, verse)| {
            verse.len() == cycle.len()
                && (0..cycle.len()).any(|shift| {
                    doubled[shift..shift + cycle.len()]
                        .iter()
                        .zip(verse)
                        .all(|(a, b)| *a == b)
                })
        }) {
            warnings.push(format!(
                "{} loops {name}'s progression ({}) started elsewhere; give the chorus its own progression or harmonic rhythm",
                span.label,
                cycle.join(" ")
            ));
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::Note;

    /// A song of Aメロ, Bメロ and サビ, four bars each, with a voice from `sung_from` bars to
    /// `sung_to` bars and a drum part playing `drum` in every bar.
    fn song(sung: (i64, i64), length_bars: i64, drum: &dyn Fn(i64) -> Vec<Note>) -> Project {
        let mut project = Project::new("Song", 48_000.0);
        let bar = Ticks::QUARTER * 4;
        project
            .sections
            .set_point(bar * sung.0, Some("Aメロ".into()));
        project
            .sections
            .set_point(bar * (sung.0 + 4), Some("Bメロ".into()));
        project
            .sections
            .set_point(bar * (sung.0 + 8), Some("サビ".into()));
        project.sections.set_point(bar * (sung.0 + 12), None);
        let voice = project.add_singer_track("Lead", "auris.synth.vocal");
        let clip = project
            .add_midi_clip(voice, "Lead", bar * sung.0, bar * (sung.1 - sung.0))
            .unwrap();
        project.midi_clip_mut(clip).unwrap().notes = (0..(sung.1 - sung.0) * 4)
            .map(|beat| Note::new(64, Ticks::QUARTER * beat, Ticks::QUARTER))
            .collect();
        let drums = project.add_drum_track("Drums", "auris.sampler.soundfont");
        let clip = project
            .add_midi_clip(drums, "Drums", Ticks::ZERO, bar * length_bars)
            .unwrap();
        project.midi_clip_mut(clip).unwrap().notes = (0..length_bars)
            .flat_map(|index| {
                drum(index).into_iter().map(move |mut note| {
                    note.start += bar * index;
                    note
                })
            })
            .collect();
        project
    }

    fn groove(_: i64) -> Vec<Note> {
        vec![Note::new(36, Ticks::ZERO, Ticks::QUARTER)]
    }

    #[test]
    fn a_short_frame_a_static_texture_and_no_fill_are_each_reported() {
        let project = song((4, 16), 20, &groove);
        let report = arrangement_shape(&project);
        let warnings: Vec<&str> = report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_str().unwrap())
            .collect();
        assert!(
            warnings.iter().any(|w| w.contains("Every section")),
            "{warnings:?}"
        );
        assert!(warnings.iter().any(|w| w.contains("fill")), "{warnings:?}");
        assert!(
            warnings.iter().any(|w| w.contains("intro of at least")),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("outro of at least")),
            "{warnings:?}"
        );
        assert_eq!(report["intro_bars"], 4.0);
        assert_eq!(report["parts"][0]["distinct_bars"], 1);
    }

    #[test]
    fn a_fill_into_the_chorus_and_a_long_frame_pass() {
        // Voice from bar 8 to bar 20 of 28; the bar before the chorus (bar 15) is a fill.
        let fill = |index: i64| {
            if index == 15 {
                (0..4)
                    .map(|beat| Note::new(47, Ticks::QUARTER * beat, Ticks::QUARTER))
                    .collect()
            } else {
                groove(index)
            }
        };
        let project = song((8, 20), 28, &fill);
        let report = arrangement_shape(&project);
        let warnings = report["warnings"].as_array().unwrap();
        assert!(
            !warnings
                .iter()
                .any(|w| w.as_str().unwrap().contains("fill")
                    || w.as_str().unwrap().contains("tro of")),
            "{warnings:?}"
        );
        assert_eq!(report["outro_bars"], 8.0);
    }

    fn warnings(project: &Project) -> Vec<String> {
        arrangement_shape(project)["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn three_sections_on_one_drum_bar_are_reported_and_a_varied_kit_is_not() {
        let same = song((8, 20), 28, &groove);
        assert!(
            warnings(&same)
                .iter()
                .any(|w| w.contains("Aメロ, Bメロ, サビ all play one main drum bar")),
            "{:?}",
            warnings(&same)
        );
        let varied = song((8, 20), 28, &|bar| {
            vec![Note::new(
                36 + (bar / 4) as u8 % 3,
                Ticks::ZERO,
                Ticks::QUARTER,
            )]
        });
        assert!(
            !warnings(&varied)
                .iter()
                .any(|w| w.contains("one main drum bar"))
        );
    }

    #[test]
    fn a_chorus_on_the_verses_loop_from_another_chord_is_reported() {
        use auris_core::theory::{key::Key, numeral::Numeral, pitch::PitchClass, scale::ScaleId};
        let mut project = song((8, 20), 28, &groove);
        project.harmony =
            auris_core::harmony::Harmony::in_key(Key::new(PitchClass::new(4), ScaleId::Minor));
        let bar = Ticks::QUARTER * 4;
        let verse = ["i", "i", "VI", "VII"];
        let chorus = ["VI", "VII", "i", "i"];
        for (index, numeral) in verse.iter().chain(&verse).chain(&chorus).enumerate() {
            project.harmony.chords.set_point(
                bar * (8 + index as i64),
                Some(Numeral::parse(numeral).unwrap()),
            );
        }
        let found = warnings(&project);
        assert!(
            found
                .iter()
                .any(|w| w.contains("サビ loops Aメロ's progression")),
            "{found:?}"
        );
        project
            .harmony
            .chords
            .set_point(bar * 16, Some(Numeral::parse("iv").unwrap()));
        assert!(!warnings(&project).iter().any(|w| w.contains("loops")));
    }

    #[test]
    fn drums_struck_alike_in_every_section_are_reported_and_a_softer_verse_is_not() {
        let flat = song((4, 16), 20, &groove);
        assert!(
            warnings(&flat).iter().any(|w| w.contains("one strength")),
            "{:?}",
            warnings(&flat)
        );
        // The Aメロ (bars 4 to 8) at 0.5, everything else at 0.8.
        let shaded = song((4, 16), 20, &|index| {
            let mut notes = groove(index);
            notes[0].velocity = if (4..8).contains(&index) { 0.5 } else { 0.8 };
            notes
        });
        assert!(
            !warnings(&shaded).iter().any(|w| w.contains("one strength")),
            "{:?}",
            warnings(&shaded)
        );
    }
}
