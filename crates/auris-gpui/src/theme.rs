//! Colours and metrics for the whole UI.
//!
//! Everything visual reads from one [`Theme`] value, and every [`Theme`] is derived from a
//! [`Scheme`] — four numbers and an accent. Retuning the palette, or adding a light one, is
//! therefore a matter of naming a scheme rather than of hunting hex literals through view code,
//! and the tests below can check every scheme against the same rules at once.

use gpui::{Font, FontFallbacks, Hsla, Pixels, hsla, px, rgb};

/// The font every panel draws in, with fallbacks for scripts the base family has no glyphs for.
///
/// The base family is each platform's own interface font, named rather than left to a
/// substitution table: asking Windows for Helvetica gets you Arial through a mapping written in
/// the nineties, which is close enough to look like nothing went wrong and far enough to look
/// unlike every other window on the screen.
///
/// None of the three covers Japanese, so a Japanese track name or menu would come out as empty
/// boxes without the fallbacks. They are listed for every platform at once because a family that
/// is not installed is simply skipped, which makes an unused entry free.
pub fn ui_font() -> Font {
    let base = if cfg!(target_os = "macos") {
        "Helvetica"
    } else if cfg!(target_os = "windows") {
        "Segoe UI"
    } else {
        "DejaVu Sans"
    };
    Font {
        fallbacks: Some(FontFallbacks::from_fonts(vec![
            // macOS
            "Hiragino Sans".into(),
            "Apple SD Gothic Neo".into(),
            // Windows
            "Segoe UI".into(),
            "Yu Gothic UI".into(),
            "Meiryo".into(),
            // Linux
            "Noto Sans CJK JP".into(),
            "DejaVu Sans".into(),
        ])),
        ..gpui::font(base)
    }
}

/// A colour scheme, as the few decisions a whole palette follows from.
///
/// Writing each scheme out as thirty hex literals was the alternative, and it does not survive a
/// fourth one: the numbers stop agreeing about which surface sits above which, and nothing catches
/// it because every field is independently plausible. A scheme is instead a hue, how much of it the
/// greys carry, where the background sits on the lightness scale, and an accent — and the thirty
/// colours are derived, so "raised is nearer the eye than sunken" holds by construction and in
/// every scheme at once.
///
/// Whether a scheme is light or dark is not a flag: it follows from [`Scheme::base`], and every
/// step away from the background is taken *towards the foreground* rather than upwards.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Scheme {
    /// Stable identifier written to the preferences file. Never change a released one.
    pub id: &'static str,
    /// Name shown to the user. Not translated, like the drum grooves: these are proper nouns.
    pub name: &'static str,
    /// Hue of the greys, from 0 to 1.
    pub hue: f32,
    /// How much of that hue the greys carry.
    pub chroma: f32,
    /// Lightness of the window background. Everything else is placed relative to it.
    pub base: f32,
    /// The interactive accent, named outright because it is the scheme's whole character.
    pub accent: Hsla,
}

/// Every colour scheme, in the order the settings window offers them.
pub const SCHEMES: &[Scheme] = &[
    // The palette the application shipped with: blue-grey, near-black, a mid blue accent.
    Scheme {
        id: "midnight",
        name: "Midnight",
        hue: 0.625,
        chroma: 0.15,
        base: 0.096,
        accent: Hsla {
            h: 0.567,
            s: 0.68,
            l: 0.59,
            a: 1.0,
        },
    },
    // Neutral greys and a warm accent, for anyone who finds a blue interface cold.
    Scheme {
        id: "graphite",
        name: "Graphite",
        hue: 0.08,
        chroma: 0.02,
        base: 0.105,
        accent: Hsla {
            h: 0.075,
            s: 0.74,
            l: 0.56,
            a: 1.0,
        },
    },
    // The same architecture read the other way up: a near-white window, and every step from it
    // taken downwards.
    Scheme {
        id: "daylight",
        name: "Daylight",
        hue: 0.60,
        chroma: 0.12,
        base: 0.965,
        accent: Hsla {
            h: 0.575,
            s: 0.62,
            l: 0.44,
            a: 1.0,
        },
    },
    // A warm light scheme: paper rather than screen, with a deep teal accent to stay off the
    // yellow the greys sit on.
    Scheme {
        id: "parchment",
        name: "Parchment",
        hue: 0.11,
        chroma: 0.24,
        base: 0.945,
        accent: Hsla {
            h: 0.49,
            s: 0.55,
            l: 0.33,
            a: 1.0,
        },
    },
];

/// The scheme used when nothing has been chosen, and when a stored choice names one this build
/// does not have.
pub const DEFAULT_SCHEME: &str = "midnight";

/// The scheme with this id.
pub fn scheme(id: &str) -> Option<&'static Scheme> {
    SCHEMES.iter().find(|scheme| scheme.id == id)
}

/// The scheme with this id, or the default when nothing answers to it.
///
/// Forgiving on purpose, like the keymap: the preferences file is user-editable text that outlives
/// the build that wrote it, and a scheme that has been renamed should cost the colour it was, not
/// the ability to start.
pub fn scheme_or_default(id: &str) -> &'static Scheme {
    scheme(id).unwrap_or_else(|| scheme(DEFAULT_SCHEME).expect("the default scheme exists"))
}

impl Scheme {
    /// `1.0` where the interface reads light-on-dark, `-1.0` where it reads dark-on-light.
    fn direction(&self) -> f32 {
        if self.base < 0.5 { 1.0 } else { -1.0 }
    }

    /// A neutral `step` of the way from the background towards the foreground.
    fn shade(&self, step: f32) -> Hsla {
        hsla(
            self.hue,
            self.chroma,
            (self.base + step * self.direction()).clamp(0.0, 1.0),
            1.0,
        )
    }

    /// One of the colours that mean something — a meter, the playhead — at a lightness that
    /// shows against this scheme's background.
    ///
    /// Their hues are fixed across every scheme, because they are not decoration: green means
    /// headroom and red means clipping wherever you are.
    fn signal(&self, hue: f32, saturation: f32) -> Hsla {
        let lightness = if self.direction() > 0.0 { 0.60 } else { 0.44 };
        hsla(hue, saturation, lightness, 1.0)
    }
}

/// A neutral shade at least `step` from the background, deepened until it reaches `ratio`.
///
/// Walked outwards a hundredth at a time rather than solved: the relationship between a step
/// along the ramp and a contrast ratio depends on the scheme's hue and chroma, so there is no
/// closed form worth writing. Four schemes times a few hundred steps, once at start-up.
fn readable_shade(scheme: &Scheme, step: f32, ratio: f32) -> Hsla {
    // Measured against the *nearest* surface in the stack rather than the background, because
    // text is drawn on all of them and the one closest to it is the one that decides. A colour
    // that clears the threshold on the window's background can still fail on a raised panel.
    let hardest = scheme.shade(SURFACE_HOVER_STEP);
    let mut step = step;
    while step < 1.0 {
        let candidate = scheme.shade(step);
        if contrast_ratio(candidate, hardest) >= ratio {
            return candidate;
        }
        step += 0.01;
    }
    scheme.shade(1.0)
}

/// The highest surface in the stack, and so the one text has the least contrast against.
const SURFACE_HOVER_STEP: f32 = 0.114;

/// The WCAG contrast ratio between two opaque colours, from 1:1 to 21:1.
///
/// Written out rather than eyeballed, because a difference in lightness is not a contrast: two
/// colours a third of the ramp apart can still be under 3:1 when one of them is saturated. 4.5
/// is the threshold for body text, 3.0 for large text and for anything that is not text.
pub fn contrast_ratio(a: Hsla, b: Hsla) -> f32 {
    let (a, b) = (relative_luminance(a), relative_luminance(b));
    let (brighter, darker) = if a >= b { (a, b) } else { (b, a) };
    (brighter + 0.05) / (darker + 0.05)
}

/// Relative luminance, as WCAG defines it: linearised channels under human sensitivity weights.
fn relative_luminance(color: Hsla) -> f32 {
    let rgba: gpui::Rgba = color.into();
    let linear = |value: f32| {
        if value <= 0.040_45 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(rgba.r) + 0.7152 * linear(rgba.g) + 0.0722 * linear(rgba.b)
}

/// Near-black or near-white, whichever can be read on `color`, tinted to match a scheme's greys.
fn readable_on(color: Hsla, scheme: &Scheme) -> Hsla {
    hsla(
        scheme.hue,
        scheme.chroma,
        if color.l > 0.55 { 0.06 } else { 0.97 },
        1.0,
    )
}

/// The application colour palette.
#[derive(Clone, Debug)]
pub struct Theme {
    /// Id of the scheme this was built from, so a picker can tick the one in force.
    pub scheme: &'static str,
    /// Window background, behind every panel.
    pub background: Hsla,
    /// Standard panel surface.
    pub surface: Hsla,
    /// Slightly raised surface, for headers and toolbars.
    pub surface_raised: Hsla,
    /// Recessed surface, for timeline and piano-roll backgrounds.
    pub surface_sunken: Hsla,
    /// Hover highlight.
    pub surface_hover: Hsla,
    /// Border between panels.
    pub border: Hsla,
    /// Softer border, for internal dividers.
    pub border_subtle: Hsla,
    /// Primary text.
    pub text: Hsla,
    /// Secondary text and inactive labels.
    pub text_muted: Hsla,
    /// Captions above a readout, and other text that should recede entirely.
    pub text_faint: Hsla,
    /// Text on an accent-filled surface.
    pub text_on_accent: Hsla,
    /// Interactive accent.
    pub accent: Hsla,
    /// Accent used for large filled areas.
    pub accent_soft: Hsla,
    /// Something went wrong, or is about to.
    ///
    /// The colour of a failure in the status bar and of the answer on a sheet that throws work
    /// away. Not derived from the accent: a scheme whose accent is already red would otherwise
    /// report every failure in the same colour it draws its buttons.
    pub danger: Hsla,
    /// Something happened that somebody should know about, but nothing failed.
    ///
    /// A step short of [`Self::danger`] and a different hue, because the log draws both at once
    /// and a warning that looked like an error would make the errors invisible.
    pub warning: Hsla,
    /// Transport playing indicator.
    pub playing: Hsla,
    /// Playhead line.
    pub playhead: Hsla,
    /// Bar lines in the timeline grid.
    pub grid_bar: Hsla,
    /// Beat lines in the timeline grid.
    pub grid_beat: Hsla,
    /// Subdivision lines in the timeline grid.
    pub grid_subdivision: Hsla,
    /// Selection outline.
    pub selection: Hsla,
    /// Loop region tint.
    pub loop_region: Hsla,
    /// Meter fill below -12 dBFS.
    pub meter_low: Hsla,
    /// Meter fill between -12 and -3 dBFS.
    pub meter_mid: Hsla,
    /// Meter fill above -3 dBFS.
    pub meter_high: Hsla,
    /// Solo button when engaged.
    pub solo: Hsla,
    /// Mute button when engaged.
    pub mute: Hsla,
    /// Record arm, and the transport's record button while a take is running.
    ///
    /// Its own entry rather than [`Self::danger`], which is the same red: one of them says a
    /// thing failed and the other says a microphone is live, and a scheme that wanted to tell
    /// those apart should be able to without the failures changing colour too.
    pub record: Hsla,
    /// White keys in the piano roll keyboard.
    pub key_white: Hsla,
    /// Black keys in the piano roll keyboard.
    pub key_black: Hsla,
    /// Lane background behind a black key's row.
    pub key_row_black: Hsla,
    /// Colour of the softest note in the piano roll. See [`Theme::velocity_color`].
    pub velocity_soft: Hsla,
    /// Colour of the hardest-struck note in the piano roll.
    pub velocity_loud: Hsla,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    /// The palette a scheme describes.
    ///
    /// The lightness steps below are the whole layout of the interface, written down once. Reading
    /// them in order — sunken, background, surface, raised, hover — is reading the stack of
    /// surfaces from furthest to nearest, and every scheme gets the same stack.
    pub fn from_scheme(scheme: &Scheme) -> Self {
        let accent = scheme.accent;
        Self {
            scheme: scheme.id,
            surface_sunken: scheme.shade(-0.020),
            background: scheme.shade(0.0),
            surface: scheme.shade(0.034),
            surface_raised: scheme.shade(0.074),
            surface_hover: scheme.shade(SURFACE_HOVER_STEP),
            border_subtle: scheme.shade(0.074),
            border: scheme.shade(0.149),
            // The declared step is a *floor*, not the answer. The same distance along the ramp
            // is a different contrast in a saturated scheme than in a grey one, and different
            // again read downwards than upwards — which is how `text_faint` came to sit under
            // 3:1 in every scheme and under 2:1 in Parchment while carrying readout captions
            // and disabled menu commands.
            text: readable_shade(scheme, 0.824, 7.0),
            text_muted: readable_shade(scheme, 0.504, 4.5),
            // 3.5 rather than 4.5: this is the tier below muted and has to stay visibly below
            // it, and pushing both to the same threshold in the light schemes would collapse
            // the two into one colour. Anything that carries information a user must read
            // belongs in `text_muted`.
            text_faint: readable_shade(scheme, 0.354, 3.5),
            grid_subdivision: scheme.shade(0.054),
            grid_beat: scheme.shade(0.104),
            grid_bar: scheme.shade(0.194),
            key_row_black: scheme.shade(-0.012),
            accent,
            // A wash of the accent for the areas large enough that the accent itself would shout:
            // half the colour, and pulled most of the way back to the background.
            accent_soft: Hsla {
                s: accent.s * 0.5,
                l: scheme.base + (accent.l - scheme.base) * 0.35,
                ..accent
            },
            selection: Hsla {
                l: (accent.l + 0.18 * scheme.direction()).clamp(0.0, 1.0),
                ..accent
            },
            text_on_accent: readable_on(accent, scheme),
            loop_region: Hsla {
                s: accent.s * 0.6,
                ..accent
            },
            playing: scheme.signal(0.38, 0.52),
            playhead: scheme.signal(0.02, 0.85),
            // A signal rather than a shade, so it lands at the lightness this scheme reserves for
            // colours that have to be read against its background — which is what a status line
            // reporting a failure in it needs.
            danger: scheme.signal(0.01, 0.72),
            // Amber, a third of the way round from the red: far enough that the two are told
            // apart at a glance in a list where they sit one line above the other.
            warning: scheme.signal(0.11, 0.78),
            meter_low: scheme.signal(0.38, 0.52),
            meter_mid: scheme.signal(0.14, 0.62),
            meter_high: scheme.signal(0.01, 0.68),
            solo: scheme.signal(0.12, 0.72),
            mute: scheme.signal(0.06, 0.70),
            // The reddest of the four signals, and further round from mute's orange than mute is
            // from solo's amber: an armed track and a muted one sit in the same row of buttons.
            record: scheme.signal(0.99, 0.78),
            // The keyboard strip stays a keyboard in every scheme. Deriving these from the ramp
            // would give a light scheme white "black" keys, which is not a stylistic choice but a
            // piano nobody can read.
            key_white: hsla(scheme.hue, scheme.chroma * 0.4, 0.86, 1.0),
            key_black: hsla(scheme.hue, scheme.chroma, 0.18, 1.0),
            // The two ends of the velocity ramp. What matters is the path between them, which is
            // a statement about hue — see [`Theme::velocity_color`].
            velocity_soft: hsla(
                0.58,
                0.52,
                if scheme.direction() > 0.0 { 0.55 } else { 0.46 },
                1.0,
            ),
            velocity_loud: hsla(
                0.02,
                0.68,
                if scheme.direction() > 0.0 { 0.58 } else { 0.50 },
                1.0,
            ),
        }
    }

    /// The palette named by `id`, or the default one when nothing answers to it.
    pub fn named(id: &str) -> Self {
        Self::from_scheme(scheme_or_default(id))
    }

    /// The default dark palette.
    pub fn dark() -> Self {
        Self::named(DEFAULT_SCHEME)
    }

    /// A label colour that can be read on `color`.
    ///
    /// [`Theme::text_on_accent`] answers this for the accent, and is the right answer only for
    /// the accent. A track's colour is chosen by the user and a meter's by its level, so a label
    /// on either has to ask about *that* colour — a light scheme puts white on its dark accent,
    /// which is exactly the wrong thing to print on a pale yellow track.
    pub fn text_on(&self, color: Hsla) -> Hsla {
        Hsla {
            h: self.background.h,
            s: self.background.s,
            l: if color.l > 0.55 { 0.06 } else { 0.97 },
            a: 1.0,
        }
    }

    /// Colour of a note struck at `velocity`, from softest to hardest.
    ///
    /// Logic colours a note by how hard it was struck, and the reason is that no amount of
    /// staring at a grid of identical rectangles says where the dynamics are. Brightness alone
    /// was tried here first and is not enough: velocity 0.8 and velocity 1.0 differ by a few per
    /// cent of lightness, which is invisible next to a note an octave away on a different row.
    ///
    /// The two ends are walked in a straight line rather than the short way round the colour
    /// wheel, so a palette chooses which way the ramp runs by which hues it names. Blue above red
    /// walks *down* through green and yellow, which is the heat map everybody already reads;
    /// taking the short way would have gone up through purple instead, which reads as nothing.
    pub fn velocity_color(&self, velocity: f32) -> Hsla {
        let amount = velocity.clamp(0.0, 1.0);
        let (soft, loud) = (self.velocity_soft, self.velocity_loud);
        let between = |from: f32, to: f32| from + (to - from) * amount;
        Hsla {
            h: between(soft.h, loud.h),
            s: between(soft.s, loud.s),
            l: between(soft.l, loud.l),
            a: between(soft.a, loud.a),
        }
    }

    /// The colour standing for one group of a list, at `hue`.
    ///
    /// Only the hue is asked for, because the groups are told apart *by* hue: sixteen hues at one
    /// lightness read as sixteen labels, and the same sixteen at sixteen lightnesses read as a
    /// gradient with no labels in it. The lightness starts where this scheme puts the colours that
    /// have to show against its background — the same place a meter sits — and moves only as far
    /// as the hue makes it, which is the one thing that has to give.
    ///
    /// Never put text in this. It is a mark beside a name, not the name — the hues that make the
    /// best labels are the ones that make the worst body text, and the two jobs cannot be done by
    /// one colour without one of them being done badly.
    pub fn group_color(&self, hue: f32) -> Hsla {
        let hue = hue.rem_euclid(1.0);
        let toward = if self.background.l < 0.5 { 1.0 } else { -1.0 };
        let mut lightness = if toward > 0.0 { 0.62 } else { 0.46 };
        // Walked outwards until the mark can actually be seen, the way `readable_shade` walks the
        // grey ramp, and for a sharper version of the same reason: lightness is not luminance, and
        // the gap between the two is *widest* across the hues. Pure blue at the lightness that
        // makes yellow comfortable carries about a quarter of yellow's luminance — which is how a
        // fixed lightness came out at 2.7:1 on Midnight's hovered rows for one group in ten while
        // the other nine were fine.
        //
        // 3.2 rather than 3.0, so the colour clears the threshold it is measured against instead of
        // landing on it and rounding under.
        for _ in 0..60 {
            let candidate = hsla(hue, 0.58, lightness, 1.0);
            if contrast_ratio(candidate, self.surface_hover) >= 3.2 {
                return candidate;
            }
            lightness = (lightness + 0.01 * toward).clamp(0.0, 1.0);
        }
        hsla(hue, 0.58, lightness, 1.0)
    }

    /// Colour for a meter or clip indicator at `level_db`.
    pub fn meter_color(&self, level_db: f32) -> Hsla {
        if level_db >= -3.0 {
            self.meter_high
        } else if level_db >= -12.0 {
            self.meter_mid
        } else {
            self.meter_low
        }
    }

    /// Converts a packed `0xRRGGBB` track colour into an `Hsla`.
    pub fn track_color(&self, packed: u32) -> Hsla {
        rgb(packed).into()
    }

    /// A translucent variant of `color`, for clip fills over a grid.
    pub fn translucent(color: Hsla, alpha: f32) -> Hsla {
        Hsla { a: alpha, ..color }
    }

    /// Blends `color` towards white by `amount` (0.0..1.0).
    pub fn lighten(color: Hsla, amount: f32) -> Hsla {
        Hsla {
            l: (color.l + (1.0 - color.l) * amount).clamp(0.0, 1.0),
            ..color
        }
    }

    /// Blends `color` towards black by `amount` (0.0..1.0).
    pub fn darken(color: Hsla, amount: f32) -> Hsla {
        Hsla {
            l: (color.l * (1.0 - amount)).clamp(0.0, 1.0),
            ..color
        }
    }

    /// `color` as it looks with the pointer over it.
    ///
    /// Towards the foreground rather than towards white: lightening a hovered button is only a
    /// highlight on a dark scheme, and on a light one it walks a pale grey towards the white it is
    /// already nearly indistinguishable from.
    pub fn hovered(&self, color: Hsla, amount: f32) -> Hsla {
        if self.background.l < 0.5 {
            Self::lighten(color, amount)
        } else {
            Self::darken(color, amount)
        }
    }
}

/// Fixed sizes shared across panels, so the timeline and its ruler stay aligned.
pub struct Metrics;

impl Metrics {
    /// Height of the top transport bar.
    ///
    /// Tall enough for the transport buttons and the readouts stacked under them, which is how
    /// Logic arranges the same controls.
    pub const TRANSPORT_HEIGHT: Pixels = px(84.0);
    /// Height of the timeline ruler above the arrangement.
    pub const RULER_HEIGHT: Pixels = px(28.0);
    /// Height of the harmony lane, between the structure lane and the clip lanes.
    ///
    /// Two rows: a thin strip of key changes over a taller strip of chords. See
    /// [`paint::harmony_rows`](crate::ui::paint::harmony_rows) for why they are not one.
    pub const HARMONY_LANE_HEIGHT: Pixels = px(34.0);
    /// Height of the structure lane, between the ruler and the harmony.
    ///
    /// One row of section names — イントロ, Aメロ, サビ — coarser than the harmony below it,
    /// which is why it sits above: the stack reads from the largest division of the song down
    /// to the smallest.
    pub const STRUCTURE_LANE_HEIGHT: Pixels = px(18.0);
    /// Width of the track header column.
    ///
    /// Wide enough for the button row an *audio* track carries — mute, solo, arm and input
    /// monitor — with the gain fader beside it still worth dragging. It was twenty pixels
    /// narrower when a track had three buttons. Only the default: the column is draggable and a
    /// layout somebody has already resized keeps whatever they set.
    pub const TRACK_HEADER_WIDTH: Pixels = px(216.0);
    /// Width of the piano-roll keyboard.
    pub const KEYBOARD_WIDTH: Pixels = px(56.0);
    /// Height of one piano-roll note row at 100 % zoom.
    pub const NOTE_ROW_HEIGHT: Pixels = px(14.0);
    /// Width the left-hand dock opens at.
    pub const LEFT_DOCK_WIDTH: Pixels = px(240.0);
    /// Width the right-hand dock opens at.
    pub const RIGHT_DOCK_WIDTH: Pixels = px(300.0);
    /// Height the bottom dock opens at, including the panel's own header strip.
    pub const BOTTOM_DOCK_HEIGHT: Pixels = px(280.0);
    /// Height of the header strip at the top of a docked panel.
    pub const PANEL_HEADER_HEIGHT: Pixels = px(22.0);
    /// Height of the status bar along the bottom of the window.
    pub const STATUS_HEIGHT: Pixels = px(22.0);
    /// Height of a control row inside the inspector.
    pub const CONTROL_HEIGHT: Pixels = px(22.0);

    /// Corner radius for small controls: buttons, sliders, meters.
    pub const RADIUS_SM: Pixels = px(4.0);
    /// Corner radius for panels, clips and readouts.
    pub const RADIUS_MD: Pixels = px(6.0);
    /// Corner radius for floating surfaces such as the export sheet.
    pub const RADIUS_LG: Pixels = px(10.0);
    /// Corner radius for notes and other very small marks.
    pub const RADIUS_XS: Pixels = px(2.5);

    /// Thickness of a draggable panel divider — the grab zone, not the drawn line.
    pub const SPLITTER: Pixels = px(6.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scheme_is_reachable_by_the_name_it_is_stored_under() {
        let mut ids = std::collections::BTreeSet::new();
        let mut names = std::collections::BTreeSet::new();
        for entry in SCHEMES {
            assert!(ids.insert(entry.id), "duplicate id `{}`", entry.id);
            assert!(
                names.insert(entry.name),
                "two schemes called {}",
                entry.name
            );
            assert_eq!(scheme(entry.id).map(|found| found.id), Some(entry.id));
            assert_eq!(Theme::named(entry.id).scheme, entry.id);
        }
        assert!(scheme(DEFAULT_SCHEME).is_some(), "the default must exist");
        // Anything else falls back rather than failing, which is what makes an old preferences
        // file harmless.
        assert_eq!(scheme_or_default("chartreuse").id, DEFAULT_SCHEME);
    }

    #[test]
    fn every_scheme_stacks_its_surfaces_the_same_way_round() {
        // The whole point of deriving a palette rather than writing thirty hex literals per
        // scheme: sunken, background, surface, raised, hover is the order of the stack from
        // furthest to nearest, and it has to hold in a light scheme too — where "nearer" is
        // darker, not lighter.
        for entry in SCHEMES {
            let theme = Theme::from_scheme(entry);
            let toward = entry.direction();
            let ladder = [
                ("sunken", theme.surface_sunken),
                ("background", theme.background),
                ("surface", theme.surface),
                ("raised", theme.surface_raised),
                ("hover", theme.surface_hover),
            ];
            for pair in ladder.windows(2) {
                let ((below, under), (above, over)) = (pair[0], pair[1]);
                assert!(
                    (over.l - under.l) * toward > 0.0,
                    "{}: {above} is not nearer the eye than {below}",
                    entry.name
                );
            }
            // And the grid climbs the same way: a bar line has to be findable among the beats.
            assert!(
                (theme.grid_bar.l - theme.grid_beat.l) * toward > 0.0
                    && (theme.grid_beat.l - theme.grid_subdivision.l) * toward > 0.0,
                "{}: the grid does not get brighter towards the downbeat",
                entry.name
            );
        }
    }

    #[test]
    fn every_scheme_keeps_its_text_readable() {
        // A scheme is four numbers, so a plausible-looking one can quietly put grey text on a
        // grey panel. Nothing else in the application would catch it.
        for entry in SCHEMES {
            let theme = Theme::from_scheme(entry);
            for (name, behind) in [
                ("background", theme.background),
                ("surface", theme.surface),
                ("raised", theme.surface_raised),
                ("sunken", theme.surface_sunken),
                // The hardest of the five, and the one a hovered row is drawn on.
                ("hover", theme.surface_hover),
            ] {
                // Real ratios, not lightness deltas. A delta cannot tell a readable colour from
                // an unreadable one — Parchment is a saturated scheme, and a third of its ramp
                // came out under 2:1 while passing a 0.25 lightness check comfortably.
                for (tier, color, least) in [
                    ("text", theme.text, 7.0),
                    ("muted text", theme.text_muted, 4.5),
                    ("faint text", theme.text_faint, 3.5),
                ] {
                    let ratio = contrast_ratio(color, behind);
                    assert!(
                        ratio >= least,
                        "{}: {tier} on {name} is {ratio:.2}:1, wanted {least}:1",
                        entry.name,
                    );
                }
            }
            // The tiers stay in order, or the hierarchy they exist to draw is a fiction.
            assert!(
                contrast_ratio(theme.text, theme.background)
                    > contrast_ratio(theme.text_muted, theme.background),
                "{}: muted text does not recede from text",
                entry.name
            );
            assert!(
                contrast_ratio(theme.text_muted, theme.background)
                    > contrast_ratio(theme.text_faint, theme.background),
                "{}: faint text does not recede from muted text",
                entry.name
            );
            // A failure has to be readable where it is reported, which is the status bar.
            assert!(
                contrast_ratio(theme.danger, theme.surface_raised) >= 3.0,
                "{}: a failure cannot be read on the status bar",
                entry.name
            );
            assert!(
                (theme.text_on_accent.l - theme.accent.l).abs() > 0.35,
                "{}: a label on the accent is unreadable",
                entry.name
            );
            // The keyboard strip stays a keyboard: whatever the scheme, a black key is dark and a
            // white key is light, or the roll's edge is unreadable as a piano.
            assert!(
                theme.key_white.l > 0.6 && theme.key_black.l < 0.4,
                "{}: the keyboard has stopped looking like one",
                entry.name
            );
        }
    }

    #[test]
    fn velocity_reads_as_a_heat_map_rather_than_as_two_shades() {
        let theme = Theme::dark();
        let same = |left: Hsla, right: Hsla| {
            (left.h - right.h).abs() < 1e-4
                && (left.s - right.s).abs() < 1e-4
                && (left.l - right.l).abs() < 1e-4
                && (left.a - right.a).abs() < 1e-4
        };
        assert!(same(theme.velocity_color(0.0), theme.velocity_soft));
        assert!(same(theme.velocity_color(1.0), theme.velocity_loud));
        // Out of range is clamped rather than extrapolated off the end of the ramp.
        assert!(same(theme.velocity_color(-1.0), theme.velocity_soft));
        assert!(same(theme.velocity_color(9.0), theme.velocity_loud));

        // The hue has to move, and move steadily: this is the whole difference from the
        // brightness ramp it replaced, where velocity 0.8 and 1.0 were the same rectangle.
        let hues: Vec<f32> = (0..=10)
            .map(|step| theme.velocity_color(step as f32 / 10.0).h)
            .collect();
        for pair in hues.windows(2) {
            assert!(pair[0] > pair[1], "the ramp doubles back: {hues:?}");
        }
        // Down through green and yellow, not up through purple. Both connect blue to red; only
        // one of them says "louder".
        let middle = theme.velocity_color(0.5).h;
        assert!(
            (0.15..0.45).contains(&middle),
            "half velocity came out at hue {middle}, which is not on the warm side of green"
        );
    }

    #[test]
    fn a_group_mark_can_be_seen_at_every_hue_in_every_scheme() {
        // The library walks the whole wheel, so the hue is not a colour somebody chose against a
        // particular background — it is whichever one the walk landed on. A mark is not text, so
        // 3:1 is the threshold; below it a group's colour is a group nobody can see.
        for entry in SCHEMES {
            let theme = Theme::from_scheme(entry);
            for step in 0..36 {
                let color = theme.group_color(step as f32 / 36.0);
                for (name, behind) in [("surface", theme.surface), ("hover", theme.surface_hover)] {
                    let ratio = contrast_ratio(color, behind);
                    assert!(
                        ratio >= 3.0,
                        "{}: hue {} on {name} is {ratio:.2}:1",
                        entry.name,
                        step as f32 / 36.0
                    );
                }
            }
        }
        // The hue is taken all the way round, so a walk that runs past 1.0 wraps rather than
        // clamping every group past the end onto one colour.
        let theme = Theme::dark();
        assert_eq!(theme.group_color(1.25), theme.group_color(0.25));
    }

    #[test]
    fn meter_colour_changes_at_the_documented_thresholds() {
        let theme = Theme::dark();
        assert_eq!(theme.meter_color(-20.0), theme.meter_low);
        assert_eq!(theme.meter_color(-6.0), theme.meter_mid);
        assert_eq!(theme.meter_color(-1.0), theme.meter_high);
    }

    #[test]
    fn the_base_family_is_one_this_platform_ships() {
        // Asking for a family the system does not have lands the whole interface on whatever
        // the substitution table picks, which is a thing you only notice in a screenshot.
        let font = ui_font();
        let expected = if cfg!(target_os = "macos") {
            "Helvetica"
        } else if cfg!(target_os = "windows") {
            "Segoe UI"
        } else {
            "DejaVu Sans"
        };
        assert_eq!(font.family.as_ref(), expected);
        assert!(
            font.fallbacks
                .as_ref()
                .is_some_and(|fallbacks| fallbacks.fallback_list().len() >= 5),
            "the Japanese fallbacks are what keep track names from being empty boxes"
        );
    }
}
