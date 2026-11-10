//! How a melody moves across the song's sections, and whether each chorus lifts.
//!
//! A chorus that sits where the verse and pre-chorus already were is heard as more of the same,
//! however good its words are. Composers asked to "open the range in the chorus" wrote choruses a
//! semitone or two above the pre-chorus on average and a whole tone above it at the peak, which
//! nobody hears as a lift. The numbers here are what a composer can check before calling the
//! score done: each section's register and movement, and for every chorus the lift over the
//! section that leads into it.

use super::arrangement_shape::MIN_FRAME_BARS;
use auris_core::theory::pitch::midi_name;
use auris_core::{Project, Ticks, TrackId, loop_passes};
use serde_json::{Value, json};

/// Average lift, in semitones, below which a chorus is reported as not rising.
///
/// A minor third: the smallest move of a whole section's centre that is heard as a new register
/// rather than as the same tune sung a little higher.
pub const MIN_MEAN_LIFT: f64 = 3.0;

/// Peak lift, in semitones, below which a chorus is reported as not reaching higher.
pub const MIN_PEAK_LIFT: i32 = 2;

/// An interval this wide or wider, in semitones, counts as a leap: a perfect fourth.
pub const LEAP: i32 = 5;

/// Whether a section label names a chorus, in the words composers actually use. A pre-chorus
/// or a post-chorus leads into one or away from it and is not one, however it is spelled:
/// `Pre-chorus 1`, `PreChorus2`, `プリサビ`.
pub fn is_chorus(label: &str) -> bool {
    let label: String = label
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace() && !"-_‐・".contains(*c))
        .collect();
    ["サビ", "chorus", "hook", "refrain"].iter().any(|word| {
        label.match_indices(word).any(|(at, _)| {
            !["pre", "post", "プリ", "プレ", "ポスト"]
                .iter()
                .any(|prefix| label[..at].ends_with(prefix))
        })
    })
}

struct Section {
    label: String,
    pitches: Vec<i32>,
}

impl Section {
    fn mean(&self) -> f64 {
        self.pitches.iter().map(|p| f64::from(*p)).sum::<f64>() / self.pitches.len() as f64
    }

    fn low(&self) -> i32 {
        self.pitches.iter().copied().min().unwrap_or_default()
    }

    fn high(&self) -> i32 {
        self.pitches.iter().copied().max().unwrap_or_default()
    }

    fn intervals(&self) -> impl Iterator<Item = i32> + '_ {
        self.pitches
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
    }
}

/// The section-by-section shape of one track's melody, with a warning for each chorus that
/// does not lift above the section leading into it.
pub fn melody_shape(project: &Project, track: TrackId) -> Result<Value, String> {
    let source = project.track(track).ok_or("Unknown track ID")?;
    let clips = source
        .kind
        .note_clips()
        .ok_or("Choose an instrument or singer track holding the melody")?;
    let mut notes: Vec<(Ticks, i32)> = Vec::new();
    let mut last_end = Ticks::ZERO;
    for clip in clips.iter().filter(|clip| !clip.muted) {
        for (offset, span) in loop_passes(clip.length, clip.loop_end) {
            for note in clip.playable_notes().filter(|note| note.start < span) {
                let at = clip.start + offset + note.start;
                notes.push((at, i32::from(note.pitch)));
                last_end = last_end.max(at + note.length.min(span - note.start));
            }
        }
    }
    notes.sort();
    // The top line of anything sounding together: a melody, not its doubled chord tones.
    notes.dedup_by(|later, earlier| {
        if later.0 == earlier.0 {
            earlier.1 = earlier.1.max(later.1);
            true
        } else {
            false
        }
    });
    if notes.is_empty() {
        return Err("The track has no notes".into());
    }
    let end = notes.last().map_or(Ticks::ZERO, |note| note.0) + Ticks(1);
    let mut sections: Vec<Section> = Vec::new();
    for span in project.sections.spans_in(Ticks::ZERO, end) {
        let pitches: Vec<i32> = notes
            .iter()
            .filter(|(at, _)| *at >= span.start && *at < span.end)
            .map(|(_, pitch)| *pitch)
            .collect();
        if !pitches.is_empty() {
            sections.push(Section {
                label: span.label,
                pitches,
            });
        }
    }
    if sections.is_empty() {
        return Err(
            "No named section holds this track's notes; name the sections with edit_harmony first"
                .into(),
        );
    }
    let song_high = sections.iter().map(Section::high).max().unwrap_or_default();
    let mut warnings = Vec::new();
    let mut lifts = Vec::new();
    for (index, chorus) in sections.iter().enumerate() {
        if !is_chorus(&chorus.label) {
            continue;
        }
        let leaps = chorus.intervals().filter(|step| *step >= LEAP).count();
        let widest = chorus.intervals().max().unwrap_or_default();
        let Some(before) = index.checked_sub(1).map(|i| &sections[i]) else {
            continue;
        };
        if is_chorus(&before.label) {
            continue;
        }
        let mean_lift = chorus.mean() - before.mean();
        let peak_lift = chorus.high() - before.high();
        lifts.push(json!({
            "chorus": chorus.label, "after": before.label,
            "mean_lift_semitones": (mean_lift * 10.0).round() / 10.0,
            "peak_lift_semitones": peak_lift,
            "chorus_leaps": leaps, "chorus_widest_interval": widest,
        }));
        if mean_lift < MIN_MEAN_LIFT {
            warnings.push(format!(
                "{} sits only {:.1} semitones above {} on average; lift its register by at least {MIN_MEAN_LIFT} or set {} lower",
                chorus.label, mean_lift, before.label, before.label
            ));
        }
        if peak_lift < MIN_PEAK_LIFT {
            warnings.push(format!(
                "{} peaks at {}, only {peak_lift} semitones above {}'s {}; give the chorus the higher peak",
                chorus.label,
                midi_name(chorus.high()),
                before.label,
                midi_name(before.high())
            ));
        }
        if leaps == 0 {
            warnings.push(format!(
                "{} moves only by step (widest {widest} semitones); a chorus hook usually needs at least one leap of a fourth or more",
                chorus.label
            ));
        }
    }
    let choruses: Vec<&Section> = sections.iter().filter(|s| is_chorus(&s.label)).collect();
    if choruses.is_empty() {
        warnings.push(
            "No section is labelled as a chorus (サビ, chorus, hook), so no lift was checked"
                .into(),
        );
    } else if choruses.iter().all(|chorus| chorus.high() < song_high) {
        let peak = sections.iter().find(|s| s.high() == song_high).unwrap();
        warnings.push(format!(
            "The song's highest note, {}, is in {}, not in a chorus",
            midi_name(song_high),
            peak.label
        ));
    }
    // The frame around the melody, while the form is still the score's to change: an intro
    // cannot be lengthened later without moving everything after it.
    let bar = |at: Ticks| {
        project
            .signatures
            .signature_at(at)
            .ticks_per_bar()
            .raw()
            .max(1) as f64
    };
    let first = notes.first().map_or(Ticks::ZERO, |note| note.0);
    let intro_bars = first.raw() as f64 / bar(Ticks::ZERO);
    let song_end = project.sections.end().max(last_end);
    let outro_bars = (song_end - last_end).raw().max(0) as f64 / bar(last_end);
    if intro_bars < MIN_FRAME_BARS {
        warnings.push(format!(
            "The melody starts after {intro_bars:.1} bars; plan an intro of at least {MIN_FRAME_BARS} bars into the form now, before anything is written after it"
        ));
    }
    if outro_bars < MIN_FRAME_BARS {
        warnings.push(format!(
            "The form ends {outro_bars:.1} bars after the melody; plan an outro of at least {MIN_FRAME_BARS} bars"
        ));
    }
    let (lines, form_warnings) = super::melody_form::melody_form(project, track);
    warnings.extend(form_warnings);
    let report: Vec<Value> = sections
        .iter()
        .map(|section| {
            json!({
                "section": section.label, "notes": section.pitches.len(),
                "low": midi_name(section.low()), "high": midi_name(section.high()),
                "range_semitones": section.high() - section.low(),
                "mean_pitch": (section.mean() * 10.0).round() / 10.0,
                "leaps": section.intervals().filter(|step| *step >= LEAP).count(),
                "widest_interval": section.intervals().max().unwrap_or_default(),
            })
        })
        .collect();
    Ok(json!({
        "track": track.0, "sections": report, "chorus_lifts": lifts, "lines": lines, "warnings": warnings,
        "intro_bars": (intro_bars * 10.0).round() / 10.0,
        "outro_bars": (outro_bars * 10.0).round() / 10.0,
        "note": "Measured on written notes, highest note where several sound together. A lift is judged against the section just before each chorus.",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::Note;

    /// A one-bar pre-chorus and one-bar chorus, framed by `intro` and `outro` bars.
    fn framed(verse: &[u8], chorus: &[u8], intro: i64, outro: i64) -> (Project, TrackId) {
        let mut project = Project::new("Song", 48_000.0);
        let track = project.add_singer_track("Lead", "auris.synth.vocal");
        let bar = Ticks::QUARTER * 4;
        project
            .sections
            .set_point(Ticks::ZERO, Some("Intro".into()));
        project
            .sections
            .set_point(bar * intro, Some("Bメロ".into()));
        project
            .sections
            .set_point(bar * (intro + 1), Some("サビ1".into()));
        project
            .sections
            .set_point(bar * (intro + 2), Some("Outro".into()));
        project.sections.set_point(bar * (intro + 2 + outro), None);
        let clip = project
            .add_midi_clip(track, "Lead", bar * intro, bar * 2)
            .unwrap();
        let notes = verse
            .iter()
            .chain(chorus)
            .enumerate()
            .map(|(i, pitch)| Note::new(*pitch, Ticks::QUARTER * i as i64, Ticks::QUARTER))
            .collect();
        project.midi_clip_mut(clip).unwrap().notes = notes;
        (project, track)
    }

    fn song(verse: &[u8], chorus: &[u8]) -> (Project, TrackId) {
        framed(verse, chorus, 8, 8)
    }

    #[test]
    fn a_short_frame_is_reported_while_the_form_can_still_change() {
        let (project, track) = framed(&[60, 62, 64, 62], &[67, 72, 67, 74], 4, 2);
        let report = melody_shape(&project, track).unwrap();
        let warnings: Vec<&str> = report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_str().unwrap())
            .collect();
        assert_eq!(report["intro_bars"], 4.0);
        assert_eq!(report["outro_bars"], 2.0);
        assert!(
            warnings[0].contains("starts after 4.0 bars"),
            "{warnings:?}"
        );
        assert!(warnings[1].contains("ends 2.0 bars after"), "{warnings:?}");
    }

    #[test]
    fn a_chorus_that_stays_where_the_pre_chorus_was_is_reported() {
        // Up a semitone and a half on average, a whole tone at the peak, and only by step.
        let (project, track) = song(&[64, 65, 67, 65], &[65, 67, 69, 66]);
        let report = melody_shape(&project, track).unwrap();
        let warnings = report["warnings"].as_array().unwrap();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings[0]
                .as_str()
                .unwrap()
                .contains("1.5 semitones above Bメロ")
        );
        assert!(warnings[1].as_str().unwrap().contains("only by step"));
        assert_eq!(report["chorus_lifts"][0]["mean_lift_semitones"], 1.5);
        assert_eq!(report["chorus_lifts"][0]["peak_lift_semitones"], 2);
    }

    #[test]
    fn a_pre_chorus_is_not_a_chorus_however_it_is_spelled() {
        for label in [
            "Pre-chorus 1",
            "PreChorus2",
            "pre chorus",
            "Post-Chorus",
            "プリサビ",
        ] {
            assert!(!is_chorus(label), "{label}");
        }
        for label in [
            "Chorus 1",
            "Final Chorus",
            "サビ2",
            "落ちサビ",
            "ラストサビ",
            "Hook",
        ] {
            assert!(is_chorus(label), "{label}");
        }
    }

    #[test]
    fn a_chorus_is_judged_against_a_pre_chorus_named_in_english() {
        let (mut project, track) = song(&[64, 65, 67, 65], &[65, 67, 69, 66]);
        let bar = Ticks::QUARTER * 4;
        project
            .sections
            .set_point(bar * 8, Some("Pre-chorus 1".into()));
        project.sections.set_point(bar * 9, Some("Chorus 1".into()));
        let report = melody_shape(&project, track).unwrap();
        assert_eq!(report["chorus_lifts"].as_array().unwrap().len(), 1);
        assert_eq!(report["chorus_lifts"][0]["chorus"], "Chorus 1");
        assert_eq!(report["chorus_lifts"][0]["after"], "Pre-chorus 1");
        let warnings = report["warnings"].as_array().unwrap();
        assert!(
            warnings
                .iter()
                .all(|w| !w.as_str().unwrap().starts_with("Pre-chorus")),
            "{warnings:?}"
        );
    }

    #[test]
    fn a_chorus_that_lifts_and_leaps_passes() {
        let (project, track) = song(&[60, 62, 64, 62], &[67, 72, 67, 74]);
        let report = melody_shape(&project, track).unwrap();
        assert_eq!(report["warnings"].as_array().unwrap().len(), 0, "{report}");
        assert_eq!(report["sections"][1]["high"], "D5");
        assert_eq!(report["sections"][1]["widest_interval"], 7);
    }
}
