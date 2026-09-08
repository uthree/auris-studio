//! Derived channel pitch gestures, sampled off the audio thread from the performed phrase.
use super::{
    ClipCurve, CurvePoint, MidiClip, Note, NoteTransform, PerformanceContext, performed_notes,
};
use crate::{Seconds, SignatureMap, TempoMap, Ticks};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Automatic pitch bend for monophonic instrument phrases. Depths are semitones.
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
            fall: 0.0,
            fall_ms: 150.0,
            glide_ms: 0.0,
        }
    }
}

impl PitchPerformance {
    /// Whether any gesture is enabled.
    pub fn is_active(&self) -> bool {
        self.scoop > 0.0 || self.vibrato > 0.0 || self.fall > 0.0 || self.glide_ms > 0.0
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
        let mut stages = Vec::new();
        settings(&self.transforms, None, &mut stages);
        !stages.is_empty()
    }

    /// Authored controllers and the bend, including an enabled generated bend.
    pub fn performance_curves(&self) -> impl Iterator<Item = ClipCurve> + '_ {
        (!self.bend.is_empty() || self.has_pitch_performance())
            .then_some(ClipCurve::Bend)
            .into_iter()
            .chain(self.curves().filter(|which| *which != ClipCurve::Bend))
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
        let mut stages = Vec::new();
        settings(&self.transforms, None, &mut stages);
        if stages.is_empty() {
            return self.bend.clone();
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
            self.bend
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
                let mut value = super::curve_at(&self.bend, at);
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
                        value +=
                            gesture(style, time - starts[i], ends[i] - starts[i], previous, next);
                    }
                }
                CurvePoint {
                    at,
                    value: if at == span {
                        0.0
                    } else {
                        value.clamp(-super::BEND_LIMIT, super::BEND_LIMIT)
                    },
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
        if which != ClipCurve::Bend || !self.has_pitch_performance() {
            return self.sounding_curve_events(which, step);
        }
        let mut out = Vec::new();
        for (pass, (offset, span)) in super::loop_passes(self.length, self.loop_end).enumerate() {
            let points = self.performed_bend_points(tempo, signatures, pass as u64, offset, span);
            out.extend(
                super::curve_events(&points, span, step)
                    .into_iter()
                    .map(|(at, value)| (at + offset, value)),
            );
        }
        out
    }
}

fn smooth(x: f64) -> f32 {
    let x = x.clamp(0.0, 1.0) as f32;
    x * x * (3.0 - 2.0 * x)
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
    let envelope = smooth(after_delay / 0.1) * smooth((length - at) / 0.08);
    let sway = (after_delay * f64::from(s.vibrato_hz.clamp(2.0, 9.0)) * std::f64::consts::TAU).sin()
        as f32;
    incoming + outgoing + s.vibrato.clamp(0.0, 1.0) * envelope * sway
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClipId;
    use crate::project::curve_at;
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
        assert_eq!(points.last().unwrap().value, 0.0);
        assert_eq!(clip.notes, source.notes);
        assert_eq!(clip.bend, source.bend);
        let loaded: MidiClip =
            serde_json::from_str(&serde_json::to_string(&clip).unwrap()).unwrap();
        assert_eq!(points, super::tests::points(&loaded));
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
