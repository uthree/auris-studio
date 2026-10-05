//! Interpretation of legacy musical sound hints as editable native instruments.

use auris_compose::Role;
use auris_core::{PluginState, TrackId};
use auris_synth::Model;

use super::Session;
use crate::SessionError;

/// Corresponding native models only; unsupported timbres and specialty kits need GM.
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
    if patch == 71 {
        return Some((auris_synth::Clarinet::ID, PluginState::empty()));
    }
    if patch == 15 {
        return Some((auris_synth::HammeredDulcimer::ID, PluginState::empty()));
    }
    if patch == 78 {
        return Some((auris_synth::TinWhistle::ID, PluginState::empty()));
    }
    let model = match patch {
        0..=3 | 6..=7 => Model::Piano,
        8 | 11 | 14 => Model::Bell,
        9 | 10 | 12 | 13 => Model::Mallet,
        24..=25 => Model::Guitar,
        26..=28 | 31 => Model::ElectricGuitar,
        32..=35 => Model::Bass,
        40..=45 | 48..=49 => Model::Violin,
        _ => return None,
    };
    let mut state = PluginState::empty();
    let mut set = |key: &str, value| {
        state.params.insert(key.into(), value);
    };
    match patch {
        // These hints are starting points for our models, not claims to reproduce the patch.
        6..=7 => {
            set("hardness", 0.8);
            set("stiffness", 0.002);
            set("decay", 2.4);
        }
        9 | 12 | 13 => {
            set("hardness", 0.25);
            set("decay", 1.4);
        }
        24 => {
            set("hardness", 0.25);
            set("position", 0.28);
        }
        28 => {
            set("damping", 0.8);
            set("decay", 0.55);
            set("release", 0.06);
        }
        33 | 35 => {
            set("hardness", 0.35);
            set("position", 0.3);
        }
        34 => {
            set("hardness", 0.85);
            set("position", 0.13);
        }
        42 | 43 => {
            set("body", 0.6);
            set("release", 0.3);
        }
        44 => {
            return Some((Model::Guitar.id(), state));
        }
        48..=49 => {
            set("bow_pressure", 0.4);
            set("release", 0.5);
        }
        _ => {}
    }
    Some((model.id(), state))
}

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
            auris_synth::Clarinet::ID
        );
        let clarinet_id = clarinet.id;
        let pressure = session
            .param_descriptors(auris_synth::Clarinet::ID)
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
            ("dulcimer", auris_synth::HammeredDulcimer::ID, "detune", 3.0),
            ("whistle", auris_synth::TinWhistle::ID, "pressure", 0.6),
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
            (71, auris_synth::Clarinet::ID),
            (15, auris_synth::HammeredDulcimer::ID),
            (78, auris_synth::TinWhistle::ID),
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
}
