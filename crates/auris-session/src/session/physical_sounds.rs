//! General MIDI programs as the built-in instruments that are models of them.

use auris_compose::Role;
use auris_core::{PluginState, TrackId};
use auris_synth::Model;

use super::Session;
use crate::SessionError;

/// The built-in instrument a General MIDI program names, and the patch that makes it that
/// instrument; `None` for a timbre nothing here is a model of, which needs its GM sound.
///
/// A physical model is an instrument family, and several programs are one family played or
/// built differently: a vibraphone and a glockenspiel are both bars, a cello and a violin are
/// both a bowed string on a body. The patch is where that difference goes. It is a starting
/// point in ordinary parameters, not a claim to reproduce anybody's sample.
pub(super) fn native_sound(bank: i32, patch: i32) -> Option<(&'static str, PluginState)> {
    if !(0..=127).contains(&patch) {
        return None;
    }
    if bank == 128 {
        return (patch == 0).then(|| (auris_synth::DrumKit::ID, PluginState::empty()));
    }
    if bank != 0 {
        return None;
    }
    // The voices General MIDI has no model for still have a nearest family: a choir and a
    // "synth voice" are ensembles of the human throat, and a square lead is what the chiptune
    // voice is. A sawtooth lead is deliberately *not* here — see the timbre test below.
    if let Some(id) = match patch {
        52..=53 => Some(auris_synth::Choir::ID),
        54 => Some(auris_synth::Vocal::ID),
        80 => Some(auris_synth::Chiptune::ID),
        _ => None,
    } {
        return Some((id, PluginState::empty()));
    }
    let (model, params): (Model, &[(&str, f32)]) = match patch {
        0 | 2 => (Model::Piano, &[]),
        1 => (Model::Piano, &[("hardness", 0.68)]),
        // A honky-tonk is a piano nobody has tuned: its unisons are cents apart.
        3 => (Model::Piano, &[("detune", 7.0), ("hardness", 0.6)]),
        // Harpsichord and clavinet: a hard, bright attack on strings that do not ring long.
        6..=7 => (
            Model::Piano,
            &[("hardness", 0.9), ("decay", 3.0), ("sympathetic", 0.0)],
        ),
        // Celesta and glockenspiel are steel bars; a music box and a kalimba are tongues of
        // metal held at one end, which is the tine.
        8 => (
            Model::Mallet,
            &[("bar", 3.0), ("hardness", 0.3), ("body", 0.9)],
        ),
        9 => (Model::Mallet, &[("bar", 3.0), ("hardness", 0.8)]),
        10 => (
            Model::Mallet,
            &[("bar", 4.0), ("hardness", 0.85), ("body", 0.2)],
        ),
        108 => (
            Model::Mallet,
            &[("bar", 4.0), ("hardness", 0.5), ("body", 0.6)],
        ),
        11 => (
            Model::Mallet,
            &[("bar", 1.0), ("hardness", 0.4), ("tremolo", 0.3)],
        ),
        12 => (Model::Mallet, &[]),
        13 => (Model::Mallet, &[("bar", 2.0), ("hardness", 0.7)]),
        14 => (Model::Bell, &[("bell", 1.0)]),
        112 => (Model::Bell, &[("bell", 2.0), ("hardness", 0.7)]),
        15 => (Model::HammeredDulcimer, &[]),
        24 => (
            Model::Guitar,
            &[("hardness", 0.06), ("position", 0.28), ("damping", 0.12)],
        ),
        25 => (
            Model::Guitar,
            &[
                ("hardness", 0.45),
                ("position", 0.22),
                ("decay", 5.5),
                ("damping", 0.08),
            ],
        ),
        26..=27 | 31 => (Model::ElectricGuitar, &[]),
        28 => (
            Model::ElectricGuitar,
            &[("damping", 0.8), ("decay", 0.55), ("release", 0.06)],
        ),
        // An upright is its body; the electric basses are a magnet under the strings.
        32 => (
            Model::Bass,
            &[("body", 1.0), ("hardness", 0.3), ("position", 0.25)],
        ),
        33 | 35 => (Model::Bass, &[("hardness", 0.35), ("position", 0.3)]),
        34 => (Model::Bass, &[("hardness", 0.85), ("position", 0.13)]),
        // One player on the instrument named, or the section that seats itself by register.
        40 => (Model::Violin, &[("instrument", 1.0), ("players", 1.0)]),
        41 => (Model::Violin, &[("instrument", 2.0), ("players", 1.0)]),
        42 => (
            Model::Violin,
            &[("instrument", 3.0), ("players", 1.0), ("release", 0.3)],
        ),
        43 => (
            Model::Violin,
            &[("instrument", 4.0), ("players", 1.0), ("release", 0.3)],
        ),
        44 => (Model::Violin, &[("players", 4.0)]),
        // Pizzicato is a string section plucked: the nearest thing here is the plucked string.
        45 => (
            Model::Guitar,
            &[("hardness", 0.3), ("decay", 1.2), ("damping", 0.5)],
        ),
        48..=49 => (
            Model::Violin,
            &[("players", 4.0), ("bow_pressure", 0.4), ("release", 0.5)],
        ),
        71 => (Model::Clarinet, &[]),
        // A recorder is a whistle in wood, voiced rounder.
        74 => (
            Model::TinWhistle,
            &[("jet_shape", 0.12), ("breath_noise", 0.06)],
        ),
        78 => (Model::TinWhistle, &[]),
        _ => return None,
    };
    let mut state = PluginState::empty();
    for (key, value) in params {
        state.params.insert((*key).into(), *value);
    }
    Some((model.id(), state))
}

/// The General MIDI program each built-in instrument is exported as.
///
/// The inverse of [`native_sound`] for the voices that function names, and kept beside it so the
/// two directions are edited together; `every_exported_program_reads_back_as_the_same_instrument`
/// holds them to it. A percussion voice has no entry on purpose: General MIDI announces a kit by
/// putting the track on channel 10, where a program number would select a *kit* rather than a
/// timbre, and the exported track is already on that channel.
///
/// A sampler preset or a hosted plug-in has no entry either. Its instrument is not one of ours,
/// and inventing a program for it would tell the next reader something the document never said.
/// [`auris_synth::Fm2`] is the same refusal in the other direction: its nearest General MIDI
/// family is the sawtooth lead, which this session does not stand in for, so the track travels
/// without a program rather than with a claim nobody made.
pub(super) const GM_PROGRAMS: &[(&str, u8)] = &[
    (Model::Piano.id(), 0),
    (Model::Mallet.id(), 12),
    (Model::Bell.id(), 14),
    (Model::HammeredDulcimer.id(), 15),
    (Model::Guitar.id(), 25),
    (Model::ElectricGuitar.id(), 27),
    (Model::Bass.id(), 33),
    (Model::Violin.id(), 48),
    (auris_synth::Choir::ID, 52),
    (auris_synth::Vocal::ID, 54),
    (Model::Clarinet.id(), 71),
    (Model::TinWhistle.id(), 78),
    (auris_synth::Chiptune::ID, 80),
];

/// A part's role chooses an initial performance; the result remains ordinary editable state.
pub(super) fn style_native(id: &str, role: Option<Role>, state: &mut PluginState) {
    if id == Model::Violin.id() && role == Some(Role::Pad) {
        state.params.entry("bow_pressure".into()).or_insert(0.35);
        state.params.entry("release".into()).or_insert(0.8);
        state.params.entry("bow_speed".into()).or_insert(0.4);
        state.params.entry("legato".into()).or_insert(0.0);
    }
    if id == Model::Violin.id() && role == Some(Role::Melody) {
        state.params.entry("legato".into()).or_insert(1.0);
        state.params.entry("bow_speed".into()).or_insert(0.65);
    }
    if id == Model::Guitar.id() && role == Some(Role::Arp) {
        state.params.entry("position".into()).or_insert(0.18);
        state.params.entry("hardness".into()).or_insert(0.65);
    }
}

impl Session {
    pub(super) fn use_backing_sound(
        &mut self,
        track: TrackId,
        bank: i32,
        patch: i32,
    ) -> Result<(), SessionError> {
        if self.use_native_sound(track, bank, patch)? {
            return Ok(());
        }
        let font = self
            .adopt_general_midi_here()
            .ok_or(SessionError::LibraryMissing)?;
        self.set_track_preset(track, auris_core::PresetRef { font, bank, patch })
    }

    pub(super) fn use_native_sound(
        &mut self,
        track: TrackId,
        bank: i32,
        patch: i32,
    ) -> Result<bool, SessionError> {
        let Some((id, state)) = native_sound(bank, patch) else {
            return Ok(false);
        };
        self.use_timbre_sound(
            track,
            &super::timbre::TimbreSound {
                name: id.into(),
                instrument_id: id.into(),
                preset: None,
            },
        )?;
        // Re-selecting a musical hint restores its starting patch, just as choosing a preset.
        for (key, value) in state.params {
            if let Some(param) = self
                .param_descriptors(id)
                .iter()
                .find(|p| p.key == key)
                .map(|p| p.id)
            {
                self.set_param(auris_core::ParamTarget::Instrument { track, param }, value);
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionOptions;
    use auris_core::time::Ticks;

    #[test]
    fn preset_signal_chains_use_registered_controls_within_their_ranges() {
        let mut session = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
        for preset in auris_compose::PRESETS {
            let piece = auris_compose::compose(&preset.spec());
            for track in &piece.tracks {
                let descriptors = session.param_descriptors(&track.instrument);
                for (key, value) in &track.state.params {
                    let control = descriptors
                        .iter()
                        .find(|control| control.key == *key)
                        .unwrap_or_else(|| {
                            panic!(
                                "{} / {}: unknown instrument control {key}",
                                preset.name, track.name
                            )
                        });
                    assert!(value.is_finite() && (control.min..=control.max).contains(value));
                }
                for effect in &track.effects {
                    assert!(
                        session.registry.has_effect(&effect.id),
                        "{}: {}",
                        preset.name,
                        effect.id
                    );
                    let descriptors = session.param_descriptors(&effect.id);
                    for (key, value) in &effect.state.params {
                        let control = descriptors
                            .iter()
                            .find(|control| control.key == *key)
                            .unwrap_or_else(|| {
                                panic!(
                                    "{} / {}: unknown effect control {key}",
                                    preset.name, effect.id
                                )
                            });
                        assert!(value.is_finite() && (control.min..=control.max).contains(value));
                    }
                }
            }
        }
    }

    #[test]
    fn composed_rock_amplifiers_and_pickups_survive_save_open_with_identical_audio() {
        let mut spec = auris_compose::preset("rock").unwrap().spec();
        spec.form = vec!["verse".into()];
        spec.sections.retain(|name, _| name == "verse");
        spec.sections.get_mut("verse").unwrap().bars = 1;
        let piece = auris_compose::compose(&spec);
        let mut session = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
        let report = session.compose_without_balance(&piece).unwrap();
        assert!(report.substituted.is_empty());
        for (name, drive) in [("lead", 34.0), ("rhythm", 28.0)] {
            let track = session
                .project()
                .tracks
                .iter()
                .find(|track| track.name == name)
                .unwrap();
            assert_eq!(
                track.kind.as_instrument().unwrap().instrument_state.params["pickup_position"],
                0.064146
            );
            assert_eq!(track.mixer.effects[0].effect_id, auris_dsp::GuitarAmp::ID);
            assert_eq!(track.mixer.effects[0].state.params["drive_db"], drive);
            assert_eq!(track.mixer.effects[0].state.params["cabinet"], 2.0);
        }
        let before = session
            .render_job()
            .render(&Default::default(), &mut Default::default())
            .unwrap();
        assert!(before.peak() > 0.001);
        let folder = tempfile::tempdir().unwrap();
        let saved = session.save_as(&folder.path().join("Rock.auris")).unwrap();
        let mut reopened = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
        assert!(reopened.open(&saved.document).unwrap().is_empty());
        assert_eq!(
            auris_compose::SongSpec::parse(reopened.project().song_spec.as_deref().unwrap())
                .unwrap(),
            spec
        );
        let after = reopened
            .render_job()
            .render(&Default::default(), &mut Default::default())
            .unwrap();
        assert_eq!(before.channels(), after.channels());
    }

    #[test]
    fn distinct_gm_timbres_are_not_replaced_by_unrelated_physical_models() {
        for patch in [
            4, 5, 29, 30, 36, 37, 38, 39, 46, 47, 50, 51, 56, 61, 65, 73, 81, 84, 88, 94, 98,
        ] {
            assert!(
                native_sound(0, patch).is_none(),
                "{} needs its GM sound",
                auris_compose::gm::Program(patch as u8).name()
            );
        }
        for patch in [8, 16, 25, 40, 48] {
            assert!(
                native_sound(128, patch).is_none(),
                "a specialty kit is not the standard physical kit"
            );
        }
    }

    #[test]
    fn role_starting_points_preserve_explicit_physical_controls() {
        let mut state = PluginState::empty();
        state.params.insert("bow_pressure".into(), 0.75);
        style_native(Model::Violin.id(), Some(Role::Pad), &mut state);
        assert_eq!(state.params["bow_pressure"], 0.75);
        assert_eq!(state.params["release"], 0.8);
        assert_eq!(state.params["legato"], 0.0);
        assert_eq!(state.params["bow_speed"], 0.4);
        let mut melody = PluginState::empty();
        style_native(Model::Violin.id(), Some(Role::Melody), &mut melody);
        assert_eq!(melody.params["legato"], 1.0);
        melody.params.insert("legato".into(), 0.0);
        melody.params.insert("bow_speed".into(), 0.8);
        style_native(Model::Violin.id(), Some(Role::Melody), &mut melody);
        assert_eq!(melody.params["legato"], 0.0);
        assert_eq!(melody.params["bow_speed"], 0.8);
    }

    #[test]
    fn composed_native_parts_and_controls_survive_save_open_and_render() {
        let text = "form = [\"verse\"]\n[section.verse]\nbars = 1\n";
        let mut text = text.to_string();
        for (name, program) in [
            ("piano", 0),
            ("guitar", 24),
            ("electric", 27),
            ("bass", 34),
            ("bell", 14),
            ("mallet", 12),
            ("violin", 40),
            ("clarinet", 71),
            ("dulcimer", 15),
            ("whistle", 78),
        ] {
            text.push_str(&format!(
                "[[part]]\nname = \"{name}\"\nrole = \"melody\"\nprogram = {program}\n"
            ));
        }
        let spec = auris_compose::SongSpec::parse(&text).unwrap();
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let report = session
            .compose_without_balance(&auris_compose::compose(&spec))
            .unwrap();
        assert!(report.substituted.is_empty());
        assert!(session.project().soundfonts.is_empty());
        for model in Model::ALL {
            assert!(session.project().tracks.iter().any(|track| {
                track
                    .kind
                    .as_instrument()
                    .is_some_and(|inner| inner.instrument_id == model.id())
            }));
        }
        let clarinet = session
            .project()
            .tracks
            .iter()
            .find(|track| track.name == "clarinet")
            .unwrap();
        assert_eq!(
            clarinet.kind.as_instrument().unwrap().instrument_id,
            Model::Clarinet.id()
        );
        let clarinet_id = clarinet.id;
        let pressure = session
            .param_descriptors(Model::Clarinet.id())
            .iter()
            .find(|param| param.key == "pressure")
            .unwrap()
            .id;
        session.set_param(
            auris_core::ParamTarget::Instrument {
                track: clarinet_id,
                param: pressure,
            },
            0.64,
        );
        for (name, id, key, value) in [
            ("dulcimer", Model::HammeredDulcimer.id(), "detune", 3.0),
            ("whistle", Model::TinWhistle.id(), "pressure", 0.6),
        ] {
            let track = session
                .project()
                .tracks
                .iter()
                .find(|track| track.name == name)
                .unwrap();
            assert_eq!(track.kind.as_instrument().unwrap().instrument_id, id);
            let track = track.id;
            let param = session
                .param_descriptors(id)
                .iter()
                .find(|param| param.key == key)
                .unwrap()
                .id;
            session.set_param(auris_core::ParamTarget::Instrument { track, param }, value);
        }
        let guitar = session
            .project()
            .tracks
            .iter()
            .find(|track| track.name == "guitar")
            .unwrap()
            .id;
        let hardness = session
            .param_descriptors(Model::Guitar.id())
            .iter()
            .find(|p| p.key == "hardness")
            .unwrap()
            .id;
        session.set_param(
            auris_core::ParamTarget::Instrument {
                track: guitar,
                param: hardness,
            },
            0.77,
        );
        let electric = session
            .project()
            .tracks
            .iter()
            .find(|track| track.name == "electric")
            .unwrap()
            .id;
        let slot = session
            .add_effect(Some(electric), auris_dsp::GuitarAmp::ID)
            .unwrap();
        session.set_param(
            auris_core::ParamTarget::Effect {
                track: Some(electric),
                slot,
                param: auris_core::ParamId(0),
            },
            28.0,
        );
        let before = session
            .render_job()
            .render(&Default::default(), &mut Default::default())
            .unwrap();
        assert!(before.peak() > 0.001);
        let folder = tempfile::tempdir().unwrap();
        let saved = session
            .save_as(&folder.path().join("Native.auris"))
            .unwrap();
        let mut reopened = Session::new(SessionOptions::headless()).unwrap();
        assert!(reopened.open(&saved.document).unwrap().is_empty());
        let after = reopened
            .render_job()
            .render(&Default::default(), &mut Default::default())
            .unwrap();
        assert_eq!(before.channels(), after.channels());
        assert!(reopened.project().soundfonts.is_empty());
    }

    #[test]
    fn mapped_families_are_available_without_assets_and_selection_is_undoable() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        for (patch, model) in [
            (0, Model::Piano),
            (25, Model::Guitar),
            (27, Model::ElectricGuitar),
            (33, Model::Bass),
            (14, Model::Bell),
            (12, Model::Mallet),
            (40, Model::Violin),
        ] {
            let track = session.add_default_instrument_track("Part").unwrap();
            session.forget_history();
            session.set_track_general_midi(track, 0, patch).unwrap();
            assert_eq!(
                session
                    .project()
                    .track(track)
                    .unwrap()
                    .kind
                    .as_instrument()
                    .unwrap()
                    .instrument_id,
                model.id()
            );
            assert!(session.project().soundfonts.is_empty());
            if model != Model::Piano {
                assert!(session.undo().is_some());
            }
        }
        for (patch, id) in [
            (71, Model::Clarinet.id()),
            (15, Model::HammeredDulcimer.id()),
            (78, Model::TinWhistle.id()),
        ] {
            let track = session.add_default_instrument_track("Part").unwrap();
            session.forget_history();
            session.set_track_general_midi(track, 0, patch).unwrap();
            assert_eq!(
                session
                    .project()
                    .track(track)
                    .unwrap()
                    .kind
                    .as_instrument()
                    .unwrap()
                    .instrument_id,
                id
            );
            assert!(session.project().soundfonts.is_empty());
            assert!(session.undo().is_some());
        }
    }

    #[test]
    fn every_exported_program_reads_back_as_the_same_instrument() {
        for (id, program) in GM_PROGRAMS {
            let (read, _) = native_sound(0, i32::from(*program)).unwrap_or_else(|| {
                panic!("{id} exports as program {program}, which the reader does not know")
            });
            assert_eq!(read, *id, "program {program} reads back as {read}");
        }
    }

    #[test]
    fn a_midi_file_written_here_reads_back_on_the_same_voices() {
        let folder = tempfile::tempdir().unwrap();
        let destination = folder.path().join("Voices.mid");
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let mut expected = Vec::new();
        for (id, _) in GM_PROGRAMS {
            let track = session.add_instrument_track(*id, id).unwrap();
            let clip = session
                .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER)
                .unwrap();
            session
                .add_note(clip, auris_core::Note::new(60, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
            expected.push((*id).to_string());
        }
        let drums = session.add_default_drum_track("Kit").unwrap();
        let clip = session
            .add_midi_clip(drums, "Beat", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        session
            .add_note(clip, auris_core::Note::new(36, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        // The kit travels as the channel it is on, not as a program, so it is expected back too.
        expected.push(auris_synth::DrumKit::ID.to_string());

        session.export_midi(&destination).unwrap();
        let report = session.import_midi(&destination).unwrap();
        assert_eq!(report.tracks, expected.len());
        let actual: Vec<String> = session
            .project()
            .tracks
            .iter()
            .filter_map(|track| track.kind.as_instrument())
            .map(|inner| inner.instrument_id.clone())
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn a_program_no_built_in_voice_covers_keeps_the_registry_default() {
        // 56 is a trumpet. The built-in voices have no brass model, so the fallback stands rather
        // than a violin being declared a trumpet because it was the nearest thing to hand.
        let folder = tempfile::tempdir().unwrap();
        let destination = folder.path().join("Brass.mid");
        let mut source = auris_core::Project::new("Brass", 48_000.0);
        let track = source.add_instrument_track("Trumpet", "auris.sampler");
        let clip = source
            .add_midi_clip(track, "Line", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        source.midi_clip_mut(clip).unwrap().notes =
            vec![auris_core::Note::new(60, Ticks::ZERO, Ticks::QUARTER)];
        let bytes = auris_io::write_midi_bytes(&source, &[("auris.sampler", 56)]).unwrap();
        std::fs::write(&destination, bytes).unwrap();

        let mut session = Session::new(SessionOptions::headless()).unwrap();
        session.import_midi(&destination).unwrap();
        assert_eq!(
            session.project().tracks[0]
                .kind
                .as_instrument()
                .unwrap()
                .instrument_id,
            crate::registry::DEFAULT_INSTRUMENT
        );
    }
}
