//! The dials on a generated clip: what each one reads, what it writes, and which presets have it.
//!
//! Everything here that decides something is a free function over a [`ClipRecipe`], for the reason
//! given in [`crate::ui::context_menu`]: a decision made inside a `render` method can only be
//! checked by opening a window and looking at it. The `impl AurisApp` block below is the part that
//! genuinely needs a window — it draws the rows and hangs the gestures off them.

use auris_i18n::Key;
use auris_session::prelude::*;

use gpui::{
    AnyElement, Bounds, IntoElement, MouseDownEvent, Pixels, Point, canvas, div, point, prelude::*,
    px, size,
};

use crate::app::{AurisApp, Drag};
use crate::theme::Metrics;
use crate::ui::paint;
use crate::ui::prompt::{Prompt, PromptTarget};
use crate::ui::widgets::{
    ButtonStyle, PickerBehavior, RowColumn, SliderFill, button, divider, dragged, picker_row,
    value_slider,
};

/// How wide the value button in one of this panel's rows is drawn.
///
/// A column, so the buttons line up down a panel whose width is the user's to change.
const VALUE_WIDTH: Pixels = px(128.0);

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
/// from a menu; these are the ones with a range, and so the ones that get a bar to drag. How
/// *loose* the clip is played stopped being one of them: the humanise is a performance
/// transform now — see `crate::ui::performance` — and turning it no longer rewrites the text.
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
    // An authored arpeggio strikes exactly the supplied rhythm. Its generated rate and
    // syncopation cannot change those onsets, so neither belongs on the pad or in a slider.
    if recipe.preset == ClipPreset::Arp && recipe.rhythm.is_some() {
        return if recipe.subdivision.is_triplet() {
            &[Dial::Gate, Dial::Intensity, Dial::Dynamics]
        } else {
            &[Dial::Gate, Dial::Intensity, Dial::Dynamics, Dial::Swing]
        };
    }
    // A kit reads neither the gate nor the syncopation: a one-shot drum ignores its note-off,
    // and where it plays is which groove it plays. It ignores the subdivision too, which is why
    // its swing is the one that is never inert. The fill is its alone — nothing else has a last
    // bar to announce.
    if recipe.preset.is_drums() {
        if matches!(recipe.preset, ClipPreset::Kick | ClipPreset::Hat) {
            return &[Dial::Density, Dial::Intensity, Dial::Dynamics, Dial::Swing];
        }
        return &[
            Dial::Density,
            Dial::Fill,
            Dial::Intensity,
            Dial::Dynamics,
            Dial::Swing,
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
        ];
    }
    &[
        Dial::Density,
        Dial::Syncopation,
        Dial::Gate,
        Dial::Intensity,
        Dial::Dynamics,
        Dial::Swing,
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
    preset.is_drums()
}

/// Whether a preset's subdivision is worth offering.
///
/// Everything but the kit, for the reason the composer gives: a groove is sixteen steps read by
/// index, so a kit on any other grid would scramble it rather than divide it.
pub fn takes_a_subdivision(preset: ClipPreset) -> bool {
    !preset.is_drums()
}

/// Whether a preset's register is worth offering.
///
/// Everything but the kit, whose pitches are General MIDI drum numbers rather than notes: moving
/// a kick up an octave would not raise it, it would turn it into a different drum.
pub fn takes_an_octave(preset: ClipPreset) -> bool {
    !preset.is_drums()
}

/// Whether a drum control reaches at least one writer rather than an authored fixed rhythm.
fn drum_control_applies(recipe: &ClipRecipe, dial: Dial) -> bool {
    if !recipe.drum_voices.is_empty() {
        return recipe.drum_voices.iter().any(|voice| {
            recipe
                .drum_map
                .as_ref()
                .is_none_or(|map| map.voices.contains_key(&voice.role))
                && voice
                    .recipe
                    .as_deref()
                    .is_some_and(|writer| drum_control_applies(writer, dial))
        });
    }
    match dial {
        Dial::Density => recipe.rhythm.is_none(),
        Dial::Fill => {
            recipe.rhythm.is_none()
                && matches!(recipe.preset, ClipPreset::Drums | ClipPreset::Snare)
        }
        Dial::Intensity | Dial::Dynamics | Dial::Swing => true,
        Dial::Gate | Dial::Syncopation => false,
    }
}

/// The recipe under the pad: right adds complexity and up plays harder.
fn with_part_position(
    recipe: &ClipRecipe,
    bounds: Bounds<Pixels>,
    at: Point<Pixels>,
) -> ClipRecipe {
    let mut next = recipe.clone();
    Dial::Density.set(
        &mut next,
        f32::from(at.x - bounds.origin.x) / f32::from(bounds.size.width).max(1.0),
    );
    Dial::Intensity.set(
        &mut next,
        1.0 - f32::from(at.y - bounds.origin.y) / f32::from(bounds.size.height).max(1.0),
    );
    next
}

/// The recipe a clip takes when its preset changes.
///
/// A dial somebody moved is theirs, and follows them across the change. A dial still sitting
/// exactly where the old preset put it is the old preset's opinion rather than anybody's, and
/// becomes the new preset's instead.
///
/// Deliberately adjusted articulation belongs to the clip when its musical role changes.
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
        let drummer = self
            .project()
            .midi_clip(clip)
            .and_then(|(track, _)| self.project().track(track))
            .is_some_and(|track| track.kind.is_drum());
        let has_pad = dials_for(&recipe).contains(&Dial::Density)
            && (!drummer || drum_control_applies(&recipe, Dial::Density));

        let mut rows: Vec<AnyElement> = vec![
            self.group_heading(if drummer {
                Key::DrummerHeading
            } else {
                Key::PartPlayerHeading
            })
            .into_any_element(),
        ];
        if !drummer {
            rows.push(self.melodic_preset_buttons(clip, recipe.preset, cx));
        }
        if has_pad {
            rows.push(self.part_pad(clip, &recipe, cx));
        }
        if drummer {
            rows.push(
                self.picker_row(
                    "part-preset",
                    Key::PartPreset,
                    self.t(crate::ui::context_menu::preset_key(recipe.preset))
                        .to_string(),
                    Self::opens_menu(cx, move |this, at| this.clip_preset_menu(at, clip)),
                )
                .into_any_element(),
            );
        } else {
            rows.push(self.group_heading(Key::PartPhrasing).into_any_element());
        }

        rows.push(
            self.command_row(
                "part-rhythm-edit",
                Key::PartRhythm,
                self.t(Key::PartRhythmEdit).to_string(),
                cx.listener(move |this, _, _, cx| this.open_rhythm_window(clip, cx)),
            )
            .into_any_element(),
        );

        if takes_a_subdivision(recipe.preset) {
            rows.push(
                self.picker_row(
                    "part-subdivision",
                    Key::PartSubdivision,
                    self.t(crate::ui::context_menu::subdivision_key(recipe.subdivision))
                        .to_string(),
                    Self::opens_menu(cx, move |this, at| this.clip_subdivision_menu(at, clip)),
                )
                .into_any_element(),
            );
        }

        if takes_an_octave(recipe.preset) {
            rows.push(self.part_octave_buttons(clip, recipe.octave, cx));
        }

        for dial in dials_for(&recipe) {
            let dial = *dial;
            if (drummer && !drum_control_applies(&recipe, dial))
                || (has_pad && matches!(dial, Dial::Density | Dial::Intensity))
            {
                continue;
            }
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
                .debug_selector(move || format!("part-dial-{}", dial_element_key(dial)))
                .into_any_element(),
            );
        }

        if takes_a_groove(recipe.preset) && drum_control_applies(&recipe, Dial::Density) {
            rows.push(
                self.picker_row(
                    "part-groove",
                    Key::PartGroove,
                    groove_catalog()
                        .iter()
                        .find(|groove| groove.name == recipe.groove)
                        .map(|groove| {
                            auris_i18n::audio::theory_description(
                                groove.description,
                                self.language(),
                            )
                            .to_string()
                        })
                        .unwrap_or_else(|| recipe.groove.clone()),
                    Self::opens_menu(cx, move |this, at| this.clip_groove_menu(at, clip)),
                )
                .into_any_element(),
            );
        }

        if drummer {
            rows.extend(self.drummer_voice_rows(clip, &recipe, cx));
        } else {
            rows.push(self.group_heading(Key::PartTakes).into_any_element());
        }

        // The seed is shown, and typeable, because "another take" is the *next* seed and not a
        // random one. That is what makes a take somebody liked reachable again — but only by
        // somebody who saw its number and can put it back.
        rows.push(
            self.command_row(
                "part-seed",
                if drummer {
                    Key::PartSeed
                } else {
                    Key::PartTakeNumber
                },
                recipe.seed.to_string(),
                cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                    let title = this.t(if drummer {
                        Key::SetSeedTitle
                    } else {
                        Key::PartTakeNumber
                    });
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
        // A standing note, not a dialog: it appears when the clip's notes drift from what the
        // recipe last wrote and stays for as long as they differ, which is the same rule the
        // costly-feature warnings follow. The buttons below still work exactly as they say.
        if self.session.clip_hand_edited(clip) {
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .min_h(Metrics::CONTROL_HEIGHT)
                    .text_xs()
                    .text_color(theme.warning)
                    .child(self.t(Key::PartEditedByHand))
                    .into_any_element(),
            );
        }
        if !drummer {
            rows.push(
                button(
                    "part-regenerate",
                    self.t(Key::MenuRegenerateClip),
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(move |this, _, _, cx| {
                        this.regenerate_clip(clip);
                        cx.notify();
                    }),
                )
                .into_any_element(),
            );
        }
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

    /// The melodic writers are visible choices, with the current role highlighted.
    fn melodic_preset_buttons(
        &self,
        clip: ClipId,
        current: ClipPreset,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let choices: Vec<_> = ClipPreset::ALL
            .into_iter()
            .filter(|preset| !preset.is_drums())
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .pb_2()
            .children(choices.chunks(2).map(|pair| {
                div().flex().gap_1().children(pair.iter().map(|&preset| {
                    button(
                        ("part-role", preset as usize),
                        self.t(crate::ui::context_menu::preset_key(preset)),
                        ButtonStyle::Normal,
                        current == preset,
                        self.theme.accent,
                        &self.theme,
                        cx.listener(move |this, _, _, cx| {
                            this.set_clip_preset(clip, preset);
                            cx.notify();
                        }),
                    )
                    .flex_1()
                    .min_w_0()
                    .debug_selector(move || format!("part-role-{}", preset.name()))
                }))
            }))
            .into_any_element()
    }

    /// One click selects a register without opening a menu over the performance controls.
    fn part_octave_buttons(
        &self,
        clip: ClipId,
        current: i32,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .py_1()
            .child(
                div()
                    .text_xs()
                    .text_color(self.theme.text_muted)
                    .child(self.t(Key::PartOctave)),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .children(octave_choices().map(|octave| {
                        button(
                            ("part-register", (octave + OCTAVE_REACH) as usize),
                            octave_text(octave),
                            ButtonStyle::Normal,
                            current == octave,
                            self.theme.accent,
                            &self.theme,
                            cx.listener(move |this, _, _, cx| {
                                this.set_clip_octave(clip, octave);
                                cx.notify();
                            }),
                        )
                        .flex_1()
                        .min_w_0()
                        .debug_selector(move || format!("part-register-{octave}"))
                    })),
            )
            .into_any_element()
    }

    /// Two musical decisions in one gesture, committed together by the normal drag transaction.
    fn part_pad(
        &self,
        clip: ClipId,
        recipe: &ClipRecipe,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        let painted = theme.clone();
        let recorded = std::rc::Rc::new(std::cell::Cell::new(None));
        let pressed = recorded.clone();
        let density = Dial::Density.fraction(recipe);
        let intensity = Dial::Intensity.fraction(recipe);
        let id = if recipe.preset.is_drums() {
            "drummer-pad"
        } else {
            "part-pad"
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(format!(
                        "{} {}%",
                        self.t(Key::DrummerComplexity),
                        (density * 100.0).round() as u8
                    ))
                    .child(format!(
                        "{} {}%",
                        self.t(Key::PartIntensity),
                        (intensity * 100.0).round() as u8
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::DrummerLoud)),
            )
            .child(
                div()
                    .id(id)
                    .debug_selector(move || id.to_string())
                    .h(px(124.0))
                    .w_full()
                    .flex_shrink_0()
                    .p_2()
                    .bg(theme.surface_sunken)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(Metrics::RADIUS_SM)
                    .cursor_pointer()
                    .child(
                        canvas(
                            move |bounds, _, _| recorded.set(Some(bounds)),
                            move |bounds, _, window, _| {
                                let x = bounds.origin.x + bounds.size.width * density;
                                let y = bounds.origin.y + bounds.size.height * (1.0 - intensity);
                                paint::vline(
                                    window,
                                    bounds,
                                    bounds.center().x,
                                    px(1.0),
                                    painted.border_subtle,
                                );
                                paint::hline(
                                    window,
                                    bounds,
                                    bounds.center().y,
                                    painted.border_subtle,
                                );
                                paint::vline(window, bounds, x, px(1.0), painted.accent_soft);
                                paint::hline(window, bounds, y, painted.accent_soft);
                                paint::rounded_rect(
                                    window,
                                    Bounds {
                                        origin: point(x - px(6.0), y - px(6.0)),
                                        size: size(px(12.0), px(12.0)),
                                    },
                                    px(6.0),
                                    painted.accent,
                                );
                            },
                        )
                        .size_full(),
                    )
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            if let Some(bounds) = pressed.get() {
                                this.begin_drag(Drag::PartPad { clip, bounds });
                                this.drag_part_pad(clip, bounds, event.position);
                                cx.notify();
                            }
                        }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::DrummerSimple))
                    .child(self.t(Key::DrummerSoft))
                    .child(self.t(Key::DrummerComplex)),
            )
            .into_any_element()
    }

    /// Independent writers keep their sound assignments and their neighbours' stored notes.
    fn drummer_voice_rows(
        &self,
        clip: ClipId,
        recipe: &ClipRecipe,
        cx: &mut gpui::Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = self.theme.clone();
        let mut rows = Vec::new();
        for (index, voice) in recipe.drum_voices.iter().enumerate() {
            let Some(writer) = voice.recipe.as_deref() else {
                continue;
            };
            if recipe
                .drum_map
                .as_ref()
                .is_some_and(|map| !map.voices.contains_key(&voice.role))
            {
                continue;
            }
            if rows.is_empty() {
                rows.push(self.group_heading(Key::DrummerKitPieces).into_any_element());
            }
            let retake = voice.name.clone();
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .text_color(theme.text)
                    .child(format!(
                        "{} · {}",
                        self.t(crate::ui::drums::role_key(voice.role)),
                        voice.name
                    ))
                    .child(button(
                        ("drummer-voice-reroll", index),
                        self.t(Key::MenuRerollClip),
                        ButtonStyle::Ghost,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(move |this, _, _, cx| {
                            this.rewrite_drum_voice(clip, &retake, true);
                            cx.notify();
                        }),
                    ))
                    .into_any_element(),
            );
            for dial in [Dial::Density, Dial::Intensity] {
                if !drum_control_applies(writer, dial) {
                    continue;
                }
                let name = voice.name.clone();
                let fraction = dial.fraction(writer);
                rows.push(
                    value_slider(
                        ("drummer-voice-dial", index * 2 + dial_element_key(dial)),
                        self.t(if dial == Dial::Density {
                            Key::DrummerComplexity
                        } else {
                            dial.label()
                        }),
                        dial_text(dial, writer, self.t(Key::PartStraight)),
                        fraction,
                        theme.accent,
                        SliderFill::FromStart,
                        &theme,
                        cx.listener(move |this, event: &MouseDownEvent, _, _| {
                            this.begin_drag(Drag::DrumVoiceDial {
                                clip,
                                voice: name.clone(),
                                dial,
                                start_fraction: fraction,
                                start_x: event.position.x,
                            });
                        }),
                    )
                    .debug_selector(move || {
                        format!("drummer-voice-dial-{index}-{}", dial_element_key(dial))
                    })
                    .into_any_element(),
                );
            }
        }
        rows
    }

    /// Moves both pad coordinates through one recipe command, preserving the take's seed.
    pub(crate) fn drag_part_pad(
        &mut self,
        clip: ClipId,
        bounds: Bounds<Pixels>,
        at: Point<Pixels>,
    ) {
        let Some(current) = self.session.clip_recipe(clip) else {
            return;
        };
        let recipe = with_part_position(current, bounds, at);
        if &recipe != current && self.session.set_clip_recipe(clip, recipe).is_ok() {
            self.forget_rewritten_notes(clip);
        }
    }

    /// Rewrites only the kit piece whose dial is held, using the session's scoped command.
    pub(crate) fn drag_drum_voice_dial(
        &mut self,
        clip: ClipId,
        voice: &str,
        dial: Dial,
        start_fraction: f32,
        delta: f32,
    ) {
        let Some(current) = self.session.clip_recipe(clip).and_then(|recipe| {
            recipe
                .drum_voices
                .iter()
                .find(|part| part.name == voice)
                .and_then(|part| part.recipe.as_deref())
        }) else {
            return;
        };
        let mut recipe = current.clone();
        dial.set(&mut recipe, dragged(start_fraction, delta));
        if &recipe != current
            && self
                .session
                .set_drum_voice_recipe(clip, voice, recipe)
                .is_ok()
        {
            self.forget_rewritten_notes(clip);
        }
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

    /// A labelled row whose value field opens a menu of the alternatives.
    ///
    /// The same shape as the instrument row above it and as a plugin's own choice parameters,
    /// deliberately: all of them are "this is what it is, press to choose another", and a second
    /// selection treatment for the same idea would only be a second thing to learn. Drawn by
    /// [`crate::ui::widgets::picker_row`]; what is decided here is the panel's proportions and
    /// the language.
    pub(crate) fn picker_row<F>(
        &self,
        id: &'static str,
        label: Key,
        value: String,
        on_click: F,
    ) -> impl IntoElement + use<F>
    where
        F: Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
    {
        picker_row(
            id,
            self.t(label),
            value,
            RowColumn::Value(VALUE_WIDTH),
            PickerBehavior::Menu,
            &self.theme,
            on_click,
        )
    }

    /// A labelled row whose value runs a command instead of opening a choice list.
    fn command_row<F>(
        &self,
        id: &'static str,
        label: Key,
        value: String,
        on_click: F,
    ) -> impl IntoElement + use<F>
    where
        F: Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
    {
        picker_row(
            id,
            self.t(label),
            value,
            RowColumn::Value(VALUE_WIDTH),
            PickerBehavior::Command,
            &self.theme,
            on_click,
        )
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
    /// The same travel as a plugin parameter, and now by construction rather than by agreement:
    /// these bars sit in the same panel as the instrument's own controls and are dragged the same
    /// way, so a hand that has learned one should not have to learn the other.
    pub(crate) fn drag_dial(&mut self, clip: ClipId, dial: Dial, start_fraction: f32, delta: f32) {
        self.set_dial(clip, dial, dragged(start_fraction, delta));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{click, drag, open, paint};
    use gpui::TestAppContext;

    fn recipe(preset: ClipPreset) -> ClipRecipe {
        ClipRecipe::new(preset, 1)
    }

    #[test]
    fn drummer_pad_maps_both_axes_and_keeps_the_take_and_other_settings() {
        let original = recipe(ClipPreset::Drums);
        let bounds = Bounds {
            origin: point(px(10.0), px(20.0)),
            size: size(px(200.0), px(100.0)),
        };
        let quiet = with_part_position(&original, bounds, point(px(-10.0), px(200.0)));
        assert_eq!((quiet.density, quiet.intensity), (0.0, 0.0));
        let busy = with_part_position(&original, bounds, point(px(300.0), px(0.0)));
        assert_eq!((busy.density, busy.intensity), (1.0, 1.0));
        let mut middle = with_part_position(&original, bounds, point(px(61.0), px(45.0)));
        assert_eq!((middle.density, middle.intensity), (0.26, 0.75));
        middle.density = original.density;
        middle.intensity = original.intensity;
        assert_eq!(middle, original);
    }

    #[test]
    fn an_authored_arpeggio_keeps_expression_controls_without_an_inert_pad() {
        let mut written = recipe(ClipPreset::Arp);
        written.rhythm = Some("x...x...x...x...".into());
        for subdivision in Subdivision::ALL {
            written.subdivision = subdivision;
            let dials = dials_for(&written);
            assert!(!dials.contains(&Dial::Density));
            assert!(!dials.contains(&Dial::Syncopation));
            assert!(dials.contains(&Dial::Intensity));
            assert!(dials.contains(&Dial::Gate));
            assert_eq!(dials.contains(&Dial::Swing), !subdivision.is_triplet());
        }
    }

    #[test]
    fn authored_drum_rhythms_have_no_complexity_or_fill_controls() {
        let mut authored = recipe(ClipPreset::Snare);
        authored.rhythm = Some("x...x...".into());
        assert!(!drum_control_applies(&authored, Dial::Density));
        assert!(!drum_control_applies(&authored, Dial::Fill));
        assert!(drum_control_applies(&authored, Dial::Intensity));
        let piece = compose(&SongSpec::default());
        let mut kit = piece
            .tracks
            .iter()
            .find_map(|track| {
                track.clips.iter().find_map(|clip| {
                    clip.recipe
                        .as_ref()
                        .filter(|recipe| !recipe.drum_voices.is_empty())
                        .cloned()
                })
            })
            .unwrap();
        assert!(drum_control_applies(&kit, Dial::Density));
        kit.drum_map = Some(DrumMap::default());
        assert!(!drum_control_applies(&kit, Dial::Density));
        assert!(!drum_control_applies(&kit, Dial::Intensity));
    }

    #[gpui::test]
    fn melodic_pad_rewrites_each_role_as_one_undoable_gesture(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let track = app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            this.session
                .stamp_named_progression("axis", Ticks::ZERO, 4)
                .unwrap();
            this.session.add_default_instrument_track("Player").unwrap()
        });
        for preset in ClipPreset::ALL
            .into_iter()
            .filter(|preset| !preset.is_drums())
        {
            let (clip, original) = app.update(cx, |this, _| {
                let clip = this
                    .session
                    .generate_clip(track, Ticks::ZERO, Ticks::QUARTER * 16, recipe(preset))
                    .unwrap();
                this.select_track(track);
                this.select_clip(Some(clip));
                (clip, this.session.midi_clip(clip).unwrap().clone())
            });
            assert!(
                !original.notes.is_empty(),
                "{} needs audible harmony",
                preset.name()
            );
            paint(&app, cx);
            let bounds = cx
                .debug_bounds("part-pad")
                .expect("melodic recipe has a pad");
            assert!(cx.debug_bounds("drummer-pad").is_none());
            assert!(cx.debug_bounds("part-dial-0").is_none());
            assert!(cx.debug_bounds("part-dial-1").is_none());
            drag(
                cx,
                bounds.center(),
                point(bounds.right() - px(12.0), bounds.top() + px(12.0)),
            );
            app.update(cx, |this, _| {
                let changed = this.session.midi_clip(clip).unwrap();
                let settings = changed.recipe.as_ref().unwrap();
                assert!(
                    settings.density > 0.9 && settings.intensity > 0.9,
                    "{}",
                    preset.name()
                );
                assert_eq!(settings.seed, original.recipe.as_ref().unwrap().seed);
                assert_eq!(changed.transforms, original.transforms);
                assert_ne!(changed.notes, original.notes, "{}", preset.name());
                assert!(this.session.undo().is_some());
                assert_eq!(this.session.midi_clip(clip), Some(&original));
            });
        }
    }

    #[gpui::test]
    fn melodic_role_register_and_take_buttons_edit_the_selected_clip(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let (clip, other, untouched) = app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            this.session
                .stamp_named_progression("axis", Ticks::ZERO, 4)
                .unwrap();
            let track = this.session.add_default_instrument_track("Player").unwrap();
            let clip = this
                .session
                .generate_clip(
                    track,
                    Ticks::ZERO,
                    Ticks::QUARTER * 16,
                    recipe(ClipPreset::Pad),
                )
                .unwrap();
            let other = this
                .session
                .generate_clip(
                    track,
                    Ticks::QUARTER * 16,
                    Ticks::QUARTER * 16,
                    recipe(ClipPreset::Lead),
                )
                .unwrap();
            this.select_track(track);
            this.select_clip(Some(clip));
            (clip, other, this.session.midi_clip(other).unwrap().clone())
        });
        paint(&app, cx);
        assert!(cx.debug_bounds("part-role-stab").is_none());
        click("part-role-chords", cx);
        let original = app.update(cx, |this, _| {
            let midi = this.session.midi_clip(clip).unwrap();
            let settings = midi.recipe.as_ref().unwrap();
            assert_eq!(settings.preset, ClipPreset::Chords);
            assert_eq!(settings.gate, recipe(ClipPreset::Chords).gate);
            assert_eq!(settings.seed, 1);
            midi.clone()
        });
        paint(&app, cx);
        click("part-register-1", cx);
        app.update(cx, |this, _| {
            let midi = this.session.midi_clip(clip).unwrap();
            assert_eq!(midi.recipe.as_ref().unwrap().octave, 1);
            assert_eq!(midi.notes.len(), original.notes.len());
            for (raised, before) in midi.notes.iter().zip(&original.notes) {
                assert_eq!(raised.pitch, before.pitch + 12);
                assert_eq!((raised.start, raised.length), (before.start, before.length));
            }
            assert!(this.session.undo().is_some());
            assert_eq!(this.session.midi_clip(clip), Some(&original));
        });
        paint(&app, cx);
        click("part-reroll", cx);
        let notes = app.update(cx, |this, _| {
            assert_eq!(this.session.clip_recipe(clip).unwrap().seed, 2);
            this.session.midi_clip(clip).unwrap().notes.clone()
        });
        paint(&app, cx);
        click("part-freeze", cx);
        app.update(cx, |this, cx| {
            assert!(this.session.clip_recipe(clip).is_none());
            assert_eq!(this.session.midi_clip(clip).unwrap().notes, notes);
            assert!(this.part_rows(cx).is_empty());
            assert_eq!(this.session.midi_clip(other), Some(&untouched));
        });
    }

    #[gpui::test]
    fn melodic_regenerate_button_follows_new_harmony_without_changing_the_take(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = open(cx);
        let clip = app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            let track = this.session.add_default_instrument_track("Player").unwrap();
            let clip = this
                .session
                .generate_clip(
                    track,
                    Ticks::ZERO,
                    Ticks::QUARTER * 16,
                    recipe(ClipPreset::Lead),
                )
                .unwrap();
            assert!(this.session.midi_clip(clip).unwrap().notes.is_empty());
            this.session
                .stamp_named_progression("axis", Ticks::ZERO, 4)
                .unwrap();
            this.select_track(track);
            this.select_clip(Some(clip));
            clip
        });
        paint(&app, cx);
        click("part-regenerate", cx);
        app.update(cx, |this, _| {
            let midi = this.session.midi_clip(clip).unwrap();
            assert!(!midi.notes.is_empty());
            assert_eq!(midi.recipe.as_ref().unwrap().seed, 1);
            assert!(this.session.undo().is_some());
            assert!(this.session.midi_clip(clip).unwrap().notes.is_empty());
        });
    }

    #[gpui::test]
    fn drummer_pad_is_one_undoable_gesture_and_freezing_removes_it(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let (clip, original) = app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            let track = this
                .session
                .add_drum_track("Kit", "auris.synth.drumkit")
                .unwrap();
            this.session
                .stamp_named_progression("axis", Ticks::ZERO, 4)
                .unwrap();
            let clip = this
                .session
                .generate_clip(
                    track,
                    Ticks::ZERO,
                    Ticks::QUARTER * 16,
                    recipe(ClipPreset::Drums),
                )
                .unwrap();
            this.select_track(track);
            this.select_clip(Some(clip));
            (clip, this.session.midi_clip(clip).unwrap().clone())
        });
        paint(&app, cx);
        let bounds = cx
            .debug_bounds("drummer-pad")
            .expect("drum recipe draws its pad");
        drag(
            cx,
            bounds.center(),
            point(
                bounds.origin.x + bounds.size.width - px(12.0),
                bounds.origin.y + px(12.0),
            ),
        );
        app.update(cx, |this, _| {
            let changed = this.session.midi_clip(clip).unwrap();
            let settings = changed.recipe.as_ref().unwrap();
            assert!(settings.density > 0.9 && settings.intensity > 0.9);
            assert_eq!(settings.seed, original.recipe.as_ref().unwrap().seed);
            assert_ne!(changed.notes, original.notes);
            assert!(this.session.undo().is_some());
            assert_eq!(this.session.midi_clip(clip), Some(&original));
        });
        paint(&app, cx);
        click("part-freeze", cx);
        paint(&app, cx);
        app.update(cx, |this, cx| {
            assert!(this.session.clip_recipe(clip).is_none());
            assert_eq!(this.session.midi_clip(clip).unwrap().notes, original.notes);
            // gpui retains old debug bounds after removing an element; inspect the renderer's
            // current rows rather than treating that historical map as a visibility query.
            assert!(this.part_rows(cx).is_empty());
        });
    }

    #[gpui::test]
    fn an_authored_drum_pattern_keeps_an_intensity_slider_without_an_inert_pad(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = open(cx);
        let (clip, original) = app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            let track = this
                .session
                .add_drum_track("Snare", "auris.synth.drumkit")
                .unwrap();
            this.session
                .stamp_named_progression("axis", Ticks::ZERO, 4)
                .unwrap();
            let mut recipe = recipe(ClipPreset::Snare);
            recipe.rhythm = Some("x...x...x...x...".into());
            let clip = this
                .session
                .generate_clip(track, Ticks::ZERO, Ticks::QUARTER * 4, recipe)
                .unwrap();
            this.select_track(track);
            this.select_clip(Some(clip));
            (clip, this.session.midi_clip(clip).unwrap().clone())
        });
        paint(&app, cx);
        assert!(cx.debug_bounds("drummer-pad").is_none());
        assert!(cx.debug_bounds("part-dial-0").is_none());
        assert!(cx.debug_bounds("perform-dial-0").is_some());
        assert!(cx.debug_bounds("perform-dial-2").is_none());
        let intensity = cx
            .debug_bounds("part-dial-1")
            .expect("the authored rhythm still has an intensity control");
        drag(
            cx,
            intensity.center(),
            point(intensity.center().x - px(70.0), intensity.center().y),
        );
        app.update(cx, |this, _| {
            let changed = this.session.midi_clip(clip).unwrap();
            let hits = |midi: &MidiClip| {
                midi.notes
                    .iter()
                    .map(|note| (note.start, note.pitch))
                    .collect::<Vec<_>>()
            };
            assert_eq!(hits(changed), hits(&original));
            assert_ne!(changed.notes, original.notes);
        });
    }

    #[gpui::test]
    fn a_kit_piece_dial_changes_only_that_voice_and_undo_restores_the_clip(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = open(cx);
        let (clip, voice, original) = app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            let spec = SongSpec::parse(
                r#"
                form = "verse"
                ending = "none"
                [section.verse]
                bars = 2
                [[part]]
                name = "kick"
                role = "kick"
                [[part]]
                name = "snare"
                role = "snare"
            "#,
            )
            .unwrap();
            this.session.compose(&compose(&spec)).unwrap();
            let track = this
                .project()
                .tracks
                .iter()
                .find(|track| track.kind.is_drum())
                .unwrap();
            let clip = track.kind.note_clips().unwrap()[0].id;
            let track = track.id;
            let original = this.session.midi_clip(clip).unwrap().clone();
            let voice = original.recipe.as_ref().unwrap().drum_voices[0]
                .name
                .clone();
            this.select_track(track);
            this.select_clip(Some(clip));
            (clip, voice, original)
        });
        paint(&app, cx);
        let bounds = cx
            .debug_bounds("drummer-voice-dial-0-1")
            .expect("kit writer has intensity");
        drag(
            cx,
            bounds.center(),
            point(bounds.center().x - px(90.0), bounds.center().y),
        );
        app.update(cx, |this, _| {
            let changed = this.session.midi_clip(clip).unwrap();
            let other = |midi: &MidiClip| {
                midi.notes
                    .iter()
                    .filter(|note| note.drum_voice != voice)
                    .cloned()
                    .collect::<Vec<_>>()
            };
            assert_eq!(other(changed), other(&original));
            assert_ne!(changed.notes, original.notes);
            assert!(this.session.undo().is_some());
            assert_eq!(this.session.midi_clip(clip), Some(&original));
        });
    }

    #[test]
    fn a_dial_reads_back_what_it_was_set_to() {
        // The bar is drawn from `fraction` and dragged into `set`, so a value that did not survive
        // the round trip would make the bar jump away from the pointer while it was being dragged.
        for dial in [Dial::Density, Dial::Gate, Dial::Intensity, Dial::Dynamics] {
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
    fn every_drum_writer_offers_only_controls_its_generator_reads() {
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
            // Fills are played by the snare, alone or inside the full kit.
            assert_eq!(
                dials.contains(&Dial::Fill),
                matches!(preset, ClipPreset::Drums | ClipPreset::Snare),
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

        // Everything reads how hard and how evenly it is played, kit included. (How *loose* is
        // the performance panel's business now, not the recipe's.)
        for preset in ClipPreset::ALL {
            let dials = dials_for(&recipe(preset));
            assert!(dials.contains(&Dial::Intensity), "{}", preset.name());
            assert!(dials.contains(&Dial::Dynamics), "{}", preset.name());
            assert!(dials.contains(&Dial::Swing), "{}", preset.name());
        }

        // The syncopation reaches a part that rolls its own figure and nothing else. A kit plays
        // its groove and a pad sounds the chord where the chord is; a dial on either would sweep
        // its whole travel and move not one note.
        for preset in ClipPreset::ALL {
            let rolls_its_own = !preset.is_drums() && preset != ClipPreset::Pad;
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
        // Choosing a role uses its defaults until a dial has been deliberately adjusted.
        let pad = recipe(ClipPreset::Pad);
        let chords = with_preset(&pad, ClipPreset::Chords);
        assert_eq!(chords.preset, ClipPreset::Chords);
        assert_eq!(chords.gate, ClipRecipe::new(ClipPreset::Chords, 1).gate);

        assert_eq!(
            chords.density,
            ClipRecipe::new(ClipPreset::Chords, 1).density
        );

        // And a dial that was moved is the person's, not the preset's.
        let mut deliberate = recipe(ClipPreset::Pad);
        Dial::Gate.set(&mut deliberate, 0.5);
        let moved = deliberate.gate;
        let chords = with_preset(&deliberate, ClipPreset::Chords);
        assert_eq!(chords.gate, moved, "the preset overwrote a deliberate gate");

        // The seed never moves: another take is the next seed, and changing what the part is
        // should not also change which take of it you are hearing.
        assert_eq!(chords.seed, deliberate.seed);
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
        for dial in [Dial::Density, Dial::Gate, Dial::Intensity, Dial::Swing] {
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
        for dial in [Dial::Density, Dial::Gate, Dial::Intensity, Dial::Swing] {
            assert!(seen.insert(dial_element_key(dial)), "{dial:?} collided");
        }
    }
}
