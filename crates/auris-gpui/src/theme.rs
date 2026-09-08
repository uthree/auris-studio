//! Colours and metrics for the whole UI.
//!
//! Everything visual reads from one [`Theme`] value, and every [`Theme`] is derived from a
//! [`Scheme`] — neutral surface parameters, an accent and optional categorical colours.
//! Retuning the palette, or adding a light one, is
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
    ui_font_for(None)
}

/// The chosen interface family, with the same multilingual fallbacks as the system default.
pub fn ui_font_for(family: Option<&str>) -> Font {
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
        ..gpui::font(family.unwrap_or(base).to_owned())
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
pub struct Scheme<'a> {
    /// Stable identifier written to the preferences file. Never change a released one.
    pub id: &'a str,
    /// Name shown to the user. Not translated, like the drum grooves: these are proper nouns.
    pub name: &'a str,
    /// Hue of the greys, from 0 to 1.
    pub hue: f32,
    /// How much of that hue the greys carry.
    pub chroma: f32,
    /// Lightness of the window background. Everything else is placed relative to it.
    pub base: f32,
    /// The interactive accent, named outright because it is the scheme's whole character.
    pub accent: Hsla,
    /// Optional RGB overrides for the document's eight stable palette slots.
    pub track_palette: [Option<u32>; 8],
}

/// Built-in colour schemes, in the order the settings window offers them.
///
/// Each categorical palette is authored for its scheme, in document slot order:
/// instrument, audio, drums, bus/effects, singer, then three unassigned colours.
/// One uses Atom's syntax hues; GitHub uses Primer's colour scales. The light palettes
/// deepen colours where needed to clear 3:1 against every surface, including hover.
/// Surface ramps remain derived independently from these categorical colours.
pub const SCHEMES: &[Scheme<'static>] = &[
    // The palette the application shipped with: blue-grey, near-black, a mid blue accent.
    Scheme {
        id: "midnight",
        track_palette: [
            Some(0x4f9dde),
            Some(0x5fc9a3),
            Some(0xd97b6c),
            Some(0xe0b452),
            Some(0xb07cc6),
            Some(0x7fb069),
            Some(0xe0a458),
            Some(0xd16b8a),
        ],
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
        track_palette: [
            Some(0xdd9f60),
            Some(0x8eafa0),
            Some(0xce8176),
            Some(0xb6ad79),
            Some(0xad98b8),
            Some(0xa8b78b),
            Some(0xcaa889),
            Some(0xb38f9c),
        ],
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
        track_palette: [
            Some(0x246caa),
            Some(0x267e70),
            Some(0xb4474e),
            Some(0x946c20),
            Some(0x8151a4),
            Some(0x4a7c3a),
            Some(0xa95b28),
            Some(0x9b4776),
        ],
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
        track_palette: [
            Some(0x26766d),
            Some(0x647849),
            Some(0xa24b3e),
            Some(0x8a651c),
            Some(0x805d88),
            Some(0x416d87),
            Some(0xa05e32),
            Some(0x925b70),
        ],
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
    // Atom's One Dark, as the four numbers its palette comes down to: the blue-grey of #282c34,
    // and the #61afef it draws a selection in. The background is a shade deeper than the editor's
    // own 0.18, because the signals sit at one lightness for the whole ramp and a failure in the
    // status bar came out at 2.95:1 against the toolbar behind it there.
    Scheme {
        id: "one-dark",
        track_palette: [
            Some(0x61afef),
            Some(0x56b6c2),
            Some(0xe06c75),
            Some(0xd19a66),
            Some(0xc678dd),
            Some(0x98c379),
            Some(0xe5c07b),
            Some(0xabb2bf),
        ],
        name: "One Dark",
        hue: 0.611,
        chroma: 0.13,
        base: 0.155,
        accent: Hsla {
            h: 0.575,
            s: 0.82,
            l: 0.66,
            a: 1.0,
        },
    },
    // Its counterpart the other way up: near-white, greys that keep a trace of blue, and #4078f2
    // — a brighter accent than the light schemes above carry, which is most of what makes this
    // one recognisable as itself.
    Scheme {
        id: "one-light",
        track_palette: [
            Some(0x3671f1),
            Some(0x0182b9),
            Some(0xe03d2e),
            Some(0x986801),
            Some(0xa626a4),
            Some(0x438742),
            Some(0xa36f01),
            Some(0xca1243),
        ],
        name: "One Light",
        hue: 0.633,
        chroma: 0.06,
        base: 0.978,
        accent: Hsla {
            h: 0.611,
            s: 0.87,
            l: 0.60,
            a: 1.0,
        },
    },
    // GitHub's dark canvas, #0d1117: the deepest background here and the bluest greys, under the
    // link blue #58a6ff at full saturation.
    Scheme {
        id: "github-dark",
        track_palette: [
            Some(0x58a6ff),
            Some(0x7ee787),
            Some(0xff7b72),
            Some(0xd29922),
            Some(0xa371f7),
            Some(0x79c0ff),
            Some(0xffa657),
            Some(0xdb61a2),
        ],
        name: "GitHub Dark",
        hue: 0.597,
        chroma: 0.28,
        base: 0.071,
        accent: Hsla {
            h: 0.589,
            s: 1.0,
            l: 0.67,
            a: 1.0,
        },
    },
    // GitHub's light canvas is white, and this is a hair under it. The sunken surface is a step
    // *away* from the background, so at 1.0 there is nowhere for it to go: the timeline would be
    // cut into the window at exactly the window's colour.
    Scheme {
        id: "github-light",
        track_palette: [
            Some(0x0969da),
            Some(0x1a7f37),
            Some(0xcf222e),
            Some(0x9a6700),
            Some(0x8250df),
            Some(0x0550ae),
            Some(0xbc4c00),
            Some(0xbf3989),
        ],
        name: "GitHub Light",
        hue: 0.583,
        chroma: 0.10,
        base: 0.975,
        accent: Hsla {
            h: 0.590,
            s: 0.92,
            l: 0.445,
            a: 1.0,
        },
    },
];

/// The scheme used when nothing has been chosen, and when a stored choice names one this build
/// does not have.
pub const DEFAULT_SCHEME: &str = "midnight";

/// The scheme with this id.
pub fn scheme(id: &str) -> Option<&'static Scheme<'static>> {
    SCHEMES.iter().find(|scheme| scheme.id == id)
}

/// The scheme with this id, or the default when nothing answers to it.
///
/// Forgiving on purpose, like the keymap: the preferences file is user-editable text that outlives
/// the build that wrote it, and a scheme that has been renamed should cost the colour it was, not
/// the ability to start.
pub fn scheme_or_default(id: &str) -> &'static Scheme<'static> {
    scheme(id).unwrap_or_else(|| scheme(DEFAULT_SCHEME).expect("the default scheme exists"))
}

impl Scheme<'_> {
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
/// closed form worth writing. Eight schemes times a few hundred steps, once at start-up.
fn readable_shade(scheme: &Scheme<'_>, step: f32, ratio: f32) -> Hsla {
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

/// Keeps accent-coloured text readable even when the chosen fill matches the background.
fn readable_accent(scheme: &Scheme<'_>) -> Hsla {
    let surfaces = [
        scheme.shade(-0.020),
        scheme.shade(0.0),
        scheme.shade(0.034),
        scheme.shade(0.074),
        scheme.shade(SURFACE_HOVER_STEP),
    ];
    let mut candidate = Hsla {
        a: 1.0,
        ..scheme.accent
    };
    for _ in 0..=100 {
        if surfaces
            .iter()
            .all(|background| contrast_ratio(candidate, *background) >= 4.5)
        {
            return candidate;
        }
        candidate.l = (candidate.l + 0.01 * scheme.direction()).clamp(0.0, 1.0);
    }
    candidate
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

/// A contrasting label on an opaque fill, tinted to match the scheme's neutral colours.
fn readable_on(color: Hsla, neutral: Hsla) -> Hsla {
    let dark = hsla(neutral.h, neutral.s, 0.06, 1.0);
    let light = hsla(neutral.h, neutral.s, 0.97, 1.0);
    let (dark_ratio, light_ratio) = (contrast_ratio(dark, color), contrast_ratio(light, color));
    let best = if dark_ratio >= light_ratio {
        dark
    } else {
        light
    };
    if dark_ratio.max(light_ratio) >= 4.5 {
        return best;
    }
    // Near-black and near-white can both fall short on a mid-tone fill. At that boundary,
    // untinted black or white guarantees readable small text without changing the fill.
    let black = hsla(0.0, 0.0, 0.0, 1.0);
    let white = hsla(0.0, 0.0, 1.0, 1.0);
    if contrast_ratio(black, color) >= contrast_ratio(white, color) {
        black
    } else {
        white
    }
}

/// Localized names for the shared track and library palette slots.
pub(crate) fn palette_label(language: auris_i18n::Language, index: usize) -> String {
    use auris_i18n::{Key, t};
    let key = match index {
        0 => Key::TrackKindInstrument,
        1 => Key::TrackKindAudio,
        2 => Key::TrackKindDrum,
        3 => Key::TrackKindBus,
        4 => Key::TrackKindSinger,
        5 => Key::ThemePaletteUnassigned1,
        6 => Key::ThemePaletteUnassigned2,
        _ => Key::ThemePaletteUnassigned3,
    };
    t(key, language).to_owned()
}

/// The application colour palette.
#[derive(Clone, Debug)]
pub struct Theme {
    /// Id of the scheme this was built from, so a picker can tick the one in force.
    pub scheme: String,
    /// Interface font, including the fallbacks used for Japanese and other scripts.
    pub font: Font,
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
    /// Accent-coloured text with at least 4.5:1 contrast on standard surfaces.
    pub accent_text: Hsla,
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
    /// Resolved track, clip and library colours, indexed by the document palette.
    pub track_palette: [Hsla; 8],
}

impl gpui::Global for Theme {}

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
    pub fn from_scheme(scheme: &Scheme<'_>) -> Self {
        let accent = scheme.accent;
        let mut theme = Self {
            scheme: scheme.id.to_owned(),
            track_palette: [accent; 8],
            font: ui_font(),
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
            accent_text: readable_accent(scheme),
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
            text_on_accent: readable_on(accent, scheme.shade(0.0)),
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
        };
        theme.track_palette = std::array::from_fn(|index| {
            scheme.track_palette[index].map_or_else(
                || theme.group_color([0.0, -0.18, -0.32, -0.44, 0.23, 0.10, 0.42, -0.08][index]),
                |packed| rgb(packed).into(),
            )
        });
        theme
    }

    /// The palette named by `id`, or the default one when nothing answers to it.
    pub fn named(id: &str) -> Self {
        Self::from_scheme(scheme_or_default(id))
    }

    /// The default dark palette.
    pub fn dark() -> Self {
        Self::named(DEFAULT_SCHEME)
    }

    /// A label colour with at least 4.5:1 contrast on the opaque fill `color`.
    ///
    /// [`Theme::text_on_accent`] answers this for the accent, and is the right answer only for
    /// the accent. A track's colour is chosen by the user and a meter's by its level, so a label
    /// on either has to ask about *that* colour — a light scheme puts white on its dark accent,
    /// which is exactly the wrong thing to print on a pale yellow track. Composite translucent
    /// fills over their backdrop before asking, since their RGB values alone are not the fill.
    pub fn text_on(&self, color: Hsla) -> Hsla {
        readable_on(color, self.background)
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

    /// A group marker, with its hue offset from the current theme's accent.
    ///
    /// Offsets preserve the distinction between groups while their hues, saturation and
    /// lightness follow built-in and custom themes. Zero uses the accent's hue.
    ///
    /// Never put text in this. It is a mark beside a name, not the name — the hues that make the
    /// best labels are the ones that make the worst body text, and the two jobs cannot be done by
    /// one colour without one of them being done badly.
    pub fn group_color(&self, hue_offset: f32) -> Hsla {
        self.categorical_color(hue_offset, 1.0)
    }

    /// Shared tint for library markers and tracks, with contrast on every panel surface.
    fn categorical_color(&self, hue_offset: f32, chroma: f32) -> Hsla {
        let hue = (self.accent.h + hue_offset.rem_euclid(1.0)).rem_euclid(1.0);
        // Keep groups distinguishable even when a custom accent is grey. A track explicitly
        // stored as grey still stays neutral through its zero chroma multiplier.
        let saturation = (self.accent.s.clamp(0.25, 0.85) * chroma).clamp(0.0, 1.0);
        let toward = if self.background.l < 0.5 { 1.0 } else { -1.0 };
        let mut lightness = if toward > 0.0 {
            self.accent.l.clamp(0.50, 0.72)
        } else {
            self.accent.l.clamp(0.30, 0.48)
        };
        // Equal HSL lightness does not mean equal contrast across hues. Move towards the
        // foreground until the tint clears 3:1 on all surfaces, with a small rounding margin.
        for _ in 0..=100 {
            let candidate = hsla(hue, saturation, lightness, 1.0);
            if [
                self.background,
                self.surface_sunken,
                self.surface,
                self.surface_raised,
                self.surface_hover,
            ]
            .into_iter()
            .all(|surface| contrast_ratio(candidate, surface) >= 3.2)
            {
                return candidate;
            }
            lightness = (lightness + 0.01 * toward).clamp(0.0, 1.0);
        }
        hsla(hue, saturation, lightness, 1.0)
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

    /// Interprets a stored `0xRRGGBB` track colour in the current theme.
    ///
    /// Document palette values identify theme slots. Other RGB values from older projects
    /// retain their hue offsets from the accent. Changing a theme never rewrites a project.
    pub fn track_color(&self, packed: u32) -> Hsla {
        if let Some(index) = auris_session::prelude::Color::PALETTE
            .iter()
            .position(|color| color.0 == packed)
        {
            return self.track_palette[index];
        }
        let source: Hsla = rgb(packed).into();
        let anchor: Hsla = rgb(auris_session::prelude::Color::PALETTE[0].0).into();
        self.categorical_color(source.h - anchor.h, source.s / anchor.s)
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
    /// Height of the transport readout bar below the title bar.
    ///
    /// The playback buttons live in the title bar; this row holds their readouts and meters.
    pub const TRANSPORT_HEIGHT: Pixels = px(54.0);
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
                contrast_ratio(theme.text_on_accent, theme.accent) >= 4.5,
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
    fn note_labels_remain_readable_across_every_velocity_and_scheme() {
        for entry in SCHEMES {
            let theme = Theme::from_scheme(entry);
            for velocity in 0..=127 {
                let fill = theme.velocity_color(velocity as f32 / 127.0);
                let ratio = contrast_ratio(theme.text_on(fill), fill);
                assert!(
                    ratio >= 4.5,
                    "{}: velocity {velocity} has only {ratio:.2}:1 label contrast",
                    entry.name
                );
            }
        }
    }

    #[test]
    fn custom_accents_keep_their_fill_and_readable_text_on_every_surface() {
        for base in SCHEMES {
            for accent in [
                base.accent,
                base.shade(0.0),
                rgb(0x000000).into(),
                rgb(0xffffff).into(),
                rgb(0xff0000).into(),
                rgb(0x00ff00).into(),
                rgb(0x0000ff).into(),
            ] {
                let theme = Theme::from_scheme(&Scheme { accent, ..*base });
                assert_eq!(theme.accent, accent, "the chosen fill is retained");
                for (name, background) in [
                    ("background", theme.background),
                    ("surface", theme.surface),
                    ("raised", theme.surface_raised),
                    ("sunken", theme.surface_sunken),
                    ("hover", theme.surface_hover),
                ] {
                    let ratio = contrast_ratio(theme.accent_text, background);
                    assert!(
                        ratio >= 4.5,
                        "{}: accent {accent:?} text on {name} has only {ratio:.2}:1 contrast",
                        base.name,
                    );
                }
                assert!(contrast_ratio(theme.text_on_accent, theme.accent) >= 4.5);
            }
        }
    }

    #[test]
    fn saturated_yellow_and_blue_need_different_label_colours_at_the_same_lightness() {
        let theme = Theme::named("daylight");
        let yellow = hsla(1.0 / 6.0, 1.0, 0.5, 1.0);
        let blue = hsla(2.0 / 3.0, 1.0, 0.5, 1.0);
        assert!(theme.text_on(yellow).l < 0.5);
        assert!(theme.text_on(blue).l > 0.5);
        assert!(contrast_ratio(theme.text_on(yellow), yellow) >= 4.5);
        assert!(contrast_ratio(theme.text_on(blue), blue) >= 4.5);
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
    fn track_tints_stay_distinct_and_readable_in_every_scheme() {
        use auris_session::prelude::Color;

        for entry in SCHEMES {
            let theme = Theme::from_scheme(entry);
            let colors = Color::PALETTE.map(|color| theme.track_color(color.0));
            for (index, color) in colors.iter().enumerate() {
                assert!(
                    !colors[..index].contains(color),
                    "{}: duplicate tint",
                    entry.name
                );
                for surface in [
                    theme.surface_sunken,
                    theme.background,
                    theme.surface,
                    theme.surface_raised,
                    theme.surface_hover,
                ] {
                    assert!(contrast_ratio(*color, surface) >= 3.0, "{}", entry.name);
                }
                assert!(contrast_ratio(theme.text_on(*color), *color) >= 4.5);
            }
            for neutral in [0x000000, 0x888888, 0xffffff] {
                assert_eq!(theme.track_color(neutral).s, 0.0);
            }
        }
    }

    #[test]
    fn categorical_tints_follow_custom_accents_without_changing_their_spacing() {
        // Only blank custom slots follow the accent. Presets declare their own colours.
        let base = Scheme {
            track_palette: [None; 8],
            ..*scheme_or_default(DEFAULT_SCHEME)
        };
        let before = Theme::from_scheme(&base);
        let after = Theme::from_scheme(&Scheme {
            accent: hsla((base.accent.h + 0.25).rem_euclid(1.0), 0.40, 0.70, 1.0),
            ..base
        });
        for packed in auris_session::prelude::Color::PALETTE {
            let old = before.track_color(packed.0);
            let new = after.track_color(packed.0);
            assert!(((new.h - old.h).rem_euclid(1.0) - 0.25).abs() < 0.0001);
            assert!(new.s < old.s);
        }
        let old = before.group_color(0.0);
        let new = after.group_color(0.0);
        assert_ne!(old.h, new.h);
        assert_ne!(old.s, new.s);
        assert_ne!(old.l, new.l);
    }

    #[test]
    fn each_preset_declares_its_own_palette_independently_of_the_accent() {
        for (index, scheme) in SCHEMES.iter().enumerate() {
            assert!(
                scheme.track_palette.iter().all(Option::is_some),
                "{}",
                scheme.id
            );
            let theme = Theme::from_scheme(scheme);
            for other in &SCHEMES[..index] {
                assert_ne!(theme.track_palette, Theme::from_scheme(other).track_palette);
            }
            let changed_accent = Theme::from_scheme(&Scheme {
                accent: rgb(0xff00ff).into(),
                ..*scheme
            });
            assert_eq!(theme.track_palette, changed_accent.track_palette);
            for (actual, stored) in theme.track_palette.iter().zip(scheme.track_palette) {
                assert_eq!(*actual, Hsla::from(rgb(stored.unwrap())));
            }
        }
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
