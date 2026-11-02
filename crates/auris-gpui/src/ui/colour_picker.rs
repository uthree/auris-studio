//! A colour picker made of the application's own controls.
//!
//! Atelier has no colour picker yet; its choosing guide says to use the application's own,
//! built from tokens. This is that: three drag sliders — hue, saturation and brightness — and a
//! colour code that can be typed, beside whatever row of preset swatches the caller already
//! shows. The sliders are [`value_slider`](crate::ui::widgets::value_slider)s, so a colour is
//! dragged, nudged with the arrow keys and fine-dragged with Shift exactly as every other value
//! in the application is.
//!
//! The arithmetic lives here as free functions and [`Hsv`], apart from any view, because it is
//! the part worth asserting on.

use auris_session::prelude::Color;

/// One of the three things a picker's sliders move.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum HsvChannel {
    /// Which colour, round the wheel.
    Hue,
    /// How far from grey.
    Saturation,
    /// How far from black.
    Value,
}

impl HsvChannel {
    /// Every channel, in the order the sliders are drawn.
    pub const ALL: [HsvChannel; 3] = [HsvChannel::Hue, HsvChannel::Saturation, HsvChannel::Value];

    /// A stable name for element ids and test selectors.
    pub fn id(self) -> &'static str {
        match self {
            HsvChannel::Hue => "hue",
            HsvChannel::Saturation => "saturation",
            HsvChannel::Value => "value",
        }
    }
}

/// A colour as hue, saturation and value.
///
/// Kept by the picker rather than worked out again from the RGB each frame, because RGB forgets
/// things the hand has not let go of: drag the saturation to nothing and every hue is the same
/// grey, so a colour recomputed from that grey would come back red whatever it had been.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Hsv {
    /// Degrees, from 0 up to but not including 360.
    pub hue: f32,
    /// From 0 (grey) to 1 (pure).
    pub saturation: f32,
    /// From 0 (black) to 1 (full brightness).
    pub value: f32,
}

impl Hsv {
    /// The hue, saturation and value of an RGB colour.
    pub fn from_color(color: Color) -> Self {
        let (r, g, b) = color.rgb();
        let (r, g, b) = (
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
        );
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let chroma = max - min;
        let hue = if chroma <= f32::EPSILON {
            0.0
        } else if max == r {
            60.0 * ((g - b) / chroma).rem_euclid(6.0)
        } else if max == g {
            60.0 * ((b - r) / chroma + 2.0)
        } else {
            60.0 * ((r - g) / chroma + 4.0)
        };
        Self {
            hue: hue.rem_euclid(360.0),
            saturation: if max <= f32::EPSILON {
                0.0
            } else {
                chroma / max
            },
            value: max,
        }
    }

    /// The RGB colour this names, each component rounded to the nearest step.
    pub fn to_color(self) -> Color {
        let hue = self.hue.rem_euclid(360.0) / 60.0;
        let saturation = self.saturation.clamp(0.0, 1.0);
        let value = self.value.clamp(0.0, 1.0);
        let chroma = value * saturation;
        let x = chroma * (1.0 - (hue.rem_euclid(2.0) - 1.0).abs());
        let (r, g, b) = match hue as u32 {
            0 => (chroma, x, 0.0),
            1 => (x, chroma, 0.0),
            2 => (0.0, chroma, x),
            3 => (0.0, x, chroma),
            4 => (x, 0.0, chroma),
            _ => (chroma, 0.0, x),
        };
        let m = value - chroma;
        let byte = |channel: f32| ((channel + m).clamp(0.0, 1.0) * 255.0).round() as u32;
        Color((byte(r) << 16) | (byte(g) << 8) | byte(b))
    }

    /// Where `channel` sits on its slider, from 0 to 1.
    pub fn fraction(self, channel: HsvChannel) -> f32 {
        match channel {
            HsvChannel::Hue => self.hue.rem_euclid(360.0) / 360.0,
            HsvChannel::Saturation => self.saturation,
            HsvChannel::Value => self.value,
        }
        .clamp(0.0, 1.0)
    }

    /// The same colour with `channel` moved to `fraction` of its slider.
    ///
    /// A hue at the very end of its slider is kept just short of 360° rather than wrapped to 0,
    /// so that the slider's thumb stays where the hand left it instead of jumping back to the
    /// start: both name the same red.
    pub fn with(self, channel: HsvChannel, fraction: f32) -> Self {
        let fraction = fraction.clamp(0.0, 1.0);
        match channel {
            HsvChannel::Hue => Self {
                hue: (fraction * 360.0).min(359.99),
                ..self
            },
            HsvChannel::Saturation => Self {
                saturation: fraction,
                ..self
            },
            HsvChannel::Value => Self {
                value: fraction,
                ..self
            },
        }
    }

    /// What the channel's slider prints: degrees for the hue, a percentage for the others.
    pub fn label(self, channel: HsvChannel) -> String {
        match channel {
            HsvChannel::Hue => format!("{:.0}°", self.hue.rem_euclid(360.0)),
            HsvChannel::Saturation => format!("{:.0}%", self.saturation * 100.0),
            HsvChannel::Value => format!("{:.0}%", self.value * 100.0),
        }
    }

    /// The colour a channel's slider is filled with.
    ///
    /// The hue slider shows the hue at full strength, so that it still says which colour it is
    /// while the saturation or brightness is down near nothing; the other two show the colour
    /// itself.
    pub fn slider_fill(self, channel: HsvChannel) -> Color {
        match channel {
            HsvChannel::Hue => Hsv {
                hue: self.hue,
                saturation: 1.0,
                value: 1.0,
            }
            .to_color(),
            HsvChannel::Saturation | HsvChannel::Value => self.to_color(),
        }
    }
}

/// A colour written as a code: `#3D7BFF`.
pub fn hex(color: Color) -> String {
    format!("#{:06X}", color.0 & 0x00ff_ffff)
}

/// Reads a colour code: six hex digits or the three-digit short form, with or without the `#`,
/// in either case, ignoring the space around it.
pub fn parse_hex(text: &str) -> Option<Color> {
    let digits = text.trim();
    let digits = digits.strip_prefix('#').unwrap_or(digits);
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match digits.len() {
        6 => u32::from_str_radix(digits, 16).ok().map(Color),
        3 => {
            let short = u32::from_str_radix(digits, 16).ok()?;
            let (r, g, b) = ((short >> 8) & 0xf, (short >> 4) & 0xf, short & 0xf);
            Some(Color((r * 0x11) << 16 | (g * 0x11) << 8 | (b * 0x11)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primaries_sit_where_the_wheel_puts_them() {
        let red = Hsv::from_color(Color(0xff0000));
        assert_eq!((red.hue, red.saturation, red.value), (0.0, 1.0, 1.0));
        assert_eq!(Hsv::from_color(Color(0x00ff00)).hue, 120.0);
        assert_eq!(Hsv::from_color(Color(0x0000ff)).hue, 240.0);
        let grey = Hsv::from_color(Color(0x808080));
        assert_eq!(grey.saturation, 0.0);
        assert!((grey.value - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(Hsv::from_color(Color(0x000000)).saturation, 0.0);
    }

    #[test]
    fn every_colour_comes_back_as_itself() {
        // A coarse walk of the whole cube, plus the presets the window offers.
        for r in (0..=255).step_by(15) {
            for g in (0..=255).step_by(15) {
                for b in (0..=255).step_by(15) {
                    let color = Color((r << 16) | (g << 8) | b);
                    assert_eq!(Hsv::from_color(color).to_color(), color, "{}", hex(color));
                }
            }
        }
        for color in [
            Color(0x3d7bff),
            Color(0xf4f1ea),
            Color(0x101c3a),
            Color(0xff5f7a),
        ] {
            assert_eq!(Hsv::from_color(color).to_color(), color);
        }
    }

    #[test]
    fn a_slider_moves_only_its_own_channel() {
        let colour = Hsv::from_color(Color(0x3d7bff));
        let greyed = colour.with(HsvChannel::Saturation, 0.0);
        assert_eq!(
            greyed.hue, colour.hue,
            "the hue survives a trip through grey"
        );
        assert_eq!(greyed.value, colour.value);
        let restored = greyed.with(HsvChannel::Saturation, colour.saturation);
        assert_eq!(restored.to_color(), Color(0x3d7bff));

        let half = colour.with(HsvChannel::Hue, 0.5);
        assert_eq!(half.hue, 180.0);
        assert_eq!(half.fraction(HsvChannel::Hue), 0.5);
        let end = colour.with(HsvChannel::Hue, 1.0);
        assert!(
            end.fraction(HsvChannel::Hue) > 0.999,
            "the thumb stays at the end"
        );
        assert_eq!(
            end.with(HsvChannel::Saturation, 1.0)
                .with(HsvChannel::Value, 1.0)
                .to_color(),
            Color(0xff0000),
            "and it is still red"
        );
        assert_eq!(
            colour.with(HsvChannel::Value, 0.0).to_color(),
            Color(0x000000)
        );
    }

    #[test]
    fn readouts_and_fills() {
        let colour = Hsv::from_color(Color(0x808000));
        assert_eq!(colour.label(HsvChannel::Hue), "60°");
        assert_eq!(colour.label(HsvChannel::Saturation), "100%");
        assert_eq!(colour.label(HsvChannel::Value), "50%");
        assert_eq!(colour.slider_fill(HsvChannel::Hue), Color(0xffff00));
        assert_eq!(colour.slider_fill(HsvChannel::Value), Color(0x808000));
    }

    #[test]
    fn colour_codes_read_and_write() {
        assert_eq!(hex(Color(0x3d7bff)), "#3D7BFF");
        assert_eq!(hex(Color(0x000a0b)), "#000A0B");
        assert_eq!(parse_hex("#3D7BFF"), Some(Color(0x3d7bff)));
        assert_eq!(parse_hex("  3d7bff "), Some(Color(0x3d7bff)));
        assert_eq!(parse_hex("#38f"), Some(Color(0x3388ff)));
        assert_eq!(parse_hex(&hex(Color(0xf4f1ea))), Some(Color(0xf4f1ea)));
        for wrong in [
            "", "#", "#12345", "#1234567", "#GG0000", "+12345", "#-12345", "red",
        ] {
            assert_eq!(parse_hex(wrong), None, "{wrong:?}");
        }
    }
}
