//! Strings that share a bridge, taken apart into the motions that do not mix.
//!
//! The two or three strings of a piano note are tuned almost alike and end on the same bridge.
//! The bridge moves only when they pull on it together, so only what they do together reaches
//! the soundboard — and only that is drained by it. Struck together they therefore lose their
//! common motion quickly, loudly; what is left is the motion in which they pull against one
//! another, which the bridge hardly feels and which rings on, quietly, for many seconds. That
//! is Weinreich's account of why a piano note falls fast and then slow.
//!
//! Three oscillators coupled this way are a small eigenvalue problem, and it can be solved once,
//! when the note is struck: the result is three *independent* motions, each with its own decay
//! and its own slight shift in frequency, and a complex weight saying how much of the hammer's
//! blow goes into it and how much of it the bridge hears. From then on nothing is coupled and
//! every motion is one more phasor in the bank.

/// A complex number in double precision. Solved once per partial per note, off the sample loop.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
    const ZERO: Self = Self { re: 0.0, im: 0.0 };

    const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    fn add(self, other: Self) -> Self {
        Self::new(self.re + other.re, self.im + other.im)
    }

    fn sub(self, other: Self) -> Self {
        Self::new(self.re - other.re, self.im - other.im)
    }

    fn mul(self, other: Self) -> Self {
        Self::new(
            self.re * other.re - self.im * other.im,
            self.re * other.im + self.im * other.re,
        )
    }

    fn scale(self, by: f64) -> Self {
        Self::new(self.re * by, self.im * by)
    }

    fn div(self, other: Self) -> Self {
        let norm = other.re * other.re + other.im * other.im;
        if norm < 1.0e-300 {
            return Self::ZERO;
        }
        Self::new(
            (self.re * other.re + self.im * other.im) / norm,
            (self.im * other.re - self.re * other.im) / norm,
        )
    }

    fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }

    /// The square root in the right half plane.
    fn sqrt(self) -> Self {
        let radius = self.abs();
        let re = ((radius + self.re) * 0.5).max(0.0).sqrt();
        let im = ((radius - self.re) * 0.5).max(0.0).sqrt();
        Self::new(re, if self.im < 0.0 { -im } else { im })
    }
}

/// One motion of a unison that keeps to itself.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Normal {
    /// Decay the bridge adds to this motion, in nepers per second.
    pub decay: f32,
    /// How far it turns from the partial's own frequency, in radians per second.
    pub shift: f32,
    /// How a unit blow is heard at the bridge through this motion: a complex weight, real and
    /// imaginary halves. The weights of a unison add up to the blow heard without any coupling.
    pub heard: (f32, f32),
}

/// Most strings one hammer strikes.
pub(crate) const STRINGS: usize = 3;

/// Takes a unison apart.
///
/// `detune` is how far each string turns from the partial's nominal frequency, in radians per
/// second; `weights` is how strongly each pulls on the bridge and `shares` how much of the blow
/// each takes. `bridge` is the rate, in nepers per second, at which the bridge drains the motion
/// the strings share. Returns one [`Normal`] per string, in no particular order; the slices'
/// common length, at most [`STRINGS`], decides how many are filled.
pub(crate) fn normals(
    detune: &[f64],
    weights: &[f64],
    shares: &[f64],
    bridge: f64,
) -> ([Normal; STRINGS], usize) {
    let count = detune
        .len()
        .min(weights.len())
        .min(shares.len())
        .min(STRINGS);
    let mut out = [Normal::default(); STRINGS];
    let total: f64 = weights.iter().take(count).sum::<f64>().max(1.0e-12);
    let length = weights
        .iter()
        .take(count)
        .map(|weight| weight * weight)
        .sum::<f64>()
        .sqrt();
    let spread = detune
        .iter()
        .take(count)
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), value| {
            (low.min(*value), high.max(*value))
        });
    let spread = spread.1 - spread.0;
    if count <= 1 || bridge <= 1.0e-9 || length <= 1.0e-12 {
        // Nothing couples them: each string is its own motion.
        for index in 0..count {
            out[index] = Normal {
                decay: 0.0,
                shift: detune[index] as f32,
                heard: ((weights[index] * shares[index] / total) as f32, 0.0),
            };
        }
        return (out, count);
    }
    let unit: [f64; STRINGS] = std::array::from_fn(|index| {
        weights
            .get(index)
            .filter(|_| index < count)
            .copied()
            .unwrap_or(0.0)
            / length
    });
    if spread <= bridge * 1.0e-7 {
        // Perfectly in tune, the strings have one motion the bridge can hear, and it is the
        // one they share. The rest pull against one another and are silent.
        let together: f64 = (0..count).map(|index| unit[index] * shares[index]).sum();
        out[0] = Normal {
            decay: bridge as f32,
            shift: detune[0] as f32,
            heard: ((length * together / total) as f32, 0.0),
        };
        return (out, 1);
    }
    let mut own: [Complex; STRINGS] =
        std::array::from_fn(|index| Complex::new(0.0, detune.get(index).copied().unwrap_or(0.0)));
    let roots = if count == 2 {
        pair(&mut own, unit, bridge)
    } else {
        triple(own, unit, bridge)
    };
    for (slot, root) in out.iter_mut().zip(roots).take(count) {
        // Each string's part in this motion: its pull on the bridge over how far the motion
        // sits from the string's own frequency.
        let part: [Complex; STRINGS] = std::array::from_fn(|index| {
            if index < count {
                Complex::new(unit[index], 0.0).div(own[index].sub(root))
            } else {
                Complex::ZERO
            }
        });
        let mut at_bridge = Complex::ZERO;
        let mut struck = Complex::ZERO;
        let mut norm = Complex::ZERO;
        for index in 0..count {
            at_bridge = at_bridge.add(part[index].scale(weights[index]));
            struck = struck.add(part[index].scale(shares[index]));
            norm = norm.add(part[index].mul(part[index]));
        }
        let heard = at_bridge.mul(struck).div(norm).scale(1.0 / total);
        *slot = Normal {
            decay: (-root.re).max(0.0) as f32,
            shift: root.im as f32,
            heard: (heard.re as f32, heard.im as f32),
        };
    }
    (out, count)
}

/// The two motions of a pair: the roots of a quadratic with complex coefficients. Where the two
/// would merge, the second string is moved a hair, in `own` as well, so that the weights worked
/// out from the roots belong to the same strings.
fn pair(own: &mut [Complex; STRINGS], unit: [f64; STRINGS], bridge: f64) -> [Complex; STRINGS] {
    for _ in 0..4 {
        let sum = own[0].add(own[1]).sub(Complex::new(bridge, 0.0));
        let product = own[0].mul(own[1]).sub(
            own[1]
                .scale(unit[0] * unit[0])
                .add(own[0].scale(unit[1] * unit[1]))
                .scale(bridge),
        );
        let discriminant = sum.mul(sum).sub(product.scale(4.0));
        if discriminant.abs() < 1.0e-6 * bridge * bridge {
            // The two motions are about to become one, where no weights can tell them apart.
            // A string a hair further out of tune is the same string to any ear.
            own[1] = own[1].add(Complex::new(0.0, bridge * 2.0e-3));
            continue;
        }
        let root = discriminant.sqrt();
        return [
            sum.add(root).scale(0.5),
            sum.sub(root).scale(0.5),
            Complex::ZERO,
        ];
    }
    [own[0], own[1], Complex::ZERO]
}

/// The three motions of a triple whose outer strings are tuned equally either side of the
/// middle one and pull equally on the bridge.
///
/// In that case the characteristic polynomial has real coefficients,
/// `λ³ + γ λ² + δ² λ + γ δ² / 3`, measured from the middle string: one root is real, a motion
/// that only decays, and the other two are a conjugate pair that beat. Its discriminant never
/// changes sign for a nonzero detuning, so the three never merge.
fn triple(own: [Complex; STRINGS], unit: [f64; STRINGS], bridge: f64) -> [Complex; STRINGS] {
    let centre = own[1];
    let delta = (own[2].im - own[0].im) * 0.5;
    let symmetric = (own[0].im + own[2].im - 2.0 * centre.im).abs() <= delta.abs() * 1.0e-6
        && (unit[0] - unit[1]).abs() < 1.0e-9
        && (unit[1] - unit[2]).abs() < 1.0e-9;
    if !symmetric {
        return general(own, unit, bridge);
    }
    let square = delta * delta;
    // Depressed by λ = t - γ/3: t³ + p t + q.
    let p = square - bridge * bridge / 3.0;
    let q = 2.0 * bridge * bridge * bridge / 27.0;
    let inner = (q * q / 4.0 + p * p * p / 27.0).max(0.0).sqrt();
    let mut real = (-q / 2.0 + inner).cbrt() + (-q / 2.0 - inner).cbrt() - bridge / 3.0;
    // Cardano loses digits when the detuning is small; two Newton steps give them back.
    for _ in 0..2 {
        let value = ((real + bridge) * real + square) * real + bridge * square / 3.0;
        let slope = (3.0 * real + 2.0 * bridge) * real + square;
        if slope.abs() > 1.0e-300 {
            real -= value / slope;
        }
    }
    // What is left is λ² + b λ + c.
    let b = bridge + real;
    let c = square + real * b;
    let beat = (c - b * b / 4.0).max(0.0).sqrt();
    [
        Complex::new(real, 0.0).add(centre),
        Complex::new(-b / 2.0, beat).add(centre),
        Complex::new(-b / 2.0, -beat).add(centre),
    ]
}

/// Any three strings: the cubic's roots by simultaneous iteration from the uncoupled ones.
fn general(own: [Complex; STRINGS], unit: [f64; STRINGS], bridge: f64) -> [Complex; STRINGS] {
    // The polynomial is `Π (d_k - λ) - γ Σ_k u_k² Π_{j≠k} (d_j - λ)`.
    let value = |at: Complex| {
        let gaps: [Complex; STRINGS] = std::array::from_fn(|index| own[index].sub(at));
        let all = gaps[0].mul(gaps[1]).mul(gaps[2]);
        let rest = gaps[1]
            .mul(gaps[2])
            .scale(unit[0] * unit[0])
            .add(gaps[0].mul(gaps[2]).scale(unit[1] * unit[1]))
            .add(gaps[0].mul(gaps[1]).scale(unit[2] * unit[2]));
        // Monic in λ after the sign of the leading term is taken out.
        all.sub(rest.scale(bridge)).scale(-1.0)
    };
    let mut roots: [Complex; STRINGS] = std::array::from_fn(|index| {
        own[index].sub(Complex::new(
            bridge * unit[index] * unit[index] * (1.0 + 0.01 * index as f64),
            1.0e-3 * bridge * index as f64,
        ))
    });
    for _ in 0..40 {
        for index in 0..STRINGS {
            let mut others = Complex::new(1.0, 0.0);
            for other in 0..STRINGS {
                if other != index {
                    others = others.mul(roots[index].sub(roots[other]));
                }
            }
            roots[index] = roots[index].sub(value(roots[index]).div(others));
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Steps the coupled strings themselves, each a complex amplitude in the frame turning at
    /// the partial's own frequency, and returns what the bridge hears at each of `times`.
    fn stepped(
        detune: &[f64],
        weights: &[f64],
        shares: &[f64],
        bridge: f64,
        times: &[f64],
    ) -> Vec<Complex> {
        let count = detune.len();
        let length: f64 = weights.iter().map(|w| w * w).sum::<f64>().sqrt();
        let total: f64 = weights.iter().sum();
        let mut state: Vec<Complex> = shares.iter().map(|s| Complex::new(*s, 0.0)).collect();
        let derivative = |state: &[Complex]| -> Vec<Complex> {
            let mut together = Complex::ZERO;
            for index in 0..count {
                together = together.add(state[index].scale(weights[index] / length));
            }
            (0..count)
                .map(|index| {
                    Complex::new(0.0, detune[index])
                        .mul(state[index])
                        .sub(together.scale(bridge * weights[index] / length))
                })
                .collect()
        };
        let step = 1.0e-4;
        let mut now = 0.0;
        let mut out = Vec::new();
        for time in times {
            while now < *time {
                // Fourth-order Runge-Kutta.
                let k1 = derivative(&state);
                let mid1: Vec<_> = (0..count)
                    .map(|i| state[i].add(k1[i].scale(step / 2.0)))
                    .collect();
                let k2 = derivative(&mid1);
                let mid2: Vec<_> = (0..count)
                    .map(|i| state[i].add(k2[i].scale(step / 2.0)))
                    .collect();
                let k3 = derivative(&mid2);
                let end: Vec<_> = (0..count)
                    .map(|i| state[i].add(k3[i].scale(step)))
                    .collect();
                let k4 = derivative(&end);
                for i in 0..count {
                    let sum = k1[i].add(k2[i].scale(2.0)).add(k3[i].scale(2.0)).add(k4[i]);
                    state[i] = state[i].add(sum.scale(step / 6.0));
                }
                now += step;
            }
            let mut heard = Complex::ZERO;
            for index in 0..count {
                heard = heard.add(state[index].scale(weights[index] / total));
            }
            out.push(heard);
        }
        out
    }

    /// What the same bridge hears from the motions `normals` found.
    fn summed(found: &[Normal], time: f64) -> Complex {
        let mut heard = Complex::ZERO;
        for normal in found {
            let size = (-f64::from(normal.decay) * time).exp();
            let turn = f64::from(normal.shift) * time;
            let phasor = Complex::new(size * turn.cos(), size * turn.sin());
            heard = heard.add(
                Complex::new(f64::from(normal.heard.0), f64::from(normal.heard.1)).mul(phasor),
            );
        }
        heard
    }

    fn check(detune: &[f64], weights: &[f64], shares: &[f64], bridge: f64) {
        let (found, count) = normals(detune, weights, shares, bridge);
        let times = [0.0, 0.05, 0.2, 0.7, 2.0];
        let truth = stepped(detune, weights, shares, bridge, &times);
        for (time, expected) in times.iter().zip(truth) {
            let got = summed(&found[..count], *time);
            let error = got.sub(expected).abs();
            assert!(
                error < 2.0e-3,
                "detune {detune:?} bridge {bridge} at {time} s: {got:?} against {expected:?}"
            );
        }
    }

    #[test]
    fn the_independent_motions_add_up_to_the_coupled_strings() {
        // From far apart to nearly in tune, and across where a pair's motions nearly merge.
        for bridge in [0.4, 1.9, 6.0] {
            for delta in [0.01, 0.3, 0.94, 0.95, 0.96, 2.0, 15.0] {
                check(&[-delta, delta], &[1.0, 1.0], &[1.5, 0.5], bridge);
                check(&[-delta, 0.0, delta], &[1.0; 3], &[1.5, 1.0, 0.5], bridge);
                check(&[0.0, delta], &[1.0, 0.7], &[1.0, 0.743], bridge);
                check(
                    &[-delta, 0.1 * delta, delta],
                    &[1.0, 0.9, 1.1],
                    &[1.2, 1.0, 0.8],
                    bridge,
                );
            }
        }
    }

    #[test]
    fn a_struck_unison_falls_fast_and_then_slow() {
        // Three strings a third of a cent apart at 262 Hz, the bridge taking 17 dB a second.
        let delta = std::f64::consts::TAU * 262.0 * (2.0_f64.powf(0.26 / 1200.0) - 1.0);
        let bridge = 17.0 * 0.115_129_25;
        let (found, count) = normals(&[-delta, 0.0, delta], &[1.0; 3], &[1.5, 1.0, 0.5], bridge);
        assert_eq!(count, 3);
        let level = |time: f64| 20.0 * summed(&found, time).abs().log10();
        let prompt = (level(0.05) - level(0.55)) / 0.5;
        let after = (level(4.0) - level(8.0)) / 4.0;
        assert!((12.0..18.0).contains(&prompt), "prompt sound {prompt} dB/s");
        assert!(after < prompt / 4.0, "aftersound {after} dB/s");
        // At the start every string is heard: the weights add up to the uncoupled blow.
        let whole: f32 = found.iter().map(|normal| normal.heard.0).sum();
        assert!((whole - 1.0).abs() < 1.0e-4, "{whole}");
    }

    #[test]
    fn strings_in_perfect_tune_have_only_the_motion_they_share() {
        let (found, count) = normals(&[0.0, 0.0, 0.0], &[1.0; 3], &[1.5, 1.0, 0.5], 2.0);
        assert_eq!(count, 1);
        assert!((found[0].decay - 2.0).abs() < 1.0e-6);
        assert!((found[0].heard.0 - 1.0).abs() < 1.0e-6);
        // And with no bridge to couple them, each string is simply itself.
        let (found, count) = normals(&[-1.0, 1.0], &[1.0, 1.0], &[1.5, 0.5], 0.0);
        assert_eq!(count, 2);
        assert_eq!((found[0].shift, found[1].shift), (-1.0, 1.0));
        assert_eq!(found[0].decay, 0.0);
    }
}
