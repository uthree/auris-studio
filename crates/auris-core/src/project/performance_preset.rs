//! Named performance stacks: a starting feel for a kind of instrument.
//!
//! A clip's performance is a [`NoteTransform`] stack with a dozen controls, and most of them are
//! answered the same way for every part of one kind: a strummed acoustic guitar strums, a finger
//! bass leaves quiet pickups and mutes its long notes, a piano ballad holds the pedal down. A
//! preset is that answer written down once, under a name and an instrument family, so a clip can
//! start from it instead of from twelve neutral dials.
//!
//! **Applying a preset copies its stack into the clip.** The clip never refers to the preset by
//! name afterwards, for the reason a kept chord progression is copied rather than quoted: a
//! document sent to somebody who has never seen your presets still plays the performance you
//! chose, and editing a preset later never rewrites a clip behind your back — the score does not
//! change, and neither does a take that has already been chosen.
//!
//! The built-in catalogue lives here, beside the transforms it is made of; presets a person or an
//! agent keeps are a file on a disk and live in `auris-session`. Both are the same
//! [`PerformancePreset`], so the one picker and the one agent tool offer them side by side.
//!
//! None of the built-ins swings. Swing is the song's feel, decided once for the whole band, and a
//! preset installed on one part would put that part's offbeats late against all the others.

use serde::{Deserialize, Serialize};

use super::transform::NoteTransform;
use super::{
    Expression, GhostNotes, GhostPattern, PitchPerformance, StrokeDirection, Strum, StrumClock,
};

/// The kind of instrument a preset was written for.
///
/// A filing label for the picker and the agent's list, not a restriction: any preset can go on
/// any clip, with the one exception that pitch gestures and octave doubling are left off a clip
/// whose instrument has no pitch to bend.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetFamily {
    /// Piano, electric piano, organ and other keyboards.
    Keys,
    /// Acoustic and electric guitars, comping or lead.
    Guitar,
    /// Electric, acoustic and synth bass.
    Bass,
    /// Drum kits and percussion.
    Drums,
    /// Bowed and plucked orchestral strings.
    Strings,
    /// Trumpets, trombones, horns and brass sections.
    Brass,
    /// Flutes, clarinets, saxophones and other winds.
    Woodwinds,
    /// Synthesiser leads, pads, arpeggios and chiptune voices.
    Synth,
    /// Anything else.
    Other,
}

impl PresetFamily {
    /// Every family, in the order a picker lists them.
    pub const ALL: [PresetFamily; 9] = [
        Self::Keys,
        Self::Guitar,
        Self::Bass,
        Self::Drums,
        Self::Strings,
        Self::Brass,
        Self::Woodwinds,
        Self::Synth,
        Self::Other,
    ];

    /// The name a file and an agent use.
    pub fn name(self) -> &'static str {
        match self {
            Self::Keys => "keys",
            Self::Guitar => "guitar",
            Self::Bass => "bass",
            Self::Drums => "drums",
            Self::Strings => "strings",
            Self::Brass => "brass",
            Self::Woodwinds => "woodwinds",
            Self::Synth => "synth",
            Self::Other => "other",
        }
    }

    /// The family a name means, if any.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|family| family.name() == name)
    }
}

/// A named performance stack for one kind of instrument.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformancePreset {
    /// What the preset is called. A built-in's name is a stable identifier in `snake_case`,
    /// which the window translates; a kept preset's name is whatever its author typed.
    pub name: String,
    /// The kind of instrument it was written for.
    pub family: PresetFamily,
    /// One line on what it sounds like and when to reach for it.
    #[serde(default)]
    pub description: String,
    /// The stages, in the order they are applied. Seeds stored here are placeholders:
    /// [`Self::stack_for`] gives every clip its own take.
    pub transforms: Vec<NoteTransform>,
}

impl PerformancePreset {
    /// The stack one clip receives: the preset's stages with every random take reseeded from
    /// `seed`, so two clips given one preset do not repeat each other's accidents.
    ///
    /// `pitched` false leaves out the pitch gestures and the octave copies, which mean nothing
    /// on a drum kit; their names are returned so a caller can say what was skipped.
    pub fn stack_for(&self, seed: u64, pitched: bool) -> (Vec<NoteTransform>, Vec<&'static str>) {
        let mut skipped = Vec::new();
        let stack = self
            .transforms
            .iter()
            .filter_map(|stage| reseeded(stage, seed, pitched, &mut skipped))
            .collect();
        (stack, skipped)
    }
}

fn reseeded(
    stage: &NoteTransform,
    seed: u64,
    pitched: bool,
    skipped: &mut Vec<&'static str>,
) -> Option<NoteTransform> {
    let mut stage = stage.clone();
    match &mut stage {
        NoteTransform::Humanize { seed: held, .. } => *held = seed,
        NoteTransform::Expression { settings } => settings.seed = seed,
        NoteTransform::Ghost { settings } => settings.seed = seed,
        NoteTransform::Pitch { .. } if !pitched => {
            skipped.push("pitch");
            return None;
        }
        NoteTransform::Octaves { .. } if !pitched => {
            skipped.push("octaves");
            return None;
        }
        NoteTransform::ForDrumVoice { transforms, .. } => {
            *transforms = transforms
                .iter()
                .filter_map(|inner| reseeded(inner, seed, pitched, skipped))
                .collect();
        }
        _ => {}
    }
    Some(stage)
}

/// The presets the application ships with, in picker order.
pub fn builtin_performance_presets() -> Vec<PerformancePreset> {
    use NoteTransform as T;
    use PresetFamily::*;
    let expression =
        |timing: f32, velocity: f32, swell: f32, accent: f32, shared: f32| T::Expression {
            settings: Expression {
                timing,
                velocity,
                swell,
                accent,
                shared,
                ..Expression::default()
            },
        };
    let ghost = |density, velocity, pattern, length_ms, target_pitch| T::Ghost {
        settings: GhostNotes {
            density,
            velocity,
            pattern,
            length_ms,
            max_gap_beats: 1.0,
            target_pitch,
            ..GhostNotes::default()
        },
    };
    let pitch = |scoop: f32, vibrato: f32, fall: f32, glide_ms: f32| T::Pitch {
        settings: PitchPerformance {
            scoop,
            vibrato,
            fall,
            glide_ms,
            scoop_ms: 75.0,
            fall_ms: 100.0,
            vibrato_delay_ms: 250.0,
            ..PitchPerformance::default()
        },
    };
    let strum = |spread_ms, clock, up_notes, up_velocity, low_accent| T::Strum {
        settings: Strum {
            spread_ms,
            clock,
            up_notes,
            up_velocity,
            low_accent,
            ..Strum::default()
        },
    };
    let preset = |name: &str, family, description: &str, transforms| PerformancePreset {
        name: name.into(),
        family,
        description: description.into(),
        transforms,
    };
    // Snare on a General MIDI kit, which is what the shipped kits and the composer's drum
    // writers both use; a kit mapped elsewhere simply gets no ghosts from these.
    let snare = Some(38);
    vec![
        preset(
            "piano_ballad",
            Keys,
            "Pedalled piano: chords ring a quarter note past their release, a slight rolled attack and a gently breathing phrase.",
            vec![
                T::Stroke {
                    spread_ms: 6.0,
                    direction: StrokeDirection::Alternate,
                },
                expression(0.25, 0.30, 0.14, 0.04, 0.5),
                T::Sustain { ticks: 960 },
            ],
        ),
        preset(
            "piano_pop_comp",
            Keys,
            "Tight pop piano comping: a quick rolled chord, slightly detached, firm on the beat.",
            vec![
                T::Gate { amount: 0.9 },
                T::Stroke {
                    spread_ms: 5.0,
                    direction: StrokeDirection::Alternate,
                },
                expression(0.20, 0.25, 0.06, 0.06, 0.45),
            ],
        ),
        preset(
            "jazz_piano_comp",
            Keys,
            "Jazz comping: wider roll, short stabs and the offbeats leaned on rather than the beat.",
            vec![
                T::Gate { amount: 0.75 },
                T::Stroke {
                    spread_ms: 9.0,
                    direction: StrokeDirection::Alternate,
                },
                expression(0.30, 0.35, 0.06, -0.08, 0.6),
            ],
        ),
        preset(
            "electric_piano_soul",
            Keys,
            "Soul or city-pop electric piano: a soft roll, an eighth-note pedal and a laid-back offbeat lilt.",
            vec![
                T::Stroke {
                    spread_ms: 7.0,
                    direction: StrokeDirection::LowToHigh,
                },
                expression(0.25, 0.30, 0.08, -0.08, 0.55),
                T::Sustain { ticks: 480 },
            ],
        ),
        preset(
            "organ_sustain",
            Keys,
            "Organ: chords held smoothly into each other with almost no change in loudness.",
            vec![
                expression(0.15, 0.05, 0.0, 0.0, 0.5),
                T::Sustain { ticks: 120 },
            ],
        ),
        preset(
            "acoustic_strum",
            Guitar,
            "Strummed acoustic guitar: continuous eighth-note hand, lighter upstrokes on the top strings, sparse brushes and muted chord tails.",
            vec![
                ghost(0.10, 0.10, GhostPattern::Sixteenths, 12.0, None),
                T::Mute { amount: 0.18 },
                strum(22.0, StrumClock::Eighths, 3, 0.85, 0.06),
                expression(0.30, 0.30, 0.06, 0.06, 0.45),
            ],
        ),
        preset(
            "rock_rhythm_guitar",
            Guitar,
            "Driving rock rhythm guitar: fast downstroke-led strums, palm-muted tails and slightly clipped chords.",
            vec![
                T::Gate { amount: 0.9 },
                T::Mute { amount: 0.25 },
                strum(14.0, StrumClock::Eighths, 2, 0.8, 0.12),
                expression(0.25, 0.25, 0.04, 0.08, 0.35),
            ],
        ),
        preset(
            "funk_cutting_guitar",
            Guitar,
            "Funk cutting: very short sixteenth-note chops with quiet scratches in the gaps.",
            vec![
                T::Gate { amount: 0.45 },
                ghost(0.35, 0.12, GhostPattern::Sixteenths, 10.0, None),
                T::Mute { amount: 0.30 },
                strum(10.0, StrumClock::Sixteenths, 3, 0.75, 0.05),
                expression(0.20, 0.30, 0.0, -0.06, 0.5),
            ],
        ),
        preset(
            "lead_guitar",
            Guitar,
            "Singing lead guitar: scoops into notes, wide vibrato, falls at phrase ends and slides between distant notes.",
            vec![
                T::Slide { amount: 0.30 },
                expression(0.30, 0.30, 0.08, 0.06, 0.35),
                pitch(0.45, 0.23, 0.40, 45.0),
            ],
        ),
        preset(
            "finger_bass",
            Bass,
            "Fingered electric bass: quiet pickups before the next note and lightly muted long notes.",
            vec![
                ghost(0.10, 0.14, GhostPattern::Pickup, 16.0, None),
                T::Mute { amount: 0.10 },
                expression(0.30, 0.30, 0.04, 0.06, 0.45),
            ],
        ),
        preset(
            "funk_bass",
            Bass,
            "Funk or city-pop bass: offbeat ghost notes, slides between registers and short, punchy notes.",
            vec![
                T::Gate { amount: 0.7 },
                ghost(0.18, 0.18, GhostPattern::Offbeats, 18.0, None),
                T::Slide { amount: 0.20 },
                T::Mute { amount: 0.10 },
                expression(0.30, 0.35, 0.04, -0.08, 0.55),
            ],
        ),
        preset(
            "synth_bass_tight",
            Bass,
            "Sequenced synth bass: on the grid, even in level and a little detached.",
            vec![
                T::Gate { amount: 0.8 },
                expression(0.0, 0.08, 0.0, 0.04, 0.2),
            ],
        ),
        preset(
            "pop_drums",
            Drums,
            "Pop band drums: the kit on the grid with gentle level variation and a few soft snare ghosts.",
            vec![
                ghost(0.10, 0.16, GhostPattern::Pickup, 16.0, snare),
                expression(0.0, 0.09, 0.0, 0.06, 0.45),
            ],
        ),
        preset(
            "funk_drums",
            Drums,
            "Funk drums: busy snare ghost notes between the backbeats and accents on the offbeats.",
            vec![
                ghost(0.30, 0.16, GhostPattern::Sixteenths, 14.0, snare),
                expression(0.0, 0.12, 0.0, -0.06, 0.5),
            ],
        ),
        preset(
            "jazz_drums",
            Drums,
            "Jazz drums: laid back behind the beat, soft snare chatter and a light, varied touch.",
            vec![
                ghost(0.20, 0.16, GhostPattern::Pickup, 16.0, snare),
                T::Lean { ticks: 12 },
                expression(0.15, 0.15, 0.0, -0.08, 0.6),
            ],
        ),
        preset(
            "machine_drums",
            Drums,
            "Drum machine: exactly on the grid and exactly as loud as written.",
            vec![],
        ),
        preset(
            "string_legato",
            Strings,
            "Legato strings: notes overlap into each other, phrases swell and long notes take a delayed vibrato.",
            vec![
                expression(0.20, 0.20, 0.18, 0.0, 0.8),
                T::Sustain { ticks: 120 },
                pitch(0.03, 0.08, 0.0, 0.0),
            ],
        ),
        preset(
            "string_pizzicato",
            Strings,
            "Pizzicato strings: short plucked notes with a lively, uneven touch.",
            vec![
                T::Gate { amount: 0.35 },
                expression(0.20, 0.25, 0.06, 0.04, 0.7),
            ],
        ),
        preset(
            "brass_section",
            Brass,
            "Brass section: punchy, slightly detached hits with small scoops and falls.",
            vec![
                T::Gate { amount: 0.9 },
                expression(0.20, 0.25, 0.10, 0.08, 0.6),
                pitch(0.20, 0.06, 0.30, 0.0),
            ],
        ),
        preset(
            "woodwind_solo",
            Woodwinds,
            "Solo flute, clarinet or saxophone: connected phrases with a delayed vibrato and a breathing swell.",
            vec![
                expression(0.25, 0.25, 0.16, 0.04, 0.4),
                pitch(0.10, 0.12, 0.08, 30.0),
            ],
        ),
        preset(
            "synth_lead",
            Synth,
            "Synthwave-style lead: gliding between notes with a light vibrato, scoop and fall.",
            vec![
                expression(0.15, 0.10, 0.08, 0.04, 0.2),
                pitch(0.18, 0.10, 0.20, 55.0),
            ],
        ),
        preset(
            "synth_pad",
            Synth,
            "Synth pad: chords bleed a half note into the next and swell across the phrase.",
            vec![
                expression(0.0, 0.05, 0.18, 0.0, 0.65),
                T::Sustain { ticks: 1920 },
            ],
        ),
        preset(
            "synth_arp_tight",
            Synth,
            "Sequenced arpeggio: on the grid, short and even.",
            vec![
                T::Gate { amount: 0.6 },
                expression(0.0, 0.06, 0.0, 0.04, 0.2),
            ],
        ),
        preset(
            "chiptune",
            Synth,
            "Chiptune voice: machine-tight timing, slightly detached notes and a tiny vibrato and glide.",
            vec![
                T::Gate { amount: 0.9 },
                expression(0.05, 0.05, 0.0, 0.0, 0.1),
                pitch(0.08, 0.06, 0.0, 18.0),
            ],
        ),
    ]
}

/// The built-in preset called `name`, if there is one.
pub fn builtin_performance_preset(name: &str) -> Option<PerformancePreset> {
    builtin_performance_presets()
        .into_iter()
        .find(|preset| preset.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_has_a_unique_identifier_and_a_description() {
        let presets = builtin_performance_presets();
        let names: std::collections::BTreeSet<_> = presets.iter().map(|p| &p.name).collect();
        assert_eq!(names.len(), presets.len());
        for preset in &presets {
            assert!(
                preset
                    .name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_'),
                "{}",
                preset.name
            );
            assert!(!preset.description.is_empty(), "{}", preset.name);
        }
        for family in PresetFamily::ALL {
            if family != PresetFamily::Other {
                assert!(
                    presets.iter().any(|p| p.family == family),
                    "{family:?} has no built-in"
                );
            }
        }
    }

    #[test]
    fn no_builtin_swings_because_swing_belongs_to_the_whole_song() {
        for preset in builtin_performance_presets() {
            assert!(
                !preset
                    .transforms
                    .iter()
                    .any(|stage| matches!(stage, NoteTransform::Swing { .. })),
                "{}",
                preset.name
            );
        }
    }

    #[test]
    fn a_clip_gets_its_own_take_and_a_drum_kit_no_pitch_gestures() {
        let lead = builtin_performance_preset("lead_guitar").unwrap();
        let (stack, skipped) = lead.stack_for(7, true);
        assert!(skipped.is_empty());
        assert!(stack.iter().any(
            |stage| matches!(stage, NoteTransform::Expression { settings } if settings.seed == 7)
        ));
        let (other, _) = lead.stack_for(8, true);
        assert_ne!(stack, other, "a different clip is a different take");

        let (drums, skipped) = lead.stack_for(7, false);
        assert_eq!(skipped, vec!["pitch"]);
        assert!(
            !drums
                .iter()
                .any(|stage| matches!(stage, NoteTransform::Pitch { .. }))
        );
    }

    #[test]
    fn a_preset_round_trips_through_its_file_form() {
        let preset = builtin_performance_preset("acoustic_strum").unwrap();
        let text = serde_json::to_string(&preset).unwrap();
        assert!(text.contains("\"family\":\"guitar\""));
        let back: PerformancePreset = serde_json::from_str(&text).unwrap();
        assert_eq!(back, preset);
        assert_eq!(
            PresetFamily::from_name("guitar"),
            Some(PresetFamily::Guitar)
        );
    }
}
