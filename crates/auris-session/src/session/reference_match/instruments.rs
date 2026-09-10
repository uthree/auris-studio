//! Frozen library choices for rendered search; no live hosted instances are replaced.

use auris_core::plugin::PluginState;
use auris_core::project::{DrumMap, InstrumentTrack, PresetRef};
use auris_core::rng::Rng;
use auris_core::{ParamTarget, PluginCategory, Project, TrackId, TrackKind};

use super::Session;

#[derive(Clone)]
struct Sound {
    instrument_id: String,
    state: PluginState,
}

/// One track's complete, seeded order of available source choices.
#[derive(Clone)]
pub(super) struct Dial {
    track: TrackId,
    sounds: Vec<Sound>,
}

/// Captures built-in patches and presets from already loaded, project-owned libraries.
pub(super) fn dials(session: &Session, project: &Project, seed: u64) -> Vec<Dial> {
    let mut melodic = Vec::new();
    for descriptor in session.registry.instruments().filter(|descriptor| {
        descriptor.id.starts_with("auris.synth.") && descriptor.category != PluginCategory::Drum
    }) {
        let Ok(instrument) = session.registry.create_instrument(&descriptor.id) else {
            continue;
        };
        melodic.push(Sound {
            instrument_id: descriptor.id.to_string(),
            state: instrument.save_state(),
        });
    }
    // Integer FM ratios keep pitched material harmonic. Noise is deliberately absent from
    // the melodic waveform choices: changing timbre should not erase the written pitches.
    for (id, parameters) in [
        (auris_synth::Chiptune::ID, vec![("waveform", 0.0)]),
        (auris_synth::Chiptune::ID, vec![("waveform", 2.0)]),
        (auris_synth::Chiptune::ID, vec![("waveform", 3.0)]),
        (
            auris_synth::Chiptune::ID,
            vec![("waveform", 1.0), ("pulse_width", 0.25)],
        ),
        (auris_synth::Fm2::ID, vec![("ratio", 1.0), ("index", 2.0)]),
        (auris_synth::Fm2::ID, vec![("ratio", 2.0), ("index", 1.0)]),
        (auris_synth::Fm2::ID, vec![("ratio", 3.0), ("index", 4.0)]),
    ] {
        let Ok(mut instrument) = session.registry.create_instrument(id) else {
            continue;
        };
        for (key, value) in parameters {
            instrument.set_param_by_key(key, value);
        }
        melodic.push(Sound {
            instrument_id: id.into(),
            state: instrument.save_state(),
        });
    }
    let mut drums = Vec::new();
    for font in session
        .soundfonts()
        .filter(|font| session.fonts.get(font.id).is_some())
    {
        let mut presets = session.soundfont_presets(font.id);
        presets.sort_by_key(|preset| (preset.bank, preset.patch));
        presets.dedup_by_key(|preset| (preset.bank, preset.patch));
        for preset in presets {
            let Ok(instrument) = session
                .registry
                .create_instrument(auris_sampler::SAMPLER_ID)
            else {
                continue;
            };
            let mut state = instrument.save_state();
            auris_sampler::store_preset(
                &mut state,
                PresetRef {
                    font: font.id,
                    bank: preset.bank,
                    patch: preset.patch,
                },
            );
            let pool = if preset.bank == 128 {
                &mut drums
            } else {
                &mut melodic
            };
            pool.push(Sound {
                instrument_id: auris_sampler::SAMPLER_ID.into(),
                state,
            });
        }
    }
    project
        .tracks
        .iter()
        .filter_map(|track| {
            let inner = track.kind.as_instrument()?;
            if track.mixer.mute || inner.is_hosted() || inner.clips.is_empty() {
                return None;
            }
            let descriptor = session.registry.descriptor(&inner.instrument_id)?;
            let percussion = matches!(track.kind, TrackKind::Drum(_))
                || descriptor.category == PluginCategory::Drum
                || auris_sampler::stored_preset(&inner.instrument_state)
                    .is_some_and(|p| p.bank == 128);
            if percussion
                && inner.instrument_id != auris_sampler::SAMPLER_ID
                && inner.instrument_id != auris_synth::DrumKit::ID
            {
                return None;
            }
            let pool = if percussion { &drums } else { &melodic };
            let mut current = session
                .registry
                .create_instrument(&inner.instrument_id)
                .ok()?;
            current.load_state(&inner.instrument_state);
            let current_state = current.save_state();
            let mut sounds: Vec<_> = pool
                .iter()
                .filter_map(|sound| {
                    let mut sound = sound.clone();
                    if sound.instrument_id == auris_sampler::SAMPLER_ID
                        && inner.instrument_id == auris_sampler::SAMPLER_ID
                    {
                        let preset = auris_sampler::stored_preset(&sound.state)?;
                        if auris_sampler::stored_preset(&inner.instrument_state) == Some(preset) {
                            return None;
                        }
                        // Choosing a sampler sound retains the player's mix, reverb and chorus.
                        sound.state = inner.instrument_state.clone();
                        auris_sampler::store_preset(&mut sound.state, preset);
                    } else if sound.instrument_id == inner.instrument_id
                        && sound.state == current_state
                    {
                        return None;
                    }
                    if let Some(map) = inner.instrument_state.extra.get(DrumMap::STATE_KEY) {
                        if !sound.state.extra.is_object() {
                            sound.state.extra = serde_json::json!({});
                        }
                        sound.state.extra[DrumMap::STATE_KEY] = map.clone();
                    }
                    Some(sound)
                })
                .collect();
            let mut rng = Rng::stream(seed, &["reference_instruments".into(), track.id.0.into()]);
            for index in (1..sounds.len()).rev() {
                sounds.swap(index, rng.below(index + 1));
            }
            (!sounds.is_empty()).then_some(Dial {
                track: track.id,
                sounds,
            })
        })
        .collect()
}

impl Dial {
    /// Tries every alternative before repeating, even if all preceding alternatives lost.
    pub(super) fn adjust(&self, project: &mut Project, original: &Project, proposal: usize) {
        let sound = &self.sounds[proposal % self.sounds.len()];
        let Some(inner) = project
            .track_mut(self.track)
            .and_then(|track| track.kind.as_instrument_mut())
        else {
            return;
        };
        inner.instrument_id.clone_from(&sound.instrument_id);
        inner.instrument_state.clone_from(&sound.state);
        inner.file = None;
        project.remove_instrument_automation(self.track);
        let same_sampler = sound.instrument_id == auris_sampler::SAMPLER_ID
            && original
                .track(self.track)
                .and_then(|track| track.kind.as_instrument())
                .is_some_and(|inner| inner.instrument_id == auris_sampler::SAMPLER_ID);
        if same_sampler {
            // Start from the original lanes, since an earlier winning source may have removed
            // them. Fader, effect and other tracks' automation remain exactly as proposed.
            for lane in original.automation.lanes().iter().filter(|lane| {
                matches!(lane.target, ParamTarget::Instrument { track, .. } if track == self.track)
            }) {
                for point in lane.points() {
                    project.automation.set_point(lane.target, lane.key().map(str::to_owned), lane.curve, point.tick, point.value);
                }
            }
        }
    }
}

/// Reports source identity and the patch values that distinguish the frozen choices.
pub(super) fn describe_changes(original: &Project, best: &Project) -> Vec<String> {
    let mut changes = Vec::new();
    for before in &original.tracks {
        let Some((source, result)) = before.kind.as_instrument().zip(
            best.track(before.id)
                .and_then(|track| track.kind.as_instrument()),
        ) else {
            continue;
        };
        if source.instrument_id != result.instrument_id
            || source.instrument_state != result.instrument_state
        {
            let previous = describe(original, source);
            let next = describe(best, result);
            if previous == next {
                changes.push(format!("{}: reset {next} patch settings", before.name));
            } else {
                changes.push(format!("{}: sound {previous} → {next}", before.name));
            }
        }
        let count = |project: &Project| {
            project.automation.lanes().iter().filter(|lane| {
                matches!(lane.target, ParamTarget::Instrument { track, .. } if track == before.id)
            }).count()
        };
        let previous = count(original);
        let next = count(best);
        if previous != next {
            changes.push(format!(
                "{}: instrument automation lanes {previous} → {next}",
                before.name
            ));
        }
    }
    changes
}

fn describe(project: &Project, instrument: &InstrumentTrack) -> String {
    if let Some(preset) = auris_sampler::stored_preset(&instrument.instrument_state) {
        let font = project
            .soundfonts
            .get(&preset.font)
            .map_or("SoundFont", |font| font.name.as_str());
        return format!("{font} [{}:{}]", preset.bank, preset.patch);
    }
    let values = &instrument.instrument_state.params;
    match instrument.instrument_id.as_str() {
        auris_synth::Chiptune::ID => {
            let waveform = values.get("waveform").copied().unwrap_or(1.0).round() as usize;
            let name = ["Sine", "Square", "Saw", "Triangle", "Noise"]
                .get(waveform)
                .unwrap_or(&"Square");
            let pulse = values.get("pulse_width").copied().unwrap_or(0.5);
            if waveform == 1 {
                format!("Chiptune ({name}, pulse {pulse:.2})")
            } else {
                format!("Chiptune ({name})")
            }
        }
        auris_synth::Fm2::ID => format!(
            "FM2 (ratio {:.1}, index {:.1})",
            values.get("ratio").copied().unwrap_or(2.0),
            values.get("index").copied().unwrap_or(5.0)
        ),
        auris_synth::Vocal::ID => "Vocal".into(),
        auris_synth::DrumKit::ID => "Drum Kit".into(),
        id => id.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionOptions;
    use auris_core::{
        AudioBuffer, Note, NoteEvent, ParamId, PrepareContext, ProcessContext, Ticks,
    };

    fn session() -> (Session, TrackId) {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::from_beats(2.0))
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        (session, track)
    }

    #[test]
    fn every_builtin_choice_renders_and_preserves_the_score_and_mix() {
        let (session, track) = session();
        let original = session.project.clone();
        let dials = dials(&session, &session.project, 42);
        assert_eq!(dials.len(), 1);
        assert!(dials[0].sounds.len() > 5);
        let mut identities = std::collections::BTreeSet::new();
        for index in 0..dials[0].sounds.len() {
            let mut candidate = original.clone();
            dials[0].adjust(&mut candidate, &original, index);
            let changed = candidate.track(track).unwrap();
            let source = changed.kind.as_instrument().unwrap();
            let baseline = original.track(track).unwrap();
            assert_eq!(source.clips, baseline.kind.as_instrument().unwrap().clips);
            assert_eq!(changed.mixer, baseline.mixer);
            identities.insert(
                serde_json::to_string(&(&source.instrument_id, &source.instrument_state)).unwrap(),
            );
            let mut instrument = session
                .registry
                .create_instrument(&source.instrument_id)
                .unwrap();
            instrument.load_state(&source.instrument_state);
            instrument.prepare(&PrepareContext::new(48_000.0, 4096, 2));
            let mut audio = AudioBuffer::stereo(4096, 48_000.0);
            instrument.process(
                &[NoteEvent::NoteOn {
                    frame: 0,
                    pitch: 60,
                    velocity: 0.8,
                }],
                &mut audio,
                &ProcessContext::realtime(48_000.0, 4096, 0, 120.0, true),
            );
            assert!(audio.peak() > 0.0, "{} was silent", source.instrument_id);
            assert!(
                audio
                    .channels()
                    .iter()
                    .flatten()
                    .all(|sample| sample.is_finite())
            );
            assert_eq!(describe_changes(&original, &candidate).len(), 1);
        }
        assert_eq!(identities.len(), dials[0].sounds.len());
    }

    #[test]
    fn proposals_are_seeded_and_exhaust_the_pool_without_a_winner() {
        let (session, _) = session();
        let sequence = |seed| {
            let dial = dials(&session, &session.project, seed).remove(0);
            (0..dial.sounds.len())
                .map(|index| {
                    let mut project = session.project.clone();
                    dial.adjust(&mut project, &session.project, index);
                    serde_json::to_string(&project).unwrap()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(sequence(42), sequence(42));
        assert_ne!(sequence(42), sequence(43));
    }

    #[test]
    fn source_changes_clear_only_instrument_automation() {
        let (mut session, track) = session();
        let parameter = ParamTarget::Instrument {
            track,
            param: ParamId(0),
        };
        session.set_automation_point(parameter, Ticks::ZERO, 1.0);
        session.set_automation_point(ParamTarget::TrackGain(track), Ticks::ZERO, -8.0);
        let original = session.project.clone();
        let dial = dials(&session, &session.project, 42).remove(0);
        let mut candidate = original.clone();
        dial.adjust(&mut candidate, &original, 0);
        assert!(candidate.automation.lane(parameter).is_none());
        assert_eq!(
            candidate.automation.lane(ParamTarget::TrackGain(track)),
            original.automation.lane(ParamTarget::TrackGain(track))
        );
        assert!(original.automation.lane(parameter).is_some());
        assert!(
            describe_changes(&original, &candidate)
                .iter()
                .any(|change| { change == "Lead: instrument automation lanes 1 → 0" })
        );
    }

    #[test]
    fn hosted_and_unmapped_noise_drums_are_not_replaced() {
        let (mut session, track) = session();
        session
            .project
            .track_mut(track)
            .unwrap()
            .kind
            .as_instrument_mut()
            .unwrap()
            .file = Some(auris_core::AssetPath::external("/plugins/test.clap"));
        let drum = session
            .add_drum_track("Noise", auris_synth::NoiseDrum::ID)
            .unwrap();
        session
            .add_midi_clip(drum, "Beat", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        assert!(dials(&session, &session.project, 42).is_empty());
    }

    fn font(
        session: &mut Session,
        scratch: &crate::session::fixtures::Scratch,
        name: &str,
        bank: u16,
    ) -> auris_core::SoundFontId {
        let path = scratch.soundfont(name);
        let mut bytes = std::fs::read(&path).unwrap();
        let offset = bytes.windows(4).position(|bytes| bytes == b"phdr").unwrap() + 8 + 22;
        bytes[offset..offset + 2].copy_from_slice(&bank.to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        session.import_soundfont(&path).unwrap()
    }

    #[test]
    fn loaded_fonts_are_partitioned_and_drum_assignments_survive() {
        let (mut session, track) = session();
        let scratch = crate::session::fixtures::Scratch::new("search-instruments");
        let melodic = font(&mut session, &scratch, "melody.sf2", 0);
        let kit = font(&mut session, &scratch, "kit.sf2", 128);
        let drum = session
            .add_drum_track("Kit", auris_synth::DrumKit::ID)
            .unwrap();
        session
            .add_midi_clip(drum, "Beat", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        let original = session.project.clone();
        let dials = dials(&session, &session.project, 42);
        let melodic_dial = dials.iter().find(|dial| dial.track == track).unwrap();
        let presets: Vec<_> = melodic_dial
            .sounds
            .iter()
            .filter_map(|sound| auris_sampler::stored_preset(&sound.state))
            .collect();
        assert_eq!(
            presets,
            vec![PresetRef {
                font: melodic,
                bank: 0,
                patch: 0
            }]
        );
        let drum_dial = dials.iter().find(|dial| dial.track == drum).unwrap();
        assert_eq!(drum_dial.sounds.len(), 1);
        let mut candidate = original.clone();
        drum_dial.adjust(&mut candidate, &original, 0);
        let source = candidate.track(drum).unwrap().kind.as_instrument().unwrap();
        let before = original.track(drum).unwrap().kind.as_instrument().unwrap();
        assert_eq!(
            auris_sampler::stored_preset(&source.instrument_state),
            Some(PresetRef {
                font: kit,
                bank: 128,
                patch: 0
            })
        );
        assert_eq!(
            DrumMap::load(&source.instrument_state),
            DrumMap::load(&before.instrument_state)
        );
        assert_eq!(source.clips, before.clips);
    }

    #[test]
    fn returning_to_a_sampler_preset_restores_its_player_settings_and_lanes() {
        let (mut session, track) = session();
        let scratch = crate::session::fixtures::Scratch::new("search-sampler-state");
        let first = font(&mut session, &scratch, "first.sf2", 0);
        let second = font(&mut session, &scratch, "second.sf2", 0);
        session
            .set_track_preset(
                track,
                PresetRef {
                    font: first,
                    bank: 0,
                    patch: 0,
                },
            )
            .unwrap();
        let target = ParamTarget::Instrument {
            track,
            param: ParamId(0),
        };
        session.set_param(target, -7.0);
        session.set_automation_point(target, Ticks::ZERO, -7.0);
        let original = session.project.clone();
        let dial = dials(&session, &session.project, 42).remove(0);
        let builtin = dial
            .sounds
            .iter()
            .position(|sound| sound.instrument_id != auris_sampler::SAMPLER_ID)
            .unwrap();
        let sampler = dial
            .sounds
            .iter()
            .position(|sound| {
                auris_sampler::stored_preset(&sound.state)
                    .is_some_and(|preset| preset.font == second)
            })
            .unwrap();
        let mut candidate = original.clone();
        dial.adjust(&mut candidate, &original, builtin);
        assert!(candidate.automation.lane(target).is_none());
        dial.adjust(&mut candidate, &original, sampler);
        assert_eq!(
            candidate.automation.lane(target),
            original.automation.lane(target)
        );
        let state = &candidate
            .track(track)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .instrument_state;
        assert_eq!(
            state.params,
            original
                .track(track)
                .unwrap()
                .kind
                .as_instrument()
                .unwrap()
                .instrument_state
                .params
        );
        assert_eq!(auris_sampler::stored_preset(state).unwrap().font, second);
    }
}
