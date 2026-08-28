//! Deterministic randomness, addressed by name rather than by call order.
//!
//! A composer that draws from one sequential generator is impossible to edit: adding a roll
//! anywhere shifts every later draw, so changing the drum density silently rewrites the melody.
//! Here every decision names its own stream — `["part", "lead", "surface", "chorus", 2]` — and a
//! stream's numbers depend only on the seed and that name. Adding a part, or a pass, or a roll
//! inside one pass, leaves every other stream untouched.
//!
//! It lives in this crate rather than in the composer because the document draws from it too: a
//! clip's note transforms wander by numbers from these streams, and a seed stored in a file has
//! to mean the same numbers wherever it is read.

/// One component of a stream name.
///
/// Streams are named with a slice of these rather than a formatted string so that a name cannot
/// be built by accidental concatenation: `["a", "bc"]` and `["ab", "c"]` are different streams.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Key<'a> {
    /// A fixed word naming a pass, a part or a phase.
    Word(&'a str),
    /// A counter: a bar index, a section instance, a variation number.
    Index(u64),
}

impl<'a> From<&'a str> for Key<'a> {
    fn from(word: &'a str) -> Self {
        Key::Word(word)
    }
}

impl From<u64> for Key<'_> {
    fn from(index: u64) -> Self {
        Key::Index(index)
    }
}

impl From<usize> for Key<'_> {
    fn from(index: usize) -> Self {
        Key::Index(index as u64)
    }
}

impl From<u32> for Key<'_> {
    fn from(index: u32) -> Self {
        Key::Index(u64::from(index))
    }
}

/// A named stream of numbers, reproducible from a seed.
///
/// The generator is SplitMix64: three lines, no state beyond a `u64`, and a documented
/// distribution. Cryptographic quality is irrelevant here and a dependency would not be.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// The stream named `path` within `seed`.
    ///
    /// FNV-1a written out here rather than `DefaultHasher`, whose values std explicitly does not
    /// promise to keep stable across releases — a seed has to mean the same piece next year.
    pub fn stream(seed: u64, path: &[Key<'_>]) -> Self {
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        let mut hash = OFFSET;
        let mut eat = |byte: u8| {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(PRIME);
        };
        for byte in seed.to_le_bytes() {
            eat(byte);
        }
        // The length goes in first so that a shorter path can never hash to the same value as a
        // longer one that happens to start the same way.
        for byte in (path.len() as u64).to_le_bytes() {
            eat(byte);
        }
        for key in path {
            match key {
                Key::Word(word) => {
                    eat(0);
                    for byte in (word.len() as u64).to_le_bytes() {
                        eat(byte);
                    }
                    for byte in word.as_bytes() {
                        eat(*byte);
                    }
                }
                Key::Index(index) => {
                    eat(1);
                    for byte in index.to_le_bytes() {
                        eat(byte);
                    }
                }
            }
        }
        Self {
            // A zero state would make SplitMix64 return a fixed sequence.
            state: hash | 1,
        }
    }

    /// The next raw value.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0.0..1.0`.
    pub fn unit(&mut self) -> f32 {
        // 24 bits is the whole mantissa of an `f32`; taking more would add nothing but a bias.
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }

    /// A value in `0..bound`, or `0` when `bound` is zero.
    pub fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        // Lemire's multiply-shift: unbiased enough for musical decisions and branch-free.
        ((self.next_u64() as u128 * bound as u128) >> 64) as usize
    }

    /// `true` with probability `chance`.
    ///
    /// Drawn whether or not the caller can use the result, which is what keeps one pinned field
    /// from shifting every later decision — see the roll-anyway rule.
    pub fn chance(&mut self, chance: f32) -> bool {
        self.unit() < chance
    }

    /// One element of `items`, or `None` when it is empty.
    pub fn pick<'t, T>(&mut self, items: &'t [T]) -> Option<&'t T> {
        items.get(self.below(items.len()))
    }

    /// An index into `weights`, chosen in proportion to them.
    ///
    /// Non-positive weights are treated as zero. Returns `0` when every weight is zero, so a
    /// caller never has to handle an empty draw.
    pub fn weighted(&mut self, weights: &[f32]) -> usize {
        let total: f32 = weights.iter().filter(|w| **w > 0.0).sum();
        if total <= 0.0 {
            return 0;
        }
        let mut target = self.unit() * total;
        for (index, weight) in weights.iter().enumerate() {
            if *weight <= 0.0 {
                continue;
            }
            target -= weight;
            if target <= 0.0 {
                return index;
            }
        }
        // Reached only through floating-point drift at the very end of the range.
        weights.len() - 1
    }

    /// A roughly normal value with mean 0 and standard deviation `sigma`, clamped at 3σ.
    ///
    /// The sum of three uniforms rather than Box-Muller: it needs no transcendental functions,
    /// it is bounded by construction, and the difference is inaudible in note timing.
    pub fn jitter(&mut self, sigma: f32) -> f32 {
        let sum = self.unit() + self.unit() + self.unit() - 1.5;
        (sum * 2.0 * sigma).clamp(-3.0 * sigma, 3.0 * sigma)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_is_reproducible_from_its_name() {
        let path = [Key::Word("part"), Key::Word("lead"), Key::Index(2)];
        let first: Vec<u64> = (0..8).map(|_| Rng::stream(7, &path).next_u64()).collect();
        let second: Vec<u64> = (0..8).map(|_| Rng::stream(7, &path).next_u64()).collect();
        assert_eq!(first, second);
    }

    #[test]
    fn different_names_and_seeds_give_different_streams() {
        let a = Rng::stream(1, &[Key::Word("melody")]).next_u64();
        let b = Rng::stream(1, &[Key::Word("bass")]).next_u64();
        let c = Rng::stream(2, &[Key::Word("melody")]).next_u64();
        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn a_path_cannot_be_confused_with_a_longer_one() {
        // Without the length in the hash these two could collide, and a part called "lead" would
        // share its numbers with a part called "lea" followed by "d".
        let short = Rng::stream(1, &[Key::Word("lead")]).next_u64();
        let long = Rng::stream(1, &[Key::Word("lead"), Key::Word("")]).next_u64();
        assert_ne!(short, long);
    }

    #[test]
    fn a_stream_is_pinned_to_an_exact_value() {
        // The whole point of a seed is that it means the same piece next year, so the hash has
        // to be one this crate owns rather than one the standard library may change.
        let mut rng = Rng::stream(42, &[Key::Word("part"), Key::Word("lead"), Key::Index(3)]);
        assert_eq!(rng.next_u64(), 11_621_116_030_062_456_483);
    }

    #[test]
    fn unit_stays_inside_its_range() {
        let mut rng = Rng::stream(3, &[Key::Word("unit")]);
        for _ in 0..5_000 {
            let value = rng.unit();
            assert!((0.0..1.0).contains(&value), "{value} escaped 0..1");
        }
    }

    #[test]
    fn below_stays_inside_its_bound_and_covers_it() {
        let mut rng = Rng::stream(4, &[Key::Word("below")]);
        let mut seen = [false; 5];
        for _ in 0..2_000 {
            let value = rng.below(5);
            assert!(value < 5);
            seen[value] = true;
        }
        assert!(seen.iter().all(|hit| *hit), "some values never came up");
        assert_eq!(rng.below(0), 0, "an empty range must not panic");
    }

    #[test]
    fn weighted_follows_its_weights() {
        let mut rng = Rng::stream(5, &[Key::Word("weighted")]);
        let mut counts = [0usize; 3];
        for _ in 0..10_000 {
            counts[rng.weighted(&[0.6, 0.3, 0.1])] += 1;
        }
        // Wide bands: this asserts the weights are honoured, not that the generator is perfect.
        assert!((5_400..6_600).contains(&counts[0]), "{counts:?}");
        assert!((2_600..3_400).contains(&counts[1]), "{counts:?}");
        assert!((700..1_300).contains(&counts[2]), "{counts:?}");
    }

    #[test]
    fn weighted_ignores_impossible_options() {
        let mut rng = Rng::stream(6, &[Key::Word("zero")]);
        for _ in 0..500 {
            assert_eq!(rng.weighted(&[0.0, 1.0, 0.0]), 1);
        }
        assert_eq!(
            rng.weighted(&[0.0, 0.0]),
            0,
            "all-zero weights must not panic"
        );
    }

    #[test]
    fn jitter_is_centred_and_bounded() {
        let mut rng = Rng::stream(8, &[Key::Word("jitter")]);
        let mut sum = 0.0;
        for _ in 0..10_000 {
            let value = rng.jitter(10.0);
            assert!(value.abs() <= 30.0, "{value} exceeded three sigma");
            sum += value;
        }
        assert!(
            (sum / 10_000.0).abs() < 0.5,
            "mean drifted to {}",
            sum / 10_000.0
        );
    }
}
