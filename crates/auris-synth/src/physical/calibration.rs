//! Training-derived register curves; reference audio stays in development tooling.

use super::{Model, Settings};

pub(super) fn register(model: Model, pitch: u8, mut settings: Settings) -> Settings {
    if model == Model::Piano && pitch < 48 {
        // Wound bass strings cannot inherit the tenor fit's inharmonicity unchanged.
        // Iowa mf C1/C2 partials give B ~= 0.00026/0.00014, against the factory
        // tenor anchor 0.00075. Join those ratios smoothly back to the C3 calibration.
        let register = ((f32::from(pitch) - 36.0) / 12.0).clamp(-1.0, 1.0);
        let distance = register.abs();
        let blend = distance * distance * (3.0 - 2.0 * distance);
        let rise = if register < 0.0 { 0.15 } else { 0.8 };
        settings.stiffness *= 0.2 + rise * blend;
        return settings;
    }
    if model == Model::ElectricGuitar {
        // Highest playable string, in standard tuning. Pick/pickup distances are specified
        // relative to its open length; fretting shortens that length without moving either.
        let open = [40, 45, 50, 55, 59, 64]
            .into_iter()
            .rev()
            .find(|open| pitch >= *open)
            .unwrap_or(40);
        let ratio = 2.0_f32.powf(f32::from(pitch.saturating_sub(open)) / 12.0);
        settings.position = (settings.position * ratio).clamp(0.02, 0.9);
        settings.pickup_position = (settings.pickup_position * ratio).clamp(0.02, 0.9);
        settings.decay = (f64::from(settings.decay)
            * f64::from(settings.decay_ratio).powf((f64::from(pitch) - 64.0) / 12.0))
        .clamp(0.1, 40.0) as f32;
        return settings;
    }
    let (center, slopes) = match model {
        Model::Guitar => (55.0, [-0.156, -0.624, 0.0624]),
        Model::Violin => (69.0, [0.0, 0.224, 0.0]),
        _ => return settings,
    };
    let x = ((f64::from(pitch) - center) / 24.0).clamp(-1.5, 1.5);
    // Compute the prepared contact/loss controls in f64, matching the offline fitter.
    // These operations occur at attacks and parameter changes, never per sample.
    settings.hardness = (f64::from(settings.hardness) + slopes[0] * x).clamp(0.0, 1.0) as f32;
    settings.decay = (f64::from(settings.decay) * (slopes[1] * x).exp()).clamp(0.1, 12.0) as f32;
    settings.damping = (f64::from(settings.damping) + slopes[2] * x).clamp(0.0, 1.0) as f32;
    settings
}

/// How far a piano key is tuned from equal temperament, in cents.
///
/// A stiff string's overtones run sharp of its harmonics. A piano with every fundamental on
/// equal temperament therefore beats in its octaves, and its bass sounds sharp, because the ear
/// hears a low note's pitch in its overtones. Tuned the way a technician tunes one, outward from
/// A4, each octave is widened to split the difference between two pure octaves: from A2 up, the
/// 2:1 (the lower note's second partial on the upper note's fundamental) and the 4:2. Below A2
/// the fundamentals thin out and the octaves are judged on higher partials in proportion, 4:2
/// and 8:4 at A1 and 8:4 and 16:8 at A0. Without stiffness every octave is pure and the piano is
/// in equal temperament.
pub(super) fn piano_stretch(pitch: u8, settings: Settings) -> f32 {
    /// Fundamental below which an octave is judged on proportionally higher partials.
    const JUDGED_FROM: f32 = 110.0;
    let partial = |key: i32, n: f32| {
        let stiffness = register(Model::Piano, key.clamp(0, 127) as u8, settings).stiffness;
        600.0 * ((1.0 + stiffness * n * n) / (1.0 + stiffness)).log2()
    };
    // Width of the octave from `key` to `key + 12` beyond 1200 cents.
    let octave = |key: i32| {
        let hz = 440.0 * ((key as f32 - 69.0) / 12.0).exp2();
        let m = (JUDGED_FROM / hz).max(1.0);
        let narrow = partial(key, 2.0 * m) - partial(key + 12, m);
        let wide = partial(key, 4.0 * m) - partial(key + 12, 2.0 * m);
        0.5 * (narrow + wide)
    };
    let pitch = i32::from(pitch);
    if pitch >= 69 {
        (69..pitch).map(octave).sum::<f32>() / 12.0
    } else {
        -(pitch..69).map(octave).sum::<f32>() / 12.0
    }
}

pub(super) fn expression(model: Model, value: f32) -> f32 {
    if model == Model::Violin {
        f64::from(value).powf(model.expression_exponent()) as f32
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physical::Physical;

    #[test]
    fn register_curves_are_continuous_and_keep_controls_bounded() {
        for model in [Model::Guitar, Model::Violin] {
            let settings = Physical::new(model).settings();
            let mut previous = register(model, 0, settings);
            for pitch in 1..=127 {
                let current = register(model, pitch, settings);
                assert!((0.0..=1.0).contains(&current.hardness));
                assert!((0.1..=12.0).contains(&current.decay));
                assert!((0.0..=1.0).contains(&current.damping));
                assert!((current.hardness - previous.hardness).abs() < 0.01);
                assert!((current.decay - previous.decay).abs() < 0.32);
                previous = current;
            }
        }
    }

    #[test]
    fn bow_expression_preserves_endpoints_and_is_monotonic() {
        assert_eq!(expression(Model::Violin, 0.0), 0.0);
        assert_eq!(expression(Model::Violin, 1.0), 1.0);
        assert!(expression(Model::Violin, 0.5) < 0.5);
        let values: Vec<_> = (0..=127)
            .map(|v| expression(Model::Violin, v as f32 / 127.0))
            .collect();
        assert!(values.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
