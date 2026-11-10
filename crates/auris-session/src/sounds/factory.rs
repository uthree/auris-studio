//! Editable factory patches grouped by their underlying physical instrument.

use std::sync::OnceLock;

use auris_core::{Instrument, PluginState};
use auris_synth::{Choir, DrumKit, Model, Physical};

use super::SoundPreset;

/// One basic physical instrument and the starting patches derived from it.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundFamily {
    /// Stable plugin identity shared by all patches in this family.
    pub instrument_id: String,
    /// Complete, independently editable patches in browser order.
    pub presets: Vec<SoundPreset>,
}

type Patch<'a> = (&'a str, &'a [(&'a str, f32)]);

fn family(instrument: impl Instrument, patches: &[Patch<'_>]) -> SoundFamily {
    let instrument_id = instrument.descriptor().id.to_string();
    let mut defaults = PluginState::empty();
    for param in instrument.parameters() {
        defaults.params.insert(param.key.to_string(), param.default);
    }
    if instrument_id == DrumKit::ID {
        use auris_core::{DrumMap, DrumRole};
        DrumMap::from_voices([
            (DrumRole::Kick, 36),
            (DrumRole::Snare, 38),
            (DrumRole::ClosedHat, 42),
            (DrumRole::OpenHat, 46),
            (DrumRole::Crash, 49),
            (DrumRole::Tom, 47),
        ])
        .store(&mut defaults);
    }
    let presets = patches
        .iter()
        .map(|(name, overrides)| {
            let mut state = defaults.clone();
            for (key, value) in *overrides {
                state.params.insert((*key).into(), *value);
            }
            SoundPreset {
                name: (*name).into(),
                instrument_id: instrument_id.clone(),
                state,
                file: None,
                soundfont: None,
                effects: Vec::new(),
            }
        })
        .collect();
    SoundFamily {
        instrument_id,
        presets,
    }
}

/// Factory patches for every physical instrument, cached outside rendering and audio callbacks.
///
/// Each family's first patch uses the instrument's existing defaults. Other patches override
/// ordinary exposed controls, and use the same application and saving path as user sounds.
pub fn factory_sound_families() -> &'static [SoundFamily] {
    static FAMILIES: OnceLock<Vec<SoundFamily>> = OnceLock::new();
    FAMILIES.get_or_init(|| {
        vec![
            family(
                Physical::new(Model::Violin),
                &[
                    ("String section", &[]),
                    (
                        "Violin",
                        &[("instrument", 1.0), ("players", 1.0), ("vibrato", 0.45)],
                    ),
                    (
                        "Viola",
                        &[
                            ("instrument", 2.0),
                            ("players", 1.0),
                            ("vibrato", 0.4),
                            ("bow_pressure", 0.45),
                        ],
                    ),
                    (
                        "Cello",
                        &[
                            ("instrument", 3.0),
                            ("players", 1.0),
                            ("vibrato", 0.4),
                            ("bow_pressure", 0.45),
                            ("release", 0.3),
                        ],
                    ),
                    (
                        "Contrabass",
                        &[
                            ("instrument", 4.0),
                            ("players", 1.0),
                            ("vibrato", 0.2),
                            ("bow_pressure", 0.5),
                            ("bow_response", 0.06),
                            ("release", 0.35),
                        ],
                    ),
                    (
                        "Sul ponticello",
                        &[("position", 0.045), ("bow_pressure", 0.6), ("players", 2.0)],
                    ),
                    (
                        "Sul tasto",
                        &[
                            ("position", 0.2),
                            ("bow_pressure", 0.25),
                            ("bow_speed", 0.6),
                            ("damping", 0.5),
                        ],
                    ),
                ],
            ),
            family(
                Physical::new(Model::Piano),
                &[
                    ("Grand piano", &[]),
                    (
                        "Felt piano",
                        &[("hardness", 0.15), ("damping", 0.2), ("sympathetic", 0.4)],
                    ),
                    ("Bright piano", &[("hardness", 0.9), ("position", 0.1)]),
                    ("Honky-tonk piano", &[("detune", 7.0), ("hardness", 0.6)]),
                ],
            ),
            family(
                Physical::new(Model::Guitar),
                &[
                    ("Acoustic guitar", &[]),
                    (
                        "Nylon guitar",
                        &[("hardness", 0.06), ("position", 0.28), ("damping", 0.12)],
                    ),
                    (
                        "Steel guitar",
                        &[
                            ("hardness", 0.45),
                            ("position", 0.22),
                            ("decay", 5.5),
                            ("damping", 0.08),
                        ],
                    ),
                    (
                        "Muted guitar",
                        &[
                            ("hardness", 0.5),
                            ("damping", 0.8),
                            ("decay", 0.55),
                            ("release", 0.06),
                        ],
                    ),
                    ("Piezo guitar", &[("pickup", 1.0)]),
                ],
            ),
            family(
                Physical::new(Model::ElectricGuitar),
                &[
                    ("Clean electric guitar", &[]),
                    (
                        "Warm electric guitar",
                        &[
                            ("tone", 1600.0),
                            ("pickup_position", 0.26),
                            ("hardness", 0.25),
                        ],
                    ),
                    (
                        "Bright electric guitar",
                        &[
                            ("tone", 6500.0),
                            ("pickup_position", 0.08),
                            ("hardness", 0.75),
                        ],
                    ),
                ],
            ),
            family(
                Physical::new(Model::Bass),
                &[
                    ("Finger bass", &[]),
                    (
                        "Pick bass",
                        &[("hardness", 0.85), ("position", 0.13), ("damping", 0.1)],
                    ),
                    (
                        "Muted bass",
                        &[
                            ("hardness", 0.25),
                            ("damping", 0.8),
                            ("decay", 0.65),
                            ("release", 0.06),
                        ],
                    ),
                    (
                        "Upright bass",
                        &[("body", 1.0), ("hardness", 0.3), ("position", 0.25)],
                    ),
                ],
            ),
            family(
                Physical::new(Model::Bell),
                &[
                    ("Classic bell", &[]),
                    (
                        "Soft bell",
                        &[("hardness", 0.15), ("damping", 0.35), ("decay", 3.0)],
                    ),
                    ("Bright bell", &[("hardness", 0.95), ("damping", 0.05)]),
                    ("Tubular bells", &[("bell", 1.0)]),
                    ("Handbell", &[("bell", 2.0), ("decay", 4.0)]),
                    (
                        "Singing bowl",
                        &[("bell", 3.0), ("hardness", 0.3), ("decay", 14.0)],
                    ),
                ],
            ),
            family(
                Physical::new(Model::Mallet),
                &[
                    ("Marimba", &[]),
                    ("Vibraphone", &[("bar", 1.0), ("tremolo", 0.3)]),
                    ("Xylophone", &[("bar", 2.0), ("hardness", 0.7)]),
                    ("Glockenspiel", &[("bar", 3.0), ("hardness", 0.8)]),
                    ("Kalimba", &[("bar", 4.0), ("hardness", 0.5), ("body", 0.6)]),
                    (
                        "Music box",
                        &[("bar", 4.0), ("hardness", 0.85), ("body", 0.2)],
                    ),
                    (
                        "Hard mallet",
                        &[("hardness", 0.9), ("decay", 0.8), ("damping", 0.2)],
                    ),
                    (
                        "Muted mallet",
                        &[("hardness", 0.3), ("decay", 0.35), ("damping", 0.8)],
                    ),
                ],
            ),
            family(
                Physical::new(Model::Clarinet),
                &[
                    ("Clarinet", &[]),
                    (
                        "Soft clarinet",
                        &[
                            ("pressure", 0.3),
                            ("reed_stiffness", 0.32),
                            ("breath_noise", 0.07),
                            ("attack", 0.12),
                        ],
                    ),
                    (
                        "Breathy clarinet",
                        &[
                            ("pressure", 0.48),
                            ("reed_stiffness", 0.58),
                            ("breath_noise", 0.5),
                            ("attack", 0.06),
                        ],
                    ),
                ],
            ),
            family(
                Physical::new(Model::HammeredDulcimer),
                &[
                    ("Hammered dulcimer", &[]),
                    (
                        "Bright dulcimer",
                        &[
                            ("hardness", 0.75),
                            ("position", 0.1),
                            ("damping", 0.08),
                            ("detune", 2.0),
                        ],
                    ),
                    (
                        "Muted dulcimer",
                        &[
                            ("hardness", 0.2),
                            ("decay", 1.0),
                            ("damping", 0.7),
                            ("release", 0.06),
                        ],
                    ),
                ],
            ),
            family(
                Physical::new(Model::TinWhistle),
                &[
                    ("Tin whistle", &[]),
                    (
                        "Soft whistle",
                        &[
                            ("pressure", 0.3),
                            ("jet_shape", 0.18),
                            ("breath_noise", 0.05),
                            ("attack", 0.03),
                        ],
                    ),
                    (
                        "Airy whistle",
                        &[
                            ("pressure", 0.4),
                            ("jet_shape", 0.28),
                            ("breath_noise", 0.32),
                        ],
                    ),
                    ("Recorder", &[("jet_shape", 0.12), ("breath_noise", 0.06)]),
                ],
            ),
            family(
                Choir::new(),
                &[
                    ("Mixed choir", &[]),
                    (
                        "Soprano choir",
                        &[("voice_size", 0.24), ("tone", 0.3), ("vowel", 1.0)],
                    ),
                    (
                        "Tenor choir",
                        &[("voice_size", 0.65), ("tone", 0.2), ("vowel", 1.0)],
                    ),
                    (
                        "Bass choir",
                        &[("voice_size", 0.85), ("tone", 0.18), ("vowel", 1.0)],
                    ),
                ],
            ),
            family(
                DrumKit::new(),
                &[
                    ("Natural drum kit", &[]),
                    (
                        "Dry drum kit",
                        &[("hardness", 0.75), ("damping", 0.85), ("decay", 0.6)],
                    ),
                    (
                        "Resonant drum kit",
                        &[("hardness", 0.5), ("damping", 0.12), ("decay", 1.8)],
                    ),
                ],
            ),
        ]
    })
}

/// Every physical factory patch, flattened in family order for search and other frontends.
pub fn factory_sounds() -> Vec<SoundPreset> {
    factory_sound_families()
        .iter()
        .flat_map(|family| family.presets.iter().cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_physical_instrument_has_unique_valid_derivatives_and_unchanged_default_parameters() {
        let registry = crate::plugin_catalogue();
        let expected: HashSet<_> = registry
            .instruments()
            .filter(|descriptor| {
                descriptor.id.starts_with("auris.physical.") || descriptor.id == DrumKit::ID
            })
            .map(|descriptor| descriptor.id.to_string())
            .collect();
        let actual: HashSet<_> = factory_sound_families()
            .iter()
            .map(|family| family.instrument_id.clone())
            .collect();
        assert_eq!(actual, expected);
        let mut names = HashSet::new();
        for family in factory_sound_families() {
            let instrument = registry.create_instrument(&family.instrument_id).unwrap();
            assert!(family.presets.len() >= 3, "{}", family.instrument_id);
            for (index, sound) in family.presets.iter().enumerate() {
                assert!(names.insert(&sound.name), "duplicate name {}", sound.name);
                assert_eq!(sound.instrument_id, family.instrument_id);
                assert_eq!(sound.state.params.len(), instrument.parameters().len());
                for param in instrument.parameters() {
                    let value = sound.state.params[param.key.as_ref()];
                    assert!(
                        value.is_finite() && (param.min..=param.max).contains(&value),
                        "{} / {} = {value}",
                        sound.name,
                        param.key
                    );
                    if index == 0 {
                        assert_eq!(value, param.default, "{} / {}", sound.name, param.key);
                    }
                }
                if index > 0 {
                    assert_ne!(sound.state, family.presets[0].state);
                }
                assert_ne!(
                    auris_i18n::audio::plugin_name(&sound.name, auris_i18n::Language::Japanese),
                    sound.name
                );
            }
        }
        assert_eq!(names.len(), factory_sounds().len());
    }
}
