//! The dials on a generated clip: what each one reads, what it writes, and which presets have it.
//!
//! Everything here that decides something is a free function over a [`ClipRecipe`], for the reason
//! given in [`crate::ui::context_menu`]: a decision made inside a `render` method can only be
//! checked by opening a window and looking at it. The `impl AurisApp` block below is the part that
//! genuinely needs a window — it draws the rows and hangs the gestures off them.

use auris_i18n::Key;
use auris_session::prelude::*;

use gpui::{AnyElement, IntoElement, MouseDownEvent, div, prelude::*, px};

use crate::app::{AurisApp, Drag};
use crate::theme::Metrics;
use crate::ui::plugin_editor::DRAG_RANGE_PIXELS;
use crate::ui::prompt::{Prompt, PromptTarget};
use crate::ui::widgets::{ButtonStyle, SliderFill, button, divider, value_slider};

/// Straight eighths. Anything less would rush the offbeat, which is not a feel anybody asks for.
pub const SWING_MIN: u8 = 50;

/// As far as the swing dial goes: the offbeat on the last sixteenth of its beat.
///
/// Past the dotted feel the second eighth is so late that it is heard as an early downbeat of the
/// next beat rather than as swing, and at 100 it lands on it exactly.
pub const SWING_MAX: u8 = 75;

/// The shortest a gate dial reaches, as a share of the gap to the next note.
///
/// Not zero: a note of no length is a note nobody hears, and a dial whose bottom end silences the
/// part is a dial with a broken position on it. A twentieth of the gap is already a click.
pub const GATE_MIN: f32 = 0.05;

/// One continuous dial on a [`ClipRecipe`].
///
/// The seed, the preset, the groove and the subdivision are all choices from a set and are picked
/// from a menu; these five are the ones with a range, and so the ones that get a bar to drag.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Dial {
    /// How busy the part is.
    Density,
    /// How far the figures pull off the beat.
    Syncopation,
    /// How long each note sounds, against the gap to the next.
    Gate,
    /// How hard it is played.
    Intensity,
    /// How far apart the hardest and softest notes are struck.
    Dynamics,
    /// How much of the last bar the snare runs as a fill.
    Fill,
    /// How late the offbeats are.
    Swing,
    /// How far timing and velocity wander.
    Humanize,
}

impl Dial {
    /// What the row is called.
    pub fn label(self) -> Key {
        match self {
            Dial::Density => Key::PartDensity,
            Dial::Syncopation => Key::PartSyncopation,
            Dial::Gate => Key::PartGate,
            Dial::Intensity => Key::PartIntensity,
            Dial::Dynamics => Key::PartDynamics,
            Dial::Fill => Key::PartFill,
            Dial::Swing => Key::PartSwing,
            Dial::Humanize => Key::PartHumanize,
        }
    }

    /// Where the bar is filled to, from 0 to 1.
    pub fn fraction(self, recipe: &ClipRecipe) -> f32 {
        let value = match self {
            Dial::Density => recipe.density,
            Dial::Syncopation => recipe.syncopation,
            Dial::Intensity => recipe.intensity,
            Dial::Dynamics => recipe.dynamics,
            Dial::Fill => recipe.fill,
            Dial::Humanize => recipe.humanize,
            Dial::Gate => (recipe.gate - GATE_MIN) / (1.0 - GATE_MIN),
            Dial::Swing => {
                let span = f32::from(SWING_MAX - SWING_MIN);
                (f32::from(recipe.swing) - f32::from(SWING_MIN)) / span
            }
        };
        value.clamp(0.0, 1.0)
    }

    /// Writes a bar position back onto the recipe, rounded to what the readout can show.
    ///
    /// Quantising is not tidiness. Every write here reruns the composer over the clip, and outside
    /// a drag it also pushes an undo step and rebuilds the render graph — so a value that moved by
    /// a thousandth would cost all of that to change nothing anybody can see or hear. Landing on
    /// whole percent lets [`AurisApp::set_dial`] recognise the no-op and do none of it.
    ///
    /// It also makes the readout true: two stored densities that printed as `52%` were two
    /// different parts, and only one of them was the one on screen.
    pub fn set(self, recipe: &mut ClipRecipe, fraction: f32) {
        let fraction = fraction.clamp(0.0, 1.0);
        match self {
            Dial::Density => recipe.density = whole_percent(fraction),
            Dial::Syncopation => recipe.syncopation = whole_percent(fraction),
            Dial::Intensity => recipe.intensity = whole_percent(fraction),
            Dial::Dynamics => recipe.dynamics = whole_percent(fraction),
            Dial::Fill => recipe.fill = whole_percent(fraction),
            Dial::Humanize => recipe.humanize = whole_percent(fraction),
            Dial::Gate => recipe.gate = GATE_MIN + whole_percent(fraction) * (1.0 - GATE_MIN),
            Dial::Swing => {
                let span = f32::from(SWING_MAX - SWING_MIN);
                recipe.swing = SWING_MIN + (fraction * span).round() as u8;
            }
        }
    }
}

/// A fraction rounded to the nearest whole percent, which is the resolution the readout has.
fn whole_percent(fraction: f32) -> f32 {
    (fraction * 100.0).round() / 100.0
}

/// The dials a recipe actually reads, in the order they are drawn.
///
/// The rule throughout: a control that cannot change what is heard is not drawn. It is the same
/// rule the composer itself keeps, stated where the interface can break it, and it costs a
/// changing row count in exchange for never lying about what is reachable.
pub fn dials_for(recipe: &ClipRecipe) -> &'static [Dial] {
    // A kit reads neither the gate nor the syncopation: a one-shot drum ignores its note-off,
    // and where it plays is which groove it plays. It ignores the subdivision too, which is why
    // its swing is the one that is never inert. The fill is its alone — nothing else has a last
    // bar to announce.
    if recipe.preset == ClipPreset::Drums {
        return &[
            Dial::Density,
            Dial::Fill,
            Dial::Intensity,
            Dial::Dynamics,
            Dial::Swing,
            Dial::Humanize,
        ];
    }
    // A pad has no figure for the syncopation to pull off the beat: it sounds each chord once,
    // where the chord is, and a dial that moved that would be moving the harmony rather than the
    // part. Swing stays, because a chord that begins on an offbeat is swung like anything else.
    if recipe.preset == ClipPreset::Pad {
        return &[
            Dial::Density,
            Dial::Gate,
            Dial::Intensity,
            Dial::Dynamics,
            Dial::Swing,
            Dial::Humanize,
        ];
    }
    // Swing exists to push a straight offbeat toward the third triplet. A part already dividing
    // its beats in three is sitting there, and has nothing left for the dial to do.
    if recipe.subdivision.is_triplet() {
        return &[
            Dial::Density,
            Dial::Syncopation,
            Dial::Gate,
            Dial::Intensity,
            Dial::Dynamics,
            Dial::Humanize,
        ];
    }
    &[
        Dial::Density,
        Dial::Syncopation,
        Dial::Gate,
        Dial::Intensity,
        Dial::Dynamics,
        Dial::Swing,
        Dial::Humanize,
    ]
}

/// How far the octave picker reaches either way.
///
/// Two is as far as a register offset stays the same part. Past that a bass is a lead and a lead
/// is out of the range its role was given, which is a different preset rather than a nudge.
pub const OCTAVE_REACH: i32 = 2;

/// Every octave offset the picker offers, lowest first.
pub fn octave_choices() -> std::ops::RangeInclusive<i32> {
    -OCTAVE_REACH..=OCTAVE_REACH
}

/// How an octave offset reads on its row: signed, because zero is not "no octave" but "the one
/// the preset chose", and `+1` says which way the other rows go.
pub fn octave_text(octave: i32) -> String {
    match octave.clamp(-OCTAVE_REACH, OCTAVE_REACH) {
        0 => "±0".to_string(),
        other => format!("{other:+}"),
    }
}

/// Whether a preset's groove is worth offering, which is to say whether anything reads it.
pub fn takes_a_groove(preset: ClipPreset) -> bool {
    matches!(preset, ClipPreset::Drums)
}

/// Whether a preset's subdivision is worth offering.
///
/// Everything but the kit, for the reason the composer gives: a groove is sixteen steps read by
/// index, so a kit on any other grid would scramble it rather than divide it.
pub fn takes_a_subdivision(preset: ClipPreset) -> bool {
    !matches!(preset, ClipPreset::Drums)
}

/// Whether a preset's register is worth offering.
///
/// Everything but the kit, whose pitches are General MIDI drum numbers rather than notes: moving
/// a kick up an octave would not raise it, it would turn it into a different drum.
pub fn takes_an_octave(preset: ClipPreset) -> bool {
    !matches!(preset, ClipPreset::Drums)
}

/// The recipe a clip takes when its preset changes.
///
/// A dial somebody moved is theirs, and follows them across the change. A dial still sitting
/// exactly where the old preset put it is the old preset's opinion rather than anybody's, and
/// becomes the new preset's instead.
///
/// Without this the stab would be unreachable from the picker: its whole identity is a gate near
/// the floor and a density near the ceiling, and choosing it from a pad would have kept the pad's
/// and given back a pad under a new name.
pub fn with_preset(recipe: &ClipRecipe, preset: ClipPreset) -> ClipRecipe {
    let was = ClipRecipe::new(recipe.preset, recipe.seed);
    let becomes = ClipRecipe::new(preset, recipe.seed);
    let untouched = |current: f32, before: f32, after: f32| {
        if current == before { after } else { current }
    };
    ClipRecipe {
        preset,
        density: untouched(recipe.density, was.density, becomes.density),
        gate: untouched(recipe.gate, was.gate, becomes.gate),
        intensity: untouched(recipe.intensity, was.intensity, becomes.intensity),
        dynamics: untouched(recipe.dynamics, was.dynamics, becomes.dynamics),
        syncopation: untouched(recipe.syncopation, was.syncopation, becomes.syncopation),
        humanize: untouched(recipe.humanize, was.humanize, becomes.humanize),
        // The seed and the octave are nobody's default: one is which take this is and the other
        // is a register somebody asked for, and neither is an opinion a preset holds.
        ..recipe.clone()
    }
}

/// What a dial reads as, given the word this language uses for unswung eighths.
pub fn dial_text(dial: Dial, recipe: &ClipRecipe, straight: &str) -> String {
    match dial {
        Dial::Swing if recipe.swing <= SWING_MIN => straight.to_string(),
        Dial::Swing => format!("{}%", recipe.swing),
        // The stored share of the gap, not the bar's position: at the bottom of its travel the
        // bar is empty and the note is still a twentieth long, and a readout saying 0% would be
        // describing the control rather than the music.
        Dial::Gate => format!("{}%", (recipe.gate * 100.0).round() as i32),
        _ => format!("{}%", (dial.fraction(recipe) * 100.0).round() as i32),
    }
}

/// A stable per-dial element key, so gpui can track hover state across frames.
fn dial_element_key(dial: Dial) -> usize {
    match dial {
        Dial::Density => 0,
        Dial::Intensity => 1,
        Dial::Swing => 2,
        Dial::Humanize => 3,
        Dial::Gate => 4,
        Dial::Dynamics => 5,
        Dial::Syncopation => 6,
        Dial::Fill => 7,
    }
}

impl AurisApp {
    /// The selected clip's recipe section, or nothing when the selection was played by hand.
    ///
    /// Returns rows rather than a panel, so the caller decides where the section sits among the
    /// track's own controls.
    pub(crate) fn part_rows(&mut self, cx: &mut gpui::Context<Self>) -> Vec<AnyElement> {
        let Some(clip) = self.selected_clip else {
            return Vec::new();
        };
        let Some(recipe) = self.session.clip_recipe(clip).cloned() else {
            return Vec::new();
        };
        let theme = self.theme.clone();
        let straight = self.t(Key::PartStraight);

        let mut rows: Vec<AnyElement> = vec![
            self.group_heading(Key::PartHeading).into_any_element(),
            self.picker_row(
                "part-preset",
                Key::PartPreset,
                self.t(crate::ui::context_menu::preset_key(recipe.preset))
                    .to_string(),
                cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                    let menu = this.clip_preset_menu(event.position(), clip);
                    this.open_menu(menu);
                    cx.notify();
                }),
            )
            .into_any_element(),
        ];

        if takes_a_subdivision(recipe.preset) {
            rows.push(
                self.picker_row(
                    "part-subdivision",
                    Key::PartSubdivision,
                    self.t(crate::ui::context_menu::subdivision_key(recipe.subdivision))
                        .to_string(),
                    cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        let menu = this.clip_subdivision_menu(event.position(), clip);
                        this.open_menu(menu);
                        cx.notify();
                    }),
                )
                .into_any_element(),
            );
        }

        if takes_an_octave(recipe.preset) {
            rows.push(
                self.picker_row(
                    "part-octave",
                    Key::PartOctave,
                    octave_text(recipe.octave),
                    cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        let menu = this.clip_octave_menu(event.position(), clip);
                        this.open_menu(menu);
                        cx.notify();
                    }),
                )
                .into_any_element(),
            );
        }

        for dial in dials_for(&recipe) {
            let dial = *dial;
            let fraction = dial.fraction(&recipe);
            rows.push(
                value_slider(
                    ("part-dial", dial_element_key(dial)),
                    self.t(dial.label()),
                    dial_text(dial, &recipe, straight),
                    fraction,
                    theme.accent,
                    SliderFill::FromStart,
                    &theme,
                    cx.listener(move |this, event: &MouseDownEvent, _, _| {
                        this.begin_drag(Drag::PartDial {
                            clip,
                            dial,
                            start_fraction: fraction,
                            start_x: event.position.x,
                        });
                    }),
                )
                .into_any_element(),
            );
        }

        if takes_a_groove(recipe.preset) {
            rows.push(
                self.picker_row(
                    "part-groove",
                    Key::PartGroove,
                    recipe.groove.clone(),
                    cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        let menu = this.clip_groove_menu(event.position(), clip);
                        this.open_menu(menu);
                        cx.notify();
                    }),
                )
                .into_any_element(),
            );
        }

        // The seed is shown, and typeable, because "another take" is the *next* seed and not a
        // random one. That is what makes a take somebody liked reachable again — but only by
        // somebody who saw its number and can put it back.
        rows.push(
            self.picker_row(
                "part-seed",
                Key::PartSeed,
                recipe.seed.to_string(),
                cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                    let title = this.t(Key::SetSeedTitle);
                    let current = this
                        .session
                        .clip_recipe(clip)
                        .map_or_else(String::new, |recipe| recipe.seed.to_string());
                    this.open_prompt(Prompt::new(title, PromptTarget::Seed(clip), current));
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        rows.push(
            div()
                .flex()
                .gap_1()
                .child(div().flex_1().child(button(
                    "part-reroll",
                    self.t(Key::MenuRerollClip),
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(move |this, _, _, cx| {
                        this.reroll_clip(clip);
                        cx.notify();
                    }),
                )))
                .child(div().flex_1().child(button(
                    "part-freeze",
                    self.t(Key::MenuFreezeClip),
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(move |this, _, _, cx| {
                        this.freeze_clip(clip);
                        cx.notify();
                    }),
                )))
                .into_any_element(),
        );
        rows.push(divider(&theme).into_any_element());
        rows
    }

    /// A muted line naming the group of controls under it.
    pub(crate) fn group_heading(&self, key: Key) -> impl IntoElement + use<> {
        div()
            .flex()
            .items_center()
            .h(Metrics::CONTROL_HEIGHT)
            .text_xs()
            .text_color(self.theme.text_muted)
            .child(self.t(key))
    }

    /// A labelled row whose value is a button opening a menu of the alternatives.
    ///
    /// The same shape as the instrument row above it, deliberately: both are "this is what it is,
    /// press to choose another", and a second shape for the same idea would only be a second thing
    /// to learn.
    fn picker_row<F>(
        &self,
        id: &'static str,
        label: Key,
        value: String,
        on_click: F,
    ) -> impl IntoElement + use<F>
    where
        F: Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
    {
        let theme = self.theme.clone();
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .h(Metrics::CONTROL_HEIGHT)
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(label)),
            )
            .child(div().w(px(128.0)).child(button(
                id,
                value,
                ButtonStyle::Normal,
                false,
                theme.accent,
                &theme,
                on_click,
            )))
    }

    /// Moves one dial and writes the clip again.
    ///
    /// A move too small to change the stored value writes nothing at all. That is what keeps one
    /// flick of a trackpad from becoming thirty undo steps and thirty graph rebuilds: a drag is
    /// wrapped in a transaction and pays for none of that, but a wheel is not a gesture with a
    /// beginning and an end, so each notch would otherwise be a separate edit.
    pub(crate) fn set_dial(&mut self, clip: ClipId, dial: Dial, fraction: f32) {
        let Some(current) = self.session.clip_recipe(clip) else {
            return;
        };
        let mut recipe = current.clone();
        dial.set(&mut recipe, fraction);
        if &recipe == current {
            return;
        }
        if self.session.set_clip_recipe(clip, recipe).is_ok() {
            self.forget_rewritten_notes(clip);
        }
    }

    /// Applies a dial drag, measured in pixels from where it began.
    ///
    /// The same travel as a plugin parameter, deliberately: these bars sit in the same panel as
    /// the instrument's own controls and are dragged the same way, so a hand that has learned one
    /// should not have to learn the other.
    pub(crate) fn drag_dial(&mut self, clip: ClipId, dial: Dial, start_fraction: f32, delta: f32) {
        self.set_dial(clip, dial, start_fraction + delta / DRAG_RANGE_PIXELS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe(preset: ClipPreset) -> ClipRecipe {
        ClipRecipe::new(preset, 1)
    }

    #[test]
    fn a_dial_reads_back_what_it_was_set_to() {
        // The bar is drawn from `fraction` and dragged into `set`, so a value that did not survive
        // the round trip would make the bar jump away from the pointer while it was being dragged.
        for dial in [Dial::Density, Dial::Gate, Dial::Intensity, Dial::Humanize] {
            for target in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let mut recipe = recipe(ClipPreset::Lead);
                dial.set(&mut recipe, target);
                assert!(
                    (dial.fraction(&recipe) - target).abs() < 1e-6,
                    "{dial:?} at {target}"
                );
            }
        }
    }

    #[test]
    fn the_swing_dial_runs_from_straight_to_dotted_and_no_further() {
        // Swing is stored as whole percent, so it round trips to within one step of the bar
        // rather than exactly — but it must never leave the range the composer can play.
        let mut recipe = recipe(ClipPreset::Drums);

        Dial::Swing.set(&mut recipe, 0.0);
        assert_eq!(
            recipe.swing, SWING_MIN,
            "the bottom of the dial is straight"
        );

        Dial::Swing.set(&mut recipe, 1.0);
        assert_eq!(recipe.swing, SWING_MAX);

        // Below the bottom is not "swing the other way", it is rushing the offbeat.
        Dial::Swing.set(&mut recipe, -1.0);
        assert_eq!(recipe.swing, SWING_MIN);

        Dial::Swing.set(&mut recipe, 2.0);
        assert_eq!(recipe.swing, SWING_MAX);

        for percent in SWING_MIN..=SWING_MAX {
            let mut recipe = recipe.clone();
            recipe.swing = percent;
            let fraction = Dial::Swing.fraction(&recipe);
            let mut round_tripped = recipe.clone();
            Dial::Swing.set(&mut round_tripped, fraction);
            assert_eq!(round_tripped.swing, percent, "{percent}%");
        }
    }

    #[test]
    fn the_swing_dial_reaches_every_whole_percent_of_its_range() {
        // The one dial that is not a float: it stores a `u8` of percent, over a range narrow
        // enough that a bar rounding to hundredths would skip values the readout can show.
        let mut recipe = recipe(ClipPreset::Drums);
        let span = f32::from(SWING_MAX - SWING_MIN);
        for percent in 0..=(SWING_MAX - SWING_MIN) {
            Dial::Swing.set(&mut recipe, f32::from(percent) / span);
            assert_eq!(recipe.swing, SWING_MIN + percent);
        }
    }

    #[test]
    fn a_drum_kit_is_offered_a_groove_where_every_other_preset_is_offered_a_density() {
        // The same rule `a_drum_kit_takes_its_density_from_its_groove_and_not_from_the_dial` pins
        // in the composer, stated where the interface can break it: a density dial on a kit would
        // be a control that does nothing, and no groove picker on one would leave the only dial a
        // kit *does* read unreachable.
        for preset in ClipPreset::ALL {
            let dials = dials_for(&recipe(preset));
            // Every preset reads the density, the kit included: it leans on the groove rather
            // than replacing it, thinning from the weakest hits and filling the free steps with
            // ghosts. Which groove is still the groove, which is why the kit has a picker too.
            assert!(dials.contains(&Dial::Density), "{}", preset.name());
            // The gate is the kit's exception: a one-shot drum ignores its note-off, so
            // shortening one changes nothing anybody can hear.
            assert_eq!(
                dials.contains(&Dial::Gate),
                !takes_a_groove(preset),
                "{} offers the wrong gate row",
                preset.name()
            );
            // And the fill is the kit's alone — nothing else has a last bar to announce.
            assert_eq!(
                dials.contains(&Dial::Fill),
                takes_a_groove(preset),
                "{} offers the wrong fill row",
                preset.name()
            );
            assert_eq!(
                takes_a_subdivision(preset),
                !takes_a_groove(preset),
                "{} offers the wrong subdivision row",
                preset.name()
            );
            // A kit's pitches are General MIDI drum numbers rather than notes: moving a kick up
            // an octave would not raise it, it would make it a different drum.
            assert_eq!(
                takes_an_octave(preset),
                !takes_a_groove(preset),
                "{} offers the wrong octave row",
                preset.name()
            );
        }
        assert!(takes_a_groove(ClipPreset::Drums));
        assert!(!takes_a_subdivision(ClipPreset::Drums));
        assert!(!takes_an_octave(ClipPreset::Drums));

        // Everything reads how hard and how loose and how evenly it is played, kit included.
        for preset in ClipPreset::ALL {
            let dials = dials_for(&recipe(preset));
            assert!(dials.contains(&Dial::Intensity), "{}", preset.name());
            assert!(dials.contains(&Dial::Dynamics), "{}", preset.name());
            assert!(dials.contains(&Dial::Humanize), "{}", preset.name());
            assert!(dials.contains(&Dial::Swing), "{}", preset.name());
        }

        // The syncopation reaches a part that rolls its own figure and nothing else. A kit plays
        // its groove and a pad sounds the chord where the chord is; a dial on either would sweep
        // its whole travel and move not one note.
        for preset in ClipPreset::ALL {
            let rolls_its_own = !matches!(preset, ClipPreset::Drums | ClipPreset::Pad);
            assert_eq!(
                dials_for(&recipe(preset)).contains(&Dial::Syncopation),
                rolls_its_own,
                "{} offers the wrong syncopation row",
                preset.name()
            );
        }
    }

    #[test]
    fn an_octave_offset_reads_with_its_sign() {
        // Zero is not "no octave" but "the one the preset chose", and a bare `0` between `-1` and
        // `1` in a menu reads as the absence of a setting rather than as the middle of one.
        assert_eq!(octave_text(0), "±0");
        assert_eq!(octave_text(1), "+1");
        assert_eq!(octave_text(-2), "-2");
        assert_eq!(octave_text(9), "+2", "clamped to what the picker offers");
        assert_eq!(octave_choices().count(), 5);
        assert!(octave_choices().contains(&0));
    }

    #[test]
    fn a_part_on_a_triplet_grid_is_not_offered_a_swing_dial() {
        // Swing pushes a straight offbeat toward the third triplet; a grid already there has
        // nothing left to be pushed. The composer returns no offset at all for one, so a dial
        // drawn here would sweep its whole travel and change not one tick.
        for subdivision in Subdivision::ALL {
            let mut recipe = recipe(ClipPreset::Chords);
            recipe.subdivision = subdivision;
            assert_eq!(
                dials_for(&recipe).contains(&Dial::Swing),
                !subdivision.is_triplet(),
                "{}",
                subdivision.name()
            );
        }

        // The kit is the exception at both ends: it ignores the subdivision, so its swing is
        // never inert whatever the rest of the recipe says.
        let mut kit = recipe(ClipPreset::Drums);
        kit.subdivision = Subdivision::EighthTriplet;
        assert!(dials_for(&kit).contains(&Dial::Swing));
    }

    #[test]
    fn changing_the_preset_keeps_the_dials_somebody_moved_and_replaces_the_ones_they_did_not() {
        // The stab is the case that forced the rule: its identity is a gate near the floor and a
        // density near the ceiling, so choosing it from a pad while keeping the pad's dials would
        // have written a pad under a new name.
        let pad = recipe(ClipPreset::Pad);
        let stab = with_preset(&pad, ClipPreset::Stab);
        assert_eq!(stab.preset, ClipPreset::Stab);
        assert_eq!(stab.gate, ClipRecipe::new(ClipPreset::Stab, 1).gate);
        assert!(stab.gate < 1.0, "a stab that is not short is a chord part");
        assert_eq!(stab.density, ClipRecipe::new(ClipPreset::Stab, 1).density);

        // And a dial that was moved is the person's, not the preset's.
        let mut deliberate = recipe(ClipPreset::Pad);
        Dial::Gate.set(&mut deliberate, 0.5);
        let moved = deliberate.gate;
        let stab = with_preset(&deliberate, ClipPreset::Stab);
        assert_eq!(stab.gate, moved, "the preset overwrote a deliberate gate");

        // The seed never moves: another take is the next seed, and changing what the part is
        // should not also change which take of it you are hearing.
        assert_eq!(stab.seed, deliberate.seed);
    }

    #[test]
    fn a_movement_too_small_to_show_moves_nothing() {
        // What `set_dial` recognises to avoid an undo step and a graph rebuild per pointer event.
        // Sweeping a bar is hundreds of them, so a dial that acted on every one would fill the
        // history with a drag nobody could take back in a single press.
        //
        // The two numbers are either side of the coarsest resolution any of these has: a swing is
        // a whole percent of a 25-point range, so it moves in fortieths, and everything else is a
        // hundredth. Three thousandths is inside all of them and a twentieth is outside all of
        // them, which is what makes one set of numbers do for the whole list.
        for dial in [
            Dial::Density,
            Dial::Gate,
            Dial::Intensity,
            Dial::Humanize,
            Dial::Swing,
        ] {
            let mut recipe = recipe(ClipPreset::Lead);
            dial.set(&mut recipe, 0.5);
            let settled = recipe.clone();

            dial.set(&mut recipe, 0.503);
            assert_eq!(recipe, settled, "{dial:?} moved on three thousandths");

            // And it is steady rather than dead: a movement it can show does show.
            dial.set(&mut recipe, 0.55);
            assert_ne!(recipe, settled, "{dial:?} did not move on a twentieth");
        }
    }

    #[test]
    fn what_a_dial_stores_is_what_the_readout_says() {
        // The readout rounds to whole percent, so the stored value does too. Otherwise 0.523 and
        // 0.519 both print "52%" while writing two different parts, and the one on screen is not
        // the one that can be got back to.
        for hundredths in 0..=100 {
            let mut recipe = recipe(ClipPreset::Lead);
            Dial::Density.set(&mut recipe, hundredths as f32 / 100.0 + 0.004);
            assert_eq!(
                dial_text(Dial::Density, &recipe, "straight"),
                format!("{hundredths}%")
            );
            assert!((recipe.density * 100.0 - hundredths as f32).abs() < 1e-4);
        }
    }

    #[test]
    fn a_dial_reads_as_a_percentage_and_straight_swing_reads_as_a_word() {
        let mut recipe = recipe(ClipPreset::Lead);
        recipe.density = 0.5;
        assert_eq!(dial_text(Dial::Density, &recipe, "straight"), "50%");

        recipe.swing = SWING_MIN;
        assert_eq!(dial_text(Dial::Swing, &recipe, "straight"), "straight");
        recipe.swing = 66;
        assert_eq!(dial_text(Dial::Swing, &recipe, "straight"), "66%");

        // The gate reads the share of the gap it stores, not the bar's position. At the bottom of
        // its travel the bar is empty and the note is still a twentieth long; a readout of 0%
        // there would be describing the control rather than the music.
        Dial::Gate.set(&mut recipe, 0.0);
        assert_eq!(Dial::Gate.fraction(&recipe), 0.0);
        assert_eq!(dial_text(Dial::Gate, &recipe, "straight"), "5%");
        Dial::Gate.set(&mut recipe, 1.0);
        assert_eq!(dial_text(Dial::Gate, &recipe, "straight"), "100%");
    }

    #[test]
    fn every_dial_gets_its_own_element_key() {
        let mut seen = std::collections::BTreeSet::new();
        for dial in [
            Dial::Density,
            Dial::Gate,
            Dial::Intensity,
            Dial::Swing,
            Dial::Humanize,
        ] {
            assert!(seen.insert(dial_element_key(dial)), "{dial:?} collided");
        }
    }
}
