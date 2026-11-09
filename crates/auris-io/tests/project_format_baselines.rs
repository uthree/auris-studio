//! Historical files are fixed inputs: these tests must not recreate them with today's serializer.

use auris_core::project::CurvePoint;
use auris_core::{
    AssetPath, AutomationCurve, ClipId, ClipPreset, DrumMap, DrumRole, EffectSlotId, Note,
    NoteTransform, Output, ParamId, ParamTarget, Project, SendId, SoundFontId, SourceId, Ticks,
    TrackId,
};
use auris_io::{load_project, save_project};
use serde_json::Value;

const FIXTURES: [(u32, &str); 6] = [
    (27, include_str!("fixtures/project_formats/v27.auris")),
    (28, include_str!("fixtures/project_formats/v28.auris")),
    (29, include_str!("fixtures/project_formats/v29.auris")),
    (30, include_str!("fixtures/project_formats/v30.auris")),
    (31, include_str!("fixtures/project_formats/v31.auris")),
    (32, include_str!("fixtures/project_formats/v32.auris")),
];

fn load_fixture(text: &str) -> Project {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Fixture.auris");
    std::fs::write(&path, text).unwrap();
    load_project(&path).unwrap()
}

#[test]
fn fixtures_retain_the_historical_shapes_and_version_specific_harmony() {
    for (version, text) in FIXTURES {
        let raw: Value = serde_json::from_str(text).unwrap();
        assert_eq!(raw["format_version"], version);
        let lead = &raw["tracks"][0];
        assert_eq!(lead.get("output").is_some(), version < 32);
        assert_eq!(lead.get("outputs").is_some(), version >= 32);
        let pitch = &lead["kind"]["clips"][0]["transforms"][2]["settings"];
        assert_eq!(pitch.get("volume_contour").is_some(), version >= 28);
        let map = &raw["tracks"][1]["kind"]["instrument_state"]["extra"][DrumMap::STATE_KEY];
        assert_eq!(map.get("lanes").is_some(), version >= 29);
        let expected_chord = match version {
            31.. => "V13",
            30 => "V11",
            _ => "V7",
        };
        assert_eq!(
            raw["harmony"]["chords"]["points"][0]["chord"],
            expected_chord
        );
    }
}

#[test]
fn opening_historical_files_keeps_the_original_bytes() {
    for (version, text) in FIXTURES {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Original.auris");
        std::fs::write(&path, text).unwrap();
        let loaded = load_project(&path).unwrap();
        assert_eq!(loaded.format_version, Project::FORMAT_VERSION);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            text,
            "format {version}"
        );
    }
}

#[test]
fn authored_notes_clip_timing_and_post_note_sustain_survive_loading() {
    for (version, text) in FIXTURES {
        let project = load_fixture(text);
        let (owner, clip) = project.midi_clip(ClipId(2)).unwrap();
        assert_eq!(owner, TrackId(1));
        assert_eq!(
            (clip.start, clip.length, clip.loop_end),
            (Ticks(960), Ticks(3840), Ticks(7680))
        );
        assert!(clip.length_is_explicit);
        let mut second = Note::new(67, Ticks(1920), Ticks(960));
        second.velocity = 0.5;
        assert_eq!(
            clip.notes,
            vec![Note::new(60, Ticks::ZERO, Ticks(1440)), second],
            "format {version}"
        );
        assert_eq!(
            clip.bend,
            vec![
                CurvePoint {
                    at: Ticks::ZERO,
                    value: 0.0
                },
                CurvePoint {
                    at: Ticks(960),
                    value: 2.0
                }
            ]
        );
        assert_eq!(
            clip.controllers[&64],
            vec![
                CurvePoint {
                    at: Ticks::ZERO,
                    value: 1.0
                },
                CurvePoint {
                    at: Ticks(3360),
                    value: 0.0
                }
            ]
        );
    }
}

#[test]
fn recipe_and_ordered_seeded_performance_are_preserved_without_regeneration() {
    for (version, text) in FIXTURES {
        let project = load_fixture(text);
        let clip = project.midi_clip(ClipId(2)).unwrap().1;
        let recipe = clip.recipe.as_ref().unwrap();
        assert_eq!((recipe.preset, recipe.seed), (ClipPreset::Lead, 1234));
        assert_eq!(clip.transforms.len(), 3);
        assert_eq!(
            clip.transforms[0],
            NoteTransform::Humanize {
                amount: 0.25,
                seed: 42
            }
        );
        assert_eq!(
            clip.transforms[1],
            NoteTransform::Transpose { semitones: 12 }
        );
        let NoteTransform::Pitch { settings } = &clip.transforms[2] else {
            panic!("format {version} lost the pitch stage");
        };
        assert_eq!(settings.volume_swell, 0.7);
        assert_eq!(settings.volume_contour, Default::default());
        let kit = project
            .track(TrackId(3))
            .unwrap()
            .kind
            .as_instrument()
            .unwrap();
        let map = DrumMap::load(&kit.instrument_state).unwrap();
        assert_eq!(map.voices.get(&DrumRole::Kick), Some(&73));
        assert_eq!(map.lanes.len(), 1);
        assert_eq!(map.lanes[0].note, 73);
    }
}

#[test]
fn routing_preserves_parallel_outputs_empty_outputs_and_send_taps() {
    for (version, text) in FIXTURES {
        let project = load_fixture(text);
        let lead = project.track(TrackId(1)).unwrap();
        let expected = if version < 32 {
            vec![Output::Bus(TrackId(12))]
        } else {
            vec![Output::Master, Output::Bus(TrackId(12))]
        };
        assert_eq!(lead.outputs, expected, "format {version}");
        let audio = project.track(TrackId(5)).unwrap();
        assert_eq!(
            audio.outputs,
            if version < 32 {
                vec![Output::Master]
            } else {
                vec![]
            }
        );
        assert_eq!((lead.mixer.gain_db, lead.mixer.pan), (-6.0, -0.25));
        let sends: Vec<_> = lead
            .sends
            .iter()
            .map(|s| (s.id, s.target, s.level_db, s.pre_fader))
            .collect();
        assert_eq!(
            sends,
            vec![
                (SendId(17), TrackId(12), -12.0, true),
                (SendId(18), TrackId(12), -18.0, false)
            ]
        );
        let effects: Vec<_> = lead
            .mixer
            .effects
            .iter()
            .map(|slot| (slot.id, slot.enabled, slot.sidechain))
            .collect();
        assert_eq!(
            effects,
            vec![
                (EffectSlotId(13), true, None),
                (EffectSlotId(14), false, Some(TrackId(3)))
            ]
        );
        assert_eq!(
            project.track(TrackId(12)).unwrap().mixer.effects[0].id,
            EffectSlotId(15)
        );
        assert_eq!(project.master.effects[0].id, EffectSlotId(16));
    }
}

#[test]
fn hosted_plugin_bytes_and_external_paths_survive_without_loading_plugins() {
    for (_, text) in FIXTURES {
        let project = load_fixture(text);
        let lead = project.track(TrackId(1)).unwrap();
        let instrument = lead.kind.as_instrument().unwrap();
        assert_eq!(
            instrument.instrument_state.hosted_bytes(),
            Some(vec![0, 255, 17, 128, 42])
        );
        assert_eq!(instrument.instrument_state.params["cutoff"], 0.375);
        assert_eq!(
            instrument.file,
            Some(AssetPath::external("/fixture/plugins/instrument.clap"))
        );
        assert_eq!(
            lead.mixer.effects[1].state.hosted_bytes(),
            Some(vec![255, 0, 34, 128])
        );
        assert_eq!(
            lead.mixer.effects[1].file,
            Some(AssetPath::external("/fixture/plugins/effect.clap"))
        );
    }
}

#[test]
fn automation_keeps_stable_targets_keys_domains_and_endpoint_holds() {
    let expected = [
        (ParamTarget::TrackGain(TrackId(1)), None, -6.0, -3.0),
        (
            ParamTarget::Instrument {
                track: TrackId(1),
                param: ParamId(0),
            },
            Some("cutoff"),
            0.375,
            0.75,
        ),
        (
            ParamTarget::Effect {
                track: Some(TrackId(1)),
                slot: EffectSlotId(13),
                param: ParamId(0),
            },
            Some("gain_db"),
            -3.0,
            0.0,
        ),
        (
            ParamTarget::Send {
                track: TrackId(1),
                send: SendId(17),
            },
            None,
            -12.0,
            -9.0,
        ),
    ];
    for (version, text) in FIXTURES {
        let project = load_fixture(text);
        assert_eq!(project.automation.lanes().len(), expected.len());
        for (target, key, from, to) in expected {
            let lane = project.automation.lane(target).unwrap();
            assert_eq!(lane.key(), key);
            assert_eq!(lane.curve, AutomationCurve::Linear);
            assert_eq!(
                lane.points()
                    .iter()
                    .map(|p| (p.tick, p.value))
                    .collect::<Vec<_>>(),
                vec![(Ticks(960), from), (Ticks(4800), to)]
            );
            assert_eq!(project.automation.value_at(target, Ticks::ZERO), Some(from));
            assert_eq!(
                project.automation.value_at(target, Ticks(2880)),
                Some((from + to) / 2.0)
            );
            assert_eq!(
                project.automation.value_at(target, Ticks(8000)),
                Some(to),
                "format {version}"
            );
        }
        assert_eq!(
            project
                .automation
                .value_at(ParamTarget::MasterGain, Ticks::ZERO),
            None
        );
    }
}

#[test]
fn singer_lyrics_voice_take_and_portable_assets_survive_loading() {
    for (_, text) in FIXTURES {
        let project = load_fixture(text);
        let singer = project.track(TrackId(9)).unwrap().kind.as_singer().unwrap();
        let note = &singer.clips[0].notes[0];
        assert_eq!(
            (note.pitch, note.start, note.length),
            (64, Ticks(480), Ticks(1920))
        );
        assert_eq!(note.lyric, "あ");
        assert_eq!(note.phonemes, ["a"]);
        assert_eq!(note.phoneme_seconds, [0.2]);
        let voice = singer.voice.as_ref().unwrap();
        assert_eq!(voice.path, AssetPath::inside("voice.voicevox.json"));
        assert_eq!(voice.speaker.as_deref(), Some("fixture-speaker"));
        let take = singer.take.as_ref().unwrap();
        assert_eq!(
            (take.source, take.fingerprint, take.seed),
            (SourceId(11), 987654321, 123)
        );
        assert_eq!(
            project.audio_sources[&SourceId(11)].path,
            AssetPath::inside("Audio/singer.wav")
        );
        assert_eq!(
            project.audio_sources[&SourceId(6)].path,
            AssetPath::inside("Audio/loop.wav")
        );
    }
}

#[test]
fn loading_keeps_tempo_loop_and_id_allocation_consistent() {
    for (_, text) in FIXTURES {
        let mut project = load_fixture(text);
        assert_eq!(project.tempo_map.bpm_at(Ticks::ZERO), 120.0);
        assert_eq!(project.tempo_map.bpm_at(Ticks(3840)), 90.0);
        assert_eq!(project.loop_region, Some((Ticks::ZERO, Ticks(7680))));
        assert!(project.loop_enabled);
        assert_eq!(project.add_bus_track("New bus"), TrackId(19));
        assert_eq!(project.conflicting_object_id(), None);
    }
}

#[test]
fn version_specific_harmony_is_not_silently_replaced_during_loading() {
    for (version, text) in FIXTURES {
        let project = load_fixture(text);
        let expected = match version {
            31.. => "V13",
            30 => "V11",
            _ => "V7",
        };
        assert_eq!(
            project.harmony.numeral_at(Ticks::ZERO).unwrap().to_text(),
            expected,
            "format {version}"
        );
    }
}

#[test]
fn audio_clip_and_library_asset_metadata_survive_without_external_files() {
    for (_, text) in FIXTURES {
        let project = load_fixture(text);
        let audio = project.track(TrackId(5)).unwrap().kind.as_audio().unwrap();
        let clip = &audio.clips[0];
        assert_eq!(
            (clip.id, clip.source, clip.start),
            (ClipId(7), SourceId(6), Ticks(1920))
        );
        assert_eq!((clip.offset_frames, clip.length_frames), (0, 96_000));
        let source = &project.audio_sources[&SourceId(6)];
        assert_eq!(
            (source.frame_count, source.sample_rate, source.channel_count),
            (96_000, 48_000.0, 2)
        );
        let font = &project.soundfonts[&SoundFontId(8)];
        assert_eq!(font.path, AssetPath::external("/fixture/fonts/library.sf2"));
        assert_eq!(font.byte_size, 123_456);
    }
}

#[test]
fn historical_projects_round_trip_through_the_current_writer_without_semantic_loss() {
    for (version, text) in FIXTURES {
        let mut project = load_fixture(text);
        let mut expected = project.clone();
        expected.saved_by = env!("CARGO_PKG_VERSION").into();
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("Upgraded.auris");
        save_project(&destination, &mut project).unwrap();
        let reloaded = load_project(&destination).unwrap();
        assert_eq!(
            project, expected,
            "saving changed format {version}'s semantic state"
        );
        assert_eq!(reloaded, expected, "format {version}");
    }
}
