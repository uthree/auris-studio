//! Colours and metrics for the whole UI.
//!
//! Everything visual reads from one [`Theme`] value, and every [`Theme`] is derived from a
//! [`Scheme`] — neutral surface parameters, an accent and optional categorical colours.
//! Retuning the palette, or adding a light one, is
//! therefore a matter of naming a scheme rather than of hunting hex literals through view code,
//! and the tests below can check every scheme against the same rules at once.

use gpui::{Font, FontFallbacks, Hsla, Pixels, hsla, px, rgb};

/// An interior velocity-gradient control point, stored independently of the UI toolkit.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GradientStop {
    /// Position between the soft (zero) and loud (one) endpoints, exclusive.
    pub position: f32,
    /// Opaque colour packed as `0xRRGGBB`.
    pub color: u32,
}

/// The font every panel draws in, with fallbacks for scripts the base family has no glyphs for.
///
/// The base family is each platform's own interface font, named rather than left to a
/// substitution table: asking Windows for Helvetica gets you Arial through a mapping written in
/// the nineties, which is close enough to look like nothing went wrong and far enough to look
/// unlike every other window on the screen.
///
/// None of the three covers Japanese, so a Japanese track name or menu would come out as empty
/// boxes without the fallbacks. Only list families for the current platform: Windows logs an
/// error for every missing family while building its DirectWrite fallback mappings.
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
    let fallbacks: &[&str] = if cfg!(target_os = "macos") {
        &["Hiragino Sans", "Apple SD Gothic Neo"]
    } else if cfg!(target_os = "windows") {
        &["Segoe UI", "Yu Gothic UI", "Meiryo"]
    } else {
        &["Noto Sans CJK JP", "DejaVu Sans"]
    };
    Font {
        fallbacks: Some(FontFallbacks::from_fonts(
            fallbacks.iter().map(|family| (*family).into()).collect(),
        )),
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
    /// Piano-roll gradient endpoints: soft then loud; blank entries use instrument/drum slots.
    pub velocity_palette: [Option<u32>; 2],
    /// Interior control points, ordered by increasing position.
    pub velocity_stops: &'a [GradientStop],
    /// Optional RGB colours for active, warning, danger and mute indicators.
    pub signal_palette: [Option<u32>; 4],
    /// Optional I–VII chord colours; blank entries follow the first seven track palette slots.
    pub chord_palette: [Option<u32>; 7],
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
        chord_palette: [None; 7],
        signal_palette: [
            Some(0x5fc9a3),
            Some(0xe0b452),
            Some(0xd97b6c),
            Some(0xe0a458),
        ],
        velocity_stops: &[
            GradientStop {
                position: 0.4,
                color: 0x5fc9a3,
            },
            GradientStop {
                position: 0.75,
                color: 0xe0b452,
            },
        ],
        velocity_palette: [Some(0x4f9dde), Some(0xd97b6c)],
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
        chord_palette: [None; 7],
        signal_palette: [
            Some(0x8eafa0),
            Some(0xb6ad79),
            Some(0xce8176),
            Some(0xdd9f60),
        ],
        velocity_stops: &[
            GradientStop {
                position: 0.4,
                color: 0xa8b78b,
            },
            GradientStop {
                position: 0.75,
                color: 0xb6ad79,
            },
        ],
        velocity_palette: [Some(0x8eafa0), Some(0xce8176)],
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
        chord_palette: [None; 7],
        signal_palette: [
            Some(0x267e70),
            Some(0x946c20),
            Some(0xb4474e),
            Some(0xa95b28),
        ],
        velocity_stops: &[
            GradientStop {
                position: 0.4,
                color: 0x267e70,
            },
            GradientStop {
                position: 0.75,
                color: 0x946c20,
            },
        ],
        velocity_palette: [Some(0x246caa), Some(0xb4474e)],
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
        chord_palette: [None; 7],
        signal_palette: [
            Some(0x26766d),
            Some(0x8a651c),
            Some(0xa24b3e),
            Some(0xa05e32),
        ],
        velocity_stops: &[
            GradientStop {
                position: 0.4,
                color: 0x647849,
            },
            GradientStop {
                position: 0.75,
                color: 0x8a651c,
            },
        ],
        velocity_palette: [Some(0x416d87), Some(0xa24b3e)],
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
        chord_palette: [None; 7],
        signal_palette: [
            Some(0x98c379),
            Some(0xe5c07b),
            Some(0xe06c75),
            Some(0xd19a66),
        ],
        velocity_stops: &[
            GradientStop {
                position: 0.4,
                color: 0x98c379,
            },
            GradientStop {
                position: 0.75,
                color: 0xe5c07b,
            },
        ],
        velocity_palette: [Some(0x61afef), Some(0xe06c75)],
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
        chord_palette: [None; 7],
        signal_palette: [
            Some(0x438742),
            Some(0x986801),
            Some(0xe03d2e),
            Some(0xa36f01),
        ],
        velocity_stops: &[
            GradientStop {
                position: 0.4,
                color: 0x438742,
            },
            GradientStop {
                position: 0.75,
                color: 0x986801,
            },
        ],
        velocity_palette: [Some(0x3671f1), Some(0xe03d2e)],
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
        chord_palette: [None; 7],
        signal_palette: [
            Some(0x58a6ff),
            Some(0xd29922),
            Some(0xff7b72),
            Some(0xffa657),
        ],
        velocity_stops: &[GradientStop {
            position: 0.5,
            color: 0xa371f7,
        }],
        velocity_palette: [Some(0x58a6ff), Some(0xff7b72)],
        track_palette: [
            Some(0x58a6ff),
            Some(0xd2a8ff),
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
        chord_palette: [None; 7],
        signal_palette: [
            Some(0x0969da),
            Some(0x9a6700),
            Some(0xcf222e),
            Some(0xbc4c00),
        ],
        velocity_stops: &[GradientStop {
            position: 0.5,
            color: 0x8250df,
        }],
        velocity_palette: [Some(0x0969da), Some(0xcf222e)],
        track_palette: [
            Some(0x0969da),
            Some(0x6639ba),
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
    /// Translucent recessed surface over the portion of a clip removed by a fade.
    pub clip_fade_scrim: Hsla,
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
    /// Shares the danger palette colour used for clipping and errors.
    pub record: Hsla,
    /// White keys in the piano roll keyboard.
    pub key_white: Hsla,
    /// Black keys in the piano roll keyboard.
    pub key_black: Hsla,
    /// Lane background behind a black key's row.
    pub key_row_black: Hsla,
    /// Soft endpoint before lane-contrast adjustment. See [`Theme::velocity_color`].
    pub velocity_soft: Hsla,
    /// Loud endpoint before lane-contrast adjustment.
    pub velocity_loud: Hsla,
    /// Resolved interior gradient control points, ordered by position.
    pub velocity_stops: Vec<(f32, Hsla)>,
    /// Resolved track, clip and library colours, indexed by the document palette.
    pub track_palette: [Hsla; 8],
    /// Resolved active, warning, danger and mute colours used throughout the interface.
    pub signal_palette: [Hsla; 4],
    /// Resolved chord-degree colours in I–VII order, readable on panel surfaces.
    pub chord_palette: [Hsla; 7],
}

impl gpui::Global for Theme {}

/// Preserves a palette hue while making text and indicators readable on hovered controls.
fn readable_indicator(mut color: Hsla, theme: &Theme) -> Hsla {
    let step = if theme.background.l < 0.5 {
        0.01
    } else {
        -0.01
    };
    for _ in 0..=100 {
        if contrast_ratio(color, theme.surface_hover) >= 4.5 {
            break;
        }
        color.l = (color.l + step).clamp(0.0, 1.0);
    }
    color
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
    pub fn from_scheme(scheme: &Scheme<'_>) -> Self {
        let accent = scheme.accent;
        let mut theme = Self {
            scheme: scheme.id.to_owned(),
            track_palette: [accent; 8],
            signal_palette: [accent; 4],
            chord_palette: [accent; 7],
            font: ui_font(),
            surface_sunken: scheme.shade(-0.020),
            clip_fade_scrim: Hsla {
                a: 0.32,
                ..scheme.shade(-0.020)
            },
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
            // Resolved from the signal palette below, after categorical fallbacks are available.
            playing: accent,
            playhead: accent,
            danger: accent,
            warning: accent,
            meter_low: accent,
            meter_mid: accent,
            meter_high: accent,
            solo: accent,
            mute: accent,
            record: accent,
            // The keyboard strip stays a keyboard in every scheme. Deriving these from the ramp
            // would give a light scheme white "black" keys, which is not a stylistic choice but a
            // piano nobody can read.
            key_white: hsla(scheme.hue, scheme.chroma * 0.4, 0.86, 1.0),
            key_black: hsla(scheme.hue, scheme.chroma, 0.18, 1.0),
            // Resolved below, once the palette is available for blank custom endpoints.
            velocity_soft: accent,
            velocity_loud: accent,
            velocity_stops: scheme
                .velocity_stops
                .iter()
                .map(|stop| (stop.position, rgb(stop.color).into()))
                .collect(),
        };
        theme.track_palette = std::array::from_fn(|index| {
            scheme.track_palette[index].map_or_else(
                || theme.group_color([0.0, -0.18, -0.32, -0.44, 0.23, 0.10, 0.42, -0.08][index]),
                |packed| rgb(packed).into(),
            )
        });
        theme.chord_palette = std::array::from_fn(|index| {
            let color = scheme.chord_palette[index]
                .map(|packed| rgb(packed).into())
                .unwrap_or(theme.track_palette[index]);
            readable_indicator(color, &theme)
        });
        let fallbacks = [
            accent,
            theme.track_palette[3],
            theme.track_palette[2],
            theme.track_palette[3],
        ];
        theme.signal_palette = std::array::from_fn(|index| {
            let color = scheme.signal_palette[index]
                .map(|packed| rgb(packed).into())
                .unwrap_or(fallbacks[index]);
            readable_indicator(color, &theme)
        });
        [theme.playing, theme.warning, theme.danger, theme.mute] = theme.signal_palette;
        theme.meter_low = theme.playing;
        theme.meter_mid = theme.warning;
        theme.meter_high = theme.danger;
        theme.record = theme.danger;
        theme.playhead = theme.danger;
        theme.solo = theme.warning;
        theme.velocity_soft = scheme.velocity_palette[0]
            .map(|packed| rgb(packed).into())
            .unwrap_or(theme.track_palette[0]);
        theme.velocity_loud = scheme.velocity_palette[1]
            .map(|packed| rgb(packed).into())
            .unwrap_or(theme.track_palette[2]);
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

    /// Opaque note fill; selection reverses luminance instead of relying on hue alone.
    pub fn note_fill(&self, velocity: f32, selected: bool) -> Hsla {
        let color = self.velocity_color(velocity);
        if selected { self.text_on(color) } else { color }
    }

    /// Colour of the primary written degree (one-based), independent of key and chord quality.
    pub fn chord_color(&self, degree: u8) -> Hsla {
        self.chord_palette[(degree.clamp(1, 7) - 1) as usize]
    }

    /// Opaque chord-block tint, composited on the harmony lane for readable label selection.
    pub fn chord_fill(&self, degree: u8, held: bool) -> Hsla {
        let color: gpui::Rgba = self.chord_color(degree).into();
        let background: gpui::Rgba = self.surface.into();
        let amount = if held { 0.42 } else { 0.22 };
        gpui::Rgba {
            r: background.r + (color.r - background.r) * amount,
            g: background.g + (color.g - background.g) * amount,
            b: background.b + (color.b - background.b) * amount,
            a: 1.0,
        }
        .into()
    }

    /// Colour of a note struck at `velocity`, from softest to hardest.
    ///
    /// Logic colours a note by how hard it was struck, and the reason is that no amount of
    /// staring at a grid of identical rectangles says where the dynamics are. Brightness alone
    /// was tried here first and is not enough: velocity 0.8 and velocity 1.0 differ by a few per
    /// cent of lightness, which is invisible next to a note an octave away on a different row.
    ///
    /// Adjacent control points interpolate along the shortest hue arc, with linear saturation
    /// and lightness. Presets choose their intermediate colours explicitly.
    /// Lightness is then adjusted to keep the fill visible against both piano-roll lane shades.
    pub fn velocity_color(&self, velocity: f32) -> Hsla {
        let position = velocity.clamp(0.0, 1.0);
        let mut left = (0.0, self.velocity_soft);
        let mut right = (1.0, self.velocity_loud);
        for &(at, color) in &self.velocity_stops {
            if at >= position {
                right = (at, color);
                break;
            }
            left = (at, color);
        }
        let amount = (position - left.0) / (right.0 - left.0);
        let (mut soft, mut loud) = (left.1, right.1);
        // Achromatic stops borrow their neighbour's hue instead of introducing a red detour.
        if soft.s < 1e-5 {
            soft.h = loud.h;
        }
        if loud.s < 1e-5 {
            loud.h = soft.h;
        }
        let between = |from: f32, to: f32| from + (to - from) * amount;
        let mut color = Hsla {
            h: (soft.h + ((loud.h - soft.h + 0.5).rem_euclid(1.0) - 0.5) * amount).rem_euclid(1.0),
            s: between(soft.s, loud.s),
            l: between(soft.l, loud.l),
            a: between(soft.a, loud.a),
        };
        let step = if self.background.l < 0.5 { 0.01 } else { -0.01 };
        for _ in 0..=100 {
            if [self.surface_sunken, self.key_row_black]
                .into_iter()
                .all(|lane| contrast_ratio(color, lane) >= 3.0)
            {
                break;
            }
            color.l = (color.l + step).clamp(0.0, 1.0);
        }
        color
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
    fn clip_fade_scrim_follows_the_recessed_surface_in_every_scheme() {
        for entry in SCHEMES {
            let theme = Theme::from_scheme(entry);
            assert_eq!(
                theme.clip_fade_scrim,
                Theme::translucent(theme.surface_sunken, 0.32),
                "{}",
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
        let same = |left: Hsla, right: Hsla| {
            (left.h - right.h).abs() < 1e-4
                && (left.s - right.s).abs() < 1e-4
                && (left.l - right.l).abs() < 1e-4
                && (left.a - right.a).abs() < 1e-4
        };
        let mut ramps = Vec::new();
        for scheme in SCHEMES {
            let theme = Theme::from_scheme(scheme);
            assert!(same(theme.velocity_color(0.0), theme.velocity_soft));
            assert!(same(theme.velocity_color(1.0), theme.velocity_loud));
            // Out of range is clamped rather than extrapolated off the end of the ramp.
            assert!(same(theme.velocity_color(-1.0), theme.velocity_soft));
            assert!(same(theme.velocity_color(9.0), theme.velocity_loud));

            // The ramp must visit its declared intermediate hues without discontinuities.
            for stop in scheme.velocity_stops {
                let expected: Hsla = rgb(stop.color).into();
                assert!((theme.velocity_color(stop.position).h - expected.h).abs() < 1e-4);
            }
            let hues: Vec<f32> = (0..=10)
                .map(|step| theme.velocity_color(step as f32 / 10.0).h)
                .collect();
            for pair in hues.windows(2) {
                let step = ((pair[0] - pair[1] + 0.5).rem_euclid(1.0) - 0.5).abs();
                assert!(
                    step > 0.0 && step < 0.15,
                    "{}: the ramp jumps or stops changing: {hues:?}",
                    scheme.id
                );
            }
            let ramp =
                std::array::from_fn::<_, 11, _>(|step| theme.velocity_color(step as f32 / 10.0));
            assert!(
                !ramps.contains(&ramp),
                "{} shares another theme's gradient",
                scheme.id
            );
            ramps.push(ramp);
        }
    }

    #[test]
    fn gradient_segments_follow_positions_and_take_the_short_hue_arc() {
        let mut custom = crate::appearance::CustomScheme::from_scheme(
            "test-gradient".into(),
            "Test gradient".into(),
            &SCHEMES[0],
        );
        custom.velocity_palette = [Some(0xff0000), Some(0xff00ff)];
        custom.velocity_stops = vec![
            GradientStop {
                position: 0.25,
                color: 0xffff00,
            },
            GradientStop {
                position: 0.75,
                color: 0x00ffff,
            },
        ];
        let theme = Theme::from_scheme(&custom.definition());
        for (position, hue) in [
            (0.125, 1.0 / 12.0),
            (0.25, 1.0 / 6.0),
            (0.5, 1.0 / 3.0),
            (0.75, 0.5),
        ] {
            assert!((theme.velocity_color(position).h - hue).abs() < 1e-4);
        }
        custom.velocity_stops.clear();
        let theme = Theme::from_scheme(&custom.definition());
        assert!((theme.velocity_color(0.5).h - 11.0 / 12.0).abs() < 1e-4);
        // A grey endpoint must not force an otherwise blue gradient through red.
        custom.velocity_palette = [Some(0xaaaaaa), Some(0x0000ff)];
        let theme = Theme::from_scheme(&custom.definition());
        assert!((theme.velocity_color(0.5).h - 2.0 / 3.0).abs() < 1e-4);
    }

    #[test]
    fn signal_palettes_follow_presets_and_stay_readable_on_every_surface() {
        let mut palettes = Vec::new();
        for scheme in SCHEMES {
            let theme = Theme::from_scheme(scheme);
            assert!(
                !palettes.contains(&theme.signal_palette),
                "{} shares another preset's indicators",
                scheme.id
            );
            palettes.push(theme.signal_palette);
            for (index, color) in theme.signal_palette.into_iter().enumerate() {
                let declared: Hsla = rgb(scheme.signal_palette[index].unwrap()).into();
                assert!((color.h - declared.h).abs() < 1e-4);
                assert!((color.s - declared.s).abs() < 1e-4);
                for background in [
                    theme.background,
                    theme.surface_sunken,
                    theme.surface,
                    theme.surface_raised,
                    theme.surface_hover,
                ] {
                    assert!(contrast_ratio(color, background) >= 4.5);
                }
                assert!(contrast_ratio(theme.text_on(color), color) >= 4.5);
            }
            assert_eq!(theme.playing, theme.signal_palette[0]);
            assert_eq!(theme.meter_color(-20.0), theme.playing);
            assert_eq!(theme.meter_color(-6.0), theme.warning);
            assert_eq!(theme.meter_color(-1.0), theme.danger);
            assert_eq!(theme.record, theme.danger);
            assert_eq!(theme.playhead, theme.danger);
            assert_eq!(theme.solo, theme.warning);
            assert_eq!(theme.mute, theme.signal_palette[3]);
        }
    }

    #[test]
    fn custom_signal_colours_and_blank_fallbacks_keep_their_hues() {
        for base in SCHEMES {
            let mut custom = crate::appearance::CustomScheme::from_scheme(
                "signals".into(),
                "Signals".into(),
                base,
            );
            custom.signal_palette = [None; 4];
            custom.accent = 0xcc3377;
            custom.track_palette[3] = Some(0x663399);
            custom.track_palette[2] = Some(0x3377cc);
            let theme = Theme::from_scheme(&custom.definition());
            for (color, packed) in
                theme
                    .signal_palette
                    .into_iter()
                    .zip([custom.accent, 0x663399, 0x3377cc, 0x663399])
            {
                let source: Hsla = rgb(packed).into();
                assert!((color.h - source.h).abs() < 1e-4);
            }
            for packed in [0x000000, 0xffffff, 0xffff00, 0x0000ff] {
                custom.signal_palette = [Some(packed); 4];
                let theme = Theme::from_scheme(&custom.definition());
                for color in theme.signal_palette {
                    assert!(contrast_ratio(color, theme.surface_hover) >= 4.5);
                    assert!(contrast_ratio(theme.text_on(color), color) >= 4.5);
                }
            }
        }
    }

    #[test]
    fn chord_degrees_have_distinct_theme_colours_and_readable_labels() {
        use auris_session::prelude::Numeral;
        let mut palettes = Vec::new();
        for scheme in SCHEMES {
            let theme = Theme::from_scheme(scheme);
            assert!(!palettes.contains(&theme.chord_palette));
            palettes.push(theme.chord_palette);
            for degree in 1..=7 {
                let color = theme.chord_color(degree);
                assert!(!theme.chord_palette[..(degree - 1) as usize].contains(&color));
                for surface in [theme.surface_sunken, theme.surface, theme.surface_hover] {
                    assert!(contrast_ratio(color, surface) >= 4.5);
                }
                for held in [false, true] {
                    let fill = theme.chord_fill(degree, held);
                    assert_eq!(fill.a, 1.0);
                    assert!(contrast_ratio(theme.text_on(fill), fill) >= 4.5);
                }
                assert_ne!(
                    theme.chord_fill(degree, false),
                    theme.chord_fill(degree, true)
                );
            }
            for (text, degree) in [
                ("Imaj7", 1),
                ("ii7", 2),
                ("bIII", 3),
                ("iv", 4),
                ("V7/V", 5),
                ("vi/3", 6),
                ("bVII7", 7),
            ] {
                let numeral = Numeral::parse(text).unwrap();
                assert_eq!(theme.chord_color(numeral.degree), theme.chord_color(degree));
            }
            let mut custom = crate::appearance::CustomScheme::from_scheme(
                "chords".into(),
                "Chords".into(),
                scheme,
            );
            custom.chord_palette[4] = Some(0xaa33cc);
            custom.track_palette[0] = Some(0xcc3377);
            let edited = Theme::from_scheme(&custom.definition());
            assert_ne!(edited.chord_color(5), theme.chord_color(5));
            assert_ne!(edited.chord_color(1), theme.chord_color(1));
            let expected: Hsla = rgb(0xaa33cc).into();
            assert!((edited.chord_color(5).h - expected.h).abs() < 1e-4);
            assert_eq!(edited.chord_color(2), theme.chord_color(2));
        }
    }

    #[test]
    fn github_presets_and_their_custom_copies_do_not_use_green() {
        for scheme in SCHEMES
            .iter()
            .filter(|scheme| scheme.id.starts_with("github-"))
        {
            let custom = crate::appearance::CustomScheme::from_scheme(
                "test-copy".into(),
                "Test copy".into(),
                scheme,
            );
            for theme in [
                Theme::from_scheme(scheme),
                Theme::from_scheme(&custom.definition()),
            ] {
                for color in theme
                    .track_palette
                    .into_iter()
                    .chain(theme.chord_palette)
                    .chain([
                        theme.playing,
                        theme.meter_low,
                        theme.meter_mid,
                        theme.meter_high,
                    ])
                    .chain((0..=127).map(|velocity| theme.velocity_color(velocity as f32 / 127.0)))
                {
                    assert!(
                        !(0.18..0.49).contains(&color.h),
                        "{} contains green: {color:?}",
                        scheme.id
                    );
                }
            }
        }
    }

    #[test]
    fn selected_notes_and_their_labels_contrast_in_every_scheme() {
        for scheme in SCHEMES {
            let theme = Theme::from_scheme(scheme);
            for velocity in 0..=127 {
                let velocity = velocity as f32 / 127.0;
                let selected = theme.note_fill(velocity, true);
                assert!(
                    contrast_ratio(selected, theme.note_fill(velocity, false)) >= 4.5,
                    "{}: selection must change luminance",
                    scheme.name
                );
                assert!(
                    contrast_ratio(theme.text_on(selected), selected) >= 4.5,
                    "{}: selected lyrics must remain readable",
                    scheme.name
                );
            }
        }
    }

    #[test]
    fn note_labels_remain_readable_across_every_velocity_and_scheme() {
        for entry in SCHEMES {
            let theme = Theme::from_scheme(entry);
            for velocity in 0..=127 {
                let fill = theme.velocity_color(velocity as f32 / 127.0);
                for lane in [theme.surface_sunken, theme.key_row_black] {
                    assert!(
                        contrast_ratio(fill, lane) >= 3.0,
                        "{}: velocity {velocity}",
                        entry.name
                    );
                }
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
            assert!(scheme.velocity_palette.iter().all(Option::is_some));
            let theme = Theme::from_scheme(scheme);
            for other in &SCHEMES[..index] {
                assert_ne!(theme.track_palette, Theme::from_scheme(other).track_palette);
            }
            let changed_accent = Theme::from_scheme(&Scheme {
                accent: rgb(0xff00ff).into(),
                ..*scheme
            });
            assert_eq!(theme.track_palette, changed_accent.track_palette);
            assert_eq!(theme.velocity_soft, changed_accent.velocity_soft);
            assert_eq!(theme.velocity_loud, changed_accent.velocity_loud);
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
    }

    #[test]
    fn fallback_families_belong_to_the_current_platform() {
        let expected: &[&str] = if cfg!(target_os = "macos") {
            &["Hiragino Sans", "Apple SD Gothic Neo"]
        } else if cfg!(target_os = "windows") {
            &["Segoe UI", "Yu Gothic UI", "Meiryo"]
        } else {
            &["Noto Sans CJK JP", "DejaVu Sans"]
        };
        // Custom interface families must use the same platform-specific CJK fallbacks.
        for font in [ui_font(), ui_font_for(Some("Custom Interface"))] {
            let fallbacks = font
                .fallbacks
                .as_ref()
                .expect("CJK fallbacks are configured");
            let families: Vec<_> = fallbacks
                .fallback_list()
                .iter()
                .map(String::as_str)
                .collect();
            assert_eq!(families, expected);
        }
    }
}
