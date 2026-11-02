//! Bounded, read-only evidence of lyrics throughout a singer's planned form.

use auris_core::{Project, SingerTrack, Ticks};
use serde_json::{Value, json};

const PAGE_SIZE: usize = 32;
const NOTE_CHECK_BUDGET: usize = 100_000;

pub(super) fn sections(
    project: &Project,
    singer: &SingerTrack,
    offset: usize,
) -> Result<Value, String> {
    sections_with_budget(project, singer, offset, NOTE_CHECK_BUDGET)
}

fn sections_with_budget(
    project: &Project,
    singer: &SingerTrack,
    offset: usize,
    mut budget: usize,
) -> Result<Value, String> {
    // Explicit section endings expose planned later verses even before their clips exist.
    let end = project.end_tick().max(project.sections.end());
    let spans = project.sections.spans_in(Ticks::ZERO, end);
    if offset > spans.len() {
        return Err("section_offset exceeds section count".into());
    }
    let unbounded_last_section = project
        .sections
        .points()
        .last()
        .is_some_and(|p| p.label.is_some() && p.tick >= end);
    let mut page = Vec::new();
    for span in spans.iter().skip(offset).take(PAGE_SIZE) {
        let mut notes = 0_u64;
        let mut lyric_notes = 0_u64;
        let mut phoneme_notes = 0_u64;
        let mut continuation_notes = 0_u64;
        let mut unassigned_notes = 0_u64;
        let mut looped_notes = 0_u64;
        let mut complete = true;
        'clips: for clip in &singer.clips {
            if clip.muted || clip.length <= Ticks::ZERO {
                continue;
            }
            let clip_start = i128::from(clip.start.0);
            let period = i128::from(clip.length.0);
            let clip_end = clip_start + i128::from(clip.sounding_length().0);
            if clip_end <= i128::from(span.start.0) || clip_start >= i128::from(span.end.0) {
                continue;
            }
            for note in &clip.notes {
                if budget == 0 {
                    complete = false;
                    break 'clips;
                }
                budget -= 1;
                if note.start < Ticks::ZERO
                    || note.start >= clip.length
                    || note.length <= Ticks::ZERO
                {
                    continue;
                }
                let first = clip_start + i128::from(note.start.0);
                let lower = (i128::from(span.start.0) - first).max(0);
                let upper = i128::from(span.end.0).min(clip_end) - first;
                if upper <= lower {
                    continue;
                }
                // Count loop attacks arithmetically; never expand a long clip loop in the UI.
                let first_pass = (lower + period - 1) / period;
                let last_pass = (upper - 1) / period;
                if last_pass < first_pass {
                    continue;
                }
                let count = (last_pass - first_pass + 1) as u64;
                notes += count;
                looped_notes += count - u64::from(first_pass == 0);
                let lyric = note.lyric.trim();
                let continuation = lyric == crate::session::LYRIC_CONTINUATION;
                lyric_notes += u64::from(!lyric.is_empty() && !continuation) * count;
                continuation_notes += u64::from(continuation) * count;
                phoneme_notes += u64::from(!note.phonemes.is_empty()) * count;
                unassigned_notes += u64::from(lyric.is_empty() && note.phonemes.is_empty()) * count;
            }
        }
        page.push(json!({
            "label":span.label,"instance":span.instance,
            "start_tick":span.start.0,"end_tick":span.end.0,
            "start_bar":project.signatures.bar_beat_at(span.start).0,
            "note_attacks":notes,"lyric_notes":lyric_notes,"phoneme_notes":phoneme_notes,
            "continuation_notes":continuation_notes,"unassigned_notes":unassigned_notes,
            "looped_note_attacks":looped_notes,"analysis_complete":complete,
            "lyric_annotation_lines":project.lyrics.spans_in(span.start, span.end).len()
        }));
    }
    let next = offset + page.len();
    Ok(json!({
        "sections":page,"total_sections":spans.len(),
        "next_section_offset":(next < spans.len()).then_some(next),
        "unbounded_last_section":unbounded_last_section,
        "interpretation":"Authored onsets in unmuted clips, including counted loop repeats; sustained notes are counted only where they begin. Counts do not prove a complete verse, valid pronunciation, audibility or musical quality. Compare every planned sung section with read_notes; instrumental sections may have no vocals. Looped words are reused lyrics, not a newly written verse. analysis_complete=false means partial counts, not verified absence. A current take verifies synthesis freshness only. lyric_annotation_lines counts song-wide lyrics annotation lines in the section; a section with lyric_notes needs at least one (edit_lyrics)."
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Session, SessionOptions, live_agent::Command};
    use auris_core::Note;

    #[test]
    fn inspection_exposes_empty_later_verse_and_unfilled_final_chorus_without_editing() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session.add_singer_track("Lead");
        let bar = Ticks::QUARTER * 4;
        for (tick, label) in [
            (0, Some("Verse 1")),
            (1, Some("Verse 2")),
            (2, Some("Final chorus")),
            (3, None),
        ] {
            session.set_section(bar * tick, label.map(str::to_owned));
        }
        let first = session
            .add_midi_clip(track, "First", Ticks::ZERO, bar)
            .unwrap();
        session
            .add_note(first, Note::new(60, Ticks::ZERO, bar))
            .unwrap();
        session.set_note_lyric(first, 0, "あ").unwrap();
        let last = session.add_midi_clip(track, "Final", bar * 2, bar).unwrap();
        session
            .add_note(last, Note::new(62, Ticks::ZERO, bar))
            .unwrap();
        let before = session.project().clone();
        let inspect = |s: &mut Session| -> Value {
            serde_json::from_str(
                &s.agent_command(Command::InspectSinger {
                    track: track.0,
                    offset: 0,
                    section_offset: 0,
                })
                .unwrap(),
            )
            .unwrap()
        };
        let report = inspect(&mut session);
        let rows = &report["section_coverage"]["sections"];
        assert_eq!(rows[0]["lyric_notes"], 1);
        assert_eq!(rows[1]["note_attacks"], 0);
        assert_eq!(rows[2]["unassigned_notes"], 1);
        assert_eq!(report["take_state"], "missing");
        assert_eq!(session.project(), &before);
        session.set_note_lyric(last, 0, "い").unwrap();
        assert_eq!(
            inspect(&mut session)["section_coverage"]["sections"][2]["lyric_notes"],
            1
        );
        session.undo();
        assert_eq!(session.project(), &before);
    }

    #[test]
    fn loops_boundaries_muting_and_paging_do_not_turn_repeats_into_new_lyrics() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session.add_singer_track("Lead");
        let bar = Ticks::QUARTER * 4;
        for index in 0..35 {
            session.set_section(bar * index, Some(format!("Section {index}")));
        }
        session.set_section(bar * 35, None);
        let clip = session
            .add_midi_clip(track, "Loop", Ticks::ZERO, bar)
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, bar * 2))
            .unwrap();
        session.set_note_lyric(clip, 0, "あ").unwrap();
        let singer = session
            .project()
            .track(track)
            .unwrap()
            .kind
            .as_singer()
            .unwrap();
        let before_looping = sections(session.project(), singer, 0).unwrap();
        assert_eq!(before_looping["total_sections"], 35);
        assert_eq!(before_looping["sections"][0]["note_attacks"], 1);
        assert_eq!(before_looping["sections"][1]["note_attacks"], 0);
        let mut project = session.project().clone();
        project.midi_clip_mut(clip).unwrap().length = bar;
        project.midi_clip_mut(clip).unwrap().loop_end = bar * 35;
        let singer = project.track(track).unwrap().kind.as_singer().unwrap();
        let report = sections(&project, singer, 0).unwrap();
        assert_eq!(report["sections"][0]["looped_note_attacks"], 0);
        assert_eq!(report["sections"][1]["looped_note_attacks"], 1);
        assert_eq!(report["next_section_offset"], 32);
        assert_eq!(
            sections(&project, singer, 32).unwrap()["sections"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert!(sections(&project, singer, 36).is_err());
        let partial = sections_with_budget(&project, singer, 0, 1).unwrap();
        assert_eq!(partial["sections"][1]["analysis_complete"], false);
        project.midi_clip_mut(clip).unwrap().muted = true;
        let singer = project.track(track).unwrap().kind.as_singer().unwrap();
        assert_eq!(
            sections(&project, singer, 0).unwrap()["sections"][1]["note_attacks"],
            0
        );
    }
}
