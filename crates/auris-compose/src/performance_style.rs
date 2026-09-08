//! Genre and instrument decisions for a composed clip's editable performance stack.

use auris_core::{
    Expression, GhostNotes, GhostPattern, NoteTransform, PitchPerformance, StrokeDirection, Strum,
    StrumClock,
};
use serde::{Deserialize, Serialize};

use crate::spec::{PartSpec, Role};

/// The performance palette a song installs on its clips, independently of its written notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PerformanceStyle {
    /// Tight oscillators with a small amount of lead motion.
    Chiptune,
    /// Restrained band phrasing and soft bass pickups.
    PopBand,
    /// Laid-back accents, saxophone gestures and syncopated bass pickups.
    CityPop,
    /// Alternating guitar strokes, muted tails and expressive lead bends.
    Rock,
    /// Piano attack spread, offbeat dynamics and quiet kit pickups.
    JazzTrio,
    /// Shared phrase dynamics and restrained solo wind/string vibrato.
    Orchestral,
    /// Tight rhythm with connected synthesizer leads.
    Synthwave,
    /// Slow shared dynamics and subtle bowed-bass motion.
    Ambient,
}

/// Builds a genre's initial stack. `seed` names the clip; `group` names the whole ensemble.
/// The humanize dial scales wander and lean, while intentional articulations keep their depth.
pub(crate) fn styled_performance(
    part: &PartSpec,
    style: PerformanceStyle,
    looseness: f32,
    seed: u64,
    group: u64,
) -> Vec<NoteTransform> {
    use PerformanceStyle::*;
    let role = part.role;
    let program = part.program.map(|program| program.0);
    let mut stack = Vec::new();
    let ghost = |density, velocity, pattern, length_ms| NoteTransform::Ghost {
        settings: GhostNotes {
            density,
            velocity,
            pattern,
            length_ms,
            max_gap_beats: 1.0,
            seed,
            ..GhostNotes::default()
        },
    };

    // A drum's original attacks keep the clock. Scope is attached when kit parts are merged.
    if role == Role::Snare {
        let density = match style {
            PopBand => 0.10,
            CityPop => 0.18,
            Rock => 0.08,
            JazzTrio => 0.20,
            _ => 0.0,
        };
        if density > 0.0 {
            stack.push(ghost(density, 0.16, GhostPattern::Pickup, 16.0));
        }
    }

    let comp = matches!(role, Role::Chords | Role::Stab);
    let guitar = program.is_some_and(|p| (24..=31).contains(&p));
    if comp && guitar {
        // Sparse sixteenth brushes preserve deliberate rests and the final release.
        stack.push(ghost(0.10, 0.10, GhostPattern::Sixteenths, 12.0));
        stack.push(NoteTransform::Strum {
            settings: Strum {
                spread_ms: if style == Rock { 14.0 } else { 22.0 },
                clock: StrumClock::Eighths,
                up_notes: 3,
                up_velocity: 0.85,
                low_accent: 0.06,
                ..Strum::default()
            },
        });
        stack.push(NoteTransform::Mute { amount: 0.18 });
    } else if comp && program.is_some_and(|p| p <= 7) {
        stack.push(NoteTransform::Stroke {
            spread_ms: if style == JazzTrio { 9.0 } else { 5.0 },
            direction: StrokeDirection::Alternate,
        });
    }
    if role == Role::Bass && program.is_some_and(|p| (32..=37).contains(&p)) {
        match style {
            CityPop => {
                stack.push(ghost(0.18, 0.18, GhostPattern::Offbeats, 18.0));
                stack.push(NoteTransform::Slide { amount: 0.20 });
                stack.push(NoteTransform::Mute { amount: 0.10 });
            }
            PopBand | Rock => {
                stack.push(ghost(0.10, 0.14, GhostPattern::Pickup, 16.0));
                stack.push(NoteTransform::Mute { amount: 0.10 });
            }
            _ => {}
        }
    }

    let shared = match style {
        Chiptune => 0.10,
        PopBand => 0.45,
        CityPop => 0.55,
        Rock => 0.35,
        JazzTrio => 0.60,
        Orchestral => 0.80,
        Synthwave => 0.20,
        Ambient => 0.65,
    };
    let swell = match role {
        Role::Melody => {
            if matches!(style, Orchestral | Ambient) {
                0.16
            } else {
                0.08
            }
        }
        Role::Pad => {
            if matches!(style, Orchestral | Ambient) {
                0.18
            } else {
                0.08
            }
        }
        Role::Chords | Role::Arp => 0.06,
        Role::Bass => 0.04,
        _ => 0.0,
    };
    // Replace the old combined wander, rather than stacking two sources of humanisation.
    stack.extend(
        crate::perform::part_performance(role, looseness, seed)
            .into_iter()
            .filter(|stage| matches!(stage, NoteTransform::Lean { .. })),
    );
    if !matches!(role, Role::Kick | Role::Crash | Role::Riser) {
        stack.push(NoteTransform::Expression {
            settings: Expression {
                timing: if role.is_drum() {
                    0.0
                } else {
                    looseness.clamp(0.0, 1.0)
                },
                velocity: looseness.clamp(0.0, 1.0) * if role.is_drum() { 0.25 } else { 1.0 },
                swell,
                accent: if matches!(style, JazzTrio | CityPop) {
                    -0.08
                } else {
                    0.06
                },
                shared,
                group,
                seed,
                ..Expression::default()
            },
        });
    }

    // A melodic role alone is insufficient: pianos, bells and other fixed-pitch sounds
    // must stay fixed when a user substitutes their instrument in a preset.
    let bendable = program.map_or_else(
        || {
            matches!(
                part.instrument.as_str(),
                "auris.synth.chiptune" | "auris.synth.fm2"
            )
        },
        |p| matches!(p, 24..=31 | 40..=43 | 56..=87),
    );
    if role == Role::Melody && bendable {
        let (scoop, vibrato, fall, glide_ms) = match style {
            Chiptune => (0.08, 0.06, 0.0, 18.0),
            PopBand => (0.25, 0.12, 0.12, 35.0),
            CityPop => (0.65, 0.17, 0.45, 45.0),
            Rock => (0.45, 0.23, 0.40, 45.0),
            JazzTrio => (0.22, 0.10, 0.12, 25.0),
            Orchestral => (0.03, 0.08, 0.0, 0.0),
            Synthwave => (0.18, 0.10, 0.20, 55.0),
            Ambient => (0.0, 0.06, 0.0, 40.0),
        };
        stack.push(NoteTransform::Pitch {
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
        });
    } else if role == Role::Bass && program.is_some_and(|p| (40..=43).contains(&p)) {
        stack.push(NoteTransform::Pitch {
            settings: PitchPerformance {
                vibrato: 0.06,
                vibrato_delay_ms: 350.0,
                glide_ms: 35.0,
                ..PitchPerformance::default()
            },
        });
    }
    stack
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::{ClipId, MidiClip, SignatureMap, Ticks};

    #[test]
    fn preset_arrangement_changes_the_performance_but_never_the_score() {
        for preset in crate::PRESETS {
            let spec = preset.spec();
            assert!(spec.performance.is_some(), "{} has no palette", preset.name);
            let styled = crate::compose(&spec);
            assert_eq!(
                styled,
                crate::compose(&spec),
                "{} is not seeded",
                preset.name
            );
            let mut plain = spec.clone();
            plain.performance = None;
            let plain = crate::compose(&plain);
            let mut heard_changes = 0;
            let mut groups = std::collections::BTreeSet::new();
            for (styled_track, plain_track) in styled.tracks.iter().zip(&plain.tracks) {
                for (clip, before) in styled_track.clips.iter().zip(&plain_track.clips) {
                    assert_eq!(clip.notes, before.notes, "{}: {}", preset.name, clip.name);
                    assert_eq!(clip.recipe, before.recipe);
                    let midi = MidiClip {
                        notes: clip.notes.clone(),
                        transforms: clip.performance.clone(),
                        ..MidiClip::new(ClipId(1), &clip.name, clip.start, clip.length)
                    };
                    let heard: Vec<_> = midi
                        .sounding_notes_with_meter(spec.tempo, SignatureMap::constant(spec.meter))
                        .collect();
                    heard_changes += usize::from(heard != clip.notes);
                    assert!(heard.iter().all(|n| n.velocity.is_finite()
                        && (0.0..=1.0).contains(&n.velocity)
                        && n.start >= Ticks::ZERO
                        && n.end() <= clip.length
                        && n.length > Ticks::ZERO));
                    for stage in &clip.performance {
                        if let NoteTransform::Expression { settings } = stage {
                            groups.insert(settings.group);
                        }
                    }
                }
            }
            assert!(heard_changes > 0, "{} is inaudible", preset.name);
            assert_eq!(groups.len(), 1, "{} split its ensemble", preset.name);
        }
    }

    #[test]
    fn instrument_substitution_keeps_fixed_pitch_sounds_and_drums_free_of_bends() {
        let mut part = PartSpec::of_role("lead", Role::Melody);
        for number in [0, 4, 10, 46, 98] {
            part.program = Some(crate::gm::Program(number));
            let stack = styled_performance(&part, PerformanceStyle::Rock, 0.5, 1, 2);
            assert!(
                !stack
                    .iter()
                    .any(|t| matches!(t, NoteTransform::Pitch { .. }))
            );
        }
        for number in [29, 42, 65, 73, 81] {
            part.program = Some(crate::gm::Program(number));
            assert!(
                styled_performance(&part, PerformanceStyle::Rock, 0.5, 1, 2)
                    .iter()
                    .any(|t| matches!(t, NoteTransform::Pitch { .. }))
            );
        }
        for role in [Role::Kick, Role::Snare, Role::Hat, Role::Crash] {
            let part = PartSpec::of_role("drum", role);
            for preset in crate::PRESETS {
                let stack =
                    styled_performance(&part, preset.spec().performance.unwrap(), 1.0, 1, 2);
                assert!(!stack.iter().any(|t| matches!(
                    t,
                    NoteTransform::Pitch { .. } | NoteTransform::Humanize { .. }
                )));
                for stage in stack {
                    if let NoteTransform::Expression { settings } = stage {
                        assert_eq!(settings.timing, 0.0);
                    }
                }
            }
        }
    }

    #[test]
    fn styled_drums_keep_ghosts_on_the_assigned_voice_after_kit_merging() {
        let mut spec = crate::preset("city-pop").unwrap().spec();
        spec.parts
            .iter_mut()
            .find(|p| p.role == Role::Snare)
            .unwrap()
            .note = Some(40);
        let piece = crate::compose(&spec);
        let kit = piece
            .tracks
            .iter()
            .find(|t| !t.drum_parts.is_empty())
            .unwrap();
        let mut inserted = 0;
        for clip in &kit.clips {
            let heard = auris_core::performed_notes(
                clip.notes.clone(),
                &clip.performance,
                auris_core::PerformanceContext {
                    bpm: spec.tempo,
                    pass: 0,
                    start: clip.start,
                    length: clip.length,
                    signatures: &SignatureMap::constant(spec.meter),
                },
            );
            for note in &heard[clip.notes.len()..] {
                assert_eq!(note.drum_voice, "snare");
                assert_eq!(note.pitch, 40);
                inserted += 1;
            }
        }
        assert!(inserted > 0, "no drum ghost was actually heard");
    }

    #[test]
    fn singer_melody_keeps_its_own_ornament_pipeline() {
        let mut spec = crate::preset("rock").unwrap().spec();
        spec.singer = Some("test-voice.onnx".into());
        let piece = crate::compose(&spec);
        let lead = piece.tracks.iter().find(|t| t.name == "lead").unwrap();
        assert!(
            lead.clips
                .iter()
                .flat_map(|c| &c.performance)
                .all(|t| matches!(
                    t,
                    NoteTransform::Lean { .. } | NoteTransform::Humanize { .. }
                ))
        );
        let rhythm = piece.tracks.iter().find(|t| t.name == "rhythm").unwrap();
        assert!(
            rhythm
                .clips
                .iter()
                .flat_map(|c| &c.performance)
                .any(|t| matches!(t, NoteTransform::Strum { .. }))
        );
    }

    #[test]
    fn unknown_palettes_are_rejected_and_omission_keeps_existing_specs_plain() {
        assert!(crate::SongSpec::parse("performance = \"rok\"").is_err());
        assert!(
            crate::SongSpec::parse("title = \"Old song\"")
                .unwrap()
                .performance
                .is_none()
        );
    }
}
