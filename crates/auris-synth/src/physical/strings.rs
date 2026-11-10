//! What every string here has in common: where its partials lie and how fast they die.
//!
//! A real string is not quite a harmonic series and does not lose energy evenly. Its stiffness
//! pushes each partial a little sharp, more the higher it is; the air, the material's own
//! friction and the bending of a stiff wire each take energy at a different rate across the
//! spectrum; and what the terminations take depends on how many times a second the wave reaches
//! them. A nylon string and a steel one differ in exactly these numbers, which is why they are
//! written here as numbers rather than as a filter's corner.

use std::f32::consts::PI;

/// Frequency of partial `n` of a stiff string, as a multiple of its fundamental.
///
/// A stiff string's partials run sharp of the harmonics by `sqrt(1 + B n²)`. Divided by the
/// same factor at `n = 1`, the fundamental stays on the note and the stiffness is heard only
/// in the stretch of the partials above it.
pub(crate) fn partial_ratio(n: f32, inharmonicity: f32) -> f32 {
    n * ((1.0 + inharmonicity * n * n) / (1.0 + inharmonicity)).sqrt()
}

/// How a string loses energy, after Valette and Woodhouse: three mechanisms, each with its own
/// dependence on frequency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Losses {
    /// What every partial loses alike, in nepers per second: air drag at low frequencies and
    /// whatever the bridge, the nut or a fretting finger takes.
    pub shared: f32,
    /// Internal friction, as a loss factor: the same fraction of a cycle's energy at every
    /// frequency, so a rate that rises in proportion to it.
    pub friction: f32,
    /// Loss factor of the string's bending stiffness. Only the stiff part of the restoring
    /// force is lossy in this way, so it bites in proportion to `B n²` and is what takes the
    /// top off a nylon string.
    pub bending: f32,
}

impl Losses {
    /// Decay rate, in nepers per second, of a partial at `hz` whose stiffness has stretched it
    /// by `stretch = B n²`.
    pub(crate) fn rate(&self, hz: f32, stretch: f32) -> f32 {
        self.shared + PI * hz * (self.friction + self.bending * stretch)
    }

    /// Losses that give a fundamental at `hz` a decay rate of `target` nepers per second, with
    /// the material's two loss factors as given. The shared loss is whatever is left over, and
    /// never less than a tenth of the target: no string rings in a vacuum.
    pub(crate) fn tuned(
        target: f32,
        hz: f32,
        inharmonicity: f32,
        friction: f32,
        bending: f32,
    ) -> Self {
        let material = PI * hz * (friction + bending * inharmonicity);
        Self {
            shared: (target - material).max(target * 0.1),
            friction,
            bending,
        }
    }
}

/// How dead a string is, from a fresh steel string at zero to a muted one at one: its loss
/// factors for internal friction and for bending.
///
/// The ends of the scale are the measured ranges for plain steel and for nylon — about
/// `6e-5` and `6e-3` for steel, `3e-4` and `2.5e-2` for nylon, which lands at 0.4 — carried on
/// geometrically to a string that has lost its top entirely.
pub(crate) fn material(damping: f32) -> (f32, f32) {
    let damping = damping.clamp(0.0, 1.0);
    (
        6.0e-5 * 50.0_f32.powf(damping),
        6.0e-3 * 25.0_f32.powf(damping),
    )
}

/// The open string a fretted note is played on, and how much shorter than open it sounds.
///
/// A player takes a note on the highest string that reaches it, so `open` is searched from the
/// top. Returns the open string's index and the ratio of the open length to the sounding one,
/// which is the ratio of the two frequencies. Below the lowest string, the lowest string
/// detuned.
pub(crate) fn fretted(pitch: f32, open: &[u8]) -> (usize, f32) {
    let index = open
        .iter()
        .rposition(|string| pitch >= f32::from(*string))
        .unwrap_or(0);
    let string = open.get(index).copied().map_or(pitch, f32::from);
    (index, ((pitch - string).max(0.0) / 12.0).exp2())
}

/// Standard tuning of a six-string guitar, low to high.
pub(crate) const GUITAR_STRINGS: [u8; 6] = [40, 45, 50, 55, 59, 64];
/// Standard tuning of a four-string bass, low to high.
pub(crate) const BASS_STRINGS: [u8; 4] = [28, 33, 38, 43];

/// How much of partial `n` a pluck at `position` along the string leaves in it, relative to a
/// pluck that excited every partial fully: the string held aside at one point is a triangle,
/// and a triangle's `n`th harmonic is `sin(n π position) / n`.
///
/// This is the *force at the bridge*, which is the slope there and so `n` times the
/// displacement: the displacement itself falls as `1 / n²`.
pub(crate) fn pluck_share(n: f32, position: f32) -> f32 {
    (PI * n * position).sin() / n
}

/// What a plucking finger or pick does to a partial at `hz`: it lets go over a finite time, so
/// partials faster than that are left out. A two-pole roll-off above `corner` hertz.
pub(crate) fn release_rolloff(hz: f32, corner: f32) -> f32 {
    let ratio = hz / corner.max(1.0);
    1.0 / (1.0 + ratio * ratio)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stiffness_stretches_the_partials_and_leaves_the_fundamental_alone() {
        assert_eq!(partial_ratio(1.0, 4.0e-4), 1.0);
        assert_eq!(partial_ratio(7.0, 0.0), 7.0);
        let cents = 1200.0 * (partial_ratio(8.0, 4.0e-4) / 8.0).log2();
        // The eighth partial of middle C on a grand is about 21 cents sharp.
        assert!((cents - 21.4).abs() < 0.5, "{cents}");
    }

    #[test]
    fn nylon_loses_its_top_long_before_steel() {
        let seconds = |damping: f32, hz: f32, stretch: f32| {
            let (friction, bending) = material(damping);
            let losses = Losses::tuned(6.9 / 5.0, 196.0, 4.0e-5, friction, bending);
            6.907_755 / losses.rate(hz, stretch)
        };
        // G3's fundamental is given five seconds either way.
        assert!((seconds(0.0, 196.0, 4.0e-5) - 5.0).abs() < 0.05);
        assert!((seconds(0.4, 196.0, 4.0e-5) - 5.0).abs() < 0.05);
        // Its 25th partial, near 5 kHz: over a second on steel, well under one on nylon.
        let steel = seconds(0.0, 4_900.0, 0.025);
        let nylon = seconds(0.4, 4_900.0, 0.025);
        assert!((1.0..2.5).contains(&steel), "steel {steel} s");
        assert!((0.2..0.7).contains(&nylon), "nylon {nylon} s");
    }

    #[test]
    fn a_note_is_fretted_on_the_highest_string_that_reaches_it() {
        assert_eq!(fretted(64.0, &GUITAR_STRINGS), (5, 1.0));
        assert_eq!(fretted(63.0, &GUITAR_STRINGS).0, 4);
        let (string, ratio) = fretted(52.0, &GUITAR_STRINGS);
        assert_eq!(string, 2);
        assert!((ratio - 2.0_f32.powf(2.0 / 12.0)).abs() < 1.0e-6);
        // Below the lowest string there is nothing shorter to play it on.
        assert_eq!(fretted(30.0, &GUITAR_STRINGS), (0, 1.0));
        assert_eq!(fretted(28.0, &BASS_STRINGS), (0, 1.0));
    }

    #[test]
    fn a_pluck_leaves_out_the_partials_with_a_node_under_the_finger() {
        assert!(pluck_share(4.0, 0.25).abs() < 1.0e-6);
        assert!(pluck_share(2.0, 0.25) > 0.49);
        assert!(release_rolloff(100.0, 2_000.0) > 0.99);
        assert!((release_rolloff(2_000.0, 2_000.0) - 0.5).abs() < 1.0e-6);
    }
}
