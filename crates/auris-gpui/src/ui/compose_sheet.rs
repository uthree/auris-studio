//! The song sheet: a whole piece asked for with dials rather than with a file.
//!
//! Everything in this half of the module is a decision, not a picture. A gpui view cannot be
//! unit-tested, so what the sheet *decides* — these dials to a [`SongSpec`] — lives in free
//! functions with tests, and the view does nothing but draw them and hand back what was moved.
//!
//! The sheet and `.asong` are two faces of one type. [`song_spec`] builds the specification, and
//! the specification is what writes the piece — so there is no second implementation of what a
//! dial means, and the round trip through [`SongSpec::to_toml`] is a test they share.

use auris_i18n::Key;
use auris_session::prelude::*;

/// How many bars a section may be asked for, either way.
///
/// Four is a phrase; thirty-two is four of them, which is longer than any one section of a song
/// that has more than one. The dial covers what the form is for and nothing beyond it.
pub const BARS: std::ops::RangeInclusive<usize> = 4..=32;

/// The tempo range the specification accepts, which is what the dial has to cover.
pub const TEMPO: std::ops::RangeInclusive<f64> = 40.0..=220.0;

/// Straight to as far as a swing dial goes, in the percentage a shuffle is written in.
pub const SWING: std::ops::RangeInclusive<u8> = 50..=75;

/// How far a part's level trim reaches, in decibels.
pub const GAIN_DB: std::ops::RangeInclusive<f32> = -30.0..=0.0;

/// How far a drag travels before a dial has been turned end to end.
const DRAG_RANGE_PIXELS: f32 = 220.0;

/// Everything the sheet is set to.
///
/// The specification's own fields, held one for one, so that reading the sheet is reading the
/// document. What it does *not* hold is the form: sections and their order are `.asong`'s job
/// until the generator can invent one, and until then the sheet sets how long a section is and
/// the default six-section shape carries the rest.
#[derive(Clone, Debug, PartialEq)]
pub struct SongDials {
    /// What the piece is called, and what the project is named after.
    pub title: String,
    /// The key everything is measured from.
    pub key: MusicalKey,
    /// Beats per minute.
    pub tempo: f64,
    /// The time signature.
    pub meter: TimeSignature,
    /// How the piece should feel.
    pub mood: Mood,
    /// How many bars one section lasts.
    pub bars: usize,
    /// The catalogue progression, by name and without its `@`. Empty means the composer's own.
    pub chords: String,
    /// The drum groove.
    pub groove: String,
    /// The seed every random decision is drawn from.
    pub seed: u64,
    /// How much the offbeats are delayed, as a percentage where 50 is straight.
    pub swing: u8,
    /// How far timing and velocity wander.
    pub humanize: f32,
    /// How far apart the hardest and softest notes are struck.
    pub dynamics: f32,
    /// How much of a section's last bar the snare runs as a fill.
    pub fill: f32,
    /// How much a repeat departs from what came before it.
    pub variation: f32,
    /// The roster, in the order the tracks are created.
    pub parts: Vec<PartSpec>,
}

impl Default for SongDials {
    fn default() -> Self {
        // From the specification's own defaults rather than from a second list of numbers: the
        // sheet opens on the song `SongSpec::default()` describes, and there is one place to
        // change what that is.
        let spec = SongSpec::default();
        Self {
            title: spec.title.clone(),
            key: spec.key,
            tempo: spec.tempo,
            meter: spec.meter,
            mood: spec.mood,
            bars: spec
                .sections
                .values()
                .next()
                .map_or(8, |section| section.bars),
            chords: String::new(),
            groove: spec.groove.clone(),
            seed: spec.seed,
            swing: spec.swing,
            humanize: spec.humanize,
            dynamics: spec.dynamics,
            fill: spec.fill,
            variation: spec.variation,
            parts: spec.parts.clone(),
        }
    }
}

/// The specification these dials describe.
///
/// The one place the sheet turns into a song. Everything downstream — writing the piece, saving
/// the `.asong`, refilling the sheet from one — goes through the specification, so a dial cannot
/// mean one thing to the composer and another to the file.
pub fn song_spec(dials: &SongDials) -> SongSpec {
    let mut spec = SongSpec {
        title: dials.title.clone(),
        key: dials.key,
        tempo: dials.tempo,
        meter: dials.meter,
        mood: dials.mood,
        seed: dials.seed,
        swing: dials.swing,
        humanize: dials.humanize,
        dynamics: dials.dynamics,
        fill: dials.fill,
        variation: dials.variation,
        groove: dials.groove.clone(),
        parts: dials.parts.clone(),
        ..SongSpec::default()
    };
    for section in spec.sections.values_mut() {
        section.bars = dials.bars.clamp(*BARS.start(), *BARS.end());
    }
    // An empty name leaves the default in place, and the default is marked as the composer's
    // own — so "no progression chosen" is how the mood gets to colour one, rather than a hole.
    if let Some(chart) = chart_named(&dials.chords) {
        spec.charts.insert("main".to_string(), chart);
    }
    spec
}

/// The catalogue chart a name asks for, or `None` for the composer's own.
pub fn chart_named(name: &str) -> Option<Chart> {
    let name = name.trim();
    (!name.is_empty()).then(|| Chart::parse(&format!("@{name}")))?
}

/// The same song, next take.
///
/// The next seed rather than a random one, for the reason a generated clip's seed is shown and
/// typeable: a take somebody liked has to be reachable again, and it is only reachable by
/// somebody who can count back to it.
pub fn another_take(dials: &mut SongDials) {
    dials.seed = dials.seed.wrapping_add(1);
}

/// A name no part in the roster is using yet.
///
/// `part`, `part 2`, `part 3`: a duplicate name would be refused by the format, and a sheet that
/// added a row the document then rejected would be a button that breaks the song.
pub fn unused_part_name(dials: &SongDials, stem: &str) -> String {
    if !dials.parts.iter().any(|part| part.name == stem) {
        return stem.to_string();
    }
    (2..)
        .map(|n| format!("{stem} {n}"))
        .find(|name| !dials.parts.iter().any(|part| &part.name == name))
        .unwrap_or_else(|| stem.to_string())
}

/// Adds a part of `role`, named after it.
pub fn add_part(dials: &mut SongDials, role: Role) {
    let name = unused_part_name(dials, role.name());
    dials.parts.push(PartSpec::of_role(name, role));
}

/// Removes the part at `index`, if the roster would still have one.
///
/// A song with no parts writes no notes, and a sheet whose Write button produces an empty
/// document is a sheet with a broken state reachable from it.
pub fn remove_part(dials: &mut SongDials, index: usize) -> bool {
    if dials.parts.len() <= 1 || index >= dials.parts.len() {
        return false;
    }
    dials.parts.remove(index);
    true
}

/// The mood word these four numbers are exactly, if any.
///
/// A mood the dials have been nudged away from is no word at all, and the picker says so. Naming
/// the word it started at would be the one caption that is reliably wrong.
pub fn mood_word(mood: Mood) -> Option<&'static str> {
    Mood::NAMES
        .into_iter()
        .find(|name| Mood::named(name) == Some(mood))
}

/// The interface's name for a mood word.
pub fn mood_key(name: &str) -> Key {
    match name {
        "bright" => Key::MoodBright,
        "dark" => Key::MoodDark,
        "calm" => Key::MoodCalm,
        "driving" => Key::MoodDriving,
        "epic" => Key::MoodEpic,
        "dreamy" => Key::MoodDreamy,
        "tense" => Key::MoodTense,
        "funky" => Key::MoodFunky,
        _ => Key::MoodNeutral,
    }
}

/// The interface's name for a part's role.
pub fn role_key(role: Role) -> Key {
    match role {
        Role::Melody => Key::RoleMelody,
        Role::Chords => Key::RoleChords,
        Role::Pad => Key::RolePad,
        Role::Arp => Key::RoleArp,
        Role::Stab => Key::RoleStab,
        Role::Bass => Key::RoleBass,
        Role::Kick => Key::RoleKick,
        Role::Snare => Key::RoleSnare,
        Role::Hat => Key::RoleHat,
    }
}

/// One continuous dial on the song.
///
/// The choices from a set — the key, the meter, the progression, the groove, the seed — are
/// picked from a menu or typed. These are the ones with a range, and so the ones that get a bar
/// to drag.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SongDial {
    /// Beats per minute.
    Tempo,
    /// How many bars one section lasts.
    Bars,
    /// Dark to bright.
    Brightness,
    /// Calm to driving.
    Energy,
    /// Plain to coloured.
    Tension,
    /// Straight to syncopated.
    Syncopation,
    /// How far the offbeats are delayed.
    Swing,
    /// How far timing and velocity wander.
    Humanize,
    /// How far apart the hardest and softest notes are struck.
    Dynamics,
    /// How much of a section's last bar runs as a fill.
    Fill,
    /// How far a repeat departs from the playing before it.
    Variation,
}

/// The song's dials, in the order they are drawn.
pub const SONG_DIALS: &[SongDial] = &[
    SongDial::Tempo,
    SongDial::Bars,
    SongDial::Brightness,
    SongDial::Energy,
    SongDial::Tension,
    SongDial::Syncopation,
    SongDial::Swing,
    SongDial::Humanize,
    SongDial::Dynamics,
    SongDial::Fill,
    SongDial::Variation,
];

impl SongDial {
    /// What the row is called.
    pub fn label(self) -> Key {
        match self {
            SongDial::Tempo => Key::Tempo,
            SongDial::Bars => Key::SongBars,
            SongDial::Brightness => Key::SongBrightness,
            SongDial::Energy => Key::SongEnergy,
            SongDial::Tension => Key::SongTension,
            SongDial::Syncopation => Key::PartSyncopation,
            SongDial::Swing => Key::PartSwing,
            SongDial::Humanize => Key::PartHumanize,
            SongDial::Dynamics => Key::PartDynamics,
            SongDial::Fill => Key::PartFill,
            SongDial::Variation => Key::SongVariation,
        }
    }

    /// Where the bar sits, from 0 to 1.
    pub fn fraction(self, dials: &SongDials) -> f32 {
        match self {
            SongDial::Tempo => between(
                dials.tempo as f32,
                *TEMPO.start() as f32,
                *TEMPO.end() as f32,
            ),
            SongDial::Bars => between(dials.bars as f32, *BARS.start() as f32, *BARS.end() as f32),
            SongDial::Brightness => dials.mood.brightness,
            SongDial::Energy => dials.mood.energy,
            SongDial::Tension => dials.mood.tension,
            SongDial::Syncopation => dials.mood.syncopation,
            SongDial::Swing => between(
                f32::from(dials.swing),
                f32::from(*SWING.start()),
                f32::from(*SWING.end()),
            ),
            SongDial::Humanize => dials.humanize,
            SongDial::Dynamics => dials.dynamics,
            SongDial::Fill => dials.fill,
            SongDial::Variation => dials.variation,
        }
    }

    /// Puts the bar at `fraction`.
    pub fn set(self, dials: &mut SongDials, fraction: f32) {
        let fraction = fraction.clamp(0.0, 1.0);
        match self {
            SongDial::Tempo => {
                // To the nearest whole beat: a tempo of 128.37 is a number nobody chose, and the
                // dial has more pixels than the range has useful values.
                let bpm = lerp(fraction, *TEMPO.start() as f32, *TEMPO.end() as f32);
                dials.tempo = f64::from(bpm.round());
            }
            SongDial::Bars => {
                let bars = lerp(fraction, *BARS.start() as f32, *BARS.end() as f32);
                dials.bars = (bars.round() as usize).clamp(*BARS.start(), *BARS.end());
            }
            SongDial::Brightness => dials.mood.brightness = fraction,
            SongDial::Energy => dials.mood.energy = fraction,
            SongDial::Tension => dials.mood.tension = fraction,
            SongDial::Syncopation => dials.mood.syncopation = fraction,
            SongDial::Swing => {
                let swing = lerp(fraction, f32::from(*SWING.start()), f32::from(*SWING.end()));
                dials.swing = swing.round() as u8;
            }
            SongDial::Humanize => dials.humanize = fraction,
            SongDial::Dynamics => dials.dynamics = fraction,
            SongDial::Fill => dials.fill = fraction,
            SongDial::Variation => dials.variation = fraction,
        }
    }

    /// What the readout at the end of the bar says.
    pub fn text(self, dials: &SongDials) -> String {
        match self {
            SongDial::Tempo => format!("{:.0}", dials.tempo),
            SongDial::Bars => dials.bars.to_string(),
            SongDial::Swing if dials.swing == 50 => "50".to_string(),
            SongDial::Swing => dials.swing.to_string(),
            other => percent(other.fraction(dials)),
        }
    }
}

/// One continuous dial on a part.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum PartDial {
    /// How busy the part is, as a share of the available steps.
    Density,
    /// How long a note is held.
    Gate,
    /// Level trim.
    Gain,
    /// Stereo position.
    Pan,
}

/// A part's dials, in the order they are drawn.
pub const PART_DIALS: &[PartDial] = &[
    PartDial::Density,
    PartDial::Gate,
    PartDial::Gain,
    PartDial::Pan,
];

impl PartDial {
    /// What the row is called.
    pub fn label(self) -> Key {
        match self {
            PartDial::Density => Key::PartDensity,
            PartDial::Gate => Key::PartGate,
            PartDial::Gain => Key::SongPartGain,
            PartDial::Pan => Key::SongPartPan,
        }
    }

    /// Where the bar sits, from 0 to 1.
    pub fn fraction(self, part: &PartSpec) -> f32 {
        match self {
            // A part that says nothing about its density is drawn where the mood would put it,
            // which is the middle of the dial: the bar has to start somewhere, and starting at
            // the floor would say the part is silent.
            PartDial::Density => part.density.unwrap_or(0.5),
            PartDial::Gate => part.gate,
            PartDial::Gain => between(part.gain_db, *GAIN_DB.start(), *GAIN_DB.end()),
            PartDial::Pan => (part.pan + 1.0) / 2.0,
        }
    }

    /// Puts the bar at `fraction`.
    pub fn set(self, part: &mut PartSpec, fraction: f32) {
        let fraction = fraction.clamp(0.0, 1.0);
        match self {
            PartDial::Density => part.density = Some(fraction),
            // Never zero: a note of no length is a note nobody hears, and a dial whose bottom end
            // silences the part is a dial with a broken position on it.
            PartDial::Gate => part.gate = fraction.max(0.05),
            PartDial::Gain => {
                part.gain_db = lerp(fraction, *GAIN_DB.start(), *GAIN_DB.end());
            }
            PartDial::Pan => part.pan = fraction * 2.0 - 1.0,
        }
    }

    /// Whether a bar grows from the middle rather than from the left.
    pub fn is_centred(self) -> bool {
        matches!(self, PartDial::Pan)
    }

    /// What the readout at the end of the bar says.
    pub fn text(self, part: &PartSpec) -> String {
        match self {
            PartDial::Gain => format!("{:.1}", part.gain_db),
            PartDial::Pan if part.pan.abs() < 0.005 => "C".to_string(),
            PartDial::Pan if part.pan < 0.0 => format!("L{:.0}", part.pan.abs() * 100.0),
            PartDial::Pan => format!("R{:.0}", part.pan * 100.0),
            other => percent(other.fraction(part)),
        }
    }
}

/// Where a drag that began at `start` and has travelled `delta` pixels puts a dial.
pub fn dragged(start: f32, delta: f32) -> f32 {
    (start + delta / DRAG_RANGE_PIXELS).clamp(0.0, 1.0)
}

/// Where `value` sits between `low` and `high`, from 0 to 1.
fn between(value: f32, low: f32, high: f32) -> f32 {
    ((value - low) / (high - low).max(f32::EPSILON)).clamp(0.0, 1.0)
}

/// The value `fraction` of the way from `low` to `high`.
fn lerp(fraction: f32, low: f32, high: f32) -> f32 {
    low + (high - low) * fraction.clamp(0.0, 1.0)
}

/// A fraction as a whole percent, which is the resolution the readout has.
fn percent(fraction: f32) -> String {
    format!("{:.0}%", (fraction * 100.0).round())
}

/// Which dial a drag is turning.
///
/// The song's dials and a part's are one gesture with two targets, so the drag state names both
/// rather than there being two of everything from the pointer down.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum DialTarget {
    /// One of the song's own.
    Song(SongDial),
    /// One belonging to the part at this position in the roster.
    Part(usize, PartDial),
}

impl DialTarget {
    /// Where the bar sits, from 0 to 1.
    pub fn fraction(self, dials: &SongDials) -> f32 {
        match self {
            DialTarget::Song(dial) => dial.fraction(dials),
            DialTarget::Part(index, dial) => dials
                .parts
                .get(index)
                .map_or(0.0, |part| dial.fraction(part)),
        }
    }

    /// Puts the bar at `fraction`.
    pub fn set(self, dials: &mut SongDials, fraction: f32) {
        match self {
            DialTarget::Song(dial) => dial.set(dials, fraction),
            DialTarget::Part(index, dial) => {
                if let Some(part) = dials.parts.get_mut(index) {
                    dial.set(part, fraction);
                }
            }
        }
    }
}

// ---------------------------------------------------------------- the view

use gpui::{
    AnyElement, Context, IntoElement, MouseDownEvent, Window, div, prelude::*, px, relative,
};

use crate::app::{AurisApp, Drag};
use crate::theme::{Metrics, Theme};
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::prompt::{Prompt, PromptTarget};
use crate::ui::widgets::{ButtonStyle, SliderFill, button, divider, value_slider};

/// How wide the label at the start of a row is drawn.
const LABEL_WIDTH: f32 = 116.0;

impl AurisApp {
    /// Opens the song sheet, on the song it was last set to or on the default one.
    pub(crate) fn open_song_sheet(&mut self) {
        if self.song_sheet.is_none() {
            self.song_sheet = Some(SongDials::default());
        }
    }

    /// The song sheet, or nothing when it is closed.
    ///
    /// A full-screen panel rather than a [`Prompt`]: a prompt asks for one value, and this asks
    /// for a song. It occludes for the same reason the export overlay does — every click behind
    /// a dimmed screen used to land on the arrangement.
    pub(crate) fn render_song_sheet(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let dials = self.song_sheet.clone()?;
        let theme = self.theme.clone();
        let spec = song_spec(&dials);
        let length = format!(
            "{} · {} {}",
            self.t(Key::SongLength),
            spec.total_bars(),
            self.t(Key::SongBarsUnit)
        );

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(Theme::translucent(theme.background, 0.72))
                .occlude()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .w(px(880.0))
                        .max_h(relative(0.92))
                        .p_4()
                        .rounded(Metrics::RADIUS_LG)
                        .bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.text)
                                .child(self.t(Key::SongSheetTitle)),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_4()
                                .flex_1()
                                .min_h_0()
                                .child(
                                    div()
                                        .id("song-sheet-song")
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .w(px(360.0))
                                        .overflow_y_scroll()
                                        .children(self.song_rows(&dials, cx)),
                                )
                                .child(
                                    div()
                                        .id("song-sheet-parts")
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_y_scroll()
                                        .children(self.song_part_rows(&dials, cx)),
                                ),
                        )
                        .child(divider(&theme))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .text_xs()
                                        .text_color(theme.text_muted)
                                        .child(length),
                                )
                                .child(button(
                                    "song-sheet-cancel",
                                    self.t(Key::Cancel),
                                    ButtonStyle::Normal,
                                    false,
                                    theme.accent,
                                    &theme,
                                    cx.listener(|this, _, _, cx| {
                                        this.song_sheet = None;
                                        cx.notify();
                                    }),
                                ))
                                .child(button(
                                    "song-sheet-save",
                                    self.t(Key::SongSaveSpec),
                                    ButtonStyle::Normal,
                                    false,
                                    theme.accent,
                                    &theme,
                                    cx.listener(|this, _, window, cx| {
                                        this.save_song_specification(window, cx);
                                    }),
                                ))
                                .child(button(
                                    "song-sheet-take",
                                    self.t(Key::SongAnotherTake),
                                    ButtonStyle::Normal,
                                    false,
                                    theme.accent,
                                    &theme,
                                    cx.listener(|this, _, _, cx| {
                                        if let Some(dials) = this.song_sheet.as_mut() {
                                            another_take(dials);
                                        }
                                        this.write_song_from_sheet();
                                        cx.notify();
                                    }),
                                ))
                                .child(button(
                                    "song-sheet-write",
                                    self.t(Key::SongWrite),
                                    ButtonStyle::Primary,
                                    false,
                                    theme.accent,
                                    &theme,
                                    // Write closes the sheet and Another Take does not: one is
                                    // "this is the song", the other is "not that one, again".
                                    cx.listener(|this, _, _, cx| {
                                        this.write_song_from_sheet();
                                        this.song_sheet = None;
                                        cx.notify();
                                    }),
                                )),
                        ),
                ),
        )
    }

    /// The left half: everything about the song that is not a part.
    fn song_rows(&mut self, dials: &SongDials, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme.clone();
        let mut rows: Vec<AnyElement> =
            vec![self.group_heading(Key::SongHeading).into_any_element()];

        rows.push(
            self.sheet_picker(
                "song-title",
                Key::SongTitleField,
                dials.title.clone(),
                cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                    let title = this.t(Key::SongTitleField);
                    let current = this
                        .song_sheet
                        .as_ref()
                        .map_or_else(String::new, |dials| dials.title.clone());
                    this.open_prompt(Prompt::new(title, PromptTarget::SongTitle, current));
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        rows.push(
            self.sheet_picker(
                "song-key",
                Key::SongKey,
                dials.key.to_text(),
                cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                    let title = this.t(Key::SongKey);
                    let current = this
                        .song_sheet
                        .as_ref()
                        .map_or_else(String::new, |dials| dials.key.to_text());
                    this.open_prompt(Prompt::new(title, PromptTarget::SongKey, current));
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        rows.push(
            self.sheet_picker(
                "song-meter",
                Key::SongMeter,
                format!("{}/{}", dials.meter.numerator, dials.meter.denominator),
                cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                    let menu = this.song_meter_menu(event.position());
                    this.open_menu(menu);
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        rows.push(
            self.sheet_picker(
                "song-mood",
                Key::SongMood,
                match mood_word(dials.mood) {
                    Some(name) => this_word(self, name),
                    None => self.t(Key::SongMoodCustom).to_string(),
                },
                cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                    let menu = this.song_mood_menu(event.position());
                    this.open_menu(menu);
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        rows.push(
            self.sheet_picker(
                "song-chords",
                Key::SongChords,
                match progression_catalog()
                    .iter()
                    .find(|entry| entry.name == dials.chords)
                {
                    Some(entry) => {
                        auris_i18n::audio::theory_description(entry.description, self.language())
                            .to_string()
                    }
                    None => self.t(Key::SongChordsOwn).to_string(),
                },
                cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                    let menu = this.song_chords_menu(event.position());
                    this.open_menu(menu);
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        rows.push(
            self.sheet_picker(
                "song-groove",
                Key::PartGroove,
                dials.groove.clone(),
                cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                    let menu = this.song_groove_menu(event.position());
                    this.open_menu(menu);
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        rows.push(
            self.sheet_picker(
                "song-seed",
                Key::PartSeed,
                dials.seed.to_string(),
                cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                    let title = this.t(Key::PartSeed);
                    let current = this
                        .song_sheet
                        .as_ref()
                        .map_or_else(String::new, |dials| dials.seed.to_string());
                    this.open_prompt(Prompt::new(title, PromptTarget::SongSeed, current));
                    cx.notify();
                }),
            )
            .into_any_element(),
        );

        for dial in SONG_DIALS {
            let dial = *dial;
            let target = DialTarget::Song(dial);
            let fraction = dial.fraction(dials);
            rows.push(
                value_slider(
                    ("song-dial", dial as usize),
                    self.t(dial.label()),
                    dial.text(dials),
                    fraction,
                    theme.accent,
                    SliderFill::FromStart,
                    &theme,
                    cx.listener(move |this, event: &MouseDownEvent, _, _| {
                        this.begin_drag(Drag::SongDial {
                            target,
                            start_fraction: fraction,
                            start_x: event.position.x,
                        });
                    }),
                    cx.listener(move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                        let notches = f32::from(event.delta.pixel_delta(px(16.0)).y) / 16.0;
                        this.nudge_song_dial(target, notches);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .into_any_element(),
            );
        }
        rows
    }

    /// The right half: one block per part, with a button that adds another.
    fn song_part_rows(&mut self, dials: &SongDials, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme.clone();
        let removable = dials.parts.len() > 1;
        let mut rows: Vec<AnyElement> = vec![
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .child(self.group_heading(Key::SongPartsHeading)),
                )
                .child(button(
                    "song-add-part",
                    self.t(Key::SongAddPart),
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                        let menu = this.song_add_part_menu(event.position());
                        this.open_menu(menu);
                        cx.notify();
                    }),
                ))
                .into_any_element(),
        ];

        for (index, part) in dials.parts.iter().enumerate() {
            let instrument = self
                .registry()
                .instruments()
                .find(|descriptor| descriptor.id == part.instrument)
                .map(|descriptor| {
                    auris_i18n::audio::plugin_name(&descriptor.name, self.language()).to_string()
                })
                .unwrap_or_else(|| part.instrument.clone());

            let mut dial_row = div().flex().gap_2();
            for dial in PART_DIALS {
                let dial = *dial;
                let target = DialTarget::Part(index, dial);
                let fraction = dial.fraction(part);
                dial_row = dial_row.child(div().flex_1().min_w_0().child(value_slider(
                    ("song-part-dial", index * PART_DIALS.len() + dial as usize),
                    self.t(dial.label()),
                    dial.text(part),
                    fraction,
                    theme.accent,
                    match dial.is_centred() {
                        true => SliderFill::FromCentre,
                        false => SliderFill::FromStart,
                    },
                    &theme,
                    cx.listener(move |this, event: &MouseDownEvent, _, _| {
                        this.begin_drag(Drag::SongDial {
                            target,
                            start_fraction: fraction,
                            start_x: event.position.x,
                        });
                    }),
                    cx.listener(move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                        let notches = f32::from(event.delta.pixel_delta(px(16.0)).y) / 16.0;
                        this.nudge_song_dial(target, notches);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )));
            }

            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .rounded(Metrics::RADIUS_SM)
                    .bg(theme.surface_sunken)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(div().w(px(96.0)).child(button(
                                ("song-part-name", index),
                                part.name.clone(),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                    let title = this.t(Key::SongPartNameTitle);
                                    let current = this.song_sheet.as_ref().map_or_else(
                                        String::new,
                                        |dials| {
                                            dials
                                                .parts
                                                .get(index)
                                                .map_or_else(String::new, |part| part.name.clone())
                                        },
                                    );
                                    this.open_prompt(Prompt::new(
                                        title,
                                        PromptTarget::SongPartName(index),
                                        current,
                                    ));
                                    cx.notify();
                                }),
                            )))
                            .child(div().w(px(88.0)).child(button(
                                ("song-part-role", index),
                                self.t(role_key(part.role)),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                    let menu = this.song_role_menu(event.position(), index);
                                    this.open_menu(menu);
                                    cx.notify();
                                }),
                            )))
                            .child(div().flex_1().min_w_0().child(button(
                                ("song-part-instrument", index),
                                instrument,
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                    let menu = this.song_instrument_menu(event.position(), index);
                                    this.open_menu(menu);
                                    cx.notify();
                                }),
                            )))
                            .child(div().w(px(52.0)).child(button(
                                ("song-part-octave", index),
                                part.octave.to_string(),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                    let menu = this.song_octave_menu(event.position(), index);
                                    this.open_menu(menu);
                                    cx.notify();
                                }),
                            )))
                            // The last part cannot go: a song with no parts writes no notes, and
                            // the button goes dead rather than Write producing an empty document.
                            .child(div().w(px(64.0)).child(button(
                                ("song-part-remove", index),
                                self.t(Key::SongRemovePart),
                                ButtonStyle::Normal,
                                false,
                                if removable {
                                    theme.danger
                                } else {
                                    theme.border
                                },
                                &theme,
                                cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                    if let Some(dials) = this.song_sheet.as_mut() {
                                        remove_part(dials, index);
                                    }
                                    cx.notify();
                                }),
                            ))),
                    )
                    .child(dial_row)
                    .into_any_element(),
            );
        }
        rows
    }

    /// A row with a label at the start and a button holding the value.
    fn sheet_picker<I, F>(
        &self,
        id: I,
        label: Key,
        value: String,
        on_click: F,
    ) -> impl IntoElement + use<I, F>
    where
        I: Into<gpui::ElementId>,
        F: Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    {
        let theme = self.theme.clone();
        div()
            .flex()
            .items_center()
            .gap_2()
            .h(Metrics::CONTROL_HEIGHT)
            .child(
                div()
                    .w(px(LABEL_WIDTH))
                    .text_xs()
                    .text_color(theme.text_muted)
                    .truncate()
                    .child(self.t(label)),
            )
            .child(div().flex_1().min_w_0().child(button(
                id,
                value,
                ButtonStyle::Normal,
                false,
                theme.accent,
                &theme,
                on_click,
            )))
    }

    /// Moves one of the sheet's dials, from a drag.
    pub(crate) fn drag_song_dial(&mut self, target: DialTarget, start_fraction: f32, delta: f32) {
        let Some(dials) = self.song_sheet.as_mut() else {
            return;
        };
        target.set(dials, dragged(start_fraction, delta));
    }

    /// Moves one of the sheet's dials by `notches` of a wheel.
    fn nudge_song_dial(&mut self, target: DialTarget, notches: f32) {
        let Some(dials) = self.song_sheet.as_mut() else {
            return;
        };
        let next = (target.fraction(dials) + notches * 0.02).clamp(0.0, 1.0);
        target.set(dials, next);
    }

    /// Writes the piece the sheet describes, replacing the document.
    pub(crate) fn write_song_from_sheet(&mut self) {
        let Some(dials) = self.song_sheet.as_ref() else {
            return;
        };
        let spec = song_spec(dials);
        self.compose_spec(&spec);
    }

    /// Saves the sheet as a specification file.
    pub(crate) fn save_song_specification(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(dials) = self.song_sheet.as_ref() else {
            return;
        };
        let text = song_spec(dials).to_toml();
        let name = format!("{}.{}", dials.title, auris_session::SPEC_EXTENSION);
        let language = self.language();
        cx.spawn(async move |this, cx| {
            let handle = rfd::AsyncFileDialog::new()
                .set_title(Key::SongSaveSpec.get(language))
                .set_file_name(&name)
                .add_filter(
                    Key::FilterSpec.get(language),
                    &[auris_session::SPEC_EXTENSION],
                )
                .save_file()
                .await;
            let Some(handle) = handle else { return };
            let path = handle.path().to_path_buf();
            let written = std::fs::write(&path, text);
            let _ = this.update(cx, |this, cx| {
                match written {
                    Ok(()) => this.set_status(auris_i18n::messages::saved(
                        this.language(),
                        &path.display().to_string(),
                    )),
                    Err(error) => this.set_failed_status(auris_i18n::messages::failed(
                        this.language(),
                        this.t(Key::SongSaveSpec),
                        &error.to_string(),
                    )),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The meters the sheet offers.
    fn song_meter_menu(&self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongMeter));
        for (numerator, denominator) in [(4, 4), (3, 4), (6, 8), (5, 4), (7, 8), (12, 8)] {
            menu = menu.item(
                format!("{numerator}/{denominator}"),
                MenuCommand::SongMeter(numerator, denominator),
            );
        }
        menu
    }

    /// The named feelings, each of which means four numbers.
    fn song_mood_menu(&self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongMood));
        for name in Mood::NAMES {
            menu = menu.item(self.t(mood_key(name)), MenuCommand::SongMood(name));
        }
        menu
    }

    /// Every progression the composer knows by name, and the option of none.
    fn song_chords_menu(&self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongChords))
            .item(self.t(Key::SongChordsOwn), MenuCommand::SongChords(""));
        for entry in progression_catalog() {
            menu = menu.item(
                auris_i18n::audio::theory_description(entry.description, self.language()),
                MenuCommand::SongChords(entry.name),
            );
        }
        menu
    }

    /// Every drum groove the composer knows by name.
    fn song_groove_menu(&self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::PartGroove));
        for groove in groove_catalog() {
            menu = menu.item(
                auris_i18n::audio::theory_description(groove.description, self.language()),
                MenuCommand::SongGroove(groove.name),
            );
        }
        menu
    }

    /// The roles a part may take.
    fn song_role_menu(&self, anchor: gpui::Point<gpui::Pixels>, part: usize) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongPartRole));
        for role in Role::ALL {
            menu = menu.item(
                self.t(role_key(role)),
                MenuCommand::SongPartRole { part, role },
            );
        }
        menu
    }

    /// A role for a part that does not exist yet.
    fn song_add_part_menu(&self, anchor: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongAddPart));
        for role in Role::ALL {
            menu = menu.item(self.t(role_key(role)), MenuCommand::SongAddPart(role));
        }
        menu
    }

    /// Every instrument this build can play.
    fn song_instrument_menu(&self, anchor: gpui::Point<gpui::Pixels>, part: usize) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::SongPartInstrument));
        let language = self.language();
        let mut instruments: Vec<(String, String)> = self
            .registry()
            .instruments()
            .map(|descriptor| {
                (
                    descriptor.id.to_string(),
                    auris_i18n::audio::plugin_name(&descriptor.name, language).to_string(),
                )
            })
            .collect();
        instruments.sort_by(|one, other| one.1.cmp(&other.1));
        for (id, name) in instruments {
            menu = menu.item(name, MenuCommand::SongPartInstrument { part, id });
        }
        menu
    }

    /// The octaves a part may sit in.
    fn song_octave_menu(&self, anchor: gpui::Point<gpui::Pixels>, part: usize) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::PartOctave));
        for octave in 1..=7 {
            menu = menu.item(
                octave.to_string(),
                MenuCommand::SongPartOctave { part, octave },
            );
        }
        menu
    }
}

/// The interface's word for a mood, as a `String` the picker can hold.
fn this_word(app: &AurisApp, name: &str) -> String {
    app.t(mood_key(name)).to_string()
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sheet_opens_on_the_song_the_specification_describes() {
        // Two lists of defaults would drift, and the one that drifted would be the one nobody
        // reads: a dialog that opens on a different song from `auris compose` with no file.
        let dials = SongDials::default();
        let spec = SongSpec::default();
        assert_eq!(song_spec(&dials).title, spec.title);
        assert_eq!(song_spec(&dials).tempo, spec.tempo);
        assert_eq!(song_spec(&dials).key, spec.key);
        assert_eq!(song_spec(&dials).parts, spec.parts);
        assert_eq!(song_spec(&dials).total_bars(), spec.total_bars());
    }

    #[test]
    fn what_the_sheet_writes_is_a_document_that_reads_back_the_same() {
        // The test the sheet and the format share. A dial the specification cannot express, or
        // expresses differently, shows up here rather than as a song that changes when it is
        // saved and opened.
        let mut dials = SongDials {
            title: "Neon Drive".to_string(),
            key: MusicalKey::parse("C minor").unwrap(),
            tempo: 124.0,
            meter: TimeSignature::new(3, 4),
            mood: Mood::named("driving").unwrap(),
            bars: 16,
            chords: "marusa".to_string(),
            groove: "four-on-the-floor".to_string(),
            seed: 7,
            swing: 54,
            humanize: 0.3,
            variation: 0.4,
            ..SongDials::default()
        };
        dials.parts[0].gain_db = -3.5;
        dials.parts[0].pan = -0.4;
        dials.parts[1].density = Some(0.65);

        let spec = song_spec(&dials);
        assert_eq!(SongSpec::parse(&spec.to_toml()).unwrap(), spec);
        assert_eq!(spec.total_bars(), 16 * 6, "six sections of sixteen bars");
        assert_eq!(spec.charts["main"].bar_count(), 4, "丸サ進行 is four bars");
    }

    #[test]
    fn every_dial_reads_back_what_it_was_set_to() {
        // The bar is drawn from `fraction` and dragged into `set`, so a value that did not
        // survive the round trip would make the bar slide away from the pointer holding it.
        for dial in SONG_DIALS {
            for target in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let mut dials = SongDials::default();
                dial.set(&mut dials, target);
                let back = dial.fraction(&dials);
                assert!(
                    (back - target).abs() < 0.03,
                    "{dial:?} set to {target} read back {back}"
                );
            }
        }
        for dial in PART_DIALS {
            for target in [0.05, 0.25, 0.5, 0.75, 1.0] {
                let mut part = PartSpec::of_role("lead", Role::Melody);
                dial.set(&mut part, target);
                let back = dial.fraction(&part);
                assert!(
                    (back - target).abs() < 0.03,
                    "{dial:?} set to {target} read back {back}"
                );
            }
        }
    }

    #[test]
    fn no_dial_can_be_turned_to_a_value_the_format_refuses() {
        // Every end of every dial, written out and read back: the sheet must not be able to
        // produce a document its own parser rejects.
        for end in [0.0, 1.0] {
            let mut dials = SongDials::default();
            for dial in SONG_DIALS {
                dial.set(&mut dials, end);
            }
            for part in &mut dials.parts {
                for dial in PART_DIALS {
                    dial.set(part, end);
                }
            }
            let spec = song_spec(&dials);
            let written = spec.to_toml();
            assert_eq!(
                SongSpec::parse(&written),
                Ok(spec),
                "every dial at {end}:\n{written}"
            );
        }
    }

    #[test]
    fn another_take_is_the_next_seed_and_nothing_else() {
        let mut dials = SongDials {
            seed: 41,
            ..SongDials::default()
        };
        let before = dials.clone();
        another_take(&mut dials);
        assert_eq!(dials.seed, 42);
        assert_eq!(
            SongDials { seed: 41, ..dials },
            before,
            "only the seed moved"
        );
    }

    #[test]
    fn a_part_added_never_takes_a_name_the_roster_already_has() {
        // Two parts of one name is an error the format reports, and a button that produces one
        // is a button that breaks the song.
        let mut dials = SongDials::default();
        add_part(&mut dials, Role::Bass);
        add_part(&mut dials, Role::Bass);
        let names: Vec<&str> = dials.parts.iter().map(|part| part.name.as_str()).collect();
        assert!(names.contains(&"bass 2"), "{names:?}");
        assert!(names.contains(&"bass 3"), "{names:?}");
        assert!(SongSpec::parse(&song_spec(&dials).to_toml()).is_ok());
    }

    #[test]
    fn the_last_part_cannot_be_removed() {
        // A song with no parts writes no notes; the button goes dead rather than the Write
        // button producing an empty document.
        let mut dials = SongDials::default();
        while dials.parts.len() > 1 {
            assert!(remove_part(&mut dials, 0));
        }
        assert!(!remove_part(&mut dials, 0));
        assert_eq!(dials.parts.len(), 1);
    }

    #[test]
    fn no_progression_chosen_leaves_the_one_the_mood_may_colour() {
        // "Nothing chosen" is how the composer gets to invent, not a hole: the default chart is
        // marked as its own and is the only kind colouring is allowed to touch.
        let dials = SongDials::default();
        assert_eq!(dials.chords, "");
        assert_eq!(
            song_spec(&dials).charts["main"].origin,
            ChartOrigin::Generated
        );

        let quoted = SongDials {
            chords: "marusa".to_string(),
            ..SongDials::default()
        };
        assert_eq!(song_spec(&quoted).charts["main"].origin, ChartOrigin::Given);
    }

    #[test]
    fn every_name_the_pickers_offer_has_a_word_in_both_languages() {
        // The pickers list the catalogues, and a name with no entry here would come out as the
        // fallback rather than as a translation.
        for name in Mood::NAMES {
            let key = mood_key(name);
            assert_ne!(
                key.get(auris_i18n::Language::English),
                key.get(auris_i18n::Language::Japanese),
                "the mood `{name}` reads the same in both languages"
            );
        }
        for role in Role::ALL {
            let key = role_key(role);
            assert_ne!(
                key.get(auris_i18n::Language::English),
                key.get(auris_i18n::Language::Japanese),
                "the role `{}` reads the same in both languages",
                role.name()
            );
        }
    }

    #[test]
    fn the_mood_word_is_the_one_the_numbers_actually_are() {
        assert_eq!(mood_word(Mood::named("dreamy").unwrap()), Some("dreamy"));
        let mut nudged = Mood::named("dreamy").unwrap();
        nudged.energy = 0.9;
        assert_eq!(mood_word(nudged), None, "a nudged mood is no word at all");
    }

    #[test]
    fn a_progression_is_named_or_left_to_the_composer() {
        assert!(chart_named("").is_none());
        assert!(chart_named("   ").is_none());
        assert!(chart_named("marusa").is_some());
        assert!(chart_named("nonsense").is_none());
        for entry in progression_catalog() {
            assert!(
                chart_named(entry.name).is_some(),
                "the picker offers `{}` and nothing answers to it",
                entry.name
            );
        }
    }
}
