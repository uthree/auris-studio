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
/// The spread of the drums' mean velocity across sections under which they are one strength,
/// and how much harder than the verses the choruses must be struck: about what a performance
/// stage's random jitter alone moves a section's mean.
pub const FLAT_DYNAMICS: f64 = 0.04;
/// What both drum strength warnings end on: where the strength is read from.
const REWRITE_VELOCITY: &str = "performance settings do not change the velocity read here";

/// Bars in a phrase whose last bar a drummer marks.
pub const PHRASE_BARS: i64 = 8;
/// Phrase ends inside sections the song must hold before none of them being marked is reported.
pub const MARKED_PHRASES: usize = 4;

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
            "cadence": cadence(project, span),
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
    let fills = phrase_fills(project, &spans, &parts);
    if let Some(warning) = fills.1 {
        warnings.push(warning);
    }

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
        "fills": fills.0,
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
/// How the drums mark the eight-bar phrases inside sections, apart from the bar into a chorus
/// that the fill check already reads: how many phrase ends there are, how many play something
/// other than the section's main bar, and the notes in an average marked phrase end against the
/// average bar into a chorus. A fill hierarchy grows from small inside a section to largest into
/// a chorus. The warning comes only when no phrase end is marked at all.
fn phrase_fills(
    project: &Project,
    spans: &[auris_core::SectionSpan],
    parts: &[(String, bool, Bars)],
) -> (Value, Option<String>) {
    let drums: Vec<&Bars> = parts
        .iter()
        .filter(|(_, drum, _)| *drum)
        .map(|(_, _, bars)| bars)
        .collect();
    if drums.is_empty() {
        return (Value::Null, None);
    }
    let bar_hits = |bar: i64| -> Vec<(i64, u8)> {
        let mut hits: Vec<(i64, u8)> = drums
            .iter()
            .filter_map(|bars| bars.get(&bar))
            .flatten()
            .map(|(at, pitch, _)| (*at, *pitch))
            .collect();
        hits.sort_unstable();
        hits.dedup();
        hits
    };
    let (mut ends, mut marked, mut marked_notes) = (0_usize, 0_usize, 0_usize);
    let mut chorus_notes = Vec::new();
    for (index, span) in spans.iter().enumerate() {
        let mut starts = Vec::new();
        let mut bar = span.start;
        while bar < span.end {
            starts.push(bar.raw());
            bar += project.signatures.signature_at(bar).ticks_per_bar();
        }
        if index + 1 < spans.len()
            && is_chorus(&spans[index + 1].label)
            && let Some(&last) = starts.last()
        {
            chorus_notes.push(bar_hits(last).len());
        }
        let mut counts: BTreeMap<Vec<(i64, u8)>, usize> = BTreeMap::new();
        for &start in &starts {
            let hits = bar_hits(start);
            if !hits.is_empty() {
                *counts.entry(hits).or_insert(0) += 1;
            }
        }
        let Some((main, _)) = counts.into_iter().max_by_key(|(_, count)| *count) else {
            continue;
        };
        // Every eighth bar, but not the section's last: that one leads into the next section.
        for position in
            (PHRASE_BARS as usize - 1..starts.len().saturating_sub(1)).step_by(PHRASE_BARS as usize)
        {
            let hits = bar_hits(starts[position]);
            if hits.is_empty() {
                continue;
            }
            ends += 1;
            if hits != main {
                marked += 1;
                marked_notes += hits.len();
            }
        }
    }
    let average = |total: usize, count: usize| {
        (count > 0).then(|| (total as f64 / count as f64 * 10.0).round() / 10.0)
    };
    let report = json!({
        "phrase_ends": ends,
        "marked": marked,
        "notes_in_a_marked_end": average(marked_notes, marked),
        "notes_into_a_chorus": average(chorus_notes.iter().sum(), chorus_notes.len()),
    });
    let warning = (ends >= MARKED_PHRASES && marked == 0).then(|| {
        format!(
            "The drums play their main bar through all {ends} eight-bar phrase ends inside sections; mark them with a small fill, smaller than the one into a chorus"
        )
    });
    (report, warning)
}

/// How a section's harmony ends, from its last two chords: `authentic` (V to I), `plagal`
/// (IV to I), `subtonic` (VII to I), `tonic` (another way onto I), `deceptive` (V to vi),
/// `half` (onto V) or `open` (anywhere else), with the two chords; `null` when the section has
/// no chord. Degrees are read in the key, so a minor song's v–i is authentic too.
fn cadence(project: &Project, span: &auris_core::SectionSpan) -> Value {
    let chords = &project.harmony.chords;
    let Some(last) = chords.numeral_at(span.end - Ticks(1)) else {
        return Value::Null;
    };
    let before = chords
        .points()
        .iter()
        .rev()
        .filter(|point| point.tick > span.start && point.tick < span.end)
        .skip_while(|point| point.chord == Some(last))
        .find_map(|point| point.chord.filter(|chord| *chord != last))
        .or_else(|| chords.numeral_at(span.start).filter(|chord| *chord != last));
    let plain = |numeral: &auris_core::theory::numeral::Numeral| {
        (numeral.accidental == 0 && numeral.secondary_of.is_none()).then_some(numeral.degree)
    };
    let kind = match (before.as_ref().and_then(plain), plain(&last)) {
        (Some(5), Some(1)) => "authentic",
        (Some(4), Some(1)) => "plagal",
        (Some(7), Some(1)) => "subtonic",
        (_, Some(1)) => "tonic",
        (Some(5), Some(6)) => "deceptive",
        (_, Some(5)) => "half",
        _ => "open",
    };
    json!({"kind": kind, "chords": [before.map(|c| c.to_string()), last.to_string()]})
}

/// A warning when the drums do not shade the form by strength: a verse and the last chorus struck
/// alike is heard as a machine, however the notes differ. Where the song has both, every chorus's
/// mean velocity must stand [`FLAT_DYNAMICS`] above every verse's, since a softer intro or bridge
/// alone leaves the verse and chorus the same. The weakest chorus is held against the hardest
/// verse because an average hides one of them: one hard chorus leaves the others at the verse's
/// strength, and one soft verse leaves another at the chorus's. A song without both is read by the
/// spread across all its sections. The arrangement phase's `drums_follow_sections` check reads it
/// too.
///
/// The warning says where the strength is read from, the notes' written velocity, because
/// composers answered it with a performance stage's accent, which shades beats against offbeats
/// and leaves a section's mean where it was.
pub(crate) fn flat_drums(project: &Project, spans: &[auris_core::SectionSpan]) -> Option<String> {
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
    let by_section: Vec<(&str, f64)> = spans
        .iter()
        .zip(&sums)
        .filter(|(_, (_, count))| *count > 0)
        .map(|(span, (sum, count))| (span.label.as_str(), sum / *count as f64))
        .collect();
    let hardest_verse = by_section
        .iter()
        .filter(|(label, _)| is_verse(label))
        .max_by(|a, b| a.1.total_cmp(&b.1));
    let weakest_chorus = by_section
        .iter()
        .filter(|(label, _)| is_chorus(label))
        .min_by(|a, b| a.1.total_cmp(&b.1));
    if by_section.len() >= DYNAMIC_SECTIONS
        && let Some((verse_label, verse)) = hardest_verse
        && let Some((chorus_label, chorus)) = weakest_chorus
    {
        return (chorus - verse < FLAT_DYNAMICS).then(|| {
            format!(
                "The drums strike {chorus_label} no harder than {verse_label} (mean velocity {chorus:.2} against {verse:.2}); rewrite the drum notes' velocity so that every chorus is struck harder than every verse, the last chorus hardest, keeping the accents within the bar; {REWRITE_VELOCITY}"
            )
        });
    }
    let means: Vec<f64> = by_section.iter().map(|(_, mean)| *mean).collect();
    let (low, high) = means
        .iter()
        .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    (means.len() >= DYNAMIC_SECTIONS && high - low < FLAT_DYNAMICS).then(|| {
        format!(
            "The drums play all {} sections at one strength (mean velocity {low:.2} to {high:.2}); rewrite the drum notes' velocity, the verses softer and the choruses harder, the last chorus hardest; {REWRITE_VELOCITY}",
            means.len()
        )
    })
}

/// Whether a section label names a verse (Aメロ).
fn is_verse(label: &str) -> bool {
    let label = label.to_lowercase();
    label.contains("verse") || label.contains("aメロ")
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
    let verses: Vec<(String, Vec<String>)> = spans
        .iter()
        .filter(|span| is_verse(&span.label))
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
        let unshaded = |project: &Project| {
            warnings(project)
                .iter()
                .any(|w| w.contains("the last chorus hardest"))
        };
        let flat = song((4, 16), 20, &groove);
        assert!(unshaded(&flat), "{:?}", warnings(&flat));
        // A softer Bメロ alone spreads the sections but leaves the verse and chorus alike.
        let soft_bridge = song((4, 16), 20, &|index| {
            let mut notes = groove(index);
            notes[0].velocity = if (8..12).contains(&index) { 0.5 } else { 0.8 };
            notes
        });
        assert!(unshaded(&soft_bridge), "{:?}", warnings(&soft_bridge));
        // The Aメロ (bars 4 to 8) at 0.5, everything else at 0.8.
        let shaded = song((4, 16), 20, &|index| {
            let mut notes = groove(index);
            notes[0].velocity = if (4..8).contains(&index) { 0.5 } else { 0.8 };
            notes
        });
        assert!(!unshaded(&shaded), "{:?}", warnings(&shaded));
    }

    #[test]
    fn one_chorus_struck_hard_does_not_cover_a_chorus_struck_like_the_verse() {
        // Aメロ in bars 4 to 8 and サビ in 12 to 16, as `song` lays them, then a last chorus.
        let two_choruses = |last: f32| {
            let mut project = song((4, 16), 24, &|index| {
                let mut notes = groove(index);
                notes[0].velocity = match index {
                    4..8 => 0.5,
                    16..20 => last,
                    _ => 0.8,
                };
                notes
            });
            let bar = Ticks::QUARTER * 4;
            project
                .sections
                .set_point(bar * 16, Some("ラストサビ".into()));
            project.sections.set_point(bar * 20, None);
            project
        };
        // The two choruses average 0.65 against the verse's 0.5, and the last is the verse again.
        let covered = warnings(&two_choruses(0.5));
        assert!(
            covered
                .iter()
                .any(|w| w.contains("The drums strike ラストサビ no harder than Aメロ")),
            "{covered:?}"
        );
        let both = warnings(&two_choruses(0.85));
        assert!(!both.iter().any(|w| w.contains("no harder")), "{both:?}");
    }

    #[test]
    fn one_verse_struck_softly_does_not_cover_a_verse_struck_like_the_chorus() {
        // Aメロ in bars 4 to 8 and サビ in 12 to 16, as `song` lays them, then a second verse.
        let two_verses = |second: f32| {
            let mut project = song((4, 16), 24, &|index| {
                let mut notes = groove(index);
                notes[0].velocity = match index {
                    4..8 => 0.3,
                    16..20 => second,
                    _ => 0.55,
                };
                notes
            });
            let bar = Ticks::QUARTER * 4;
            project.sections.set_point(bar * 16, Some("Aメロ2".into()));
            project.sections.set_point(bar * 20, None);
            project
        };
        // The two verses average 0.43 against the chorus's 0.55, and the second is the chorus.
        let covered = warnings(&two_verses(0.55));
        assert!(
            covered.iter().any(|w| w.contains(
                "The drums strike サビ no harder than Aメロ2 (mean velocity 0.55 against 0.55)"
            ) && w
                .contains("performance settings do not change the velocity read here")),
            "{covered:?}"
        );
        let both = warnings(&two_verses(0.35));
        assert!(!both.iter().any(|w| w.contains("no harder")), "{both:?}");
    }

    #[test]
    fn each_section_names_how_its_harmony_ends() {
        use auris_core::theory::numeral::Numeral;
        let mut project = song((4, 16), 20, &groove);
        let bar = Ticks::QUARTER * 4;
        // Aメロ ends I–V, Bメロ ii–V–vi, サビ IV–V–I.
        for (at, chord) in [
            (4, "I"),
            (7, "V"),
            (8, "ii"),
            (10, "V"),
            (11, "vi"),
            (12, "IV"),
            (14, "V"),
            (15, "I"),
        ] {
            project
                .harmony
                .chords
                .set_point(bar * at, Numeral::parse(chord));
        }
        let report = arrangement_shape(&project);
        let kinds: Vec<&str> = report["sections"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|section| section["cadence"]["kind"].as_str())
            .collect();
        assert_eq!(kinds, ["half", "deceptive", "authentic"], "{report}");
    }

    #[test]
    fn unmarked_phrase_ends_are_reported_and_a_fill_every_eight_bars_is_not() {
        // A 40-bar song of one 32-bar Aメロ from bar 4: its phrase ends are bars 11, 19 and 27.
        let long_verse = |mut project: Project| {
            let bar = Ticks::QUARTER * 4;
            project.sections = Default::default();
            project.sections.set_point(bar * 4, Some("Aメロ".into()));
            project.sections.set_point(bar * 36, None);
            project
        };
        let plain = long_verse(song((4, 36), 40, &groove));
        let report = arrangement_shape(&plain);
        assert_eq!(report["fills"]["phrase_ends"], 3, "{report}");
        assert_eq!(report["fills"]["marked"], 0, "{report}");
        let marked = long_verse(song((4, 36), 40, &|index| {
            if (index - 4) % 8 == 7 {
                (0..4)
                    .map(|beat| Note::new(38, Ticks::QUARTER * beat, Ticks::QUARTER))
                    .collect()
            } else {
                groove(index)
            }
        }));
        let report = arrangement_shape(&marked);
        assert!(report["fills"]["marked"].as_u64().unwrap() >= 3, "{report}");
        assert_eq!(report["fills"]["notes_in_a_marked_end"], 4.0);
    }
}
