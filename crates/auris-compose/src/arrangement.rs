//! Score-level cooperation between a foreground line and its accompaniment.
//!
//! This offline pass edits ordinary notes before they enter the document. The rhythm section
//! keeps its clock; chord and arpeggio parts leave space during busy foreground phrases and
//! can answer in an existing breath. Authored rhythm patterns bypass this pass.

use auris_core::{Note, Ticks, TimeSignature};

use crate::frame::Frame;
use crate::parts::PartDraft;
use crate::{PerformanceStyle, Role, SongSpec};

/// Shapes one generated accompaniment clip around a foreground line in clip-relative ticks.
///
/// The foreground is never edited. Sustained ambient textures keep their attacks, and bass
/// and drum parts are outside this pass. `phrase_ends` bounds the occasional response to a
/// planned breath; an empty list still permits thinning, but never invents a response.
/// `answer_length` bounds a pitch's requested duration to compatible harmony, returning zero
/// if it is invalid at the onset. It also protects harmonic changes during thinning.
pub fn accompany_foreground(
    notes: &mut Vec<Note>,
    foreground: &[Note],
    meter: TimeSignature,
    role: Role,
    style: Option<PerformanceStyle>,
    phrase_ends: &[Ticks],
    answer_length: impl Fn(Ticks, u8, Ticks) -> Ticks,
) {
    if foreground.is_empty()
        || notes.is_empty()
        || !matches!(role, Role::Chords | Role::Stab | Role::Arp)
        || style == Some(PerformanceStyle::Ambient)
    {
        return;
    }
    let beat = meter.ticks_per_beat().raw().max(1);
    let bar = meter.ticks_per_bar().raw().max(1);
    let original = notes.clone();
    // Select whole, existing attack groups. An offbeat figure stays offbeat, and each
    // occupied pulse bucket retains a chord rather than falling back to one isolated voice.
    let spacing = match (style, role) {
        (
            Some(PerformanceStyle::Chiptune | PerformanceStyle::Synthwave | PerformanceStyle::Rock),
            _,
        ) => beat,
        (_, Role::Arp) => beat,
        _ => beat * 2,
    };
    let mut groups = std::collections::BTreeMap::<Ticks, Vec<u8>>::new();
    for note in &original {
        groups.entry(note.start).or_default().push(note.pitch);
    }
    let mut pillars = std::collections::BTreeSet::new();
    let mut previous_bucket = None;
    let mut previous_pitches = Vec::new();
    for (&onset, pitches) in &groups {
        let raw = onset.raw().max(0);
        let bucket = (raw / bar, raw % bar / spacing);
        if previous_bucket != Some(bucket)
            || previous_pitches
                .iter()
                .any(|&pitch| answer_length(onset, pitch, Ticks(1)) <= Ticks::ZERO)
        {
            pillars.insert(onset);
            previous_bucket = Some(bucket);
            previous_pitches.clone_from(pitches);
        }
    }
    // Count attacks once, even if the foreground includes doubled notes.
    let mut attacks: Vec<_> = foreground.iter().map(|n| n.start).collect();
    attacks.sort_unstable();
    attacks.dedup();
    let mut counts = std::collections::BTreeMap::<i64, usize>::new();
    for at in attacks {
        *counts.entry(at.raw().max(0) / bar).or_default() += 1;
    }
    let mut intervals: Vec<_> = foreground.iter().map(|n| (n.start, n.end())).collect();
    intervals.sort_unstable();
    let mut sounding_spans: Vec<(Ticks, Ticks)> = Vec::new();
    for (start, end) in intervals {
        if let Some(previous) = sounding_spans.last_mut()
            && start <= previous.1
        {
            previous.1 = previous.1.max(end);
        } else {
            sounding_spans.push((start, end));
        }
    }
    let sounding = |at: Ticks| {
        let next = sounding_spans.partition_point(|(start, _)| *start <= at);
        next > 0 && at < sounding_spans[next - 1].1
    };
    notes.retain_mut(|note| {
        let start = note.start.raw().max(0);
        let busy = counts.get(&(start / bar)).is_some_and(|count| *count >= 4);
        if !busy || !sounding(note.start) {
            return true;
        }
        let pillar = pillars.contains(&note.start);
        if pillar {
            note.velocity *= 0.90;
        }
        pillar
    });
    // Answer only in the final breath of a phrase, using the most recent chord in that same
    // bar. It cannot anticipate a different bar's harmony or create a new melodic part.
    for &end in phrase_ends {
        let start = foreground
            .iter()
            .filter(|n| n.start < end && n.start >= end - Ticks(bar))
            .map(|n| n.start + n.length)
            .max();
        let Some(start) = start else { continue };
        if end - start < Ticks(beat / 2) || sounding(start) {
            continue;
        }
        let bar_start = Ticks(start.raw().max(0) / bar * bar);
        let source = original
            .iter()
            .filter(|n| n.start >= bar_start && n.start < start)
            .map(|n| n.start)
            .max();
        let Some(source) = source else { continue };
        if notes.iter().any(|n| n.start >= start && n.start < end) {
            continue;
        }
        let requested = (end - start).min(Ticks(beat));
        let answer: Vec<_> = original
            .iter()
            .filter(|n| n.start == source)
            .take(4)
            .filter_map(|n| {
                let length = answer_length(start, n.pitch, requested).min(requested);
                if length <= Ticks::ZERO {
                    return None;
                }
                let mut n = n.clone();
                n.start = start;
                n.length = length;
                n.velocity *= 0.85;
                Some(n)
            })
            .collect();
        if answer.is_empty() {
            continue;
        }
        // A held accompaniment releases before its own answer.
        for note in notes
            .iter_mut()
            .filter(|n| n.start < start && n.start + n.length > start)
        {
            note.length = start - note.start;
        }
        notes.extend(answer);
    }
    notes.sort_by_key(|n| (n.start, n.pitch));
}

pub(crate) fn arrange(spec: &SongSpec, frame: &Frame, drafts: &mut [PartDraft]) {
    for (section_index, section) in frame.sections.iter().enumerate() {
        if section.coda
            || spec
                .sections
                .get(&section.name)
                .is_some_and(|s| !s.lyrics.trim().is_empty())
        {
            // The session will use the real, lyric-conditioned voice once it exists.
            continue;
        }
        let lead = drafts
            .iter()
            .filter(|draft| {
                spec.parts
                    .iter()
                    .any(|p| p.name == draft.name && p.role == Role::Melody)
            })
            .filter(|draft| draft.notes.iter().any(|n| n.section == section_index))
            .min_by_key(|draft| &draft.name);
        let Some(lead) = lead else { continue };
        let foreground: Vec<_> = lead
            .notes
            .iter()
            .filter(|n| n.section == section_index)
            .map(|n| Note {
                velocity: n.velocity,
                ..Note::new(n.pitch, n.start - section.start, n.length)
            })
            .collect();
        let phrase_ends: Vec<_> = section
            .phrases
            .iter()
            .map(|p| frame.grid.bar_ticks() * p.end_bar() as i64)
            .collect();
        for draft in drafts.iter_mut() {
            let Some(part) = spec.parts.iter().find(|p| p.name == draft.name) else {
                continue;
            };
            let played = section.played(part);
            if played.rhythm.is_some()
                || !matches!(played.role, Role::Chords | Role::Stab | Role::Arp)
            {
                continue;
            }
            let mut notes: Vec<_> = draft
                .notes
                .iter()
                .filter(|n| n.section == section_index)
                .map(|n| Note {
                    velocity: n.velocity,
                    ..Note::new(n.pitch, n.start - section.start, n.length)
                })
                .collect();
            accompany_foreground(
                &mut notes,
                &foreground,
                spec.meter,
                played.role,
                spec.writing_style,
                &phrase_ends,
                |at, pitch, requested| {
                    if !section
                        .chord_at(at)
                        .is_some_and(|event| event.chord.contains_midi(i32::from(pitch)))
                    {
                        return Ticks::ZERO;
                    }
                    section
                        .events
                        .iter()
                        .filter(|event| event.start > at && event.start < at + requested)
                        .find(|event| !event.chord.contains_midi(i32::from(pitch)))
                        .map_or(requested, |event| event.start - at)
                },
            );
            draft.notes.retain(|n| n.section != section_index);
            draft
                .notes
                .extend(notes.into_iter().map(|n| crate::parts::Draft {
                    section: section_index,
                    pitch: n.pitch,
                    start: n.start + section.start,
                    length: n.length,
                    velocity: n.velocity,
                }));
        }
    }
    for draft in drafts {
        draft.notes.sort_by_key(|n| (n.start, n.pitch));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_foreground_has_space_and_an_answer_in_its_breath() {
        let foreground: Vec<_> = (0..6)
            .map(|i| Note::new(72, Ticks(i * 480), Ticks(480)))
            .collect();
        let mut comp: Vec<_> = (0..6)
            .flat_map(|i| [60, 64, 67].map(|p| Note::new(p, Ticks(i * 480), Ticks(480))))
            .collect();
        let original = foreground.clone();
        accompany_foreground(
            &mut comp,
            &foreground,
            TimeSignature::default(),
            Role::Chords,
            Some(PerformanceStyle::PopBand),
            &[Ticks(3840)],
            |_, _, length| length,
        );
        assert_eq!(foreground, original);
        assert!(comp.iter().filter(|n| n.start < Ticks(2880)).count() < 18);
        assert!(comp.iter().any(|n| n.start == Ticks(2880)));
        assert!(
            comp.iter()
                .all(|n| n.length > Ticks::ZERO && n.start + n.length <= Ticks(3840))
        );
    }

    #[test]
    fn a_sustained_texture_and_the_rhythm_section_keep_their_notes() {
        for (role, style) in [
            (Role::Bass, None),
            (Role::Kick, None),
            (Role::Chords, Some(PerformanceStyle::Ambient)),
        ] {
            let mut notes = vec![Note::new(48, Ticks::ZERO, Ticks(3840))];
            let original = notes.clone();
            let foreground: Vec<_> = (0..8)
                .map(|i| Note::new(72, Ticks(i * 480), Ticks(480)))
                .collect();
            accompany_foreground(
                &mut notes,
                &foreground,
                TimeSignature::default(),
                role,
                style,
                &[Ticks(3840)],
                |_, _, length| length,
            );
            assert_eq!(notes, original);
        }
    }

    #[test]
    fn a_response_obeys_the_chord_at_its_destination() {
        let foreground = vec![Note::new(72, Ticks::ZERO, Ticks(2880))];
        let original = vec![Note::new(60, Ticks::ZERO, Ticks(3840))];
        let mut notes = original.clone();
        accompany_foreground(
            &mut notes,
            &foreground,
            TimeSignature::default(),
            Role::Chords,
            None,
            &[Ticks(3840)],
            |at, pitch, length| {
                if at < Ticks(1920) || pitch == 62 {
                    length
                } else {
                    Ticks::ZERO
                }
            },
        );
        assert_eq!(
            notes, original,
            "a rejected answer must not shorten a held chord"
        );
    }

    #[test]
    fn thinning_retains_whole_offbeat_chords_in_every_bar() {
        let foreground: Vec<_> = (0..16)
            .map(|i| Note::new(72, Ticks(i * 480), Ticks(480)))
            .collect();
        let mut notes: Vec<_> = (0..8)
            .flat_map(|i| {
                [60, 64, 67].map(|pitch| Note::new(pitch, Ticks(480 + i * 960), Ticks(480)))
            })
            .collect();
        accompany_foreground(
            &mut notes,
            &foreground,
            TimeSignature::default(),
            Role::Chords,
            None,
            &[],
            |_, _, length| length,
        );
        let expected: Vec<_> = [480, 2400, 4320, 6240]
            .into_iter()
            .flat_map(|start| [60, 64, 67].map(|pitch| (Ticks(start), pitch)))
            .collect();
        assert_eq!(
            notes.iter().map(|n| (n.start, n.pitch)).collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn an_answer_releases_at_the_next_incompatible_chord() {
        let foreground = vec![Note::new(72, Ticks::ZERO, Ticks(3000))];
        let mut notes = vec![Note::new(60, Ticks::ZERO, Ticks(3840))];
        accompany_foreground(
            &mut notes,
            &foreground,
            TimeSignature::default(),
            Role::Chords,
            None,
            &[Ticks(3840)],
            |at, _, length| length.min((Ticks(3360) - at).max_zero()),
        );
        let answer = notes.iter().find(|n| n.start == Ticks(3000)).unwrap();
        assert_eq!(answer.end(), Ticks(3360));
    }
}
