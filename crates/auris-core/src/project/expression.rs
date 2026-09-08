//! Independent wander, phrase dynamics and shared ensemble motion.

use super::performance::chords;
use super::transform::humanized_axes;
use super::{Note, PerformanceContext};
use crate::Ticks;
use serde::{Deserialize, Serialize};

/// Note-domain expression, evaluated without altering the stored score.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Expression {
    /// Independent timing wander, in 0..=1, at the existing humanisation scale.
    pub timing: f32,
    /// Independent velocity wander, in 0..=1.
    pub velocity: f32,
    /// Strength of an arch over each phrase, split at gaps longer than a quarter note.
    pub swell: f32,
    /// Positive emphasizes quarter-note beats, negative emphasizes their eighth-note offbeats.
    pub accent: f32,
    /// Blend from the private take to a timeline-aligned ensemble gesture, in 0..=1.
    pub shared: f32,
    /// Ensemble group. Equal groups share the same motion across tracks and clip origins.
    pub group: u64,
    /// Deliberate push (negative) or delay (positive), clamped to -50..=50 ms.
    pub delay_ms: f32,
    /// Private take identity, retained when changing either wander control.
    pub seed: u64,
}

impl Default for Expression {
    fn default() -> Self {
        Self {
            timing: 0.0,
            velocity: 0.0,
            swell: 0.0,
            accent: 0.0,
            shared: 0.0,
            group: 1,
            delay_ms: 0.0,
            seed: 0,
        }
    }
}

pub(super) fn express(notes: &mut [Note], settings: &Expression, context: PerformanceContext<'_>) {
    let groups = chords(notes);
    let mut arches = vec![0.0_f32; notes.len()];
    let mut first = 0;
    while first < groups.len() {
        let start = notes[groups[first][0]].start;
        let mut end = groups[first]
            .iter()
            .map(|&i| notes[i].end())
            .max()
            .unwrap_or(start);
        let mut last = first + 1;
        while last < groups.len() && notes[groups[last][0]].start <= end + Ticks::QUARTER {
            end = end.max(
                groups[last]
                    .iter()
                    .map(|&i| notes[i].end())
                    .max()
                    .unwrap_or(end),
            );
            last += 1;
        }
        let last_attack = notes[groups[last - 1][0]].start;
        if last_attack > start {
            for group in &groups[first..last] {
                let phase = (notes[group[0]].start - start).raw() as f32
                    / (last_attack - start).raw() as f32;
                for &i in group {
                    arches[i] = (phase * std::f32::consts::PI).sin().max(0.0);
                }
            }
        }
        first = last;
    }
    for (note, arch) in notes.iter_mut().zip(arches) {
        let original = note.clone();
        let absolute = context.start + original.start;
        *note = humanized_axes(
            original.clone(),
            settings.timing,
            settings.velocity,
            settings.seed,
            context.pass,
            context.bpm,
        );
        let shared = settings.shared.clamp(0.0, 1.0);
        if shared > 0.0 {
            // The ensemble pulse is beat-relative (the humanisation scale at 120 BPM),
            // so simultaneous notes agree even if their clips begin under different tempos.
            let common = humanized_axes(
                Note {
                    start: absolute,
                    pitch: 0,
                    velocity: 0.5,
                    ..original.clone()
                },
                settings.timing,
                settings.velocity,
                settings.group,
                0,
                120.0,
            );
            let delta = (note.start - original.start).raw() as f32 * (1.0 - shared)
                + (common.start - absolute).raw() as f32 * shared;
            note.start = (original.start + Ticks(delta.round() as i64)).max_zero();
            note.velocity = note.velocity * (1.0 - shared)
                + original.velocity * (common.velocity * 2.0) * shared;
        }
        let delay =
            settings.delay_ms.clamp(-50.0, 50.0) * Ticks::QUARTER.raw() as f32 * context.bpm as f32
                / 60_000.0;
        note.start = (note.start + Ticks(delay.round() as i64)).max_zero();
        let phase = (absolute - context.signatures.bar_floor(absolute))
            .raw()
            .rem_euclid(Ticks::QUARTER.raw()) as f32
            / Ticks::QUARTER.raw() as f32;
        let accent =
            1.0 + 0.25 * settings.accent.clamp(-1.0, 1.0) * (phase * std::f32::consts::TAU).cos();
        note.velocity =
            (note.velocity * accent * (1.0 + 0.6 * settings.swell.clamp(0.0, 1.0) * arch))
                .clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NoteTransform, SignatureMap, performed_notes};
    fn run(source: Vec<Note>, settings: Expression, start: i64, bpm: f64, pass: u64) -> Vec<Note> {
        performed_notes(
            source,
            &[NoteTransform::Expression { settings }],
            PerformanceContext {
                bpm,
                start: Ticks(start),
                length: Ticks(16000),
                pass,
                signatures: &SignatureMap::default(),
            },
        )
    }
    fn note(pitch: u8, start: i64) -> Note {
        Note {
            velocity: 0.5,
            ..Note::new(pitch, Ticks(start), Ticks(240))
        }
    }
    #[test]
    fn accents_follow_meter_boundaries_and_delay_is_bounded_by_the_clip_start() {
        let mut meter = SignatureMap::constant(crate::TimeSignature::new(3, 8));
        meter.set_point(Ticks(1440), crate::TimeSignature::new(3, 4));
        let context = PerformanceContext {
            bpm: 120.0,
            pass: 0,
            start: Ticks(1440),
            length: Ticks(1920),
            signatures: &meter,
        };
        let source = vec![note(60, 0), note(60, 480)];
        let shifted = performed_notes(
            source,
            &[NoteTransform::Expression {
                settings: Expression {
                    accent: 1.0,
                    delay_ms: -50.0,
                    ..Expression::default()
                },
            }],
            context,
        );
        assert_eq!(shifted[0].velocity, 0.625);
        assert_eq!(shifted[1].velocity, 0.375);
        assert_eq!(shifted[0].start, Ticks::ZERO);
        assert_eq!(shifted[1].start, Ticks(384));
    }

    #[test]
    fn timing_and_velocity_controls_are_independent_and_reproducible() {
        let source = vec![note(60, 960), note(64, 1440)];
        let timing = run(
            source.clone(),
            Expression {
                timing: 1.0,
                seed: 41,
                ..Expression::default()
            },
            0,
            120.0,
            0,
        );
        let velocity = run(
            source.clone(),
            Expression {
                velocity: 1.0,
                seed: 41,
                ..Expression::default()
            },
            0,
            120.0,
            0,
        );
        assert_eq!(
            timing.iter().map(|n| n.velocity).collect::<Vec<_>>(),
            vec![0.5, 0.5]
        );
        assert_eq!(
            velocity.iter().map(|n| n.start).collect::<Vec<_>>(),
            vec![Ticks(960), Ticks(1440)]
        );
        assert_ne!(timing, source);
        assert_ne!(velocity, source);
        let both = run(
            source.clone(),
            Expression {
                timing: 1.0,
                velocity: 1.0,
                seed: 41,
                ..Expression::default()
            },
            0,
            120.0,
            0,
        );
        let legacy: Vec<_> = source
            .into_iter()
            .map(|n| {
                crate::performed(
                    n,
                    &[NoteTransform::Humanize {
                        amount: 1.0,
                        seed: 41,
                    }],
                    0,
                    120.0,
                )
            })
            .collect();
        assert_eq!(both, legacy);
    }
    #[test]
    fn the_ensemble_agrees_across_clip_origins_pitches_tempos_and_pass_numbers() {
        let a = run(
            vec![note(60, 4320)],
            Expression {
                timing: 1.0,
                velocity: 1.0,
                shared: 1.0,
                seed: 1,
                ..Expression::default()
            },
            0,
            120.0,
            0,
        );
        let b = run(
            vec![note(36, 480)],
            Expression {
                timing: 1.0,
                velocity: 1.0,
                shared: 1.0,
                seed: 99,
                ..Expression::default()
            },
            3840,
            90.0,
            7,
        );
        assert_eq!(a[0].start, b[0].start + Ticks(3840));
        assert_eq!(a[0].velocity, b[0].velocity);
    }
    #[test]
    fn phrases_arch_and_signed_accents_follow_the_musical_grid() {
        let source = vec![
            note(60, 0),
            note(60, 480),
            note(60, 960),
            note(60, 1440),
            note(60, 1920),
            note(60, 6000),
        ];
        let arch = run(
            source.clone(),
            Expression {
                swell: 1.0,
                ..Expression::default()
            },
            0,
            120.0,
            0,
        );
        assert_eq!(arch[0].velocity, 0.5);
        assert!((arch[2].velocity - 0.8).abs() < 1e-6);
        assert!((arch[4].velocity - 0.5).abs() < 1e-6);
        assert_eq!(arch[5].velocity, 0.5, "a rest starts a new phrase");
        let accent = run(
            source,
            Expression {
                accent: -1.0,
                ..Expression::default()
            },
            0,
            120.0,
            0,
        );
        assert!(accent[1].velocity > accent[0].velocity);
        assert_eq!(accent[0].start, Ticks::ZERO);
    }
}
