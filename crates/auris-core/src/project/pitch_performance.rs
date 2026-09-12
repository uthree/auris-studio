//! Derived channel gestures, sampled off the audio thread from the performed phrase.
use super::{
    ClipCurve, CurvePoint, MidiClip, Note, NoteTransform, PerformanceContext, performed_notes,
};
use crate::{Seconds, SignatureMap, TempoMap, Ticks};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Automatic pitch and controller gestures for monophonic instrument phrases.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PitchPerformance {
    /// Scoop depth below the attack, in 0..=4 semitones; zero disables it.
    pub scoop: f32,
    /// Scoop duration in 10..=300 milliseconds, capped at half the note.
    pub scoop_ms: f32,
    /// Vibrato depth either side of the pitch, in 0..=1 semitones.
    pub vibrato: f32,
    /// Vibrato frequency in 2..=9 Hz, following elapsed time across tempo changes.
    pub vibrato_hz: f32,
    /// Delay before vibrato, in 0..=1000 milliseconds.
    pub vibrato_delay_ms: f32,
    /// Modulation-wheel depth in 0..=1, sharing vibrato's eligibility and onset envelope.
    pub modulation: f32,
    /// Long-note volume contour strength in 0..=1; zero leaves channel volume untouched.
    pub volume_swell: f32,
    /// Editable shape stretched over each long note; defaults to the bowed envelope.
    pub volume_contour: super::VolumeContour,
    /// Fall depth below the release, in 0..=12 semitones.
    pub fall: f32,
    /// Fall duration in 10..=500 milliseconds, capped at half the note.
    pub fall_ms: f32,
    /// Duration on each side of a melodic connection, in 0..=250 milliseconds.
    /// Zero disables it. Gaps over 60 ms and intervals over two octaves are not connected.
    pub glide_ms: f32,
}

impl Default for PitchPerformance {
    fn default() -> Self {
        Self {
            scoop: 0.0,
            scoop_ms: 100.0,
            vibrato: 0.0,
            vibrato_hz: 5.8,
            vibrato_delay_ms: 300.0,
            modulation: 0.0,
            volume_swell: 0.0,
            volume_contour: super::VolumeContour::default(),
            fall: 0.0,
            fall_ms: 150.0,
            glide_ms: 0.0,
        }
    }
}

impl PitchPerformance {
    /// Whether any gesture is enabled.
    pub fn is_active(&self) -> bool {
        self.scoop > 0.0
            || self.vibrato > 0.0
            || self.fall > 0.0
            || self.glide_ms > 0.0
            || self.modulation > 0.0
            || self.volume_swell > 0.0
    }
}

fn settings<'a>(
    stack: &'a [NoteTransform],
    voice: Option<&'a str>,
    out: &mut Vec<(&'a PitchPerformance, Option<&'a str>)>,
) {
    for stage in stack {
        match stage {
            NoteTransform::Pitch { settings } if settings.is_active() => {
                out.push((settings, voice))
            }
            NoteTransform::ForDrumVoice {
                voice: next,
                transforms,
            } if voice.is_none_or(|v| v == next) => settings(transforms, Some(next), out),
            _ => {}
        }
    }
}

impl MidiClip {
    /// Whether a derived pitch gesture is enabled anywhere in the stack.
    pub fn has_pitch_performance(&self) -> bool {
        self.has_generated_curve(ClipCurve::Bend)
    }

    /// Authored and generated controller/bend curves, each listed once.
    pub fn performance_curves(&self) -> impl Iterator<Item = ClipCurve> + '_ {
        (!self.bend.is_empty() || self.has_pitch_performance())
            .then_some(ClipCurve::Bend)
            .into_iter()
            .chain(
                [ClipCurve::MODULATION, ClipCurve::Controller(7)]
                    .into_iter()
                    .filter(|which| self.has_generated_curve(*which)),
            )
            .chain(
                self.curves()
                    .filter(|which| *which != ClipCurve::Bend && !self.has_generated_curve(*which)),
            )
    }

    /// Whether this curve has an enabled non-destructive generator.
    pub fn has_generated_curve(&self, which: ClipCurve) -> bool {
        let mut stages = Vec::new();
        settings(&self.transforms, None, &mut stages);
        stages.iter().any(|(s, _)| match which {
            ClipCurve::Bend => s.scoop > 0.0 || s.vibrato > 0.0 || s.fall > 0.0 || s.glide_ms > 0.0,
            ClipCurve::Controller(1) => s.modulation > 0.0,
            ClipCurve::Controller(7) => s.volume_swell > 0.0,
            _ => false,
        })
    }

    /// The first or repeated pass's combined authored and generated bend, relative to that pass.
    /// Generated gestures follow the final performed notes and are added to the authored bend.
    /// Overlapping notes and notes shorter than 80 ms receive no automatic bend.
    pub fn performed_bend_points(
        &self,
        tempo: &TempoMap,
        signatures: &SignatureMap,
        pass: u64,
        offset: Ticks,
        span: Ticks,
    ) -> Vec<CurvePoint> {
        self.performed_curve_points(ClipCurve::Bend, tempo, signatures, pass, offset, span)
    }

    /// Combined authored and generated curve for one performance pass.
    /// Modulation adds to authored CC1; volume multiplies authored CC7 (default full volume).
    pub fn performed_curve_points(
        &self,
        which: ClipCurve,
        tempo: &TempoMap,
        signatures: &SignatureMap,
        pass: u64,
        offset: Ticks,
        span: Ticks,
    ) -> Vec<CurvePoint> {
        let mut stages = Vec::new();
        settings(&self.transforms, None, &mut stages);
        if !self.has_generated_curve(which) {
            return self.curve(which).to_vec();
        }
        let base = self.start + offset;
        let mut notes: Vec<Note> = performed_notes(
            self.playable_notes().collect(),
            &self.transforms,
            PerformanceContext {
                bpm: tempo.bpm_at(self.start),
                pass,
                start: base,
                length: self.length,
                signatures,
            },
        )
        .into_iter()
        .filter(|n| n.start < span)
        .map(|mut n| {
            n.length = n.length.min(span - n.start);
            n
        })
        .collect();
        notes.sort_by_key(|n| (n.start, n.pitch));
        // Exact octave layers share one channel gesture. Other chords and overlapping
        // voices remain in the eligibility scan, where they suppress unsafe channel motion.
        if has_octaves(&self.transforms) {
            notes.dedup_by(|b, a| {
                a.start == b.start
                    && a.end() == b.end()
                    && a.pitch % 12 == b.pitch % 12
                    && a.drum_voice == b.drum_voice
            });
        }
        let seconds = |at| tempo.ticks_to_seconds(base + at).0;
        let starts: Vec<_> = notes.iter().map(|n| seconds(n.start)).collect();
        let ends: Vec<_> = notes.iter().map(|n| seconds(n.end())).collect();
        let mut occupied_until = Ticks::ZERO;
        let eligible: Vec<_> = notes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let alone = occupied_until <= n.start
                    && notes.get(i + 1).is_none_or(|next| next.start >= n.end());
                occupied_until = occupied_until.max(n.end());
                alone && ends[i] - starts[i] >= 0.08
            })
            .collect();
        let mut ticks = BTreeSet::from([Ticks::ZERO, span]);
        ticks.extend(
            self.curve(which)
                .iter()
                .filter(|p| p.at >= Ticks::ZERO && p.at <= span)
                .map(|p| p.at),
        );
        for (i, note) in notes.iter().enumerate().filter(|(i, _)| eligible[*i]) {
            ticks.extend([
                (note.start - Ticks(1)).max_zero(),
                note.start,
                note.end() - Ticks(1),
                note.end(),
            ]);
            if which == ClipCurve::Controller(7) {
                for (style, _) in &stages {
                    ticks.extend(style.volume_contour.points().iter().map(|point| {
                        let fraction =
                            point.at.raw() as f64 / super::VolumeContour::END.raw() as f64;
                        (tempo.seconds_to_ticks(Seconds(
                            starts[i] + (ends[i] - starts[i]) * fraction,
                        )) - base)
                            .clamp(note.start, note.end())
                    }));
                }
            }
            let mut time = starts[i] + 0.005;
            while time < ends[i] {
                ticks.insert(
                    (tempo.seconds_to_ticks(Seconds(time)) - base)
                        .max(note.start)
                        .min(note.end()),
                );
                time += 0.005;
            }
        }
        ticks
            .into_iter()
            .map(|at| {
                let mut value = if which == ClipCurve::Controller(7) && self.curve(which).is_empty()
                {
                    1.0
                } else {
                    super::curve_at(self.curve(which), at)
                };
                if let Some(i) = notes.iter().position(|n| n.start <= at && at < n.end())
                    && eligible[i]
                {
                    let time = seconds(at);
                    for (style, voice) in &stages {
                        if voice.is_some_and(|v| notes[i].drum_voice != v) {
                            continue;
                        }
                        let connects = |a: usize, b: usize| {
                            eligible[a]
                                && eligible[b]
                                && voice.is_none_or(|v| {
                                    notes[a].drum_voice == v && notes[b].drum_voice == v
                                })
                                && starts[b] - ends[a] <= 0.0600001
                                && (i16::from(notes[b].pitch) - i16::from(notes[a].pitch)).abs()
                                    <= 24
                        };
                        let previous = (style.glide_ms > 0.0 && i > 0 && connects(i - 1, i))
                            .then(|| f32::from(notes[i - 1].pitch) - f32::from(notes[i].pitch));
                        let next =
                            (style.glide_ms > 0.0 && i + 1 < notes.len() && connects(i, i + 1))
                                .then(|| f32::from(notes[i + 1].pitch) - f32::from(notes[i].pitch));
                        let elapsed = time - starts[i];
                        let length = ends[i] - starts[i];
                        match which {
                            ClipCurve::Bend => {
                                value += gesture(style, elapsed, length, previous, next)
                            }
                            ClipCurve::Controller(1) => {
                                value += style.modulation.clamp(0.0, 1.0)
                                    * vibrato_envelope(style, elapsed, length)
                            }
                            ClipCurve::Controller(7) if length >= 0.6 => {
                                value *= 1.0
                                    + style.volume_swell.clamp(0.0, 1.0)
                                        * (style.volume_contour.level_at(elapsed / length) - 1.0)
                            }
                            _ => {}
                        }
                    }
                }
                CurvePoint {
                    at,
                    // Keep the authored endpoint so interpolation cannot ramp it down.
                    // The event sampler appends the channel reset after the curve ends.
                    value: value.clamp(
                        if which.is_bipolar() {
                            -which.limit()
                        } else {
                            0.0
                        },
                        which.limit(),
                    ),
                }
            })
            .collect()
    }

    /// Playback/export curve events, with tempo-aware generated pitch and loop resets.
    pub fn sounding_performance_curve_events(
        &self,
        which: ClipCurve,
        step: Ticks,
        tempo: &TempoMap,
        signatures: &SignatureMap,
    ) -> Vec<(Ticks, f32)> {
        if !self.has_generated_curve(which) {
            return self.sounding_curve_events(which, step);
        }
        let mut out = Vec::new();
        for (pass, (offset, span)) in super::loop_passes(self.length, self.loop_end).enumerate() {
            let points =
                self.performed_curve_points(which, tempo, signatures, pass as u64, offset, span);
            let mut events = super::curve_events(&points, span, step);
            if which == ClipCurve::Controller(7) {
                events.retain(|(at, _)| *at < span);
                events.push((span, 1.0));
            }
            out.extend(events.into_iter().map(|(at, value)| (at + offset, value)));
        }
        out
    }
}

fn smooth(x: f64) -> f32 {
    let x = x.clamp(0.0, 1.0) as f32;
    x * x * (3.0 - 2.0 * x)
}

fn has_octaves(stack: &[NoteTransform]) -> bool {
    stack.iter().any(|stage| match stage {
        NoteTransform::Octaves { above, below } => *above > 0.0 || *below > 0.0,
        NoteTransform::ForDrumVoice { transforms, .. } => has_octaves(transforms),
        _ => false,
    })
}

fn vibrato_envelope(s: &PitchPerformance, at: f64, length: f64) -> f32 {
    let after_delay = (at - f64::from(s.vibrato_delay_ms.clamp(0.0, 1000.0)) / 1000.0).max(0.0);
    smooth(after_delay / 0.1) * smooth((length - at) / 0.08)
}

fn gesture(
    s: &PitchPerformance,
    at: f64,
    length: f64,
    previous: Option<f32>,
    next: Option<f32>,
) -> f32 {
    let reach = |ms: f32, max| (f64::from(ms.clamp(10.0, max)) / 1000.0).min(length / 2.0);
    let incoming = match previous {
        Some(interval) => interval / 2.0 * (1.0 - smooth(at / reach(s.glide_ms, 250.0))),
        None => -s.scoop.clamp(0.0, 4.0) * (1.0 - smooth(at / reach(s.scoop_ms, 300.0))),
    };
    let outgoing = match next {
        Some(interval) => interval / 2.0 * smooth(1.0 - (length - at) / reach(s.glide_ms, 250.0)),
        None => -s.fall.clamp(0.0, 12.0) * smooth(1.0 - (length - at) / reach(s.fall_ms, 500.0)),
    };
    let after_delay = (at - f64::from(s.vibrato_delay_ms.clamp(0.0, 1000.0)) / 1000.0).max(0.0);
    let envelope = vibrato_envelope(s, at, length);
    let sway = (after_delay * f64::from(s.vibrato_hz.clamp(2.0, 9.0)) * std::f64::consts::TAU).sin()
        as f32;
    incoming + outgoing + s.vibrato.clamp(0.0, 1.0) * envelope * sway
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClipId;
    use crate::project::curve_at;
    #[test]
    fn custom_volume_knots_follow_note_length_and_are_sampled_exactly() {
        let clip = clip(
            vec![Note::new(60, Ticks::ZERO, Ticks(3840))],
            PitchPerformance {
                volume_swell: 1.0,
                volume_contour: super::super::VolumeContour::new(vec![
                    CurvePoint {
                        at: Ticks::ZERO,
                        value: 0.4,
                    },
                    CurvePoint {
                        at: Ticks(2500),
                        value: 0.9,
                    },
                    CurvePoint {
                        at: Ticks(5000),
                        value: 0.2,
                    },
                    CurvePoint {
                        at: Ticks(10_000),
                        value: 0.7,
                    },
                ]),
                ..PitchPerformance::default()
            },
        );
        let points = clip.performed_curve_points(
            ClipCurve::Controller(7),
            &TempoMap::constant(120.0),
            &SignatureMap::default(),
            0,
            Ticks::ZERO,
            clip.length,
        );
        assert!((curve_at(&points, Ticks(960)) - 0.9).abs() < 0.0001);
        assert!((curve_at(&points, Ticks(1920)) - 0.2).abs() < 0.0001);
        assert!(clip.controllers.is_empty());
    }
    #[test]
    fn new_controls_default_off_in_old_settings_and_round_trip_when_enabled() {
        let old: PitchPerformance = serde_json::from_str(r#"{"vibrato":0.2}"#).unwrap();
        assert_eq!(old.modulation, 0.0);
        assert_eq!(old.volume_swell, 0.0);
        let mut clip = clip(
            vec![Note::new(60, Ticks::ZERO, Ticks(1920))],
            PitchPerformance {
                modulation: 0.7,
                volume_swell: 0.8,
                ..old
            },
        );
        clip.transforms.push(NoteTransform::Octaves {
            above: 0.6,
            below: 0.4,
        });
        let saved = serde_json::to_string(&clip).unwrap();
        let restored: MidiClip = serde_json::from_str(&saved).unwrap();
        assert_eq!(restored.transforms, clip.transforms);
        assert_eq!(restored.notes, clip.notes);
    }
    #[test]
    fn delayed_modulation_is_independent_of_pitch_and_resets_between_notes() {
        let clip = clip(
            vec![Note::new(60, Ticks(960), Ticks(1920))],
            PitchPerformance {
                modulation: 0.7,
                ..PitchPerformance::default()
            },
        );
        let points = clip.performed_curve_points(
            ClipCurve::MODULATION,
            &TempoMap::constant(120.0),
            &SignatureMap::default(),
            0,
            Ticks::ZERO,
            clip.length,
        );
        assert!(!clip.has_pitch_performance());
        assert_eq!(curve_at(&points, Ticks(1200)), 0.0);
        assert!((curve_at(&points, Ticks(1920)) - 0.7).abs() < 0.001);
        assert_eq!(curve_at(&points, Ticks(2880)), 0.0);
        assert!(clip.controllers.is_empty());
    }

    #[test]
    fn exact_octave_layers_share_the_melodys_controller_gestures() {
        let mut clip = clip(
            vec![Note::new(60, Ticks::ZERO, Ticks(3840))],
            PitchPerformance {
                modulation: 0.8,
                volume_swell: 1.0,
                ..PitchPerformance::default()
            },
        );
        let generate = |clip: &MidiClip, which| {
            clip.performed_curve_points(
                which,
                &TempoMap::constant(120.0),
                &SignatureMap::default(),
                0,
                Ticks::ZERO,
                clip.length,
            )
        };
        let expected = generate(&clip, ClipCurve::MODULATION);
        clip.transforms.push(NoteTransform::Octaves {
            above: 0.5,
            below: 0.5,
        });
        assert_eq!(generate(&clip, ClipCurve::MODULATION), expected);
        assert!(
            generate(&clip, ClipCurve::Controller(7))
                .iter()
                .any(|p| p.value < 0.4)
        );
    }

    #[test]
    fn volume_dips_swells_and_resets_without_changing_authored_volume() {
        let mut clip = clip(
            vec![Note::new(60, Ticks::ZERO, Ticks(3840))],
            PitchPerformance {
                volume_swell: 1.0,
                ..PitchPerformance::default()
            },
        );
        let which = ClipCurve::Controller(7);
        clip.controllers.insert(
            7,
            vec![CurvePoint {
                at: Ticks::ZERO,
                value: 0.8,
            }],
        );
        let points = clip.performed_curve_points(
            which,
            &TempoMap::constant(120.0),
            &SignatureMap::default(),
            0,
            Ticks::ZERO,
            clip.length,
        );
        assert!((curve_at(&points, Ticks::ZERO) - 0.64).abs() < 0.001);
        assert!(curve_at(&points, Ticks(1500)) < 0.25);
        assert!(curve_at(&points, Ticks(3400)) > 0.75);
        assert!(curve_at(&points, Ticks(3800)) < 0.6);
        assert_eq!(curve_at(&points, Ticks(4000)), 0.8);
        let events = clip.sounding_performance_curve_events(
            which,
            Ticks(20),
            &TempoMap::constant(120.0),
            &SignatureMap::default(),
        );
        assert_eq!(events.last(), Some(&(clip.length, 1.0)));
        assert_eq!(clip.controllers[&7][0].value, 0.8);
    }

    #[test]
    fn short_notes_and_overlapping_voices_receive_no_controller_gestures() {
        let clip = clip(
            vec![
                Note::new(60, Ticks::ZERO, Ticks(100)),
                Note::new(62, Ticks(960), Ticks(1920)),
                Note::new(67, Ticks(960), Ticks(1920)),
            ],
            PitchPerformance {
                modulation: 1.0,
                volume_swell: 1.0,
                ..PitchPerformance::default()
            },
        );
        for (which, expected) in [
            (ClipCurve::MODULATION, 0.0),
            (ClipCurve::Controller(7), 1.0),
        ] {
            let points = clip.performed_curve_points(
                which,
                &TempoMap::constant(120.0),
                &SignatureMap::default(),
                0,
                Ticks::ZERO,
                clip.length,
            );
            assert!(points.iter().all(|p| p.value == expected));
        }
    }

    #[test]
    fn octave_copies_preserve_source_and_respect_pitch_bounds_and_existing_voices() {
        let mut clip = clip(
            vec![
                Note::new(60, Ticks::ZERO, Ticks(960)),
                Note::new(72, Ticks::ZERO, Ticks(960)),
                Note::new(5, Ticks(960), Ticks(960)),
            ],
            PitchPerformance::default(),
        );
        clip.transforms = vec![NoteTransform::Octaves {
            above: 0.5,
            below: 1.0,
        }];
        let source = clip.notes.clone();
        let notes = performed_notes(
            source.clone(),
            &clip.transforms,
            PerformanceContext {
                bpm: 120.0,
                pass: 0,
                start: Ticks::ZERO,
                length: clip.length,
                signatures: &SignatureMap::default(),
            },
        );
        assert_eq!(clip.notes, source);
        assert_eq!(notes.len(), 6);
        assert_eq!(notes.iter().filter(|n| n.pitch == 60).count(), 1);
        assert_eq!(notes.iter().filter(|n| n.pitch == 72).count(), 1);
        let upper = notes.iter().find(|n| n.pitch == 84).unwrap();
        assert_eq!(upper.velocity, source[1].velocity * 0.5);
        assert_eq!(upper.length, source[1].length);
    }
    fn clip(notes: Vec<Note>, style: PitchPerformance) -> MidiClip {
        MidiClip {
            notes,
            transforms: vec![NoteTransform::Pitch { settings: style }],
            ..MidiClip::new(ClipId(1), "Lead", Ticks::ZERO, Ticks(7680))
        }
    }
    fn points(clip: &MidiClip) -> Vec<CurvePoint> {
        clip.performed_bend_points(
            &TempoMap::constant(120.0),
            &SignatureMap::default(),
            0,
            Ticks::ZERO,
            clip.length,
        )
    }
    #[test]
    fn gestures_add_to_authored_bend_and_leave_source_notes_and_points_untouched() {
        let mut clip = clip(
            vec![Note::new(60, Ticks(960), Ticks(1920))],
            PitchPerformance {
                scoop: 2.0,
                fall: 3.0,
                ..PitchPerformance::default()
            },
        );
        clip.bend = vec![CurvePoint {
            at: Ticks::ZERO,
            value: 0.25,
        }];
        let source = clip.clone();
        let points = points(&clip);
        assert_eq!(curve_at(&points, Ticks(959)), 0.25);
        assert_eq!(curve_at(&points, Ticks(960)), -1.75);
        assert!((curve_at(&points, Ticks(1440)) - 0.25).abs() < 1e-6);
        assert!((curve_at(&points, Ticks(2879)) + 2.75).abs() < 0.01);
        assert_eq!(curve_at(&points, Ticks(2880)), 0.25);
        assert_eq!(points.last().unwrap().value, 0.25);
        assert_eq!(
            clip.sounding_performance_curve_events(
                ClipCurve::Bend,
                super::super::CURVE_STEP,
                &TempoMap::constant(120.0),
                &SignatureMap::default(),
            )
            .last(),
            Some(&(clip.length, 0.0))
        );
        assert_eq!(clip.notes, source.notes);
        assert_eq!(clip.bend, source.bend);
        let loaded: MidiClip =
            serde_json::from_str(&serde_json::to_string(&clip).unwrap()).unwrap();
        assert_eq!(points, super::tests::points(&loaded));
    }

    #[test]
    fn pitch_gestures_preserve_authored_bend_through_chords_and_silent_tails() {
        for notes in [
            vec![
                Note::new(60, Ticks::ZERO, Ticks(7680)),
                Note::new(64, Ticks::ZERO, Ticks(7680)),
            ],
            vec![Note::new(60, Ticks(960), Ticks(1920))],
        ] {
            let mut clip = clip(
                notes,
                PitchPerformance {
                    scoop: 1.0,
                    ..PitchPerformance::default()
                },
            );
            clip.bend = vec![CurvePoint {
                at: Ticks::ZERO,
                value: 2.0,
            }];
            let points = points(&clip);
            for at in [Ticks(3840), clip.length - Ticks(1), clip.length] {
                assert_eq!(
                    curve_at(&points, at),
                    2.0,
                    "authored bend changed at {at:?}"
                );
            }
            let events = clip.sounding_performance_curve_events(
                ClipCurve::Bend,
                super::super::CURVE_STEP,
                &TempoMap::constant(120.0),
                &SignatureMap::default(),
            );
            assert!(
                events
                    .iter()
                    .filter(|(at, _)| *at >= Ticks(3840) && *at < clip.length)
                    .all(|(_, value)| *value == 2.0)
            );
            assert_eq!(events.last(), Some(&(clip.length, 0.0)));
        }
    }

    #[test]
    fn connections_meet_at_the_midpoint_in_both_directions_and_stop_at_rests() {
        for pitches in [[60, 67], [67, 60]] {
            let clip = clip(
                vec![
                    Note::new(pitches[0], Ticks::ZERO, Ticks(960)),
                    Note::new(pitches[1], Ticks(960), Ticks(960)),
                ],
                PitchPerformance {
                    glide_ms: 100.0,
                    scoop: 1.0,
                    fall: 2.0,
                    ..PitchPerformance::default()
                },
            );
            let points = points(&clip);
            let before = f32::from(pitches[0]) + curve_at(&points, Ticks(959));
            let after = f32::from(pitches[1]) + curve_at(&points, Ticks(960));
            assert!((before - after).abs() < 0.002);
            assert!((after - (f32::from(pitches[0]) + f32::from(pitches[1])) / 2.0).abs() < 1e-6);
            assert_eq!(curve_at(&points, Ticks(1440)), 0.0);
        }
        let clip = clip(
            vec![
                Note::new(60, Ticks::ZERO, Ticks(960)),
                Note::new(67, Ticks(1920), Ticks(960)),
            ],
            PitchPerformance {
                glide_ms: 100.0,
                ..PitchPerformance::default()
            },
        );
        assert!(points(&clip).iter().all(|p| p.value == 0.0));
    }
    #[test]
    fn chords_overlaps_and_short_ornaments_do_not_get_channel_gestures() {
        let clip = clip(
            vec![
                Note::new(60, Ticks::ZERO, Ticks(960)),
                Note::new(64, Ticks(100), Ticks(960)),
                Note::new(67, Ticks(1920), Ticks(23)),
            ],
            PitchPerformance {
                scoop: 1.0,
                vibrato: 0.4,
                fall: 2.0,
                ..PitchPerformance::default()
            },
        );
        assert!(points(&clip).iter().all(|p| p.value == 0.0));
    }
    #[test]
    fn vibrato_uses_seconds_through_tempo_changes_and_bend_resets_each_partial_loop() {
        let mut clip = clip(
            vec![Note::new(60, Ticks::ZERO, Ticks(3840))],
            PitchPerformance {
                vibrato: 0.5,
                vibrato_hz: 5.0,
                vibrato_delay_ms: 200.0,
                ..PitchPerformance::default()
            },
        );
        clip.length = Ticks(3840);
        clip.loop_end = Ticks(5000);
        let mut tempo = TempoMap::constant(120.0);
        tempo.set_point(Ticks(960), 240.0);
        let points = clip.performed_bend_points(
            &tempo,
            &SignatureMap::default(),
            0,
            Ticks::ZERO,
            clip.length,
        );
        assert_eq!(curve_at(&points, Ticks(100)), 0.0);
        for (time, expected) in [(0.45, 0.5), (0.55, -0.5), (0.65, 0.5)] {
            assert!(
                (curve_at(&points, tempo.seconds_to_ticks(Seconds(time))) - expected).abs() < 0.01
            );
        }
        let events = clip.sounding_performance_curve_events(
            ClipCurve::Bend,
            super::super::CURVE_STEP,
            &tempo,
            &SignatureMap::default(),
        );
        assert_eq!(events.last().unwrap(), &(Ticks(5000), 0.0));
        assert!(events.contains(&(Ticks(3840), 0.0)));
        assert!(events.iter().all(|(at, _)| *at <= Ticks(5000)));
    }
    #[test]
    fn gestures_follow_humanized_notes_and_zero_strength_preserves_the_manual_curve() {
        let mut clip = clip(
            vec![Note::new(60, Ticks(960), Ticks(960))],
            PitchPerformance {
                scoop: 1.0,
                ..PitchPerformance::default()
            },
        );
        clip.transforms.push(NoteTransform::Humanize {
            amount: 1.0,
            seed: 41,
        });
        let note = clip.sounding_notes(120.0).next().unwrap();
        assert_eq!(curve_at(&points(&clip), note.start), -1.0);
        clip.transforms = vec![NoteTransform::Pitch {
            settings: PitchPerformance::default(),
        }];
        clip.bend = vec![CurvePoint {
            at: Ticks(50),
            value: 0.7,
        }];
        assert_eq!(points(&clip), clip.bend);
    }
}
