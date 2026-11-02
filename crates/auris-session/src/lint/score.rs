//! The score rules: everything that can be read off the notes and the harmony annotation.

use std::collections::{BTreeMap, HashSet};

use auris_core::project::{MidiClip, Project, TrackId, TrackKind, loop_passes};
use auris_core::time::{TICKS_PER_QUARTER, Ticks};

use auris_core::theory::chord::{Chord, Quality};
use auris_core::theory::numeral::Numeral;
use auris_core::theory::pitch::PitchClass;

use super::{Diagnostic, Fix, NoteRef, Rule, Severity, sort};

/// Notes one track may strike on one tick before the voicing is unplayable mush.
pub const CROWDED_TRACK: usize = 8;
/// Pitched notes the whole arrangement may strike on one tick.
pub const CROWDED_MIX: usize = 12;
/// Distinct attacks per quarter note, averaged over a bar: continuous 32nds is eight.
pub const DENSE_ATTACKS_PER_BEAT: f32 = 8.0;
/// Notes per quarter note, chords counted note by note, averaged over a bar.
pub const DENSE_NOTES_PER_BEAT: f32 = 24.0;
/// How long two notes a semitone apart must ring together to count: one beat.
pub const HARSH_OVERLAP: Ticks = Ticks(TICKS_PER_QUARTER);
/// The top of the register where seconds turn to mud (B2).
pub const MUDDY_CEILING: u8 = 47;
/// How long two low notes a second apart must ring together to count: an eighth.
pub const MUDDY_OVERLAP: Ticks = Ticks(TICKS_PER_QUARTER / 2);
/// Shortest note the key and chord rules consider; anything quicker is a passing tone.
pub const SUSTAINED: Ticks = Ticks(TICKS_PER_QUARTER / 2);
/// How long an avoid note must ring against its chord to count: one beat.
pub const AVOID_OVERLAP: Ticks = Ticks(TICKS_PER_QUARTER);
/// Velocity below which a note is not heard.
pub const INAUDIBLE_VELOCITY: f32 = 0.05;
/// The range of a piano, which covers every pitched instrument worth writing for.
pub const INSTRUMENT_RANGE: (u8, u8) = (21, 108);
/// The range a singing voice can be asked for, from a bass's C2 to a soprano's C6.
pub const VOICE_RANGE: (u8, u8) = (36, 84);

/// Distinct pitch classes that must sound at one instant for a bar to count as chordal writing.
pub const CHORD_TONES: usize = 3;
/// A pitch class's share of a bar's sounding time below which it is ornament, not harmony.
pub const CHORD_SHARE: f32 = 0.12;

/// Ticks within which two attacks count as the same moment.
const SAME_MOMENT: i64 = 15;
/// No document is linted past this many sounding notes; a looped clip can repeat thousands.
const MAX_EVENTS: usize = 200_000;
/// The most problems one rule reports, so one mistake repeated by a loop is not a wall of text.
const MAX_PER_RULE: usize = 200;

/// One sounding note: a stored note on one pass of its clip, placed on the timeline.
#[derive(Clone, Copy, Debug)]
pub(super) struct Event {
    pub(super) track: TrackId,
    pub(super) note: NoteRef,
    pub(super) pitch: u8,
    pub(super) start: Ticks,
    pub(super) end: Ticks,
    pub(super) drum: bool,
}

/// Lints a document's notes against fixed rules and its own harmony annotation.
///
/// Muted tracks and clips are skipped: they are not heard, and a muted take is often the wrong
/// one on purpose. Solo is a listening aid and is ignored.
pub fn lint_score(project: &Project) -> Vec<Diagnostic> {
    let events = events(project);
    let mut diagnostics = Vec::new();
    diagnostics.extend(limit(crowded_onsets(&events)));
    diagnostics.extend(limit(dense_bars(project, &events)));
    diagnostics.extend(intervals(&events));
    diagnostics.extend(harmony(project, &events));
    diagnostics.extend(limit(missing_chords(project, &events)));
    diagnostics.extend(clip_rules(project));
    diagnostics.extend(singer_rules(project));
    sort(&mut diagnostics);
    diagnostics
}

fn limit(mut diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
    diagnostics.truncate(MAX_PER_RULE);
    diagnostics
}

fn sounding_tracks(project: &Project) -> impl Iterator<Item = (TrackId, &[MidiClip], bool)> {
    project
        .tracks
        .iter()
        .filter(|t| !t.mixer.mute)
        .filter_map(|track| {
            let (clips, drum) = match &track.kind {
                TrackKind::Instrument(instrument) => (&instrument.clips, false),
                TrackKind::Drum(instrument) => (&instrument.clips, true),
                TrackKind::Singer(singer) => (&singer.clips, false),
                TrackKind::Audio(_) | TrackKind::Bus => return None,
            };
            Some((track.id, clips.as_slice(), drum))
        })
}

pub(super) fn events(project: &Project) -> Vec<Event> {
    let mut events = Vec::new();
    for (track, clips, drum) in sounding_tracks(project) {
        for clip in clips.iter().filter(|clip| !clip.muted) {
            let notes: Vec<_> = clip.playable_notes_with_indices().collect();
            for (offset, span) in loop_passes(clip.length, clip.loop_end) {
                for (index, note) in &notes {
                    if note.start >= span {
                        continue;
                    }
                    if events.len() >= MAX_EVENTS {
                        return events;
                    }
                    let start = clip.start + offset + note.start;
                    let length = note.length.min(span - note.start).max(Ticks(1));
                    events.push(Event {
                        track,
                        note: NoteRef {
                            clip: clip.id,
                            index: *index,
                        },
                        pitch: note.pitch,
                        start,
                        end: start + length,
                        drum,
                    });
                }
            }
        }
    }
    events.sort_by_key(|event| (event.start, event.pitch));
    events
}

/// Runs of attacks that fall within [`SAME_MOMENT`] of the first in the run.
fn moments<'a>(events: impl Iterator<Item = &'a Event>) -> Vec<Vec<&'a Event>> {
    let mut groups: Vec<Vec<&Event>> = Vec::new();
    for event in events {
        match groups.last_mut() {
            Some(group) if (event.start - group[0].start).raw() <= SAME_MOMENT => group.push(event),
            _ => groups.push(vec![event]),
        }
    }
    groups
}

fn crowded_onsets(events: &[Event]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for group in moments(events.iter().filter(|event| !event.drum)) {
        if group.len() <= CROWDED_TRACK {
            continue;
        }
        let mut by_track: BTreeMap<u64, Vec<&Event>> = BTreeMap::new();
        for event in &group {
            by_track.entry(event.track.0).or_default().push(event);
        }
        let mut reported = false;
        for (track, members) in by_track {
            if members.len() > CROWDED_TRACK {
                diagnostics.push(crowd(&members, Some(TrackId(track)), CROWDED_TRACK));
                reported = true;
            }
        }
        if !reported && group.len() > CROWDED_MIX {
            diagnostics.push(crowd(&group, None, CROWDED_MIX));
        }
    }
    diagnostics
}

fn crowd(members: &[&Event], track: Option<TrackId>, limit: usize) -> Diagnostic {
    let start = members[0].start;
    let shortest = members
        .iter()
        .map(|event| event.end - event.start)
        .min()
        .unwrap_or(Ticks(1));
    let mut diagnostic = Diagnostic::new(
        Rule::CrowdedOnset,
        Severity::Warning,
        start,
        start + shortest.max(Ticks(TICKS_PER_QUARTER / 4)),
    );
    diagnostic.track = track;
    diagnostic.notes = members.iter().map(|event| event.note).collect();
    diagnostic.notes.sort();
    diagnostic.notes.dedup();
    diagnostic.pitches = sorted_pitches(members.iter().map(|event| event.pitch));
    diagnostic.value = Some(members.len() as f32);
    diagnostic.limit = Some(limit as f32);
    diagnostic
}

fn sorted_pitches(pitches: impl Iterator<Item = u8>) -> Vec<u8> {
    let mut pitches: Vec<u8> = pitches.collect();
    pitches.sort_unstable();
    pitches.dedup();
    pitches
}

fn dense_bars(project: &Project, events: &[Event]) -> Vec<Diagnostic> {
    let signatures = &project.signatures;
    // (track, bar) → attacks and notes in it.
    let mut bars: BTreeMap<(u64, u32), (HashSet<i64>, usize)> = BTreeMap::new();
    for event in events {
        let bar = signatures.bar_of(event.start);
        let entry = bars.entry((event.track.0, bar)).or_default();
        entry.0.insert(event.start.raw() / SAME_MOMENT);
        entry.1 += 1;
    }
    let mut diagnostics = Vec::new();
    for ((track, bar), (attacks, notes)) in bars {
        let start = signatures.bar_start(bar);
        let end = signatures.bar_start(bar + 1);
        let beats = ((end - start).raw() as f32 / TICKS_PER_QUARTER as f32).max(0.25);
        let attack_rate = attacks.len() as f32 / beats;
        let note_rate = notes as f32 / beats;
        let (value, limit) = if attack_rate > DENSE_ATTACKS_PER_BEAT {
            (attack_rate, DENSE_ATTACKS_PER_BEAT)
        } else if note_rate > DENSE_NOTES_PER_BEAT {
            (note_rate, DENSE_NOTES_PER_BEAT)
        } else {
            continue;
        };
        let mut diagnostic = Diagnostic::new(Rule::DenseNotes, Severity::Warning, start, end);
        diagnostic.track = Some(TrackId(track));
        diagnostic.value = Some(value);
        diagnostic.limit = Some(limit);
        diagnostics.push(diagnostic);
    }
    diagnostics
}

/// Held seconds: a semitone anywhere, and either second down in the bass.
fn intervals(events: &[Event]) -> Vec<Diagnostic> {
    let mut harsh = Vec::new();
    let mut muddy = Vec::new();
    let mut seen = HashSet::new();
    let mut active: Vec<&Event> = Vec::new();
    for event in events.iter().filter(|event| !event.drum) {
        active.retain(|other| other.end > event.start);
        for other in &active {
            let distance = event.pitch.abs_diff(other.pitch);
            let overlap = event.end.min(other.end) - event.start;
            let rule = if distance == 1 && overlap >= HARSH_OVERLAP {
                Rule::HarshInterval
            } else if (1..=2).contains(&distance)
                && event.pitch.max(other.pitch) <= MUDDY_CEILING
                && overlap >= MUDDY_OVERLAP
            {
                Rule::MuddyLowInterval
            } else {
                continue;
            };
            let pair = if other.note < event.note {
                (other.note, event.note)
            } else {
                (event.note, other.note)
            };
            if !seen.insert((rule, pair)) {
                continue;
            }
            let bucket = match rule {
                Rule::HarshInterval => &mut harsh,
                _ => &mut muddy,
            };
            if bucket.len() >= MAX_PER_RULE {
                continue;
            }
            let mut diagnostic =
                Diagnostic::new(rule, Severity::Warning, event.start, event.start + overlap);
            diagnostic.track = (event.track == other.track).then_some(event.track);
            diagnostic.notes = vec![pair.0, pair.1];
            diagnostic.pitches = sorted_pitches([event.pitch, other.pitch].into_iter());
            bucket.push(diagnostic);
        }
        active.push(event);
    }
    harsh.extend(muddy);
    harsh
}

/// Chords the reading of a bar may name: the triads and sevenths whose notes spell exactly one
/// root. Augmented and diminished-seventh chords are left out because they spell three and four
/// roots, and sixths because a C6 is an Am7 by its notes.
const OBVIOUS_QUALITIES: [Quality; 7] = [
    Quality::Major,
    Quality::Minor,
    Quality::Diminished,
    Quality::Major7,
    Quality::Dominant7,
    Quality::Minor7,
    Quality::HalfDiminished7,
];

/// The chord a bar's notes spell beyond doubt, if they spell one.
///
/// A pitch class counts when it fills at least [`CHORD_SHARE`] of the bar's sounding time, so a
/// passing tone does not; the chord must hold exactly those classes, every one of them.
pub(super) fn obvious_chord(weights: &[i64; 12]) -> Option<Chord> {
    let total: i64 = weights.iter().sum();
    if total <= 0 {
        return None;
    }
    let present: Vec<i32> = (0..12)
        .filter(|class| weights[*class as usize] as f32 >= total as f32 * CHORD_SHARE)
        .collect();
    if !(3..=4).contains(&present.len()) {
        return None;
    }
    let mut found = None;
    for root in 0..12 {
        for quality in OBVIOUS_QUALITIES {
            let chord = Chord::new(PitchClass::new(root), quality);
            let mut classes: Vec<i32> = chord.classes().iter().map(|c| c.semitones()).collect();
            classes.sort_unstable();
            classes.dedup();
            if classes == present {
                if found.is_some() {
                    return None;
                }
                found = Some(chord);
            }
        }
    }
    found
}

/// Bars that sound chords under no chord annotation.
///
/// A bar counts when at least [`CHORD_TONES`] pitch classes sound together in it and no chord is
/// written anywhere in it. Neighbouring bars are reported together; where every bar of a run
/// spells one chord beyond doubt ([`obvious_chord`]), writing those chords is the fix.
fn missing_chords(project: &Project, events: &[Event]) -> Vec<Diagnostic> {
    let signatures = &project.signatures;
    let chords = &project.harmony.chords;
    let pitched: Vec<&Event> = events.iter().filter(|event| !event.drum).collect();
    let Some(last) = pitched.iter().map(|event| event.end).max() else {
        return Vec::new();
    };
    let last_bar = signatures.bar_of(last - Ticks(1));
    // Each bar: whether it is chordal and unannotated, and the chord it spells.
    let mut bars: Vec<(u32, Option<Chord>)> = Vec::new();
    let mut first = 0;
    for bar in 1..=last_bar {
        let start = signatures.bar_start(bar);
        let end = signatures.bar_start(bar + 1);
        let written = chords.numeral_at(start).is_some()
            || chords
                .points()
                .iter()
                .any(|point| point.tick > start && point.tick < end && point.chord.is_some());
        // Events are sorted by start, so skip the ones that ended before this bar.
        while first < pitched.len() && pitched[first].start < start && pitched[first].end <= start {
            first += 1;
        }
        if written {
            continue;
        }
        let inside: Vec<&&Event> = pitched[first..]
            .iter()
            .take_while(|event| event.start < end)
            .filter(|event| event.end > start)
            .collect();
        let chordal = inside.iter().any(|onset| {
            let at = onset.start.max(start);
            let mut classes = [false; 12];
            for event in &inside {
                if event.start <= at && event.end > at {
                    classes[(event.pitch % 12) as usize] = true;
                }
            }
            classes.iter().filter(|sounding| **sounding).count() >= CHORD_TONES
        });
        if !chordal {
            continue;
        }
        let mut weights = [0i64; 12];
        for event in &inside {
            weights[(event.pitch % 12) as usize] +=
                (event.end.min(end) - event.start.max(start)).raw();
        }
        bars.push((bar, obvious_chord(&weights)));
    }
    let mut diagnostics = Vec::new();
    let mut index = 0;
    while index < bars.len() {
        let obvious = bars[index].1.is_some();
        let mut stop = index + 1;
        while stop < bars.len()
            && bars[stop].0 == bars[stop - 1].0 + 1
            && bars[stop].1.is_some() == obvious
        {
            stop += 1;
        }
        let run = &bars[index..stop];
        let start = signatures.bar_start(run[0].0);
        let end = signatures.bar_start(run[run.len() - 1].0 + 1);
        let mut diagnostic = Diagnostic::new(Rule::MissingChord, Severity::Info, start, end);
        diagnostic.value = Some(run.len() as f32);
        if obvious {
            let harmony = &project.harmony;
            diagnostic.fix = Some(Fix::SetChords {
                chords: run
                    .iter()
                    .filter_map(|(bar, chord)| {
                        let at = signatures.bar_start(*bar);
                        Some((at, Numeral::from_chord_in((*chord)?, harmony.key_at(at))))
                    })
                    .collect(),
                end,
            });
        }
        diagnostics.push(diagnostic);
        index = stop;
    }
    diagnostics
}

/// Sustained notes measured against the written key and chord. Only where a chord is written:
/// a key on its own is too often the default nobody chose.
fn harmony(project: &Project, events: &[Event]) -> Vec<Diagnostic> {
    let harmony = &project.harmony;
    if harmony.is_empty() {
        return Vec::new();
    }
    let mut out_of_key = Vec::new();
    let mut avoid = Vec::new();
    let mut seen = HashSet::new();
    for event in events.iter().filter(|event| !event.drum) {
        if event.end - event.start < SUSTAINED {
            continue;
        }
        let Some(chord) = harmony.chord_at(event.start) else {
            continue;
        };
        let key = harmony.key_at(event.start);
        let class = auris_core::theory::pitch::PitchClass::new(event.pitch as i32);
        if chord.contains(class) {
            continue;
        }
        // How long the note rings against this one chord.
        let points = harmony.chords.points();
        let next = points.partition_point(|point| point.tick <= event.start);
        let chord_end = points.get(next).map_or(event.end, |point| point.tick);
        let against = event.end.min(chord_end) - event.start;
        let (rule, severity, target) = if !key.scale.contains(key.tonic, class) {
            let target = chord.nearest_tone(event.pitch as i32);
            (Rule::OutOfKey, Severity::Warning, target)
        } else if chord.contains(class.transposed(-1)) && against >= AVOID_OVERLAP {
            (Rule::AvoidNote, Severity::Info, event.pitch as i32 - 1)
        } else {
            continue;
        };
        if !seen.insert((rule, event.note)) {
            continue;
        }
        let bucket = if rule == Rule::OutOfKey {
            &mut out_of_key
        } else {
            &mut avoid
        };
        if bucket.len() >= MAX_PER_RULE {
            continue;
        }
        let mut diagnostic = Diagnostic::new(rule, severity, event.start, event.end);
        diagnostic.track = Some(event.track);
        diagnostic.notes = vec![event.note];
        diagnostic.pitches = vec![event.pitch];
        if let Ok(target) = u8::try_from(target)
            && target <= 127
        {
            diagnostic.fix = Some(Fix::SetPitches {
                clip: event.note.clip,
                pitches: vec![(event.note.index, target)],
            });
        }
        bucket.push(diagnostic);
    }
    out_of_key.extend(avoid);
    out_of_key
}

/// The rules about single clips' stored notes, reported once however often a loop repeats them.
fn clip_rules(project: &Project) -> Vec<Diagnostic> {
    let mut duplicates = Vec::new();
    let mut quiet = Vec::new();
    let mut extreme = Vec::new();
    for track in project.tracks.iter().filter(|t| !t.mixer.mute) {
        let (clips, drum, singer) = match &track.kind {
            TrackKind::Instrument(instrument) => (&instrument.clips, false, false),
            TrackKind::Drum(instrument) => (&instrument.clips, true, false),
            TrackKind::Singer(singer) => (&singer.clips, false, true),
            TrackKind::Audio(_) | TrackKind::Bus => continue,
        };
        for clip in clips.iter().filter(|clip| !clip.muted) {
            duplicates.extend(duplicate_notes(track.id, clip));
            let notes: Vec<_> = clip.playable_notes_with_indices().collect();
            if !singer {
                let silent: Vec<_> = notes
                    .iter()
                    .filter(|(_, note)| note.velocity < INAUDIBLE_VELOCITY)
                    .collect();
                for (index, note) in silent {
                    let start = clip.start + note.start;
                    let mut diagnostic = Diagnostic::new(
                        Rule::InaudibleVelocity,
                        Severity::Warning,
                        start,
                        start + note.length,
                    );
                    diagnostic.track = Some(track.id);
                    diagnostic.notes = vec![NoteRef {
                        clip: clip.id,
                        index: *index,
                    }];
                    diagnostic.pitches = vec![note.pitch];
                    diagnostic.value = Some(note.velocity);
                    diagnostic.limit = Some(INAUDIBLE_VELOCITY);
                    diagnostic.fix = Some(Fix::SetVelocity {
                        clip: clip.id,
                        indices: vec![*index],
                        velocity: crate::DEFAULT_VELOCITY,
                    });
                    quiet.push(diagnostic);
                }
            }
            if drum {
                continue;
            }
            let (low, high) = if singer {
                VOICE_RANGE
            } else {
                INSTRUMENT_RANGE
            };
            for (index, note) in &notes {
                if (low..=high).contains(&note.pitch) {
                    continue;
                }
                let mut target = note.pitch as i32;
                while target < low as i32 {
                    target += 12;
                }
                while target > high as i32 {
                    target -= 12;
                }
                let start = clip.start + note.start;
                let mut diagnostic = Diagnostic::new(
                    Rule::ExtremePitch,
                    Severity::Warning,
                    start,
                    start + note.length,
                );
                diagnostic.track = Some(track.id);
                diagnostic.notes = vec![NoteRef {
                    clip: clip.id,
                    index: *index,
                }];
                diagnostic.pitches = vec![note.pitch];
                if (low as i32..=high as i32).contains(&target) {
                    diagnostic.fix = Some(Fix::SetPitches {
                        clip: clip.id,
                        pitches: vec![(*index, target as u8)],
                    });
                }
                extreme.push(diagnostic);
            }
        }
    }
    let mut all = limit(duplicates);
    all.extend(limit(quiet));
    all.extend(limit(extreme));
    all
}

/// What a voice cannot do: sing two notes at once, or sing a note with no words on it.
///
/// Overlaps are read within a clip, where trimming the earlier note is the obvious repair: a
/// singer moves to the next note when it begins. A note with no lyric and no phonemes is sung as
/// a stock vowel; in a clip with words elsewhere that is a gap, and a clip with none at all is
/// reported once.
fn singer_rules(project: &Project) -> Vec<Diagnostic> {
    let mut overlaps = Vec::new();
    let mut wordless = Vec::new();
    for track in project.tracks.iter().filter(|t| !t.mixer.mute) {
        let TrackKind::Singer(singer) = &track.kind else {
            continue;
        };
        for clip in singer.clips.iter().filter(|clip| !clip.muted) {
            let mut notes: Vec<_> = clip.playable_notes_with_indices().collect();
            notes.sort_by_key(|(index, note)| (note.start, *index));
            for pair in notes.windows(2) {
                let ((first, a), (second, b)) = (&pair[0], &pair[1]);
                if b.start >= a.end() || (a.start == b.start && a.pitch == b.pitch) {
                    continue;
                }
                let start = clip.start + b.start;
                let end = clip.start + a.end().min(b.end());
                let mut diagnostic =
                    Diagnostic::new(Rule::SingerOverlap, Severity::Warning, start, end);
                diagnostic.track = Some(track.id);
                diagnostic.notes = vec![
                    NoteRef {
                        clip: clip.id,
                        index: *first,
                    },
                    NoteRef {
                        clip: clip.id,
                        index: *second,
                    },
                ];
                diagnostic.pitches = vec![a.pitch, b.pitch];
                if b.start > a.start {
                    diagnostic.fix = Some(Fix::TrimNotes {
                        clip: clip.id,
                        lengths: vec![(*first, (b.start - a.start).raw())],
                    });
                }
                overlaps.push(diagnostic);
            }
            let silent: Vec<_> = notes
                .iter()
                .filter(|(_, note)| note.lyric.trim().is_empty() && note.phonemes.is_empty())
                .collect();
            if silent.is_empty() {
                continue;
            }
            if silent.len() == notes.len() {
                let mut diagnostic = Diagnostic::new(
                    Rule::MissingLyric,
                    Severity::Info,
                    clip.start + notes[0].1.start,
                    clip.start + notes.iter().map(|(_, n)| n.end()).max().unwrap_or_default(),
                );
                diagnostic.track = Some(track.id);
                diagnostic.notes = vec![NoteRef {
                    clip: clip.id,
                    index: notes[0].0,
                }];
                diagnostic.value = Some(notes.len() as f32);
                wordless.push(diagnostic);
                continue;
            }
            for (index, note) in silent {
                let start = clip.start + note.start;
                let mut diagnostic = Diagnostic::new(
                    Rule::MissingLyric,
                    Severity::Warning,
                    start,
                    start + note.length,
                );
                diagnostic.track = Some(track.id);
                diagnostic.notes = vec![NoteRef {
                    clip: clip.id,
                    index: *index,
                }];
                diagnostic.pitches = vec![note.pitch];
                diagnostic.value = Some(1.0);
                wordless.push(diagnostic);
            }
        }
    }
    let mut all = limit(overlaps);
    all.extend(limit(wordless));
    all
}

/// Same pitch (and, on a drum track, the same voice) written over itself in one clip.
fn duplicate_notes(track: TrackId, clip: &MidiClip) -> Vec<Diagnostic> {
    let mut order: Vec<(usize, &auris_core::project::Note)> =
        clip.notes.iter().enumerate().collect();
    order.sort_by_key(|(index, note)| (note.pitch, note.drum_voice.clone(), note.start, *index));
    let mut diagnostics = Vec::new();
    for pair in order.windows(2) {
        let (first_index, first) = pair[0];
        let (second_index, second) = pair[1];
        if first.pitch != second.pitch
            || first.drum_voice != second.drum_voice
            || second.start >= first.end()
        {
            continue;
        }
        let start = clip.start + second.start;
        let end = clip.start + first.end().min(second.end());
        let mut diagnostic = Diagnostic::new(Rule::DuplicateNote, Severity::Warning, start, end);
        diagnostic.track = Some(track);
        diagnostic.notes = vec![
            NoteRef {
                clip: clip.id,
                index: first_index,
            },
            NoteRef {
                clip: clip.id,
                index: second_index,
            },
        ];
        diagnostic.pitches = vec![first.pitch];
        diagnostic.fix = Some(if first.start == second.start {
            // The same attack twice: keep the longer, which is the one somebody can see.
            let drop = if second.length > first.length {
                first_index
            } else {
                second_index
            };
            Fix::RemoveNotes {
                clip: clip.id,
                indices: vec![drop],
            }
        } else {
            Fix::TrimNotes {
                clip: clip.id,
                lengths: vec![(first_index, (second.start - first.start).raw())],
            }
        });
        diagnostics.push(diagnostic);
    }
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::project::ClipId;
    use auris_core::project::Note;
    use auris_core::theory::{key::Key, numeral::Numeral, pitch::PitchClass, scale::ScaleId};

    fn project_with(notes: Vec<Note>) -> (Project, ClipId) {
        let mut project = Project::new("Lint", 48_000.0);
        let track = project.add_instrument_track("Keys", "auris.synth.pulse");
        let clip = project
            .add_midi_clip(track, "Clip", Ticks::ZERO, Ticks(3840 * 4))
            .unwrap();
        project.midi_clip_mut(clip).unwrap().notes = notes;
        (project, clip)
    }

    fn rules(project: &Project) -> Vec<Rule> {
        lint_score(project).iter().map(|d| d.rule).collect()
    }

    #[test]
    fn a_plain_triad_is_clean() {
        let notes = [60, 64, 67]
            .into_iter()
            .map(|p| Note::new(p, Ticks::ZERO, Ticks(3840)))
            .collect();
        let (mut project, _) = project_with(notes);
        in_c_with_chord(&mut project, "I");
        assert!(lint_score(&project).is_empty());
    }

    #[test]
    fn nine_notes_struck_together_by_one_track_are_crowded() {
        let notes = (0..9)
            .map(|i| Note::new(48 + i * 4, Ticks(i as i64), Ticks(960)))
            .collect();
        let (project, _) = project_with(notes);
        let found = lint_score(&project);
        let crowded: Vec<_> = found
            .iter()
            .filter(|d| d.rule == Rule::CrowdedOnset)
            .collect();
        assert_eq!(crowded.len(), 1);
        assert_eq!(crowded[0].value, Some(9.0));
        assert_eq!(crowded[0].notes.len(), 9);
        assert!(crowded[0].fix.is_none());
    }

    #[test]
    fn a_bar_of_sixty_fourth_notes_is_too_dense_and_a_bar_of_sixteenths_is_not() {
        let fast = (0..64)
            .map(|i| Note::new(72, Ticks(i * 60), Ticks(50)))
            .collect();
        let (project, _) = project_with(fast);
        let dense: Vec<_> = lint_score(&project)
            .into_iter()
            .filter(|d| d.rule == Rule::DenseNotes)
            .collect();
        assert_eq!(dense.len(), 1);
        assert_eq!(dense[0].value, Some(16.0));
        assert_eq!((dense[0].start, dense[0].end), (Ticks(0), Ticks(3840)));

        let even = (0..16)
            .map(|i| Note::new(72, Ticks(i * 240), Ticks(200)))
            .collect();
        let (project, _) = project_with(even);
        assert!(!rules(&project).contains(&Rule::DenseNotes));
    }

    #[test]
    fn a_held_minor_second_is_harsh_but_a_passing_one_is_not() {
        let (project, _) = project_with(vec![
            Note::new(64, Ticks(0), Ticks(1920)),
            Note::new(65, Ticks(0), Ticks(1920)),
        ]);
        assert_eq!(rules(&project), vec![Rule::HarshInterval]);

        let (project, _) = project_with(vec![
            Note::new(64, Ticks(0), Ticks(1920)),
            Note::new(65, Ticks(0), Ticks(240)),
        ]);
        assert!(rules(&project).is_empty());
    }

    #[test]
    fn a_low_major_second_is_muddy_but_the_same_interval_up_high_is_not() {
        let (project, _) = project_with(vec![
            Note::new(36, Ticks(0), Ticks(960)),
            Note::new(38, Ticks(0), Ticks(960)),
        ]);
        assert_eq!(rules(&project), vec![Rule::MuddyLowInterval]);
        let (project, _) = project_with(vec![
            Note::new(72, Ticks(0), Ticks(960)),
            Note::new(74, Ticks(0), Ticks(960)),
        ]);
        assert!(rules(&project).is_empty());
    }

    fn in_c_with_chord(project: &mut Project, numeral: &str) {
        project.harmony =
            auris_core::harmony::Harmony::in_key(Key::new(PitchClass::new(0), ScaleId::Major));
        project
            .harmony
            .chords
            .set_point(Ticks::ZERO, Some(Numeral::parse(numeral).unwrap()));
    }

    #[test]
    fn a_held_note_outside_key_and_chord_offers_the_nearest_chord_tone() {
        let (mut project, clip) = project_with(vec![Note::new(61, Ticks(0), Ticks(1920))]);
        in_c_with_chord(&mut project, "I");
        let found = lint_score(&project);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rule, Rule::OutOfKey);
        assert_eq!(
            found[0].fix,
            Some(Fix::SetPitches {
                clip,
                pitches: vec![(0, 60)]
            })
        );
    }

    #[test]
    fn a_borrowed_chord_tone_is_not_out_of_key() {
        // A flat VI chord: A flat is outside C major but inside the written chord.
        let (mut project, _) = project_with(vec![Note::new(68, Ticks(0), Ticks(1920))]);
        in_c_with_chord(&mut project, "bVI");
        assert!(!rules(&project).contains(&Rule::OutOfKey));
    }

    #[test]
    fn a_held_fourth_over_a_major_chord_is_an_avoid_note_resolving_down() {
        let (mut project, clip) = project_with(vec![Note::new(65, Ticks(0), Ticks(1920))]);
        in_c_with_chord(&mut project, "I");
        let found = lint_score(&project);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rule, Rule::AvoidNote);
        assert_eq!(found[0].severity, Severity::Info);
        assert_eq!(
            found[0].fix,
            Some(Fix::SetPitches {
                clip,
                pitches: vec![(0, 64)]
            })
        );
    }

    #[test]
    fn no_written_chords_means_no_key_complaints() {
        let (project, _) = project_with(vec![Note::new(61, Ticks(0), Ticks(1920))]);
        assert!(lint_score(&project).is_empty());
    }

    #[test]
    fn duplicates_are_removed_and_overlapping_repeats_trimmed() {
        let (project, clip) = project_with(vec![
            Note::new(60, Ticks(0), Ticks(960)),
            Note::new(60, Ticks(0), Ticks(480)),
            Note::new(67, Ticks(0), Ticks(960)),
            Note::new(67, Ticks(480), Ticks(960)),
        ]);
        let fixes: Vec<_> = lint_score(&project)
            .into_iter()
            .filter(|d| d.rule == Rule::DuplicateNote)
            .map(|d| d.fix.unwrap())
            .collect();
        assert_eq!(
            fixes,
            vec![
                Fix::RemoveNotes {
                    clip,
                    indices: vec![1]
                },
                Fix::TrimNotes {
                    clip,
                    lengths: vec![(2, 480)]
                },
            ]
        );
    }

    #[test]
    fn whisper_quiet_and_impossible_notes_are_reported_with_fixes() {
        let mut quiet = Note::new(60, Ticks(0), Ticks(480));
        quiet.velocity = 0.0;
        let (project, clip) = project_with(vec![quiet, Note::new(118, Ticks(960), Ticks(480))]);
        let found = lint_score(&project);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].rule, Rule::InaudibleVelocity);
        assert_eq!(found[1].rule, Rule::ExtremePitch);
        assert_eq!(
            found[1].fix,
            Some(Fix::SetPitches {
                clip,
                pitches: vec![(1, 106)]
            })
        );
    }

    #[test]
    fn muted_tracks_are_not_heard_and_so_not_linted() {
        let (mut project, _) = project_with(vec![
            Note::new(64, Ticks(0), Ticks(1920)),
            Note::new(65, Ticks(0), Ticks(1920)),
        ]);
        project.tracks[0].mixer.mute = true;
        assert!(lint_score(&project).is_empty());
    }

    #[test]
    fn a_looped_clip_reports_each_stored_problem_once() {
        let (mut project, _) = project_with(vec![
            Note::new(64, Ticks(0), Ticks(1920)),
            Note::new(65, Ticks(0), Ticks(1920)),
        ]);
        let clip = &mut project.tracks[0].kind.note_clips_mut().unwrap()[0];
        clip.length = Ticks(3840);
        clip.loop_end = Ticks(3840 * 8);
        assert_eq!(rules(&project), vec![Rule::HarshInterval]);
    }

    fn held(pitches: &[u8], bar: i64) -> Vec<Note> {
        pitches
            .iter()
            .map(|p| Note::new(*p, Ticks(bar * 3840), Ticks(3840)))
            .collect()
    }

    fn missing(project: &Project) -> Vec<Diagnostic> {
        lint_score(project)
            .into_iter()
            .filter(|d| d.rule == Rule::MissingChord)
            .collect()
    }

    #[test]
    fn unannotated_chords_that_spell_one_chord_per_bar_offer_those_chords() {
        // C, then G7 in second inversion with a passing A in the melody, then a lone melody.
        let mut notes = held(&[48, 64, 67, 72], 0);
        notes.extend(held(&[50, 55, 59, 65], 1));
        notes.push(Note::new(81, Ticks(3840 + 960), Ticks(240)));
        notes.push(Note::new(72, Ticks(2 * 3840), Ticks(3840)));
        let (project, _) = project_with(notes);
        let found = missing(&project);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].start, found[0].end), (Ticks(0), Ticks(7680)));
        assert_eq!(found[0].value, Some(2.0));
        let Some(Fix::SetChords { chords, end }) = &found[0].fix else {
            panic!("expected chords, got {:?}", found[0].fix);
        };
        assert_eq!(*end, Ticks(7680));
        let key = project.harmony.key_at(Ticks::ZERO);
        let names: Vec<_> = chords
            .iter()
            .map(|(at, n)| (at.raw(), n.name_in(key)))
            .collect();
        assert_eq!(names, vec![(0, "C".to_string()), (3840, "G7".to_string())]);
    }

    #[test]
    fn a_cluster_that_spells_no_single_chord_is_reported_without_a_fix() {
        let (project, _) = project_with(held(&[60, 62, 64, 65, 67], 0));
        let found = missing(&project);
        assert_eq!(found.len(), 1);
        assert!(found[0].fix.is_none());
    }

    #[test]
    fn a_melody_alone_or_written_chords_ask_for_nothing() {
        let melody = (0..8)
            .map(|i| Note::new(60 + (i % 5) as u8, Ticks(i * 480), Ticks(480)))
            .collect();
        let (project, _) = project_with(melody);
        assert!(missing(&project).is_empty());

        let (mut project, _) = project_with(held(&[60, 64, 67], 0));
        in_c_with_chord(&mut project, "I");
        assert!(missing(&project).is_empty());
    }

    fn singer_with(notes: Vec<Note>) -> (Project, ClipId) {
        let mut project = Project::new("Lint", 48_000.0);
        let track = project.add_singer_track("Voice", "auris.synth.pulse");
        let clip = project
            .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks(3840))
            .unwrap();
        project.midi_clip_mut(clip).unwrap().notes = notes;
        (project, clip)
    }

    fn sung(pitch: u8, start: i64, length: i64, lyric: &str) -> Note {
        Note {
            lyric: lyric.into(),
            ..Note::new(pitch, Ticks(start), Ticks(length))
        }
    }

    #[test]
    fn a_singer_asked_for_two_notes_at_once_is_told_to_let_go_of_the_first() {
        let (project, clip) = singer_with(vec![
            sung(60, 0, 960, "ら"),
            sung(62, 480, 480, "ら"),
            sung(64, 960, 480, "ら"),
        ]);
        let found: Vec<_> = lint_score(&project)
            .into_iter()
            .filter(|d| d.rule == Rule::SingerOverlap)
            .collect();
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].start, found[0].end), (Ticks(480), Ticks(960)));
        assert_eq!(
            found[0].fix,
            Some(Fix::TrimNotes {
                clip,
                lengths: vec![(0, 480)]
            })
        );
    }

    #[test]
    fn a_wordless_sung_note_is_reported_alone_and_a_wordless_clip_once() {
        let (project, _) = singer_with(vec![
            sung(60, 0, 480, "あ"),
            sung(62, 480, 480, ""),
            sung(64, 960, 480, "い"),
        ]);
        let found: Vec<_> = lint_score(&project)
            .into_iter()
            .filter(|d| d.rule == Rule::MissingLyric)
            .collect();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].pitches, vec![62]);
        assert_eq!(found[0].severity, Severity::Warning);

        let (project, _) = singer_with((0..4).map(|i| sung(60, i * 480, 480, "")).collect());
        let found: Vec<_> = lint_score(&project)
            .into_iter()
            .filter(|d| d.rule == Rule::MissingLyric)
            .collect();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].value, Some(4.0));
        assert!(found[0].pitches.is_empty());
    }
}
