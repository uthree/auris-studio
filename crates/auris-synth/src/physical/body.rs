//! Where a string's vibration becomes sound: the body it is mounted on, as a bank of modes.
//!
//! A string moves almost no air. What is heard is the soundboard, the top plate or the shell it
//! drives through the bridge, and that is a plate with hundreds of resonances of its own. The few
//! lowest are far enough apart to be heard one by one — a guitar's air resonance and its top
//! plate, a violin's two corpus modes — and are written out here by frequency. Above them the
//! modes crowd together, and what matters is how dense they are, how quickly they die and how
//! loud the crowd is across the spectrum, so that is what a [`BodySpec`] says about them. The
//! crowd is then laid out by a seeded hash, the same on every machine and in every render.
//!
//! This is what an equaliser cannot do. Every partial of every note is weighed by the mode it
//! happens to fall on, so two neighbouring notes differ in colour the way they do on wood; a
//! vibrato sweeps each partial across the resonances and its harmonics shimmer one against
//! another; and a pluck or a hammer blow, which is a sudden change of force at the bridge, sets
//! the low modes ringing on their own — the knock of the box.
//!
//! Every mode is heard in the same sense, the sense in which the bridge feels it, and under
//! them the bridge radiates a little by itself, before any mode has answered. That is what
//! keeps a body from refusing a note. Modes heard in opposite senses cancel wherever a partial
//! is as far from one as from the other, and a seeded crowd of them has a dozen such holes: a
//! key whose fundamental falls into one is all but silent beside its neighbours. Heard in one
//! sense, over something no mode can cancel, the troughs between two resonances are shallow,
//! every key is as loud as the next, and a blow reaches the air at once.
//!
//! The two halves of a body do not radiate alike, and that is the instrument's own width. Each
//! mode *leans* to one side by a seeded amount that grows with frequency, as a plate's motion
//! breaks up into smaller and smaller patches. The lean is heard only in the difference between
//! the channels, so the mono downmix is exactly the body without it.

use std::f32::consts::TAU;

use super::calibration::dial;
use super::dials;
use super::rotors::{LANES, pole};

/// One resonance a maker tunes and a listener can pick out.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Signature {
    /// Centre frequency, before the body's size scales it.
    pub hz: f32,
    /// Quality factor: the centre frequency over the half-power bandwidth.
    pub q: f32,
    /// Level of its peak, in decibels above the body's average.
    pub db: f32,
    /// How far it leans to one side, from -1 to 1.
    pub lean: f32,
}

/// The modes above the signature ones, described rather than listed.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Crowd {
    /// Where the crowd starts and where the table stops, before scaling.
    pub span: (f32, f32),
    /// Mean spacing between neighbours at the two ends of the span, in hertz.
    pub spacing: (f32, f32),
    /// Quality factor at the two ends of the span.
    pub q: (f32, f32),
    /// The most a mode leans to one side, reached at the top of the span.
    pub lean: f32,
    /// Chooses this body's own layout.
    pub seed: u32,
}

/// What a body is, from which its modes are laid out for a sample rate.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BodySpec {
    /// The low modes, by name.
    pub signature: &'static [Signature],
    /// Everything above them.
    pub crowd: Crowd,
    /// Average level over frequency: `(hertz, decibels)` corners joined straight on a
    /// logarithmic frequency axis, flat beyond the ends.
    pub tilt: &'static [(f32, f32)],
    /// Level of what the bridge radiates by itself, before any mode has answered, relative
    /// to the body's average. It is the floor of every trough between two resonances.
    pub direct_db: f32,
    /// The band the bridge radiates in by itself, before scaling: its level falls away at six
    /// decibels an octave below the first frequency and above the second.
    pub direct_span: (f32, f32),
    /// Frequency below which the body stops radiating, before scaling. Under its lowest
    /// resonance a box moves air out of one opening and in at another, and the two cancel:
    /// its output falls away at twelve decibels an octave.
    pub floor_hz: f32,
    /// Band over which the body's average level is brought to unity.
    pub reference: (f32, f32),
}

/// Most modes one body may hold. A wider spacing is chosen before this is exceeded.
const MAX_MODES: usize = 192;

fn hash(seed: u32, index: u32) -> u32 {
    let mut x = seed ^ index.wrapping_mul(0x9E37_79B9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^ (x >> 16)
}

/// A seeded number in `0.0..1.0`, the same for the same two arguments everywhere.
pub(crate) fn seeded(seed: u32, index: u32) -> f32 {
    (hash(seed, index) >> 8) as f32 / (1_u32 << 24) as f32
}

fn db(value: f32) -> f32 {
    10.0_f32.powf(value / 20.0)
}

/// A value running from `ends.0` at `span.0` to `ends.1` at `span.1`, geometrically in both.
fn along(span: (f32, f32), ends: (f32, f32), hz: f32) -> f32 {
    let t = ((hz / span.0).ln() / (span.1 / span.0).ln()).clamp(0.0, 1.0);
    ends.0 * (ends.1 / ends.0).powf(t)
}

impl BodySpec {
    /// The body's average level at `hz`, in decibels, when its frequencies are scaled by `scale`.
    pub(crate) fn tilt_db(&self, hz: f32, scale: f32) -> f32 {
        let hz = hz / scale;
        // A calibration run's octave dials, which are all zero in an application build.
        let octave = (hz / 50.0).max(1.0).log2().clamp(0.0, 9.0);
        let index = (octave as usize).min(8);
        let low = dial(dials::TILT + index, 0.0);
        let high = dial(dials::TILT + index + 1, 0.0);
        let turned = low + (high - low) * (octave - index as f32);
        let Some(first) = self.tilt.first() else {
            return turned;
        };
        if hz <= first.0 {
            return first.1 + turned;
        }
        for pair in self.tilt.windows(2) {
            let (low, high) = (pair[0], pair[1]);
            if hz <= high.0 {
                let t = (hz / low.0).ln() / (high.0 / low.0).ln();
                return low.1 + (high.1 - low.1) * t + turned;
            }
        }
        self.tilt.last().map_or(0.0, |corner| corner.1) + turned
    }

    /// How readily the bridge takes energy from a string at `hz`, from zero to about one: the
    /// signature modes' share of the bridge's conductance, largest on the strongest of them.
    ///
    /// A partial that falls on a body resonance is passed to the air faster, so it is both
    /// louder and shorter than its neighbours. The body filter makes it louder; this is what
    /// makes it shorter.
    pub(crate) fn conductance(&self, hz: f32, scale: f32) -> f32 {
        let strongest = self
            .signature
            .iter()
            .map(|mode| db(mode.db))
            .fold(0.0_f32, f32::max)
            .max(1.0e-6);
        self.signature
            .iter()
            .map(|mode| {
                let centre = mode.hz * scale;
                let detuning = mode.q * (hz / centre - centre / hz);
                db(mode.db) / strongest / (1.0 + detuning * detuning)
            })
            .sum::<f32>()
            .min(1.5)
    }

    /// Lays the modes out below `limit` hertz, beside what the bridge radiates by itself.
    fn modes(&self, scale: f32, limit: f32, rate: f32, direct: &Direct) -> Vec<ModeSpec> {
        let mut modes: Vec<ModeSpec> = self
            .signature
            .iter()
            .filter(|mode| mode.hz * scale < limit)
            .map(|mode| ModeSpec {
                hz: mode.hz * scale,
                q: mode.q,
                gain: db(mode.db),
                lean: mode.lean,
            })
            .collect();
        let crowd = self.crowd;
        let span = (crowd.span.0 * scale, (crowd.span.1 * scale).min(limit));
        if span.1 <= span.0 {
            return modes;
        }
        // Count the crowd at its natural spacing first, and thin it if it would not fit.
        let natural = |hz: f32| along(span, crowd.spacing, hz) * scale;
        let mut count = 0_usize;
        let mut hz = span.0;
        while hz < span.1 && count < 100_000 {
            hz += natural(hz);
            count += 1;
        }
        let room = MAX_MODES.saturating_sub(modes.len()).max(1);
        let thinning = (count as f32 / room as f32).max(1.0);
        let mut hz = span.0;
        let mut index = 0_u32;
        while hz < span.1 && modes.len() < MAX_MODES {
            let spacing = natural(hz) * thinning;
            let centre = hz + spacing * (0.5 + 0.6 * (seeded(crowd.seed, index * 4) - 0.5));
            let q = along(span, crowd.q, centre);
            // Resonances of unit peak, `centre / q` wide and `spacing` apart and all heard in
            // one sense, come to `π/2 · overlap` on average, and their mean square to
            // `2 coth(π · overlap) − 1` times the square of that. What the bridge radiates by
            // itself adds to the first. The peak that brings the two onto the tilt together
            // is the root of a quadratic; `level` below then corrects it for a crowd that is
            // not evenly spaced.
            let overlap = f64::from(centre / (q * spacing)).max(1.0e-3);
            let (direct_re, direct_im) = direct.response(centre, rate);
            let wanted = f64::from(db(self.tilt_db(centre, scale)));
            let spread = 2.0 / (std::f64::consts::PI * overlap).tanh() - 1.0;
            let short = wanted * wanted - (direct_re * direct_re + direct_im * direct_im);
            let inside = direct_re * direct_re + spread * short.max(0.0);
            let mean = ((inside.sqrt() - direct_re) / spread).max(0.05 * wanted);
            let level = (mean / (std::f64::consts::FRAC_PI_2 * overlap)) as f32;
            let size = 0.8 + 0.4 * seeded(crowd.seed, index * 4 + 2);
            let reach = ((centre / span.0).ln() / (span.1 / span.0).ln()).clamp(0.0, 1.0);
            let lean = crowd.lean * reach.sqrt() * (2.0 * seeded(crowd.seed, index * 4 + 3) - 1.0);
            if centre < limit {
                modes.push(ModeSpec {
                    hz: centre,
                    q,
                    gain: size * level,
                    lean,
                });
            }
            hz += spacing;
            index += 1;
        }
        modes
    }
}

#[derive(Clone, Copy, Debug)]
struct ModeSpec {
    hz: f32,
    q: f32,
    /// Peak gain.
    gain: f32,
    lean: f32,
}

/// What the bridge radiates by itself: a gain, and the two one-pole filters that bound its band.
#[derive(Clone, Copy, Debug, Default)]
struct Direct {
    gain: f32,
    /// Pole of the low-pass at the top of the band.
    top: f32,
    /// Pole of the low-pass at the bottom of the band, whose output is taken away.
    bottom: f32,
}

impl Direct {
    fn of(spec: &BodySpec, scale: f32, rate: f32) -> Self {
        let pole = |hz: f32| (-TAU * (hz * scale).min(rate * 0.45) / rate).exp();
        Self {
            gain: db(spec.direct_db),
            top: pole(spec.direct_span.1),
            bottom: pole(spec.direct_span.0),
        }
    }

    /// Complex response at `hz`. Its real half is positive at every frequency, as every
    /// mode's is, which is why nothing in a body can cancel anything else in it.
    fn response(&self, hz: f32, rate: f32) -> (f64, f64) {
        let omega = f64::from(TAU) * f64::from(hz) / f64::from(rate);
        let (sin_w, cos_w) = omega.sin_cos();
        // (1 - p) / (1 - p e^{-jω}): a one-pole low-pass.
        let low = |pole: f32| {
            let pole = f64::from(pole);
            let (den_re, den_im) = (1.0 - pole * cos_w, pole * sin_w);
            let norm = den_re * den_re + den_im * den_im;
            ((1.0 - pole) * den_re / norm, -(1.0 - pole) * den_im / norm)
        };
        let (top_re, top_im) = low(self.top);
        let (bottom_re, bottom_im) = low(self.bottom);
        let (pass_re, pass_im) = (1.0 - bottom_re, -bottom_im);
        let gain = f64::from(self.gain);
        (
            gain * (top_re * pass_re - top_im * pass_im),
            gain * (top_re * pass_im + top_im * pass_re),
        )
    }
}

/// A mode as the sums below need it: its pole, and its gain on the real half.
struct Pole {
    cosine: f64,
    sine: f64,
    gain: f64,
}

fn poles(modes: &[ModeSpec], rate: f32) -> Vec<Pole> {
    modes
        .iter()
        .map(|mode| {
            let nepers = std::f32::consts::PI * mode.hz / mode.q;
            let (cosine, sine) = pole(mode.hz, nepers, rate);
            let (cosine, sine) = (f64::from(cosine), f64::from(sine));
            Pole {
                cosine,
                sine,
                gain: f64::from(mode.gain) * 2.0 * (1.0 - cosine.hypot(sine)),
            }
        })
        .collect()
}

/// Complex response of a body at `hz`: its modes, each heard on its real half at its peak
/// gain, and what the bridge radiates by itself.
fn response(poles: &[Pole], direct: &Direct, hz: f32, rate: f32) -> (f64, f64) {
    let omega = f64::from(TAU) * f64::from(hz) / f64::from(rate);
    let (sin_w, cos_w) = omega.sin_cos();
    let (mut total_re, mut total_im) = direct.response(hz, rate);
    for pole in poles {
        let c = pole.cosine;
        // 1 / (1 - p e^{-jω}) for the pole and for its conjugate, halved: the real half.
        for s in [pole.sine, -pole.sine] {
            let (den_re, den_im) = (1.0 - (c * cos_w + s * sin_w), c * sin_w - s * cos_w);
            let norm = den_re * den_re + den_im * den_im;
            total_re += 0.5 * pole.gain * den_re / norm;
            total_im -= 0.5 * pole.gain * den_im / norm;
        }
    }
    (total_re, total_im)
}

/// Brings the crowd's local average onto the tilt.
///
/// The first guess takes each mode's neighbours to be evenly spaced and all alike. They are
/// not: the spacing is seeded, the tilt changes across a mode's own skirts, and at either end of
/// the crowd there are neighbours on one side only. So the body as laid out is measured — its
/// mean square over a band either side of each mode — and each mode is brought towards what the
/// tilt asks for there. A few rounds settle it.
fn level(
    spec: &BodySpec,
    modes: &mut [ModeSpec],
    crowd_from: usize,
    direct: &Direct,
    scale: f32,
    rate: f32,
) {
    const ROUNDS: usize = 4;
    const PROBES: i32 = 3;
    let crowd_from = crowd_from.min(modes.len());
    for _ in 0..ROUNDS {
        let laid = poles(modes, rate);
        let factors: Vec<f32> = modes[crowd_from..]
            .iter()
            .map(|mode| {
                let (mut heard, mut wanted) = (0.0_f64, 0.0_f64);
                for step in -PROBES..=PROBES {
                    let hz = mode.hz * 2.0_f32.powf(0.1 * step as f32);
                    if hz >= rate * 0.49 {
                        continue;
                    }
                    let (re, im) = response(&laid, direct, hz, rate);
                    heard += re * re + im * im;
                    let asked = f64::from(db(spec.tilt_db(hz, scale)));
                    wanted += asked * asked;
                }
                if heard > 0.0 {
                    ((wanted / heard).sqrt() as f32).clamp(0.5, 2.0)
                } else {
                    1.0
                }
            })
            .collect();
        for (mode, factor) in modes[crowd_from..].iter_mut().zip(factors) {
            mode.gain *= factor;
        }
    }
}

type Lane = [f32; LANES];

/// A body, prepared for a sample rate: its modes, and the state of the two signals in it.
///
/// The *mid* signal is the instrument as a listener in front of it hears it. The *side* signal
/// is what the right ear hears more of than the left: the notes a player has to the right, and
/// the half of each mode that leans that way. Both pass through the same modes, each with its
/// own state, so placing a note never changes its colour.
#[derive(Clone, Debug, Default)]
pub(crate) struct Radiator {
    cosine: Vec<Lane>,
    sine: Vec<Lane>,
    gain: Vec<Lane>,
    /// Gain times lean: what a mode adds to the side from the mid signal alone.
    leaning: Vec<Lane>,
    mid_re: Vec<Lane>,
    mid_im: Vec<Lane>,
    side_re: Vec<Lane>,
    side_im: Vec<Lane>,
    groups: usize,
    /// What the bridge radiates by itself, and the state of its two filters for mid and side.
    direct: Direct,
    direct_state: [[f32; 2]; 2],
    /// Two one-pole high-pass stages on each output: the floor of what the body radiates.
    floor_state: [[f32; 2]; 2],
    floor_pole: f32,
    /// Gain of the unprocessed signal when the body control is turned down.
    dry_gain: f32,
    /// Control ticks since each signal last had anything in it.
    idle: [u32; 2],
    /// Whether each signal was given anything since the last tick.
    heard: [bool; 2],
    /// Whether each signal's modes are all at rest, and so not advanced.
    asleep: [bool; 2],
    count: usize,
    /// What was laid out, and how: the body, the scale of its frequencies, the sample rate,
    /// and what its level was divided by to bring its average to unity.
    spec: Option<&'static BodySpec>,
    scale: f32,
    rate: f32,
    average: f32,
}

/// Silent control ticks after which a body's state is checked for having died away.
const IDLE_TICKS: u32 = 16;

impl Radiator {
    /// Lays out `spec` for `rate`, with every frequency multiplied by `scale`. Allocates.
    pub(crate) fn prepare(&mut self, spec: &'static BodySpec, scale: f32, rate: f32) {
        let limit = (rate * 0.45).min(20_000.0);
        let mut direct = Direct::of(spec, scale, rate);
        let mut modes = spec.modes(scale, limit, rate, &direct);
        let crowd_from = spec
            .signature
            .iter()
            .filter(|mode| mode.hz * scale < limit)
            .count();
        level(spec, &mut modes, crowd_from, &direct, scale, rate);
        // Unity average over the reference band, measured on the laid-out body itself.
        let reference = (
            spec.reference.0 * scale,
            (spec.reference.1 * scale).min(limit),
        );
        let mut power = 0.0_f64;
        const PROBES: usize = 160;
        let laid = poles(&modes, rate);
        for probe in 0..PROBES {
            let hz = reference.0
                * (reference.1 / reference.0).powf((probe as f32 + 0.5) / PROBES as f32);
            let level = f64::from(db(spec.tilt_db(hz, scale)));
            let (re, im) = response(&laid, &direct, hz, rate);
            power += (re * re + im * im) / (level * level);
        }
        let average = (power / PROBES as f64).sqrt().max(1.0e-9) as f32;
        for mode in &mut modes {
            mode.gain /= average;
        }
        direct.gain /= average;
        let groups = modes.len().div_ceil(LANES).max(1);
        for lanes in [
            &mut self.cosine,
            &mut self.sine,
            &mut self.gain,
            &mut self.leaning,
            &mut self.mid_re,
            &mut self.mid_im,
            &mut self.side_re,
            &mut self.side_im,
        ] {
            lanes.clear();
            lanes.resize(groups, [0.0; LANES]);
        }
        for (index, mode) in modes.iter().enumerate() {
            let (group, lane) = (index / LANES, index % LANES);
            // A mode `hz / q` wide at half power decays at `π hz / q` nepers a second.
            let nepers = std::f32::consts::PI * mode.hz / mode.q;
            let (cosine, sine) = pole(mode.hz, nepers, rate);
            // Peak gain of an undriven phasor's real half is `1 / (2 (1 - radius))`.
            let radius = cosine.hypot(sine);
            let gain = mode.gain * 2.0 * (1.0 - radius);
            self.cosine[group][lane] = cosine;
            self.sine[group][lane] = sine;
            self.gain[group][lane] = gain;
            self.leaning[group][lane] = gain * mode.lean;
        }
        self.groups = groups;
        self.count = modes.len();
        self.direct = direct;
        self.spec = Some(spec);
        self.scale = scale;
        self.rate = rate;
        self.average = average;
        self.floor_pole = (-TAU * (spec.floor_hz * scale).min(rate * 0.2) / rate).exp();
        self.dry_gain = 1.0;
        self.reset();
    }

    /// Modes laid out.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.count
    }

    /// How much louder than the notes beside it this body makes a note at `hz`: the level of
    /// the note's first harmonics through the body, against what its tilt alone would make of
    /// them. One is a note the body neither favours nor refuses.
    ///
    /// A body's low resonances are a few semitones wide and a few times as loud as what lies
    /// between them, and a note that sits on one is that much louder than its neighbours.
    /// Every player of such an instrument learns which notes those are and plays them more
    /// gently; this is what they have learned. It walks the modes once and does not allocate,
    /// so it can be asked for a note as it starts.
    pub(crate) fn favour(&self, hz: f32) -> f32 {
        let Some(spec) = self.spec else {
            return 1.0;
        };
        let (mut heard, mut expected) = (0.0_f64, 0.0_f64);
        for harmonic in 1..=6_u32 {
            let at = hz * harmonic as f32;
            if at >= self.rate * 0.45 {
                break;
            }
            // A string's harmonics fall away as their number, and their power as its square.
            let weight = 1.0 / f64::from(harmonic * harmonic);
            let level = f64::from(db(spec.tilt_db(at, self.scale)) / self.average.max(1.0e-9));
            heard += weight * self.power_at(at);
            expected += weight * level * level;
        }
        if heard <= 0.0 || expected <= 0.0 {
            return 1.0;
        }
        ((heard / expected).sqrt() as f32).clamp(0.25, 4.0)
    }

    /// Squared magnitude of the body's response at `hz`, before the floor.
    fn power_at(&self, hz: f32) -> f64 {
        let omega = f64::from(TAU) * f64::from(hz) / f64::from(self.rate);
        let (sin_w, cos_w) = omega.sin_cos();
        let (mut re, mut im) = self.direct.response(hz, self.rate);
        let poles = self
            .cosine
            .iter()
            .zip(self.sine.iter())
            .zip(self.gain.iter());
        for ((cosine, sine), gain) in poles.take(self.groups) {
            for lane in 0..LANES {
                let (c, gain) = (f64::from(cosine[lane]), f64::from(gain[lane]));
                for s in [f64::from(sine[lane]), -f64::from(sine[lane])] {
                    let (den_re, den_im) = (1.0 - (c * cos_w + s * sin_w), c * sin_w - s * cos_w);
                    let norm = den_re * den_re + den_im * den_im;
                    re += 0.5 * gain * den_re / norm;
                    im -= 0.5 * gain * den_im / norm;
                }
            }
        }
        re * re + im * im
    }

    /// Stops the body ringing.
    pub(crate) fn reset(&mut self) {
        for lanes in [
            &mut self.mid_re,
            &mut self.mid_im,
            &mut self.side_re,
            &mut self.side_im,
        ] {
            lanes.fill([0.0; LANES]);
        }
        self.direct_state = [[0.0; 2]; 2];
        self.floor_state = [[0.0; 2]; 2];
        self.idle = [0; 2];
        self.heard = [false; 2];
        self.asleep = [true; 2];
    }

    /// Marks a control tick. A signal left silent for long enough has its modes looked at, and
    /// is put to sleep once they have all died away, so a body nobody is playing costs nothing
    /// and its arithmetic never reaches the denormal range. Counting ticks rather than calls
    /// keeps a render the same whatever block size the host asks for.
    pub(crate) fn tick(&mut self) {
        for index in 0..2 {
            if std::mem::take(&mut self.heard[index]) {
                self.idle[index] = 0;
            } else if !self.asleep[index] {
                self.idle[index] += 1;
                if self.idle[index] >= IDLE_TICKS {
                    self.idle[index] = 0;
                    self.asleep[index] = !self.settle(index == 1);
                }
            }
        }
    }

    /// Passes a block through the body in place.
    ///
    /// `amount` runs from the bare bridge signal at zero to the body alone at one, and `width`
    /// scales how much of each mode's lean reaches the side. Blocks longer than
    /// [`Radiator::BLOCK`] are cut to it; callers render in control ticks far shorter.
    pub(crate) fn process(&mut self, mid: &mut [f32], side: &mut [f32], amount: f32, width: f32) {
        let frames = mid.len().min(side.len()).min(Self::BLOCK);
        let (mut mid_in, mut side_in) = ([0.0_f32; Self::BLOCK], [0.0_f32; Self::BLOCK]);
        mid_in[..frames].copy_from_slice(&mid[..frames]);
        side_in[..frames].copy_from_slice(&side[..frames]);
        for (index, block) in [&mid_in, &side_in].into_iter().enumerate() {
            if block[..frames].iter().any(|sample| *sample != 0.0) {
                self.heard[index] = true;
                self.asleep[index] = false;
            }
        }
        let amount = amount.clamp(0.0, 1.0);
        let dry = (1.0 - amount) * self.dry_gain;
        let (mid_awake, side_awake) = (!self.asleep[0], !self.asleep[1]);
        for frame in 0..frames {
            let (push_mid, push_side) = (mid_in[frame], side_in[frame]);
            let mut heard_mid = [0.0_f32; LANES];
            let mut heard_side = [0.0_f32; LANES];
            if mid_awake {
                let state = self.mid_re.iter_mut().zip(self.mid_im.iter_mut());
                let poles = self.cosine.iter().zip(self.sine.iter());
                let ears = self.gain.iter().zip(self.leaning.iter());
                for (((re, im), (cosine, sine)), (gain, leaning)) in
                    state.zip(poles).zip(ears).take(self.groups)
                {
                    for lane in 0..LANES {
                        let turned = cosine[lane] * re[lane] - sine[lane] * im[lane] + push_mid;
                        im[lane] = sine[lane] * re[lane] + cosine[lane] * im[lane];
                        re[lane] = turned;
                        heard_mid[lane] += gain[lane] * turned;
                        heard_side[lane] += leaning[lane] * turned;
                    }
                }
            }
            let mut placed = [0.0_f32; LANES];
            if side_awake {
                let state = self.side_re.iter_mut().zip(self.side_im.iter_mut());
                let poles = self.cosine.iter().zip(self.sine.iter());
                for (((re, im), (cosine, sine)), gain) in
                    state.zip(poles).zip(self.gain.iter()).take(self.groups)
                {
                    for lane in 0..LANES {
                        let turned = cosine[lane] * re[lane] - sine[lane] * im[lane] + push_side;
                        im[lane] = sine[lane] * re[lane] + cosine[lane] * im[lane];
                        re[lane] = turned;
                        placed[lane] += gain[lane] * turned;
                    }
                }
            }
            // What the bridge radiates by itself, in its own band.
            let mut direct = [0.0_f32; 2];
            for ((out, push), [top, bottom]) in direct
                .iter_mut()
                .zip([push_mid, push_side])
                .zip(self.direct_state.iter_mut())
            {
                *top = push + self.direct.top * (*top - push);
                *bottom = *top + self.direct.bottom * (*bottom - *top);
                *out = (*top - *bottom) * self.direct.gain;
            }
            let mut body = [
                heard_mid.iter().sum::<f32>() + direct[0],
                placed.iter().sum::<f32>() + direct[1] + width * heard_side.iter().sum::<f32>(),
            ];
            for (signal, stages) in body.iter_mut().zip(self.floor_state.iter_mut()) {
                for low in stages {
                    *low = *signal + self.floor_pole * (*low - *signal);
                    *signal -= *low;
                }
            }
            mid[frame] = dry * push_mid + amount * body[0];
            side[frame] = dry * push_side + amount * body[1];
        }
    }

    /// Longest block [`Radiator::process`] takes at once.
    pub(crate) const BLOCK: usize = 64;

    fn halves(&mut self, side: bool) -> (&mut Vec<Lane>, &mut Vec<Lane>) {
        if side {
            (&mut self.side_re, &mut self.side_im)
        } else {
            (&mut self.mid_re, &mut self.mid_im)
        }
    }

    /// Whether either signal still has modes moving.
    #[cfg(test)]
    fn is_ringing(&self) -> bool {
        self.asleep != [true; 2]
    }

    /// Puts to exact rest the groups that have died away, and says whether any is still moving.
    fn settle(&mut self, side: bool) -> bool {
        let mut ringing = false;
        let (re, im) = self.halves(side);
        for (re, im) in re.iter_mut().zip(im.iter_mut()) {
            let moving: f32 = re.iter().chain(im.iter()).map(|value| value.abs()).sum();
            if moving.is_finite() && moving > 1.0e-12 {
                ringing = true;
            } else {
                *re = [0.0; LANES];
                *im = [0.0; LANES];
            }
        }
        if !ringing {
            self.direct_state[usize::from(side)] = [0.0; 2];
            self.floor_state[usize::from(side)] = [0.0; 2];
        }
        ringing
    }

    /// Displacement of the body's first `out.len()` modes, mid signal only, for a display.
    pub(crate) fn observe(&self, out: &mut [f32]) {
        for (index, value) in out.iter_mut().enumerate() {
            let (group, lane) = (index / LANES, index % LANES);
            *value = if index < self.count {
                self.mid_im.get(group).map_or(0.0, |lanes| lanes[lane])
                    * self.gain.get(group).map_or(0.0, |lanes| lanes[lane]).abs()
            } else {
                0.0
            };
        }
    }
}

/// A grand piano's soundboard: no mode a listener could name, and a great many of them.
pub(crate) const SOUNDBOARD: BodySpec = BodySpec {
    signature: &[],
    crowd: Crowd {
        span: (46.0, 5_200.0),
        spacing: (13.0, 48.0),
        q: (20.0, 55.0),
        // A piano's width is its keyboard's; the board adds only a little of its own.
        lean: 0.3,
        seed: 0x0050_1A40,
    },
    // A board is a poor radiator below its first modes — a low string's fundamental is
    // twenty decibels under its strongest partials on the recorded grand, and that is this
    // slope; it is at its best around a kilohertz, where the waves in it come to match the
    // waves in the air; and above a few kilohertz its ribs divide it into patches too small
    // to move much air.
    tilt: &[
        (40.0, -24.0),
        (65.0, -14.0),
        (130.0, -9.0),
        (190.0, -1.5),
        (260.0, 0.0),
        (450.0, 0.0),
        (1_000.0, 3.0),
        (1_800.0, 1.0),
        (4_000.0, -7.0),
        (8_000.0, -15.0),
    ],
    direct_db: -4.0,
    direct_span: (200.0, 2_200.0),
    floor_hz: 55.0,
    reference: (150.0, 2_500.0),
};

/// A flat-top guitar: the air in the box, the top plate, and the plates' higher modes.
pub(crate) const GUITAR: BodySpec = BodySpec {
    signature: &[
        Signature {
            hz: 102.0,
            q: 20.0,
            db: 7.0,
            lean: 0.0,
        },
        Signature {
            hz: 204.0,
            q: 22.0,
            db: 8.0,
            lean: 0.0,
        },
        Signature {
            hz: 244.0,
            q: 28.0,
            db: 1.0,
            lean: 0.15,
        },
        Signature {
            hz: 302.0,
            q: 30.0,
            db: -1.0,
            lean: 0.5,
        },
        Signature {
            hz: 392.0,
            q: 32.0,
            db: 3.0,
            lean: -0.3,
        },
        Signature {
            hz: 468.0,
            q: 35.0,
            db: 0.0,
            lean: 0.4,
        },
        Signature {
            hz: 545.0,
            q: 35.0,
            db: 2.0,
            lean: -0.5,
        },
        Signature {
            hz: 610.0,
            q: 38.0,
            db: 0.0,
            lean: 0.3,
        },
    ],
    crowd: Crowd {
        span: (660.0, 6_500.0),
        spacing: (32.0, 78.0),
        q: (38.0, 60.0),
        lean: 0.7,
        seed: 0x6017_A200,
    },
    tilt: &[
        (70.0, -14.0),
        (95.0, -2.0),
        (180.0, 0.0),
        (700.0, 0.0),
        (1_500.0, -2.0),
        (3_000.0, -6.0),
        (6_000.0, -13.0),
    ],
    direct_db: -5.0,
    direct_span: (170.0, 2_500.0),
    floor_hz: 88.0,
    reference: (100.0, 3_000.0),
};

/// A violin, and with its frequencies scaled down the viola, cello and bass built like it.
///
/// The air resonance, the two corpus modes either side of 500 Hz and the broad rise near
/// 2.5 kHz, where the bridge itself rocks, are what every instrument of the family shares.
pub(crate) const VIOLIN: BodySpec = BodySpec {
    signature: &[
        Signature {
            hz: 275.0,
            q: 16.0,
            db: 5.0,
            lean: 0.0,
        },
        Signature {
            hz: 405.0,
            q: 30.0,
            db: -6.0,
            lean: 0.3,
        },
        Signature {
            hz: 465.0,
            q: 32.0,
            db: 5.0,
            lean: -0.2,
        },
        Signature {
            hz: 548.0,
            q: 30.0,
            db: 6.0,
            lean: 0.25,
        },
        Signature {
            hz: 690.0,
            q: 30.0,
            db: 1.0,
            lean: -0.4,
        },
        Signature {
            hz: 790.0,
            q: 32.0,
            db: 2.0,
            lean: 0.5,
        },
        Signature {
            hz: 880.0,
            q: 32.0,
            db: 0.0,
            lean: -0.5,
        },
        Signature {
            hz: 990.0,
            q: 34.0,
            db: 1.0,
            lean: 0.4,
        },
    ],
    crowd: Crowd {
        span: (1_080.0, 7_500.0),
        spacing: (60.0, 112.0),
        q: (34.0, 45.0),
        lean: 0.75,
        seed: 0x0710_1140,
    },
    tilt: &[
        (190.0, -15.0),
        (260.0, -3.0),
        (450.0, 0.0),
        (1_100.0, -3.0),
        (1_600.0, -3.5),
        (2_300.0, 3.0),
        (3_000.0, 3.0),
        (4_200.0, -4.0),
        (6_000.0, -14.0),
        (8_000.0, -22.0),
    ],
    direct_db: -8.0,
    direct_span: (280.0, 1_800.0),
    floor_hz: 235.0,
    reference: (250.0, 4_000.0),
};

/// A hammered dulcimer's shallow trapezoid: a small box, and a bright one.
pub(crate) const DULCIMER: BodySpec = BodySpec {
    signature: &[
        Signature {
            hz: 155.0,
            q: 12.0,
            db: 4.0,
            lean: 0.0,
        },
        Signature {
            hz: 240.0,
            q: 20.0,
            db: 3.0,
            lean: 0.2,
        },
        Signature {
            hz: 330.0,
            q: 25.0,
            db: 1.0,
            lean: -0.3,
        },
    ],
    crowd: Crowd {
        span: (400.0, 7_000.0),
        spacing: (36.0, 88.0),
        q: (30.0, 50.0),
        lean: 0.7,
        seed: 0x0D01_C1AE,
    },
    tilt: &[
        (100.0, -12.0),
        (150.0, 0.0),
        (2_500.0, 0.0),
        (5_000.0, -4.0),
        (8_000.0, -10.0),
    ],
    direct_db: -3.0,
    direct_span: (160.0, 5_000.0),
    floor_hz: 130.0,
    reference: (150.0, 4_000.0),
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{goertzel, peak, rms};

    fn impulse(body: &mut Radiator, frames: usize, side: f32, width: f32) -> (Vec<f32>, Vec<f32>) {
        let (mut mid_out, mut side_out) = (Vec::new(), Vec::new());
        for start in (0..frames).step_by(Radiator::BLOCK) {
            let mut mid = [0.0; Radiator::BLOCK];
            let mut placed = [0.0; Radiator::BLOCK];
            if start == 0 {
                mid[0] = 1.0;
                placed[0] = side;
            }
            body.tick();
            body.process(&mut mid, &mut placed, 1.0, width);
            mid_out.extend_from_slice(&mid);
            side_out.extend_from_slice(&placed);
        }
        (mid_out, side_out)
    }

    fn prepared(spec: &'static BodySpec, scale: f32, rate: f32) -> Radiator {
        let mut body = Radiator::default();
        body.prepare(spec, scale, rate);
        body
    }

    #[test]
    fn every_body_is_stable_and_dies_away_at_every_rate() {
        for spec in [&SOUNDBOARD, &GUITAR, &VIOLIN, &DULCIMER] {
            for rate in [8_000.0, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
                for scale in [1.0, 0.23] {
                    let mut body = prepared(spec, scale, rate);
                    let (mid, side) = impulse(&mut body, rate as usize * 2, 0.0, 1.0);
                    assert!(mid.iter().chain(&side).all(|sample| sample.is_finite()));
                    assert!(peak(&mid) < 4.0, "peak {}", peak(&mid));
                    let tail = &mid[(rate * 1.9) as usize..];
                    assert!(peak(tail) < 1.0e-4, "still ringing at {}", peak(tail));
                }
            }
        }
    }

    #[test]
    fn a_guitar_rings_on_its_air_and_top_plate_and_is_weak_between_and_below() {
        let mut body = prepared(&GUITAR, 1.0, 48_000.0);
        let (mid, _) = impulse(&mut body, 48_000, 0.0, 0.0);
        let at = |hz: f64| goertzel(&mid, 48_000.0, hz);
        assert!(
            at(102.0) > at(140.0) * 2.5,
            "air {} vs {}",
            at(102.0),
            at(140.0)
        );
        assert!(at(204.0) > at(150.0) * 2.5);
        assert!(
            at(102.0) > at(50.0) * 12.0,
            "nothing radiates below the air mode"
        );
        // The knock of the box: a tap rings for a good part of a second, low.
        let late = &mid[4_800..9_600];
        assert!(rms(late) > 1.0e-5);
        assert!(goertzel(late, 48_000.0, 102.0) > goertzel(late, 48_000.0, 2_000.0) * 5.0);
    }

    #[test]
    fn the_crowd_follows_the_tilt_on_average_whatever_its_density() {
        for (spec, scale) in [(&SOUNDBOARD, 1.0), (&VIOLIN, 1.0), (&VIOLIN, 0.375)] {
            let mut body = prepared(spec, scale, 48_000.0);
            let (mid, _) = impulse(&mut body, 96_000, 0.0, 0.0);
            // Third-octave power against the tilt, at centres inside the crowd.
            for centre in [1_500.0_f32, 2_500.0, 4_000.0] {
                let centre = centre * scale.max(0.5);
                let mut power = 0.0;
                const BINS: usize = 60;
                for bin in 0..BINS {
                    let hz = centre * 2.0_f32.powf((bin as f32 / BINS as f32 - 0.5) / 1.5);
                    // An impulse has unit amplitude at every frequency; undo Goertzel's 2/N.
                    let level = goertzel(&mid, 48_000.0, f64::from(hz)) * mid.len() as f64 / 2.0;
                    power += level * level;
                }
                let measured = 10.0 * (power / BINS as f64).log10();
                let expected = f64::from(spec.tilt_db(centre, scale));
                assert!(
                    (measured - expected).abs() < 4.5,
                    "{centre} Hz at scale {scale}: {measured:.1} dB, tilt {expected:.1} dB"
                );
            }
        }
    }

    #[test]
    fn the_lean_is_heard_only_in_the_side_and_only_as_wide_as_asked() {
        let mut centred = prepared(&VIOLIN, 1.0, 48_000.0);
        let mut wide = prepared(&VIOLIN, 1.0, 48_000.0);
        let (mid_centred, side_centred) = impulse(&mut centred, 9_600, 0.0, 0.0);
        let (mid_wide, side_wide) = impulse(&mut wide, 9_600, 0.0, 1.0);
        assert_eq!(
            mid_centred, mid_wide,
            "width must not colour the mid signal"
        );
        assert_eq!(peak(&side_centred), 0.0);
        assert!(rms(&side_wide) > rms(&mid_wide) * 0.1);
        assert!(rms(&side_wide) < rms(&mid_wide) * 0.8);
        // The low modes do not lean: below the crowd the side is far under the mid.
        let low = |audio: &[f32]| goertzel(audio, 48_000.0, 275.0);
        assert!(low(&side_wide) < low(&mid_wide) * 0.05);
    }

    #[test]
    fn no_body_refuses_a_note() {
        // A semitone at a time from below the body's range to five kilohertz: what comes out,
        // against what the tilt asks for there.
        let apart = |body: &Radiator, spec: &BodySpec, hz: f32| {
            let asked = f64::from(spec.tilt_db(hz, 1.0)) - 20.0 * f64::from(body.average).log10();
            10.0 * body.power_at(hz).log10() - asked
        };
        for spec in [&SOUNDBOARD, &GUITAR, &VIOLIN, &DULCIMER] {
            let body = prepared(spec, 1.0, 48_000.0);
            let mut hz = spec.reference.0 * 0.75;
            while hz < 5_000.0 {
                let apart = apart(&body, spec, hz);
                assert!(
                    apart > -9.0,
                    "{hz} Hz is {apart:.1} dB under what was asked"
                );
                hz *= 2.0_f32.powf(1.0 / 12.0);
            }
        }
        // A soundboard is all crowd and no resonance a listener could name: every key comes
        // out within a few decibels of the next, from the second octave to the top.
        let board = prepared(&SOUNDBOARD, 1.0, 48_000.0);
        let mut total = 0.0;
        for key in 36..=108 {
            let hz = 440.0 * 2.0_f32.powf((key as f32 - 69.0) / 12.0);
            let apart = apart(&board, &SOUNDBOARD, hz);
            assert!(
                apart.abs() < 5.0,
                "key {key} is {apart:.1} dB from the tilt"
            );
            total += apart * apart;
        }
        let rough = (total / 73.0).sqrt();
        assert!(rough < 2.2, "the keys are {rough:.2} dB rms from the tilt");
    }

    #[test]
    fn a_player_is_told_which_notes_a_body_favours() {
        let guitar = prepared(&GUITAR, 1.0, 48_000.0);
        // On the air resonance, and in the trough between it and the top plate's.
        assert!(guitar.favour(102.0) > 1.4, "{}", guitar.favour(102.0));
        assert!(guitar.favour(146.0) < 0.9, "{}", guitar.favour(146.0));
        // A soundboard favours nothing much.
        let board = prepared(&SOUNDBOARD, 1.0, 48_000.0);
        for hz in [131.0, 262.0, 523.0, 1_047.0, 2_093.0] {
            let favour = board.favour(hz);
            assert!((0.75..1.35).contains(&favour), "{hz} Hz: {favour}");
        }
        // A body that has not been laid out has no opinion.
        assert_eq!(Radiator::default().favour(440.0), 1.0);
    }

    #[test]
    fn a_placed_note_keeps_its_colour() {
        // The side input is the mid input scaled: what comes out must be scaled the same.
        let mut body = prepared(&GUITAR, 1.0, 48_000.0);
        let (mid, side) = impulse(&mut body, 9_600, 0.5, 0.0);
        for (m, s) in mid.iter().zip(&side) {
            assert!((s - 0.5 * m).abs() < 1.0e-6);
        }
    }

    #[test]
    fn a_body_turned_down_passes_the_bridge_signal_and_a_silent_one_comes_to_rest() {
        let mut body = prepared(&SOUNDBOARD, 1.0, 48_000.0);
        let mut mid = [0.25; 32];
        let mut side = [0.1; 32];
        body.process(&mut mid, &mut side, 0.0, 1.0);
        assert_eq!(mid, [0.25; 32]);
        assert_eq!(side, [0.1; 32]);
        body.reset();
        let _ = impulse(&mut body, 48_000 * 4, 0.3, 1.0);
        assert!(!body.is_ringing());
    }

    #[test]
    fn a_body_holds_as_many_modes_as_it_has_room_for_and_no_more() {
        for (spec, scale) in [(&SOUNDBOARD, 1.0), (&GUITAR, 1.0), (&VIOLIN, 0.23)] {
            let body = prepared(spec, scale, 96_000.0);
            assert!(body.len() > 60 && body.len() <= MAX_MODES, "{}", body.len());
        }
    }

    #[test]
    fn a_string_loses_most_to_the_bridge_where_the_body_resonates() {
        assert!(GUITAR.conductance(204.0, 1.0) > 0.9);
        assert!(GUITAR.conductance(150.0, 1.0) < 0.2);
        assert!(GUITAR.conductance(1_500.0, 1.0) < 0.1);
        // Scaling the body moves its resonances with it.
        assert!(VIOLIN.conductance(548.0 * 0.375, 0.375) > 0.9);
    }
}
