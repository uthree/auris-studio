//! The measured envelope in the sound finder: the four range choices, their chips, and the
//! attack/decay/sustain/release drawing in the inspector.

use auris_i18n::Key;
use auris_session::{Bounds, EnvelopeTargets, MeasuredEnvelope, SoundEnvelope};

/// One stage of the envelope the finder can constrain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    Attack,
    Decay,
    Sustain,
    Release,
}

impl Stage {
    pub(super) const ALL: [Stage; 4] =
        [Stage::Attack, Stage::Decay, Stage::Sustain, Stage::Release];

    pub(super) fn label(self) -> Key {
        match self {
            Stage::Attack => Key::EnvelopeAttack,
            Stage::Decay => Key::EnvelopeDecay,
            Stage::Sustain => Key::EnvelopeSustain,
            Stage::Release => Key::EnvelopeRelease,
        }
    }

    /// The three named ranges after "Any", in the order the control shows them.
    pub(super) fn choices(self) -> [Key; 3] {
        match self {
            Stage::Attack => [Key::EnvelopeFast, Key::EnvelopeMedium, Key::EnvelopeSlow],
            Stage::Decay | Stage::Release => {
                [Key::EnvelopeShort, Key::EnvelopeMedium, Key::EnvelopeLong]
            }
            Stage::Sustain => [Key::EnvelopeHolds, Key::EnvelopeDips, Key::EnvelopeFades],
        }
    }

    /// The bounds of choice `1..=3`; the names are the ranges, so the two must change together.
    pub(super) fn bounds(self, choice: usize) -> Option<Bounds> {
        let (low, high) = match self {
            // Milliseconds to the peak.
            Stage::Attack => (15., 150.),
            // Milliseconds to the sustain, or to −60 dB.
            Stage::Decay => (300., 2000.),
            // Milliseconds after note-off to −60 dB.
            Stage::Release => (200., 1000.),
            // dB below the peak, ordered from holding to fading.
            Stage::Sustain => {
                return match choice {
                    1 => Some(Bounds {
                        min: Some(-6.),
                        max: None,
                    }),
                    2 => Some(Bounds {
                        min: Some(-30.),
                        max: Some(-6.),
                    }),
                    3 => Some(Bounds {
                        min: None,
                        max: Some(-30.),
                    }),
                    _ => None,
                };
            }
        };
        match choice {
            1 => Some(Bounds {
                min: None,
                max: Some(low),
            }),
            2 => Some(Bounds {
                min: Some(low),
                max: Some(high),
            }),
            3 => Some(Bounds {
                min: Some(high),
                max: None,
            }),
            _ => None,
        }
    }
}

/// The finder's envelope conditions: per stage, 0 for any or one of three named ranges.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EnvelopeFilter {
    pub(super) choice: [usize; 4],
}

impl EnvelopeFilter {
    pub(super) fn get(&self, stage: Stage) -> usize {
        self.choice[stage as usize]
    }

    pub(super) fn set(&mut self, stage: Stage, choice: usize) {
        self.choice[stage as usize] = choice.min(3);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.choice.iter().all(|c| *c == 0)
    }

    /// The ranges the session ranks by, the same type the agent's tool takes.
    pub(crate) fn targets(&self) -> EnvelopeTargets {
        EnvelopeTargets {
            attack_ms: Stage::Attack.bounds(self.get(Stage::Attack)),
            decay_ms: Stage::Decay.bounds(self.get(Stage::Decay)),
            sustain_db: Stage::Sustain.bounds(self.get(Stage::Sustain)),
            release_ms: Stage::Release.bounds(self.get(Stage::Release)),
        }
    }

    /// The stages with a range set, and the name of that range.
    pub(super) fn active(&self) -> impl Iterator<Item = (Stage, Key)> + '_ {
        Stage::ALL
            .into_iter()
            .filter(|stage| self.get(*stage) > 0)
            .map(|stage| (stage, stage.choices()[self.get(stage) - 1]))
    }
}

/// `12 ms`, `1.4 s`, with `≈` when the value was extrapolated.
pub(super) fn time(ms: f64, estimated: bool) -> String {
    let mark = if estimated { "≈" } else { "" };
    if ms >= ENDLESS_MS {
        format!("{mark}>{:.0} s", ENDLESS_MS / 1000.)
    } else if ms >= 1000. {
        format!("{mark}{:.1} s", ms / 1000.)
    } else {
        format!("{mark}{ms:.0} ms")
    }
}

/// The longest extrapolation the session's measurement reports; a sound at it does not end.
const ENDLESS_MS: f64 = 30_000.;

/// One line of the four measured values, as the list and the inspector show them.
pub(super) fn summary(e: &MeasuredEnvelope) -> String {
    format!(
        "A {} · D {} · S {:.0} dB · R {}",
        time(e.attack_ms, false),
        time(e.decay_ms, e.decay_estimated),
        e.sustain_db,
        time(e.release_ms, e.release_estimated),
    )
}

/// Seconds the key is drawn held after the sustain is reached, so the plateau shows.
const DRAWN_HOLD: f64 = 0.4;
/// The drawing's floor, in dB below the peak.
pub(super) const FLOOR_DB: f32 = -60.;

/// The envelope as a line, in seconds and dB below the peak: up from the floor to the peak, down
/// to the sustain (or along the decay), held, then released to the floor.
pub(super) fn curve(e: &MeasuredEnvelope, span: f64) -> Vec<(f32, f32)> {
    let attack = e.attack_ms / 1000.;
    let floor = f64::from(FLOOR_DB);
    let mut points = vec![(0., floor), (attack, 0.)];
    let off = if e.decaying {
        // The fall continues while the key is held; draw it to where it was let go.
        let rate = -floor / (e.decay_ms / 1000.).max(1e-3);
        let held = (-e.sustain_db.max(floor) / rate).min(span * 0.6);
        points.push((attack + held, (-rate * held).max(floor)));
        attack + held
    } else {
        let decayed = attack + e.decay_ms / 1000.;
        points.push((decayed, e.sustain_db.max(floor)));
        points.push((decayed + DRAWN_HOLD, e.sustain_db.max(floor)));
        decayed + DRAWN_HOLD
    };
    let level = points.last().map_or(0., |p| p.1);
    // The release is 60 dB from the level at note-off; draw the part above the floor.
    let release = e.release_ms / 1000. * ((level - floor) / 60.).clamp(0., 1.);
    points.push((off + release, floor));
    points
        .into_iter()
        .map(|(t, db)| (t.min(span) as f32, db as f32))
        .collect()
}

/// Seconds wide enough for every pitch's line, capped so one endless tail does not flatten
/// the rest.
pub(super) fn span(envelope: &SoundEnvelope) -> f64 {
    envelope
        .by_pitch
        .iter()
        .map(|(_, e)| e)
        .chain(std::iter::once(&envelope.summary))
        .map(|e| curve(e, f64::MAX).last().map_or(1., |p| f64::from(p.0)))
        .fold(0.5f64, f64::max)
        .min(8.)
}

/// `C2`, `C4`, `C6` for the pitches the session measures.
pub(super) fn pitch_name(pitch: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!(
        "{}{}",
        NAMES[usize::from(pitch % 12)],
        i32::from(pitch / 12) - 1
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(decaying: bool) -> MeasuredEnvelope {
        MeasuredEnvelope {
            attack_ms: 100.,
            decay_ms: 500.,
            sustain_db: -12.,
            release_ms: 600.,
            decaying,
            decay_estimated: false,
            release_estimated: false,
        }
    }

    #[test]
    fn every_named_range_is_ordered_and_any_is_no_range() {
        for stage in Stage::ALL {
            assert_eq!(stage.bounds(0), None);
            let ranges: Vec<_> = (1..=3).map(|c| stage.bounds(c).unwrap()).collect();
            // Adjacent ranges meet: the upper bound of one is the lower bound of the next, in
            // whichever direction the stage's names run.
            let touches = |a: Bounds, b: Bounds| a.max == b.min || a.min == b.max;
            assert!(
                touches(ranges[0], ranges[1]) && touches(ranges[1], ranges[2]),
                "{stage:?}"
            );
        }
        let mut filter = EnvelopeFilter::default();
        assert!(filter.is_empty() && filter.targets().is_empty());
        filter.set(Stage::Release, 3);
        assert_eq!(filter.targets().release_ms.unwrap().min, Some(1000.));
        assert_eq!(
            filter.active().collect::<Vec<_>>(),
            [(Stage::Release, Key::EnvelopeLong)]
        );
    }

    #[test]
    fn the_curve_rises_holds_and_falls_back_to_the_floor() {
        let held = curve(&envelope(false), 10.);
        assert_eq!(held[0], (0., FLOOR_DB));
        assert_eq!(held[1], (0.1, 0.));
        assert!((held[2].0 - 0.6).abs() < 1e-6 && held[2].1 == -12.);
        assert_eq!(held[2].1, held[3].1);
        let end = *held.last().unwrap();
        assert_eq!(end.1, FLOOR_DB);
        // 48 dB of the 60 dB release remain above the floor.
        assert!((end.0 - (1.0 + 0.48)).abs() < 1e-5, "{end:?}");
        let struck = curve(&envelope(true), 10.);
        assert!(struck.windows(2).skip(1).all(|w| w[1].1 <= w[0].1));
        assert!(curve(&envelope(false), 0.5).iter().all(|p| p.0 <= 0.5));
    }

    #[test]
    fn times_read_in_the_unit_that_suits_them() {
        assert_eq!(time(12.4, false), "12 ms");
        assert_eq!(time(1400., true), "≈1.4 s");
        assert_eq!(time(30_000., true), "≈>30 s");
        assert_eq!(pitch_name(36), "C2");
        assert_eq!(pitch_name(84), "C6");
    }
}
