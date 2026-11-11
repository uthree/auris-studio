//! General MIDI programs as the built-in instruments that are models of them.

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
        // GS put its drum machines on programs 24 (Electronic) and 25 (the analogue one).
        let id = match patch {
            0 => auris_synth::DrumKit::ID,
            24 => auris_synth::ElectroStyle::Edm.id(),
            25 => auris_synth::ElectroStyle::Electro.id(),
            _ => return None,
        };
        return Some((id, PluginState::empty()));
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
        for patch in [8, 16, 40, 48] {
            assert!(
                native_sound(128, patch).is_none(),
                "a specialty kit is not the standard physical kit"
            );
        }
    }

    #[test]
    fn the_gs_drum_machines_become_the_electronic_kits() {
        let id = |patch| native_sound(128, patch).map(|(id, _)| id);
        assert_eq!(id(0), Some(auris_synth::DrumKit::ID));
        assert_eq!(id(24), Some("auris.synth.edm_kit"));
        assert_eq!(id(25), Some("auris.synth.electro_kit"));
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
