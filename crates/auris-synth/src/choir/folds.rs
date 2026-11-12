//! Two-mass vocal folds that oscillate on their own against the tract they feed.
//!
//! Nothing here draws a waveform. Lung pressure pushes air through the slit between two folds,
//! each modelled as a lower and an upper mass on springs (Steinecke & Herzel 1995). The
//! Bernoulli pressure of that air and the folds' collision make them swing, and the flow they
//! let through is solved each step against the pressure wave coming back down the vocal tract,
//! so the tract pushes back on the source as it does in a throat.
//!
//! What a singer controls is tension, not frequency. Tension `Q` scales the masses down and the
//! springs up, which moves the natural frequency in proportion, but what actually sounds also
//! depends on breath and on the vowel. So the period is measured at every cycle and the
//! tension corrected towards the note — the way a singer finds a pitch by ear.

/// Air density, kilograms per cubic metre.
const RHO: f64 = 1.14;
/// Speed of sound in the warm, humid tract, metres per second.
const C: f64 = 350.0;
/// Fold integration steps for each output sample.
pub(super) const OVERSAMPLE: usize = 2;
/// Fold steps between re-derivations of the tension-dependent coefficients.
const CONTROL: u32 = 32;
/// How far each measured period moves the tension towards the note, as a power.
const PITCH_FEEDBACK: f64 = 0.25;
/// How tension is shared between lighter and stiffer folds: mass goes as `Q^-QA`.
const QA: f64 = 1.5;
/// Natural frequency of the unscaled (adult male) folds at unit tension, in hertz.
const BASE_HZ: f64 = 135.0;
/// Resting half-gap the folds return to between notes, square metres per unit scale.
pub(super) const ADDUCTED: f64 = 0.05e-4;

/// Acoustic impedance of a tube of `area` square metres.
pub(super) fn impedance(area: f64) -> f64 {
    RHO * C / area.max(1e-9)
}

/// A pair of folds of one size, scaled as a whole: masses by `s³`, stiffness by `s`, lengths by
/// `s`. Their natural frequency therefore goes as `1/s`, which is how a smaller larynx sings
/// higher with the same equations.
#[derive(Clone, Copy)]
struct Geometry {
    m1: f64,
    m2: f64,
    k1: f64,
    k2: f64,
    kc: f64,
    r1: f64,
    r2: f64,
    /// Fold length times lower and upper thickness: the area a pressure pushes on.
    ld1: f64,
    ld2: f64,
    /// Twice the fold length: how much area one metre of excursion opens.
    two_l: f64,
    /// Areas scale with the square of size.
    area: f64,
    base_hz: f64,
    scale: f64,
}

impl Geometry {
    fn scaled(s: f64) -> Self {
        let length = 1.4e-2 * s;
        Self {
            m1: 0.125e-3 * s * s * s,
            m2: 0.025e-3 * s * s * s,
            k1: 80.0 * s,
            k2: 8.0 * s,
            kc: 25.0 * s,
            r1: 0.02 * s * s,
            r2: 0.02 * s * s,
            ld1: length * 0.25e-2 * s,
            ld2: length * 0.05e-2 * s,
            two_l: 2.0 * length,
            area: s * s,
            base_hz: BASE_HZ / s,
            scale: s,
        }
    }
}

/// One step's result: the volume flow through the glottis and the jet's pressure drop.
#[derive(Clone, Copy, Default)]
pub(super) struct Glottis {
    /// Volume velocity into the tract, cubic metres per second.
    pub(super) flow: f64,
    /// Pressure lost across the glottis, pascals; the turbulence that rides on it is breath.
    pub(super) drop: f64,
    /// Open area, square metres.
    pub(super) area: f64,
}

pub(super) struct Folds {
    geometry: Geometry,
    dt: f64,
    x1: f64,
    x2: f64,
    v1: f64,
    v2: f64,
    /// Tension relative to what the note asks for, learnt from the measured periods.
    correction: f64,
    mean: f64,
    previous: f64,
    since_crossing: f64,
    target: f64,
    countdown: u32,
    inv_m1: f64,
    inv_m2: f64,
    k1: f64,
    k2: f64,
    kc: f64,
    r1: f64,
    r2: f64,
}

impl Folds {
    pub(super) fn new(sample_rate: f32) -> Self {
        let mut folds = Self {
            geometry: Geometry::scaled(1.0),
            dt: 1.0 / (f64::from(sample_rate) * OVERSAMPLE as f64),
            x1: 0.0,
            x2: 0.0,
            v1: 0.0,
            v2: 0.0,
            correction: 1.0,
            mean: 0.0,
            previous: 0.0,
            since_crossing: 0.0,
            target: 110.0,
            countdown: 0,
            inv_m1: 0.0,
            inv_m2: 0.0,
            k1: 0.0,
            k2: 0.0,
            kc: 0.0,
            r1: 0.0,
            r2: 0.0,
        };
        folds.retune(110.0);
        folds
    }

    /// Brings the folds to rest at the size that sings `hertz` most comfortably.
    pub(super) fn reset(&mut self, hertz: f32) {
        self.choose_size(hertz);
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.v1 = 0.0;
        self.v2 = 0.0;
        self.correction = 1.0;
        self.mean = 0.0;
        self.previous = 0.0;
        self.since_crossing = 0.0;
        self.countdown = 0;
        self.retune(hertz);
    }

    /// Takes the larynx a singer of this note would sing it with.
    ///
    /// Folds sing well over roughly a factor of two of tension (`Q` from about 1.5 to 3), and
    /// a choir is basses to sopranos. A note keeps the size it started with, so bends and
    /// vibrato move the tension, as they do in a throat.
    pub(super) fn choose_size(&mut self, hertz: f32) {
        let s = (SIZE_HZ / f64::from(hertz).max(1.0)).clamp(MIN_SIZE, MAX_SIZE);
        self.geometry = Geometry::scaled(s);
    }

    fn retune(&mut self, hertz: f32) {
        self.target = f64::from(hertz);
        // A note bent far from where it started is sung in another register.
        let comfortable = SIZE_HZ / self.geometry.scale;
        let away = self.target / comfortable;
        if !(1.0 / REGISTER_SPAN..=REGISTER_SPAN).contains(&away) {
            // The learnt correction is relative to the note, so it carries across unchanged.
            self.choose_size(hertz);
        }
        let q = self.target / self.geometry.base_hz * self.correction;
        let qm = q.powf(QA);
        let qk = q * q / qm;
        let qr = q / qm;
        let g = &self.geometry;
        self.inv_m1 = self.dt * qm / g.m1;
        self.inv_m2 = self.dt * qm / g.m2;
        self.k1 = g.k1 * qk;
        self.k2 = g.k2 * qk;
        self.kc = g.kc * qk;
        self.r1 = g.r1 * qr;
        self.r2 = g.r2 * qr;
    }

    /// One fold step at twice the sample rate.
    ///
    /// `lung` is subglottal pressure and `rest` the half-gap the folds are held at, both before
    /// size scaling; `chink` is a gap that never closes; `incoming` is the pressure wave
    /// arriving back at the glottis from the tract and `z0` the tract's impedance there.
    pub(super) fn step(
        &mut self,
        hertz: f32,
        lung: f64,
        rest: f64,
        chink: f64,
        incoming: f64,
        z0: f64,
    ) -> Glottis {
        if self.countdown == 0 {
            self.retune(hertz);
            self.countdown = CONTROL;
        }
        self.countdown -= 1;
        let g = self.geometry;
        let a0 = rest * g.area;
        let a1 = a0 + g.two_l * self.x1;
        let a2 = a0 + g.two_l * self.x2;
        let narrowest = a1.min(a2);
        let open = narrowest.max(0.0) + chink * g.area;

        // Flow through the slit against the tract: (ρ/2A²)·U|U| + Z0·U = Ps − 2p⁻.
        let drive = lung - 2.0 * incoming;
        let mut flow = 0.0;
        if open > 1e-10 {
            let q = RHO / (2.0 * open * open);
            let magnitude = drive.abs();
            flow = 2.0 * magnitude / (z0 + (z0 * z0 + 4.0 * q * magnitude).sqrt());
            flow = flow.copysign(drive);
        }
        let above = 2.0 * incoming + z0 * flow;
        let drop = lung - above;
        let p1 = if a1 > 0.0 {
            if narrowest > 0.0 {
                let ratio = narrowest / a1;
                lung - drop * ratio * ratio
            } else {
                lung
            }
        } else {
            0.0
        };
        let p2 = if a2 > 0.0 { above } else { 0.0 };
        let mut f1 =
            g.ld1 * p1 - self.k1 * self.x1 - self.r1 * self.v1 - self.kc * (self.x1 - self.x2);
        let mut f2 =
            g.ld2 * p2 - self.k2 * self.x2 - self.r2 * self.v2 - self.kc * (self.x2 - self.x1);
        // Colliding folds push each other apart, three times as stiffly as they bend.
        if a1 < 0.0 {
            f1 -= 3.0 * self.k1 * a1 / g.two_l;
        }
        if a2 < 0.0 {
            f2 -= 3.0 * self.k2 * a2 / g.two_l;
        }
        self.v1 += self.inv_m1 * f1;
        self.x1 += self.dt * self.v1;
        self.v2 += self.inv_m2 * f2;
        self.x2 += self.dt * self.v2;
        self.listen(lung);
        Glottis {
            flow,
            drop,
            area: open,
        }
    }

    /// Measures each period from the lower mass's upward crossings and corrects the tension.
    fn listen(&mut self, lung: f64) {
        self.mean += (self.x1 - self.mean) * 0.0004;
        let centred = self.x1 - self.mean;
        self.since_crossing += 1.0;
        if centred > 0.0 && self.previous <= 0.0 {
            let fraction = centred / (centred - self.previous);
            let period = self.since_crossing - fraction;
            self.since_crossing = fraction;
            let measured = 1.0 / (period * self.dt);
            let target = self.target;
            if measured > target * 0.6 && measured < target * 1.6 && lung > 100.0 {
                let corrected = self.correction * (target / measured).powf(PITCH_FEEDBACK);
                self.correction = corrected.clamp(0.5, 2.5);
            }
        }
        self.previous = centred;
    }
}

/// Frequency over which a size is chosen: the fold scale is this over the note, so that
/// every note starts at a tension near the middle of what folds sing well.
const SIZE_HZ: f64 = BASE_HZ * 2.2;
const MIN_SIZE: f64 = 0.3;
/// How far, as a ratio, a note may be bent from its register's centre before it changes.
const REGISTER_SPAN: f64 = 1.5;
const MAX_SIZE: f64 = 1.3;
