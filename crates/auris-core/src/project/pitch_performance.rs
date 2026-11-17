//! Derived channel gestures, sampled off the audio thread from the performed phrase.
use super::{
    ClipCurve, CurvePoint, MidiClip, Note, NoteTransform, PerformanceContext, performed_notes,
};
use crate::rng::{Key, Rng};
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
    /// How far soft playing darkens the sound, in 0..=1, through the brightness controller.
    ///
    /// A real instrument is not a recording turned down: struck or blown softly it loses its
    /// upper partials. At 1 a note at velocity 0.5 sits a quarter of the controller's travel
    /// under its resting middle, and a long note's volume contour moves it with the swell. Read
    /// on every note, chords included, since brightness is not a pitch gesture.
    pub brightness: f32,
    /// How unevenly the vibrato is played, in 0..=1.
    ///
    /// A player's vibrato is never a sine: each note's rate and depth differ a little, the depth
    /// grows into a held note, and the rate drifts. At 1 a note's rate varies by up to ±8% and
    /// its depth by ±25%, the depth opens from 65% to full across the note, and the rate drifts
    /// ±4% at 0.7 Hz. Drawn from where the note is, so the same text plays the same vibrato.
    pub vibrato_variation: f32,
    /// How much a player's intonation moves each note off equal temperament, in 0..=1.
    ///
    /// At 1 every note is up to ±6 cents off, drawn from where it is, and a note approached by
    /// a rising semitone is played 5 cents sharp and by a falling one 4 cents flat — the
    /// leading-tone pull string and wind players lean into. Monophonic notes only, as every
    /// bend gesture is.
    pub intonation: f32,
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
            brightness: 0.0,
            vibrato_variation: 0.0,
            intonation: 0.0,
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
            || self.brightness > 0.0
            || self.intonation > 0.0
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
                [
                    ClipCurve::MODULATION,
                    ClipCurve::Controller(7),
                    ClipCurve::SUSTAIN,
                    ClipCurve::BRIGHTNESS,
                ]
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
        if which == ClipCurve::SUSTAIN {
            return pedal_stage(&self.transforms).is_some();
        }
        let mut stages = Vec::new();
        settings(&self.transforms, None, &mut stages);
        stages.iter().any(|(s, _)| match which {
            ClipCurve::Bend => {
                s.scoop > 0.0
                    || s.vibrato > 0.0
                    || s.fall > 0.0
                    || s.glide_ms > 0.0
                    || s.intonation > 0.0
            }
            ClipCurve::Controller(1) => s.modulation > 0.0,
            ClipCurve::Controller(7) => s.volume_swell > 0.0,
            ClipCurve::BRIGHTNESS => s.brightness > 0.0,
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
    /// Modulation adds to authored CC1; volume multiplies authored CC7 (default full volume);
    /// brightness subtracts from authored CC74 (default the middle).
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
        if which == ClipCurve::SUSTAIN {
            return self.performed_pedal_points(tempo, signatures, pass, offset, span);
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
        if which == ClipCurve::BRIGHTNESS {
            ticks.extend(
                notes
                    .iter()
                    .flat_map(|n| [(n.start - Ticks(1)).max_zero(), n.start]),
            );
        }
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
            if matches!(which, ClipCurve::Controller(7) | ClipCurve::BRIGHTNESS) {
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
                let mut value = if self.curve(which).is_empty() {
                    super::clip::released_value(which)
                } else {
                    super::curve_at(self.curve(which), at)
                };
                if which == ClipCurve::BRIGHTNESS {
                    value -= darkening(&stages, &notes, &eligible, &starts, &ends, at, seconds(at));
                }
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
                                let shape = Shape::of(style, &notes, &eligible, &starts, &ends, i);
                                value += gesture(style, &shape, elapsed, length, previous, next)
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
            let rest = super::clip::released_value(which);
            if rest != 0.0 {
                events.retain(|(at, _)| *at < span);
                events.push((span, rest));
            }
            out.extend(events.into_iter().map(|(at, value)| (at + offset, value)));
        }
        out
    }
}

/// The last pedal stage at the top of `stack` that pedals at all: where it stands, and how far.
///
/// A pedal inside a drum writer's scope is not one: a kit has no dampers to lift.
fn pedal_stage(stack: &[NoteTransform]) -> Option<(usize, f32)> {
    stack
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, stage)| match stage {
            NoteTransform::Pedal { amount } if *amount > 0.0 => Some((index, *amount)),
            _ => None,
        })
}

impl MidiClip {
    /// The sustain pedal for one performance pass: the pedal stage's, read from the notes as
    /// that stage sees them, and wherever the authored lane is further down, the authored one.
    fn performed_pedal_points(
        &self,
        tempo: &TempoMap,
        signatures: &SignatureMap,
        pass: u64,
        offset: Ticks,
        span: Ticks,
    ) -> Vec<CurvePoint> {
        let authored = self.curve(ClipCurve::SUSTAIN);
        let Some((index, amount)) = pedal_stage(&self.transforms) else {
            return authored.to_vec();
        };
        let bpm = tempo.bpm_at(self.start);
        let notes: Vec<Note> = performed_notes(
            self.playable_notes().collect(),
            &self.transforms[..index],
            PerformanceContext {
                bpm,
                pass,
                start: self.start + offset,
                length: self.length,
                signatures,
            },
        )
        .into_iter()
        .filter(|n| n.start < span)
        .collect();
        let generated = super::interpretation::pedal_points(&notes, amount, bpm, span);
        let ticks: BTreeSet<Ticks> = generated
            .iter()
            .chain(
                authored
                    .iter()
                    .filter(|p| p.at >= Ticks::ZERO && p.at <= span),
            )
            .map(|p| p.at)
            .collect();
        ticks
            .into_iter()
            .map(|at| CurvePoint {
                at,
                value: super::curve_at(&generated, at).max(if authored.is_empty() {
                    0.0
                } else {
                    super::curve_at(authored, at)
                }),
            })
            .collect()
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

/// How far below its resting value the brightness controller sits at `at`.
///
/// Read from the most recent attack still sounding — or, between notes, the last one struck,
/// so the controller holds through a release rather than opening on its tail — and, on a long
/// monophonic note, moved by the volume contour the way the swell moves the level.
fn darkening(
    stages: &[(&PitchPerformance, Option<&str>)],
    notes: &[Note],
    eligible: &[bool],
    starts: &[f64],
    ends: &[f64],
    at: Ticks,
    time: f64,
) -> f32 {
    let Some(latest) = notes.iter().rposition(|n| n.start <= at) else {
        return 0.0;
    };
    let attack = notes[latest].start;
    let mut total = 0.0;
    for (style, voice) in stages {
        let amount = style.brightness.clamp(0.0, 1.0);
        if amount <= 0.0 {
            continue;
        }
        let chord = notes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.start == attack && voice.is_none_or(|v| n.drum_voice == v));
        let Some((index, velocity)) = chord
            .map(|(i, n)| (i, n.velocity))
            .max_by(|a, b| a.1.total_cmp(&b.1))
        else {
            continue;
        };
        let mut level = velocity.clamp(0.0, 1.0);
        let length = ends[index] - starts[index];
        let sounding = notes[index].start <= at && at < notes[index].end();
        if sounding && eligible[index] && length >= 0.6 && style.volume_swell > 0.0 {
            let elapsed = time - starts[index];
            level *= 1.0
                + style.volume_swell.clamp(0.0, 1.0)
                    * (style.volume_contour.level_at(elapsed / length) - 1.0);
        }
        total += amount * 0.5 * (1.0 - level.clamp(0.0, 1.0));
    }
    total
}

/// One note's own vibrato and intonation, drawn from where it is.
struct Shape {
    /// Vibrato rate as a multiple of the setting.
    rate: f64,
    /// Vibrato depth as a multiple of the setting.
    depth: f32,
    /// How unevenly the vibrato is played, from the setting.
    variation: f32,
    /// A constant offset from the written pitch, in semitones.
    offset: f32,
}

impl Shape {
    fn of(
        s: &PitchPerformance,
        notes: &[Note],
        eligible: &[bool],
        starts: &[f64],
        ends: &[f64],
        i: usize,
    ) -> Self {
        let variation = s.vibrato_variation.clamp(0.0, 1.0);
        let intonation = s.intonation.clamp(0.0, 1.0);
        let note = &notes[i];
        let mut rng = Rng::stream(
            0,
            &[
                Key::Word("vibrato"),
                Key::Index(note.start.raw().max(0) as u64),
                Key::Index(u64::from(note.pitch)),
            ],
        );
        // Roll every draw whether its dial is up, so turning one does not move the others.
        let rate = 1.0 + 0.08 * f64::from(variation) * f64::from(rng.unit() * 2.0 - 1.0);
        let depth = 1.0 + 0.25 * variation * (rng.unit() * 2.0 - 1.0);
        let drift = 0.06 * (rng.unit() * 2.0 - 1.0);
        let approach = (i > 0 && eligible[i - 1] && starts[i] - ends[i - 1] <= 0.25)
            .then(|| i16::from(note.pitch) - i16::from(notes[i - 1].pitch));
        let lean = match approach {
            Some(1) => 0.05,
            Some(-1) => -0.04,
            _ => 0.0,
        };
        Self {
            rate,
            depth,
            variation,
            offset: intonation * (drift + lean),
        }
    }
}

fn gesture(
    s: &PitchPerformance,
    shape: &Shape,
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
    let tau = std::f64::consts::TAU;
    let rate = f64::from(s.vibrato_hz.clamp(2.0, 9.0)) * shape.rate;
    // The rate drifts ±4% at 0.7 Hz; the phase is its integral, so the sway stays continuous.
    let drift =
        0.04 * f64::from(shape.variation) * (1.0 - (tau * 0.7 * after_delay).cos()) / (tau * 0.7);
    let sway = (tau * rate * (after_delay + drift)).sin() as f32;
    // The depth opens into the note: 65% of itself at the start at full variation.
    let progress = (at / length.max(1e-9)).clamp(0.0, 1.0) as f32;
    let growth = 1.0 - 0.35 * shape.variation * (1.0 - progress);
    let depth = s.vibrato.clamp(0.0, 1.0) * shape.depth * growth;
    incoming + outgoing + depth * envelope * sway + shape.offset
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

    fn note(pitch: u8, start: i64, length: i64, velocity: f32) -> Note {
        Note {
            velocity,
            ..Note::new(pitch, Ticks(start), Ticks(length))
        }
    }

    #[test]
    fn soft_attacks_darken_the_brightness_controller_and_it_rests_at_the_middle() {
        let clip = clip(
            vec![
                note(60, 0, 960, 1.0),
                note(64, 1920, 960, 0.5),
                note(48, 3840, 960, 0.4),
                note(67, 3840, 960, 0.8),
            ],
            PitchPerformance {
                brightness: 1.0,
                ..PitchPerformance::default()
            },
        );
        assert!(clip.has_generated_curve(ClipCurve::BRIGHTNESS));
        let points = clip.performed_curve_points(
            ClipCurve::BRIGHTNESS,
            &TempoMap::constant(120.0),
            &SignatureMap::default(),
            0,
            Ticks::ZERO,
            clip.length,
        );
        let at = |tick| curve_at(&points, Ticks(tick));
        assert!((at(480) - 0.5).abs() < 1e-6);
        // Half velocity is a quarter of the travel under the middle, and holds through the rest.
        assert!((at(2400) - 0.25).abs() < 1e-6);
        assert!((at(3600) - 0.25).abs() < 1e-6);
        // A chord is as bright as its loudest note.
        assert!((at(4000) - 0.4).abs() < 1e-6);
        let events = clip.sounding_performance_curve_events(
            ClipCurve::BRIGHTNESS,
            Ticks(60),
            &TempoMap::constant(120.0),
            &SignatureMap::default(),
        );
        assert_eq!(events.last(), Some(&(clip.length, 0.5)));
    }

    #[test]
    fn intonation_leans_a_rising_semitone_sharp_and_draws_the_rest_from_position() {
        let clip = clip(
            vec![note(71, 0, 960, 0.8), note(72, 960, 960, 0.8)],
            PitchPerformance {
                intonation: 1.0,
                ..PitchPerformance::default()
            },
        );
        assert!(clip.has_pitch_performance());
        let drift = |start: u64, pitch: u64| {
            let mut rng = Rng::stream(
                0,
                &[Key::Word("vibrato"), Key::Index(start), Key::Index(pitch)],
            );
            rng.unit();
            rng.unit();
            0.06 * (rng.unit() * 2.0 - 1.0)
        };
        let bend = points(&clip);
        let first = curve_at(&bend, Ticks(480));
        let second = curve_at(&bend, Ticks(1440));
        assert!((first - drift(0, 71)).abs() < 1e-5, "{first}");
        assert!((second - (drift(960, 72) + 0.05)).abs() < 1e-5, "{second}");
        assert!(first.abs() <= 0.06 && second.abs() <= 0.11);
    }

    #[test]
    fn vibrato_variation_opens_the_depth_into_the_note_and_zero_is_the_old_sine() {
        let style = PitchPerformance {
            vibrato: 0.5,
            vibrato_delay_ms: 0.0,
            ..PitchPerformance::default()
        };
        let steady = clip(vec![note(60, 0, 7680, 0.8)], style.clone());
        let tempo = TempoMap::constant(120.0);
        let bend = points(&steady);
        // At zero variation the sway is the plain sine at 5.8 Hz.
        let t = 1.0;
        let expected = 0.5 * (t * 5.8 * std::f64::consts::TAU).sin() as f32;
        let at = tempo.seconds_to_ticks(Seconds(t));
        assert!(
            (curve_at(&bend, at) - expected).abs() < 0.02,
            "{}",
            curve_at(&bend, at)
        );
        let varied = clip(
            vec![note(60, 0, 7680, 0.8)],
            PitchPerformance {
                vibrato_variation: 1.0,
                ..style
            },
        );
        let bend = points(&varied);
        let peak = |from: i64, to: i64| {
            bend.iter()
                .filter(|p| p.at.raw() >= from && p.at.raw() < to)
                .map(|p| p.value.abs())
                .fold(0.0_f32, f32::max)
        };
        let early = peak(240, 1200);
        let late = peak(6000, 6960);
        assert!(late > early * 1.3, "early {early}, late {late}");
    }

    #[test]
    fn a_pedalled_clip_writes_its_pedal_and_an_authored_one_still_counts() {
        let mut clip = MidiClip {
            notes: vec![
                Note::new(48, Ticks(0), Ticks(480)),
                Note::new(64, Ticks(0), Ticks(480)),
                Note::new(53, Ticks(1920), Ticks(480)),
                Note::new(69, Ticks(1920), Ticks(480)),
            ],
            transforms: vec![NoteTransform::Pedal { amount: 1.0 }],
            ..MidiClip::new(ClipId(1), "Piano", Ticks::ZERO, Ticks(7680))
        };
        assert!(clip.performance_curves().any(|c| c == ClipCurve::SUSTAIN));
        let pedal = |clip: &MidiClip| {
            clip.performed_curve_points(
                ClipCurve::SUSTAIN,
                &TempoMap::constant(120.0),
                &SignatureMap::default(),
                0,
                Ticks::ZERO,
                clip.length,
            )
        };
        let points = pedal(&clip);
        assert_eq!(curve_at(&points, Ticks(960)), 1.0);
        assert_eq!(curve_at(&points, Ticks(1920 + 58)), 0.0);
        assert_eq!(curve_at(&points, Ticks(1920 + 116)), 1.0);
        assert!(
            clip.notes.iter().all(|n| n.length == Ticks(480)),
            "the text is untouched"
        );
        // An authored pedal held through the change keeps the dampers up there.
        clip.controllers.insert(
            64,
            vec![
                CurvePoint {
                    at: Ticks::ZERO,
                    value: 1.0,
                },
                CurvePoint {
                    at: Ticks(3840),
                    value: 1.0,
                },
            ],
        );
        assert_eq!(curve_at(&pedal(&clip), Ticks(1920 + 58)), 1.0);
        // No pedal stage, no generated pedal: the lane is the authored one.
        clip.transforms.clear();
        assert!(!clip.has_generated_curve(ClipCurve::SUSTAIN));
        assert_eq!(pedal(&clip), clip.controllers[&64]);
    }
}
