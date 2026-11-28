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
/// A patch that changes how loud the instrument comes out carries its own `level` as well, set
/// so that it plays at [`auris_synth::level_reference::REFERENCE_LUFS`] like its family's first.
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
                            ("level", -12.5),
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
                            ("level", -13.0),
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
                            ("level", -13.0),
                        ],
                    ),
                    (
                        "Pizzicato strings",
                        &[("technique", 1.0), ("players", 4.0), ("level", -2.0)],
                    ),
                    (
                        "Tremolo strings",
                        &[("technique", 2.0), ("players", 4.0), ("level", -9.5)],
                    ),
                    (
                        "Sul ponticello",
                        &[
                            ("position", 0.045),
                            ("bow_pressure", 0.6),
                            ("players", 2.0),
                            ("level", -13.5),
                        ],
                    ),
                    (
                        "Sul tasto",
                        &[
                            ("position", 0.2),
                            ("bow_pressure", 0.25),
                            ("bow_speed", 0.6),
                            ("damping", 0.5),
                            ("level", -8.5),
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
                        &[
                            ("hardness", 0.15),
                            ("damping", 0.2),
                            ("sympathetic", 0.4),
                            ("level", -11.0),
                        ],
                    ),
                    ("Bright piano", &[("hardness", 0.9), ("position", 0.1)]),
                    (
                        "Honky-tonk piano",
                        &[("detune", 7.0), ("hardness", 0.6), ("level", -10.9)],
                    ),
                ],
            ),
            family(
                Physical::new(Model::Guitar),
                &[
                    ("Acoustic guitar", &[]),
                    (
                        "Nylon guitar",
                        &[("hardness", 0.06), ("position", 0.28), ("level", -12.0)],
                    ),
                    (
                        "Steel guitar",
                        &[
                            ("hardness", 0.45),
                            ("position", 0.22),
                            ("decay", 5.5),
                            ("damping", 0.08),
                            ("level", -11.5),
                        ],
                    ),
                    (
                        "Muted guitar",
                        &[
                            ("hardness", 0.5),
                            ("damping", 0.8),
                            ("decay", 0.55),
                            ("release", 0.06),
                            ("level", 0.5),
                        ],
                    ),
                    ("Piezo guitar", &[("pickup", 1.0), ("level", -15.5)]),
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
                            ("hardness", 0.12),
                            ("level", -12.0),
                        ],
                    ),
                    (
                        "Bright electric guitar",
                        &[
                            ("tone", 6500.0),
                            ("pickup_position", 0.08),
                            ("hardness", 0.4),
                            ("level", -10.0),
                        ],
                    ),
                    ("Guitar harmonics", &[("technique", 1.0), ("level", -8.0)]),
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
                            ("level", 3.0),
                        ],
                    ),
                    (
                        "Upright bass",
                        &[
                            ("body", 1.0),
                            ("hardness", 0.3),
                            ("position", 0.25),
                            ("decay", 7.0),
                            ("level", -9.0),
                        ],
                    ),
                    // The hand over the last fret, the bone of the thumb, fresh strings and
                    // a pickup with its top left open.
                    (
                        "Slap bass",
                        &[
                            ("technique", 1.0),
                            ("position", 0.32),
                            ("hardness", 0.7),
                            ("damping", 0.05),
                            ("tone", 4500.0),
                            ("level", -10.0),
                        ],
                    ),
                ],
            ),
            family(
                Physical::new(Model::Bell),
                &[
                    ("Classic bell", &[]),
                    (
                        "Soft bell",
                        &[
                            ("hardness", 0.15),
                            ("damping", 0.35),
                            ("decay", 3.0),
                            ("level", -6.5),
                        ],
                    ),
                    (
                        "Bright bell",
                        &[("hardness", 0.95), ("damping", 0.05), ("level", -13.0)],
                    ),
                    ("Tubular bells", &[("bell", 1.0), ("level", -10.5)]),
                    (
                        "Handbell",
                        &[("bell", 2.0), ("decay", 4.0), ("level", -7.0)],
                    ),
                    (
                        "Singing bowl",
                        &[
                            ("bell", 3.0),
                            ("hardness", 0.3),
                            ("decay", 14.0),
                            ("level", -4.0),
                        ],
                    ),
                ],
            ),
            family(
                Physical::new(Model::Mallet),
                &[
                    ("Marimba", &[]),
                    (
                        "Vibraphone",
                        &[("bar", 1.0), ("tremolo", 0.3), ("level", -16.0)],
                    ),
                    (
                        "Xylophone",
                        &[("bar", 2.0), ("hardness", 0.7), ("level", -9.5)],
                    ),
                    (
                        "Glockenspiel",
                        &[("bar", 3.0), ("hardness", 0.8), ("level", -11.5)],
                    ),
                    (
                        "Kalimba",
                        &[
                            ("bar", 4.0),
                            ("hardness", 0.5),
                            ("body", 0.6),
                            ("level", -11.0),
                        ],
                    ),
                    (
                        "Music box",
                        &[
                            ("bar", 4.0),
                            ("hardness", 0.85),
                            ("body", 0.2),
                            ("level", -11.0),
                        ],
                    ),
                    (
                        "Hard mallet",
                        &[
                            ("hardness", 0.9),
                            ("decay", 0.8),
                            ("damping", 0.2),
                            ("level", -8.0),
                        ],
                    ),
                    (
                        "Muted mallet",
                        &[
                            ("hardness", 0.3),
                            ("decay", 0.35),
                            ("damping", 0.8),
                            ("level", 1.0),
                        ],
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
                            ("pressure", 0.38),
                            ("reed_stiffness", 0.32),
                            ("breath_noise", 0.07),
                            ("attack", 0.12),
                            ("level", 2.5),
                        ],
                    ),
                    (
                        "Breathy clarinet",
                        &[
                            ("pressure", 0.48),
                            ("reed_stiffness", 0.58),
                            ("breath_noise", 0.5),
                            ("attack", 0.06),
                            ("level", -10.0),
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
                            ("level", -8.5),
                        ],
                    ),
                    (
                        "Muted dulcimer",
                        &[
                            ("hardness", 0.2),
                            ("decay", 1.0),
                            ("damping", 0.7),
                            ("release", 0.06),
                            ("level", -16.5),
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
                            ("level", -6.5),
                        ],
                    ),
                    (
                        "Airy whistle",
                        &[
                            ("pressure", 0.4),
                            ("jet_shape", 0.28),
                            ("breath_noise", 0.32),
                            ("level", -9.5),
                        ],
                    ),
                    (
                        "Recorder",
                        &[
                            ("jet_shape", 0.12),
                            ("breath_noise", 0.06),
                            ("level", -12.5),
                        ],
                    ),
                ],
            ),
            family(
                Choir::new(),
                &[
                    ("Mixed choir", &[]),
                    (
                        "Soprano choir",
                        &[
                            ("voice_size", 0.24),
                            ("tone", 0.3),
                            ("vowel", 1.0),
                            ("level", -6.5),
                        ],
                    ),
                    (
                        "Tenor choir",
                        &[("voice_size", 0.65), ("tone", 0.2), ("vowel", 1.0)],
                    ),
                    (
                        "Bass choir",
                        &[
                            ("voice_size", 0.85),
                            ("tone", 0.18),
                            ("vowel", 1.0),
                            ("level", -7.0),
                        ],
                    ),
                ],
            ),
            family(
                DrumKit::new(),
                &[
                    ("Natural drum kit", &[]),
                    (
                        "Dry drum kit",
                        &[
                            ("hardness", 0.75),
                            ("damping", 0.85),
                            ("decay", 0.6),
                            ("level", 3.3),
                        ],
                    ),
                    (
                        "Resonant drum kit",
                        &[
                            ("hardness", 0.5),
                            ("damping", 0.12),
                            ("decay", 1.8),
                            ("level", -6.3),
                        ],
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

    /// Where a patch's melody sits when its family's register would put it out of its own:
    /// the low strings and the bass choir lower, the small steel bars higher.
    fn patch_register(name: &str, id: &str) -> u8 {
        match name {
            "Contrabass" => 36,
            "Cello" | "Bass choir" => 48,
            "Glockenspiel" | "Music box" => 72,
            _ => auris_synth::level_reference::register(id),
        }
    }

    #[test]
    fn every_factory_patch_plays_at_the_built_in_reference_loudness() {
        use auris_synth::level_reference::{REFERENCE_LUFS, beat, melody, phrase_loudness};
        // The same band as the instruments' own: a patch that changes the sound sets its own
        // `level` to make up what the change costs or adds, so that picking one never needs the
        // fader. The soft clarinet is the one whose breath was raised instead, from just under
        // where the reed barely speaks to just over it.
        const TOLERANCE_DB: f32 = 1.0;
        let registry = crate::plugin_catalogue();
        let mut table = String::new();
        let mut outliers = Vec::new();
        for sound in factory_sounds() {
            let mut instrument = registry.create_instrument(&sound.instrument_id).unwrap();
            instrument.load_state(&sound.state);
            let events = if auris_synth::is_drum_kit(&sound.instrument_id) {
                beat(&sound.instrument_id)
            } else {
                melody(patch_register(&sound.name, &sound.instrument_id))
            };
            let lufs = phrase_loudness(instrument.as_mut(), &events).unwrap_or(f32::NEG_INFINITY);
            table.push_str(&format!(
                "{:24} {lufs:7.2} LUFS ({:+.2})\n",
                sound.name,
                lufs - REFERENCE_LUFS
            ));
            if (lufs - REFERENCE_LUFS).abs() > TOLERANCE_DB {
                outliers.push(sound.name);
            }
        }
        println!("{table}");
        assert!(outliers.is_empty(), "{outliers:?}\n{table}");
    }
}
