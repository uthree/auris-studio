//! Restricts proposal enumeration without trimming the project that will be rendered or adopted.

use auris_core::Project;

use super::ReferenceMatchSettings;

pub(super) fn enumeration_project(
    original: &Project,
    settings: &ReferenceMatchSettings,
) -> Project {
    let excerpt_start = settings.project_start_seconds;
    let excerpt_end = excerpt_start + settings.duration_seconds;
    let audible = original.solo_resolution();
    let has_effects = !original.master.effects.is_empty()
        || original
            .tracks
            .iter()
            .any(|track| !track.mixer.effects.is_empty());
    let overlaps = |start, end, retain_tails| {
        start < excerpt_end && end > start && (end > excerpt_start || retain_tails)
    };
    let mut enumeration = original.clone();
    for (track, audible) in enumeration.tracks.iter_mut().zip(audible) {
        track.mixer.mute |= !audible;
        if let Some(clips) = track.kind.note_clips_mut() {
            clips.retain(|clip| {
                !clip.muted
                    && overlaps(
                        original.tempo_map.ticks_to_seconds(clip.start).0,
                        original.tempo_map.ticks_to_seconds(clip.sounding_end()).0,
                        // Instrument release and routed effect tails have no universal duration
                        // bound. Earlier clips remain candidates; future clips cannot contribute.
                        true,
                    )
            });
        }
        if let Some(audio) = track.kind.as_audio_mut() {
            audio.clips.retain(|clip| {
                !clip.muted
                    && overlaps(
                        original.tempo_map.ticks_to_seconds(clip.start).0,
                        original
                            .tempo_map
                            .ticks_to_seconds(clip.start + original.audio_clip_sounding_ticks(clip))
                            .0,
                        has_effects,
                    )
            });
        }
    }
    enumeration.tracks.retain(|track| {
        !track.mixer.mute
            && (track
                .kind
                .note_clips()
                .is_some_and(|clips| !clips.is_empty())
                || track
                    .kind
                    .as_audio()
                    .is_some_and(|audio| !audio.clips.is_empty()))
    });
    enumeration
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::{AssetPath, Note, Ticks};

    fn settings(start: f64, duration: f64) -> ReferenceMatchSettings {
        ReferenceMatchSettings {
            project_start_seconds: start,
            duration_seconds: duration,
            ..ReferenceMatchSettings::default()
        }
    }

    #[test]
    fn future_and_muted_material_does_not_consume_excerpt_proposals() {
        let mut project = Project::new("Excerpt", 48_000.0);
        project.tempo_map.set_point(Ticks::QUARTER, 60.0);
        let track = project.add_instrument_track("Lead", auris_synth::Chiptune::ID);
        let early = project
            .add_midi_clip(track, "Now", Ticks::ZERO, Ticks::from_beats(2.0))
            .unwrap();
        let future = project
            .add_midi_clip(
                track,
                "Later",
                Ticks::from_beats(2.0),
                Ticks::from_beats(2.0),
            )
            .unwrap();
        let muted = project
            .add_midi_clip(track, "Muted", Ticks::ZERO, Ticks::from_beats(2.0))
            .unwrap();
        project.midi_clip_mut(muted).unwrap().muted = true;
        let original = project.clone();
        let filtered = enumeration_project(&project, &settings(0.0, 1.25));
        assert!(filtered.midi_clip(early).is_some());
        assert!(filtered.midi_clip(future).is_none());
        assert!(filtered.midi_clip(muted).is_none());
        assert_eq!(project, original);
    }

    #[test]
    fn loops_and_prior_release_tails_remain_eligible() {
        let mut project = Project::new("Excerpt", 48_000.0);
        let track = project.add_instrument_track("Lead", auris_synth::Chiptune::ID);
        let looped = project
            .add_midi_clip(track, "Loop", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        project.midi_clip_mut(looped).unwrap().loop_end = Ticks::from_beats(8.0);
        let prior = project
            .add_midi_clip(track, "Release", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        let filtered = enumeration_project(&project, &settings(2.0, 1.0));
        assert!(filtered.midi_clip(looped).is_some());
        assert!(filtered.midi_clip(prior).is_some());
    }

    #[test]
    fn dry_audio_uses_its_sample_length_and_repeated_extent() {
        let mut project = Project::new("Excerpt", 48_000.0);
        let track = project.add_audio_track("Audio");
        let source = project.add_audio_source(
            "Tone",
            AssetPath::external("/tone.wav"),
            48_000,
            48_000.0,
            2,
        );
        let expired = project.add_audio_clip(track, source, Ticks::ZERO).unwrap();
        let looped = project.add_audio_clip(track, source, Ticks::ZERO).unwrap();
        project.audio_clip_mut(looped).unwrap().loop_end = Ticks::from_beats(8.0);
        let filtered = enumeration_project(&project, &settings(2.0, 1.0));
        assert!(filtered.audio_clip(expired).is_none());
        assert!(filtered.audio_clip(looped).is_some());
    }

    #[test]
    fn solo_and_future_tracks_do_not_create_search_families() {
        let mut session = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        let heard = session.add_default_instrument_track("Heard").unwrap();
        let hidden = session.add_default_instrument_track("Unheard").unwrap();
        for track in [heard, hidden] {
            let clip = session
                .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER)
                .unwrap();
            session
                .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
        }
        session.project.track_mut(heard).unwrap().mixer.solo = true;
        let filtered = enumeration_project(&session.project, &settings(0.0, 1.0));
        assert_eq!(filtered.tracks.len(), 1);
        assert_eq!(filtered.tracks[0].id, heard);
        let late = session
            .project
            .track_mut(heard)
            .unwrap()
            .kind
            .note_clips_mut()
            .unwrap();
        late[0].start = Ticks::from_beats(20.0);
        assert!(super::super::search_families(&session, &settings(0.0, 1.0)).is_empty());
    }

    #[test]
    fn excerpt_performance_changes_only_captured_clips_in_the_full_candidate() {
        use super::super::{Control, gate, search_dials};

        let mut original = Project::new("Excerpt performance", 48_000.0);
        let track = original.add_instrument_track("Lead", auris_synth::Chiptune::ID);
        let heard = original
            .add_midi_clip(track, "Heard", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        let future = original
            .add_midi_clip(track, "Future", Ticks::from_beats(20.0), Ticks::QUARTER)
            .unwrap();
        for id in [heard, future] {
            original.midi_clip_mut(id).unwrap().notes.push(Note::new(
                60,
                Ticks::ZERO,
                Ticks::QUARTER,
            ));
        }
        let settings = settings(0.0, 1.0);
        let filtered = enumeration_project(&original, &settings);
        let dial = search_dials(&filtered, &settings)
            .into_iter()
            .find(|dial| matches!(dial.control, Control::Gate))
            .unwrap();
        let mut candidate = original.clone();
        dial.adjust(&mut candidate, &original, -1.0);
        assert_eq!(candidate.tracks.len(), original.tracks.len());
        assert_eq!(candidate.midi_clip(future), original.midi_clip(future));
        let adjusted = candidate.midi_clip(heard).unwrap().1;
        assert_eq!(adjusted.notes, original.midi_clip(heard).unwrap().1.notes);
        assert_eq!(gate(adjusted), 0.95);

        // Explicit enumeration of a full project still addresses all its eligible clips.
        let whole = search_dials(&original, &settings)
            .into_iter()
            .find(|dial| matches!(dial.control, Control::Gate))
            .unwrap();
        whole.adjust(&mut candidate, &original, -1.0);
        assert_eq!(gate(candidate.midi_clip(future).unwrap().1), 0.95);
    }
}
