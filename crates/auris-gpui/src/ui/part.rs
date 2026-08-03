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

/// One continuous dial on a [`ClipRecipe`].
///
/// The seed, the preset and the groove are all choices from a set and are picked from a menu; these
/// four are the ones with a range, and so the ones that get a bar to drag.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Dial {
    /// How busy the part is.
    Density,
    /// How hard it is played.
    Intensity,
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
            Dial::Intensity => Key::PartIntensity,
            Dial::Swing => Key::PartSwing,
            Dial::Humanize => Key::PartHumanize,
        }
    }

    /// Where the bar is filled to, from 0 to 1.
    pub fn fraction(self, recipe: &ClipRecipe) -> f32 {
        let value = match self {
            Dial::Density => recipe.density,
            Dial::Intensity => recipe.intensity,
            Dial::Humanize => recipe.humanize,
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
            Dial::Intensity => recipe.intensity = whole_percent(fraction),
            Dial::Humanize => recipe.humanize = whole_percent(fraction),
            Dial::Swing => {
                let span = f32::from(SWING_MAX - SWING_MIN);
                recipe.swing = SWING_MIN + (fraction * span).round() as u8;
            }
        }
    }

    /// How far one notch of the wheel moves the bar: one of whatever this dial stores.
    ///
    /// Exactly one step, not two. A notch coarser than the stored unit would let a fraction of a
    /// notch still cross a boundary, and the sub-notch events a trackpad emits by the dozen would
    /// each be a real edit again — which is the whole thing [`Self::set`] rounds to avoid. A drag
    /// is where a dial is swept; the wheel is where it is nudged.
    pub fn step(self) -> f32 {
        match self {
            Dial::Swing => 1.0 / f32::from(SWING_MAX - SWING_MIN),
            _ => 0.01,
        }
    }
}

/// A fraction rounded to the nearest whole percent, which is the resolution the readout has.
fn whole_percent(fraction: f32) -> f32 {
    (fraction * 100.0).round() / 100.0
}

/// The dials a preset actually reads, in the order they are drawn.
///
/// A drum kit has no density row. How busy a kit is *is* which groove it plays, so the dial is
/// replaced by the groove picker rather than shown doing nothing — the same rule the composer
/// itself keeps, and for the same reason.
pub fn dials_for(preset: ClipPreset) -> &'static [Dial] {
    match preset {
        ClipPreset::Drums => &[Dial::Intensity, Dial::Swing, Dial::Humanize],
        _ => &[Dial::Density, Dial::Intensity, Dial::Swing, Dial::Humanize],
    }
}

/// Whether a preset's groove is worth offering, which is to say whether anything reads it.
pub fn takes_a_groove(preset: ClipPreset) -> bool {
    matches!(preset, ClipPreset::Drums)
}

/// What a dial reads as, given the word this language uses for unswung eighths.
pub fn dial_text(dial: Dial, recipe: &ClipRecipe, straight: &str) -> String {
    match dial {
        Dial::Swing if recipe.swing <= SWING_MIN => straight.to_string(),
        Dial::Swing => format!("{}%", recipe.swing),
        _ => format!("{}%", (dial.fraction(recipe) * 100.0).round() as i32),
    }
}

/// The value a dial takes after `notches` of the wheel.
pub fn value_after_scroll(dial: Dial, recipe: &ClipRecipe, notches: f32) -> f32 {
    dial.fraction(recipe) + notches * dial.step()
}

/// A stable per-dial element key, so gpui can track hover state across frames.
fn dial_element_key(dial: Dial) -> usize {
    match dial {
        Dial::Density => 0,
        Dial::Intensity => 1,
        Dial::Swing => 2,
        Dial::Humanize => 3,
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

        for dial in dials_for(recipe.preset) {
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
                    cx.listener(move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                        let notches = f32::from(event.delta.pixel_delta(px(16.0)).y) / 16.0;
                        let Some(recipe) = this.session.clip_recipe(clip) else {
                            return;
                        };
                        let next = value_after_scroll(dial, recipe, notches);
                        this.set_dial(clip, dial, next);
                        // The panel behind these rows scrolls, and a wheel that both turned the
                        // dial and scrolled it out from under the pointer would be unusable.
                        cx.stop_propagation();
                        cx.notify();
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
        let _ = self.session.set_clip_recipe(clip, recipe);
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
        for dial in [Dial::Density, Dial::Intensity, Dial::Humanize] {
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
    fn one_notch_of_the_wheel_moves_the_swing_dial_one_percent() {
        // A step finer than the stored unit would round back to where it started, and the wheel
        // would look broken on the one dial that is not a float.
        let mut recipe = recipe(ClipPreset::Drums);
        let next = value_after_scroll(Dial::Swing, &recipe, 1.0);
        Dial::Swing.set(&mut recipe, next);
        assert_eq!(recipe.swing, SWING_MIN + 1);

        let next = value_after_scroll(Dial::Swing, &recipe, -1.0);
        Dial::Swing.set(&mut recipe, next);
        assert_eq!(recipe.swing, SWING_MIN);
    }

    #[test]
    fn a_drum_kit_is_offered_a_groove_where_every_other_preset_is_offered_a_density() {
        // The same rule `a_drum_kit_takes_its_density_from_its_groove_and_not_from_the_dial` pins
        // in the composer, stated where the interface can break it: a density dial on a kit would
        // be a control that does nothing, and no groove picker on one would leave the only dial a
        // kit *does* read unreachable.
        for preset in ClipPreset::ALL {
            let has_density = dials_for(preset).contains(&Dial::Density);
            assert_eq!(
                has_density,
                !takes_a_groove(preset),
                "{} offers the wrong dials",
                preset.name()
            );
        }
        assert!(takes_a_groove(ClipPreset::Drums));

        // Everything reads how hard and how loose it is played, kit included.
        for preset in ClipPreset::ALL {
            let dials = dials_for(preset);
            assert!(dials.contains(&Dial::Intensity), "{}", preset.name());
            assert!(dials.contains(&Dial::Humanize), "{}", preset.name());
            assert!(dials.contains(&Dial::Swing), "{}", preset.name());
        }
    }

    #[test]
    fn a_movement_too_small_to_show_moves_nothing() {
        // What `set_dial` recognises to avoid an undo step and a graph rebuild per wheel event.
        // A trackpad emits a stream of fractional notches, and most of them land inside a step.
        for dial in [Dial::Density, Dial::Intensity, Dial::Humanize, Dial::Swing] {
            let mut recipe = recipe(ClipPreset::Lead);
            dial.set(&mut recipe, 0.5);
            let settled = recipe.clone();

            let nudged = value_after_scroll(dial, &recipe, 0.3);
            dial.set(&mut recipe, nudged);
            assert_eq!(recipe, settled, "{dial:?} moved on a third of a notch");

            // A whole notch does move it, or the wheel would be dead rather than steady.
            let notch = value_after_scroll(dial, &recipe, 1.0);
            dial.set(&mut recipe, notch);
            assert_ne!(recipe, settled, "{dial:?} did not move on a full notch");
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
    }

    #[test]
    fn every_dial_gets_its_own_element_key() {
        let mut seen = std::collections::BTreeSet::new();
        for dial in [Dial::Density, Dial::Intensity, Dial::Swing, Dial::Humanize] {
            assert!(seen.insert(dial_element_key(dial)), "{dial:?} collided");
        }
    }
}
