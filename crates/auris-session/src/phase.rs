//! What a later production phase keeps fixed, and the check that keeps it so.
//!
//! The arrangement keeps the score: the foreground tracks' notes, the harmony, the sections, the
//! tempo map and the meter. The mix keeps every note, every track's sound and every clip's
//! performance as well. Rather than a table of which tool may change what — which would be out
//! of date with the next tool — an agent's edit is run and then compared: the [`Print`] of the
//! locked material before and after must be equal, or the edit is withdrawn.
//!
//! A note is printed where it sounds on the timeline, loops unrolled, so splitting, joining or
//! re-looping a clip without changing what plays is not a change. Muted clips print nothing:
//! muting a locked phrase changes what is heard.

use auris_core::{
    Project,
    project::{MidiClip, TrackKind, loop_passes},
    time::Ticks,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The material a phase holds fixed while an agent edits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Lock {
    /// The score: the foreground tracks' notes and lyrics, harmony, sections, tempo and meter.
    Arrangement {
        /// Registered foreground track IDs.
        foreground: Vec<u64>,
    },
    /// The score plus every note, every track's sound and every clip's performance.
    Mix,
}

/// One note as the timeline plays it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Placed {
    start: i64,
    length: i64,
    pitch: u8,
    velocity: u32,
    lyric: String,
    drum_voice: String,
    /// The clip's performance, serialized; empty unless the mix is locked.
    performance: String,
}

/// A track's locked content.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
struct TrackPrint {
    notes: Vec<Placed>,
    /// Instrument, sample file, followed source and voice; empty unless the mix is locked.
    sound: String,
}

/// The locked material of one document, comparable with another's.
#[derive(Clone, Debug, PartialEq)]
pub struct Print {
    tempo: auris_core::time::TempoMap,
    meter: auris_core::time::SignatureMap,
    harmony: auris_core::harmony::Harmony,
    sections: auris_core::structure::SectionMap,
    tracks: BTreeMap<u64, TrackPrint>,
}

/// What an edit changed that the lock holds fixed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Change {
    /// The tempo map.
    Tempo,
    /// The bar/beat grid.
    Meter,
    /// The key or the chords.
    Harmony,
    /// The section map.
    Sections,
    /// A locked track's notes, lyrics or (in the mix) performance, over these 1-based bars.
    Notes {
        /// Track ID.
        track: u64,
        /// First changed bar.
        first_bar: u64,
        /// Last changed bar, inclusive.
        last_bar: u64,
    },
    /// A locked track's instrument, sample file or voice, or the track itself was removed.
    Sound {
        /// Track ID.
        track: u64,
    },
    /// A locked track was deleted.
    TrackRemoved {
        /// Track ID.
        track: u64,
    },
}

fn placed(clip: &MidiClip, performance: &str, out: &mut Vec<Placed>) {
    if clip.muted {
        return;
    }
    for (offset, span) in loop_passes(clip.length, clip.loop_end) {
        for note in clip.playable_notes().filter(|note| note.start < span) {
            out.push(Placed {
                start: (clip.start + offset + note.start).raw(),
                length: note.length.min(span - note.start).raw(),
                pitch: note.pitch,
                velocity: note.velocity.to_bits(),
                lyric: note.lyric.clone(),
                drum_voice: note.drum_voice.clone(),
                performance: performance.to_owned(),
            });
        }
    }
}

fn sound(kind: &TrackKind) -> String {
    let value = match kind {
        TrackKind::Instrument(track) | TrackKind::Drum(track) => {
            serde_json::json!([track.instrument_id, track.file, track.follows])
        }
        TrackKind::Singer(track) => {
            serde_json::json!([track.instrument_id, track.voice.as_ref().map(|v| &v.path)])
        }
        _ => serde_json::Value::Null,
    };
    value.to_string()
}

/// Print the material `lock` holds fixed in `project`.
pub fn print(project: &Project, lock: &Lock) -> Print {
    let mix = matches!(lock, Lock::Mix);
    let tracks = project
        .tracks
        .iter()
        .filter(|track| match lock {
            Lock::Arrangement { foreground } => foreground.contains(&track.id.0),
            Lock::Mix => track.kind.note_clips().is_some(),
        })
        .map(|track| {
            let mut notes = Vec::new();
            for clip in track.kind.note_clips().into_iter().flatten() {
                let performance = if mix {
                    serde_json::to_string(&clip.transforms).unwrap_or_default()
                } else {
                    String::new()
                };
                placed(clip, &performance, &mut notes);
            }
            notes.sort();
            let sound = if mix {
                sound(&track.kind)
            } else {
                String::new()
            };
            (track.id.0, TrackPrint { notes, sound })
        })
        .collect();
    Print {
        tempo: project.tempo_map.clone(),
        meter: project.signatures.clone(),
        harmony: project.harmony.clone(),
        sections: project.sections.clone(),
        tracks,
    }
}

/// The 1-based bars spanned by the notes that differ between two sorted note lists.
fn changed_span(before: &[Placed], after: &[Placed]) -> Option<(i64, i64)> {
    let mut span: Option<(i64, i64)> = None;
    let mut widen = |note: &Placed| {
        let end = note.start + note.length.max(1) - 1;
        span = Some(span.map_or((note.start, end), |(a, b)| (a.min(note.start), b.max(end))));
    };
    let (mut i, mut j) = (0, 0);
    while i < before.len() || j < after.len() {
        match (before.get(i), after.get(j)) {
            (Some(a), Some(b)) if a == b => {
                i += 1;
                j += 1;
            }
            (Some(a), Some(b)) if a < b => {
                widen(a);
                i += 1;
            }
            (Some(_), Some(b)) => {
                widen(b);
                j += 1;
            }
            (Some(a), None) => {
                widen(a);
                i += 1;
            }
            (None, Some(b)) => {
                widen(b);
                j += 1;
            }
            (None, None) => break,
        }
    }
    span
}

/// What `after` changed of the material printed in `before`, located in `project`'s bars.
pub fn changes(before: &Print, after: &Print, project: &Project) -> Vec<Change> {
    let bar = |tick: i64| u64::from(project.signatures.bar_beat_at(Ticks(tick)).0);
    let mut changes = Vec::new();
    if before.tempo != after.tempo {
        changes.push(Change::Tempo);
    }
    if before.meter != after.meter {
        changes.push(Change::Meter);
    }
    if before.harmony != after.harmony {
        changes.push(Change::Harmony);
    }
    if before.sections != after.sections {
        changes.push(Change::Sections);
    }
    for (id, old) in &before.tracks {
        let Some(new) = after.tracks.get(id) else {
            changes.push(Change::TrackRemoved { track: *id });
            continue;
        };
        if let Some((first, last)) = changed_span(&old.notes, &new.notes) {
            changes.push(Change::Notes {
                track: *id,
                first_bar: bar(first),
                last_bar: bar(last),
            });
        }
        if old.sound != new.sound {
            changes.push(Change::Sound { track: *id });
        }
    }
    changes
}

/// The refusal an agent reads when its edit was withdrawn: what changed and how to change it.
pub fn refusal(lock: &Lock, changes: &[Change]) -> String {
    let (phase, back) = match lock {
        Lock::Arrangement { .. } => (
            "arrangement",
            "The score is locked in the arrangement: foreground notes and lyrics, harmony, sections, tempo and meter. If the user asked for this change, call production_record enter_phase score with their words as the reason, make it there, then enter_phase arrangement again.",
        ),
        Lock::Mix => (
            "mix",
            "Notes, sounds and performance are locked in the mix, as is the score. If the user asked for this change, call production_record enter_phase arrangement (or score for the score) with their words as the reason, make it there, then return to the mix.",
        ),
    };
    serde_json::json!({
        "withdrawn": true,
        "reason": format!("This edit changed material the {phase} phase keeps fixed, so it was withdrawn and the document is as it was before the call."),
        "changed": changes,
        "next": back,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::project::Note;

    fn note(start: i64, pitch: u8) -> Note {
        Note::new(pitch, Ticks(start), Ticks::QUARTER)
    }

    fn song() -> (Project, u64, u64) {
        let mut project = Project::default();
        let lead = project.add_instrument_track("Lead", "builtin.square");
        let pad = project.add_instrument_track("Pad", "builtin.square");
        let bar = Ticks::QUARTER * 4;
        for (track, pitch) in [(lead, 72), (pad, 60)] {
            let clip = project
                .add_midi_clip(track, "Phrase", Ticks::ZERO, bar * 2)
                .unwrap();
            let notes = &mut project.midi_clip_mut(clip).unwrap().notes;
            notes.extend((0..8).map(|i| note(Ticks::QUARTER.raw() * i, pitch)));
        }
        (project, lead.0, pad.0)
    }

    #[test]
    fn the_arrangement_may_change_backing_but_not_the_foreground() {
        let (mut project, lead, pad) = song();
        let lock = Lock::Arrangement {
            foreground: vec![lead],
        };
        let before = print(&project, &lock);
        let pad_clip = project.tracks[1].kind.note_clips().unwrap()[0].id;
        project.midi_clip_mut(pad_clip).unwrap().notes[0].pitch = 62;
        assert!(changes(&before, &print(&project, &lock), &project).is_empty());

        let lead_clip = project.tracks[0].kind.note_clips().unwrap()[0].id;
        project.midi_clip_mut(lead_clip).unwrap().notes[5].pitch = 74;
        assert_eq!(
            changes(&before, &print(&project, &lock), &project),
            vec![Change::Notes {
                track: lead,
                first_bar: 2,
                last_bar: 2
            }]
        );
        assert!(pad != lead);
    }

    #[test]
    fn splitting_a_foreground_clip_without_changing_what_plays_is_not_a_change() {
        let (mut project, lead, _) = song();
        let lock = Lock::Arrangement {
            foreground: vec![lead],
        };
        let before = print(&project, &lock);
        let clip = project.tracks[0].kind.note_clips().unwrap()[0].clone();
        let bar = Ticks::QUARTER * 4;
        let (first, second): (Vec<_>, Vec<_>) =
            clip.notes.iter().cloned().partition(|n| n.start < bar);
        let clips = project.tracks[0].kind.note_clips_mut().unwrap();
        clips[0].length = bar;
        clips[0].notes = first;
        let mut tail = clip.clone();
        tail.id = auris_core::project::ClipId(9_999);
        tail.start = bar;
        tail.length = bar;
        tail.notes = second
            .into_iter()
            .map(|n| Note {
                start: n.start - bar,
                ..n
            })
            .collect();
        clips.push(tail);
        assert!(changes(&before, &print(&project, &lock), &project).is_empty());
    }

    #[test]
    fn muting_or_deleting_the_foreground_and_retiming_the_song_are_changes() {
        let (mut project, lead, _) = song();
        let lock = Lock::Arrangement {
            foreground: vec![lead],
        };
        let before = print(&project, &lock);
        project.tracks[0].kind.note_clips_mut().unwrap()[0].muted = true;
        assert!(matches!(
            changes(&before, &print(&project, &lock), &project)[..],
            [Change::Notes {
                first_bar: 1,
                last_bar: 2,
                ..
            }]
        ));
        project.tracks[0].kind.note_clips_mut().unwrap()[0].muted = false;
        project.tempo_map = auris_core::time::TempoMap::constant(90.0);
        assert_eq!(
            changes(&before, &print(&project, &lock), &project),
            vec![Change::Tempo]
        );
        project.tracks.remove(0);
        assert!(
            changes(&before, &print(&project, &lock), &project)
                .contains(&Change::TrackRemoved { track: lead })
        );
    }

    #[test]
    fn the_mix_locks_every_note_sound_and_performance_but_not_the_fader() {
        let (mut project, _, pad) = song();
        let lock = Lock::Mix;
        let before = print(&project, &lock);
        project.tracks[1].mixer.gain_db = -6.0;
        assert!(changes(&before, &print(&project, &lock), &project).is_empty());
        project.tracks[1]
            .kind
            .as_instrument_mut()
            .unwrap()
            .instrument_id = "builtin.saw".into();
        assert_eq!(
            changes(&before, &print(&project, &lock), &project),
            vec![Change::Sound { track: pad }]
        );
    }

    #[test]
    fn a_withdrawn_edit_leaves_the_requests_earlier_edits_and_its_single_undo_step() {
        use crate::live_agent::Command;
        let mut session =
            crate::Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
        let lead = session.add_default_instrument_track("Lead").unwrap();
        session
            .add_midi_clip(lead, "Phrase", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        let start = session.project().clone();
        let lock = Lock::Arrangement {
            foreground: vec![lead.0],
        };
        session.begin_agent_request();
        let add: Command = serde_json::from_value(
            serde_json::json!({"action":"add_track","name":"Bass","kind":"instrument"}),
        )
        .unwrap();
        session.agent_command_locked(add, Some(&lock), &[]).unwrap();
        let arranged = session.project().clone();
        assert_eq!(arranged.tracks.len(), start.tracks.len() + 1);

        let harmony = Command::EditHarmony {
            start_bar: 1,
            bars: 1,
            key: Some("C major".into()),
            chords: Some("| I |".into()),
            section: None,
            clear_chords: false,
            clear_key: false,
            clear_section: false,
        };
        let refusal = session
            .agent_command_locked(harmony, Some(&lock), &[])
            .unwrap_err();
        assert!(refusal.contains("\"withdrawn\":true"), "{refusal}");
        assert_eq!(session.project(), &arranged);

        session.end_agent_request();
        session.undo();
        assert_eq!(session.project(), &start);
    }

    #[test]
    fn a_refusal_names_the_way_back() {
        let text = refusal(
            &Lock::Arrangement {
                foreground: vec![1],
            },
            &[Change::Harmony],
        );
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["withdrawn"], true);
        assert_eq!(value["changed"][0]["kind"], "harmony");
        assert!(
            value["next"]
                .as_str()
                .unwrap()
                .contains("enter_phase score")
        );
    }
}
