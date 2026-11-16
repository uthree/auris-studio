//! The one colour ramp every picture shown to a model uses for "how much".
//!
//! Dark navy for nothing, through indigo, purple and magenta to orange and a pale yellow at full
//! scale — the look of a conventional audio spectrogram, so a model (or a person reading the tool
//! log) meets one convention in the mel image and the modulation image alike. The ramp is fixed;
//! every caller maps its own fixed scale onto 0..1, never a per-image range.
use image::Rgb;

/// The ramp's stops, as (position, colour), in the order a caption names them.
const STOPS: [(f32, [f32; 3]); 7] = [
    (0.0, [10.0, 12.0, 38.0]),
    (0.2, [38.0, 24.0, 92.0]),
    (0.4, [108.0, 34.0, 140.0]),
    (0.6, [192.0, 48.0, 122.0]),
    (0.78, [240.0, 108.0, 62.0]),
    (0.9, [252.0, 180.0, 64.0]),
    (1.0, [255.0, 246.0, 204.0]),
];

/// How the ramp reads in a caption, low to high.
pub const DESCRIPTION: &str =
    "dark navy 0, indigo 0.2, purple 0.4, magenta 0.6, orange 0.8, pale yellow 1.0";

/// The colour of `value` on the ramp; values outside 0..1 are clamped and NaN reads as 0.
pub fn heat(value: f32) -> Rgb<u8> {
    let value = if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    };
    let upper = STOPS
        .iter()
        .position(|(at, _)| *at >= value)
        .unwrap_or(STOPS.len() - 1)
        .max(1);
    let (a, from) = STOPS[upper - 1];
    let (b, to) = STOPS[upper];
    let t = (value - a) / (b - a);
    Rgb(std::array::from_fn(|i| {
        (from[i] + (to[i] - from[i]) * t).round() as u8
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ramp_runs_from_navy_to_pale_yellow_and_brightens_throughout() {
        assert_eq!(heat(0.0).0, [10, 12, 38]);
        assert_eq!(heat(1.0).0, [255, 246, 204]);
        assert_eq!(heat(-3.0), heat(0.0));
        assert_eq!(heat(f32::NAN), heat(0.0));
        assert_eq!(heat(7.0), heat(1.0));
        let luminance = |c: Rgb<u8>| {
            0.2126 * f32::from(c[0]) + 0.7152 * f32::from(c[1]) + 0.0722 * f32::from(c[2])
        };
        let mut previous = luminance(heat(0.0));
        for step in 1..=100 {
            let current = luminance(heat(step as f32 / 100.0));
            assert!(current >= previous - 0.5, "{step}: {current} < {previous}");
            previous = current;
        }
    }
}
