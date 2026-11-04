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

/// FNV-1a, so a bar's digest means the same thing in every build that reads the record back.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
        // A separator, so that ("ab","c") and ("a","bc") differ.
        self.0 ^= 0xff;
        self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
    }

    fn hex(&self) -> String {
        format!("{:08x}", self.0 as u32)
    }
}

/// A digest of each bar of the score, for telling which bars a return to the score changed.
///
/// `global` holds one 8-hex-digit digest per bar of what every track obeys there — key, chord,
/// section, tempo and meter, sampled every sixteenth. `tracks` holds one per bar for each note
/// track, of the notes that start in the bar, or `--------` where the track is silent. Strings
/// rather than arrays, because the record keeps them and they are written to disk in it.
pub fn score_bars(project: &Project) -> serde_json::Value {
    let end = project.end_tick();
    if end <= Ticks::ZERO {
        return serde_json::json!({"global":"","tracks":{}});
    }
    let bars = project.signatures.bar_beat_at(end - Ticks(1)).0;
    let ranges: Vec<(i64, i64)> = (1..=bars)
        .map(|bar| {
            (
                project.signatures.bar_start(bar).raw(),
                project.signatures.bar_start(bar + 1).raw(),
            )
        })
        .collect();
    let step = (Ticks::QUARTER.raw() / 4).max(1);
    let global: String = ranges
        .iter()
        .map(|&(start, end)| {
            let mut hash = Fnv::new();
            let mut tick = start;
            while tick < end {
                let at = Ticks(tick);
                hash.write(format!("{:?}", project.harmony.key_at(at)).as_bytes());
                hash.write(format!("{:?}", project.harmony.chord_at(at)).as_bytes());
                hash.write(project.sections.label_at(at).unwrap_or_default().as_bytes());
                hash.write(&project.tempo_map.bpm_at(at).to_bits().to_le_bytes());
                hash.write(format!("{:?}", project.signatures.signature_at(at)).as_bytes());
                tick += step;
            }
            hash.hex()
        })
        .collect();
    let tracks: serde_json::Map<String, serde_json::Value> = project
        .tracks
        .iter()
        .filter_map(|track| {
            let clips = track.kind.note_clips()?;
            let mut notes = Vec::new();
            for clip in clips {
                placed(clip, "", &mut notes);
            }
            let digests: String = ranges
                .iter()
                .map(|&(start, end)| {
                    let sounding = notes
                        .iter()
                        .any(|n| n.start < end && n.start + n.length.max(1) > start);
                    if !sounding {
                        return "--------".to_owned();
                    }
                    let mut hash = Fnv::new();
                    let mut starting: Vec<_> = notes
                        .iter()
                        .filter(|n| n.start >= start && n.start < end)
                        .collect();
                    starting.sort();
                    for note in starting {
                        hash.write(&(note.start - start).to_le_bytes());
                        hash.write(&note.length.to_le_bytes());
                        hash.write(&[note.pitch]);
                        hash.write(&note.velocity.to_le_bytes());
                        hash.write(note.lyric.as_bytes());
                        hash.write(note.drum_voice.as_bytes());
                    }
                    hash.hex()
                })
                .collect();
            Some((track.id.0.to_string(), serde_json::Value::String(digests)))
        })
        .collect();
    serde_json::json!({"global":global,"tracks":tracks})
}

/// 1-based inclusive bar ranges from a sorted list of bars.
fn bar_ranges(bars: &[u64]) -> Vec<[u64; 2]> {
    let mut ranges: Vec<[u64; 2]> = Vec::new();
    for &bar in bars {
        match ranges.last_mut() {
            Some(range) if range[1] + 1 == bar => range[1] = bar,
            _ => ranges.push([bar, bar]),
        }
    }
    ranges
}

/// What to look at again after a return to the score: the bars it changed, and every track
/// outside the foreground that sounds in them.
///
/// `before` is [`score_bars`] as production left for the score, `after` as it came back. A bar
/// changed when what every track obeys there changed, or a foreground track's notes did; a bar
/// only one side has is a change too. Backing notes are not compared, because rewriting them is
/// what the review asks for.
pub fn review(
    before: &serde_json::Value,
    after: &serde_json::Value,
    foreground: &[u64],
) -> serde_json::Value {
    let cells = |value: &serde_json::Value| -> Vec<String> {
        let text = value.as_str().unwrap_or_default();
        text.as_bytes()
            .chunks(8)
            .map(|cell| String::from_utf8_lossy(cell).into_owned())
            .collect()
    };
    let (old, new) = (cells(&before["global"]), cells(&after["global"]));
    let mut changed: Vec<u64> = (0..old.len().max(new.len()))
        .filter(|&i| old.get(i) != new.get(i))
        .map(|i| i as u64 + 1)
        .collect();
    for id in foreground {
        let (old, new) = (
            cells(&before["tracks"][id.to_string()]),
            cells(&after["tracks"][id.to_string()]),
        );
        changed.extend(
            (0..old.len().max(new.len()))
                .filter(|&i| old.get(i) != new.get(i))
                .map(|i| i as u64 + 1),
        );
    }
    changed.sort_unstable();
    changed.dedup();
    let tracks: Vec<_> = after["tracks"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(id, digests)| {
            let id: u64 = id.parse().ok()?;
            if foreground.contains(&id) {
                return None;
            }
            let cells = cells(digests);
            let bars: Vec<u64> = changed
                .iter()
                .copied()
                .filter(|bar| {
                    cells
                        .get(*bar as usize - 1)
                        .is_some_and(|cell| cell != "--------")
                })
                .collect();
            (!bars.is_empty()).then(|| serde_json::json!({"track":id,"bars":bar_ranges(&bars)}))
        })
        .collect();
    serde_json::json!({
        "changed_bars": bar_ranges(&changed),
        "tracks_to_review": tracks,
        "instruction": "The score changed in these bars since production left the arrangement. Read each listed track over its bars and fix what no longer fits the new chords, melody or timing: regenerate generated parts, rewrite authored ones. Re-sing changed vocal takes.",
    })
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
    fn a_changed_melody_bar_lists_the_backing_that_plays_there() {
        let (mut project, lead, pad) = song();
        let before = score_bars(&project);
        assert_eq!(before["global"].as_str().unwrap().len(), 16);
        let lead_clip = project.tracks[0].kind.note_clips().unwrap()[0].id;
        project.midi_clip_mut(lead_clip).unwrap().notes[6].pitch = 76;
        let after = score_bars(&project);
        let report = review(&before, &after, &[lead]);
        assert_eq!(report["changed_bars"], serde_json::json!([[2, 2]]));
        assert_eq!(
            report["tracks_to_review"],
            serde_json::json!([{"track":pad,"bars":[[2,2]]}])
        );
        // Rewriting the backing itself is what the review asks for, not a reason to list it.
        let pad_clip = project.tracks[1].kind.note_clips().unwrap()[0].id;
        project.midi_clip_mut(pad_clip).unwrap().notes[0].pitch = 62;
        assert_eq!(
            review(&after, &score_bars(&project), &[lead])["changed_bars"],
            serde_json::json!([])
        );
    }

    #[test]
    fn a_withdrawn_job_or_step_is_undone_like_a_command() {
        let mut session =
            crate::Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
        let lead = session.add_default_instrument_track("Lead").unwrap();
        session
            .agent_command(crate::live_agent::Command::EditHarmony {
                start_bar: 1,
                bars: 2,
                key: Some("C major".into()),
                chords: Some("| I | V |".into()),
                section: None,
                clear_chords: false,
                clear_key: false,
                clear_section: false,
            })
            .unwrap();
        let start = session.project().clone();
        let lock = Lock::Arrangement {
            foreground: vec![lead.0],
        };
        let phrase = || crate::live_agent::Command::GeneratePhrase {
            track: lead.0,
            part: "lead".into(),
            start_bar: 1,
            bars: 2,
            seed: 42,
        };
        let error = session
            .agent_locked(Some(&lock), |session| session.agent_command(phrase()))
            .unwrap_err();
        assert!(error.contains("withdrawn"), "{error}");
        assert_eq!(session.project(), &start);
        // Without a lock nothing is compared.
        session
            .agent_locked(None, |session| session.agent_command(phrase()))
            .unwrap();
        assert_ne!(session.project(), &start);
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
