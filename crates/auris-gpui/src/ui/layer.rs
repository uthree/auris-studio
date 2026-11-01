//! The inspector's layer section: what a layer follows, and how it shapes the notes it borrows.
//!
//! A layer is a whole track — instrument, strip, routing — so everything else about it is the
//! ordinary track inspector. What is its own is the [`LayerShape`], and every slider here writes
//! one field of it through [`Session::set_layer_shape`](auris_session::Session::set_layer_shape).
//! The mapping between a slider's travel and a field is the free functions at the top, so the
//! arithmetic is tested without a window.

use auris_i18n::Key;
use auris_session::prelude::*;

use gpui::{AnyElement, IntoElement, MouseDownEvent, Pixels, Point, div, prelude::*};

use crate::app::{AurisApp, Drag};
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::timeline::pitch_name;
use crate::ui::widgets::{
    ButtonStyle, SliderFill, SliderKeyboardEvent, button, divider, value_slider,
};

/// Widest transpose the slider reaches, in semitones either way.
pub const TRANSPOSE_RANGE: i32 = 24;

/// Grid the timing slider lands on, in ticks: a sixty-fourth note.
const OFFSET_STEP: i64 = TICKS_PER_QUARTER / 16;

/// One slider of the layer section.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LayerDial {
    /// Lowest source pitch kept.
    Lowest,
    /// Highest source pitch kept.
    Highest,
    /// Semitones moved.
    Transpose,
    /// Velocity multiplier.
    Velocity,
    /// Length multiplier.
    Length,
    /// Ticks early or late.
    Offset,
}

/// The sliders in the order the shape applies them.
pub const LAYER_DIALS: &[LayerDial] = &[
    LayerDial::Lowest,
    LayerDial::Highest,
    LayerDial::Transpose,
    LayerDial::Velocity,
    LayerDial::Length,
    LayerDial::Offset,
];

impl LayerDial {
    fn label(self) -> Key {
        match self {
            Self::Lowest => Key::LayerLowest,
            Self::Highest => Key::LayerHighest,
            Self::Transpose => Key::LayerTranspose,
            Self::Velocity => Key::LayerVelocity,
            Self::Length => Key::LayerLength,
            Self::Offset => Key::LayerOffset,
        }
    }

    fn element_key(self) -> usize {
        self as usize
    }

    fn fill(self) -> SliderFill {
        match self {
            Self::Transpose | Self::Offset => SliderFill::FromCentre,
            _ => SliderFill::FromStart,
        }
    }

    /// One arrow-key press, as a fraction of the slider: one semitone, one note, one percent,
    /// one grid step.
    fn keyboard_step(self) -> f32 {
        match self {
            Self::Lowest | Self::Highest => 1.0 / 127.0,
            Self::Transpose => 1.0 / (2 * TRANSPOSE_RANGE) as f32,
            Self::Velocity => 0.005,
            Self::Length => 1.0 / 48.0,
            Self::Offset => OFFSET_STEP as f32 / (2 * MAX_LAYER_OFFSET.raw()) as f32,
        }
    }
}

/// Where the slider for `dial` sits for `shape`, from 0 to 1.
pub fn layer_fraction(shape: &LayerShape, dial: LayerDial) -> f32 {
    let fraction = match dial {
        LayerDial::Lowest => f32::from(shape.lowest) / 127.0,
        LayerDial::Highest => f32::from(shape.highest) / 127.0,
        LayerDial::Transpose => {
            (shape.transpose + TRANSPOSE_RANGE) as f32 / (2 * TRANSPOSE_RANGE) as f32
        }
        LayerDial::Velocity => shape.velocity / 2.0,
        // A doubling is the same distance at every length, so the slider is logarithmic: a
        // quarter of the length at the left, four times it at the right, as written in the middle.
        LayerDial::Length => (shape.length.max(0.25).log2() + 2.0) / 4.0,
        LayerDial::Offset => {
            let range = MAX_LAYER_OFFSET.raw();
            (shape.offset.raw() + range) as f32 / (2 * range) as f32
        }
    };
    fraction.clamp(0.0, 1.0)
}

/// `shape` with the field `dial` drives moved to `fraction` of its slider.
///
/// The two ends of the key range push each other rather than crossing, so dragging the lowest
/// note past the highest carries the highest along — a range is never left empty by accident.
pub fn with_layer_dial(shape: &LayerShape, dial: LayerDial, fraction: f32) -> LayerShape {
    let fraction = fraction.clamp(0.0, 1.0);
    let mut next = shape.clone();
    match dial {
        LayerDial::Lowest => {
            next.lowest = (fraction * 127.0).round() as u8;
            next.highest = next.highest.max(next.lowest);
        }
        LayerDial::Highest => {
            next.highest = (fraction * 127.0).round() as u8;
            next.lowest = next.lowest.min(next.highest);
        }
        LayerDial::Transpose => {
            next.transpose =
                (fraction * (2 * TRANSPOSE_RANGE) as f32).round() as i32 - TRANSPOSE_RANGE;
        }
        // Whole percents, so a value the slider shows is the value stored.
        LayerDial::Velocity => next.velocity = (fraction * 200.0).round() / 100.0,
        LayerDial::Length => {
            let length = 2f32.powf(fraction * 4.0 - 2.0);
            next.length = (length * 100.0).round() / 100.0;
        }
        LayerDial::Offset => {
            let range = MAX_LAYER_OFFSET.raw();
            let ticks = (fraction as f64 * (2 * range) as f64).round() as i64 - range;
            next.offset = Ticks((ticks as f64 / OFFSET_STEP as f64).round() as i64 * OFFSET_STEP);
        }
    }
    next
}

/// What the slider for `dial` reads.
pub fn layer_text(shape: &LayerShape, dial: LayerDial) -> String {
    match dial {
        LayerDial::Lowest => format!("{} ({})", pitch_name(shape.lowest), shape.lowest),
        LayerDial::Highest => format!("{} ({})", pitch_name(shape.highest), shape.highest),
        LayerDial::Transpose => signed(shape.transpose),
        LayerDial::Velocity => format!("{:.0}%", shape.velocity * 100.0),
        LayerDial::Length => format!("{:.0}%", shape.length * 100.0),
        // In quarter notes, which is what the grid it snaps to is counted in.
        LayerDial::Offset => {
            let beats = shape.offset.raw() as f64 / TICKS_PER_QUARTER as f64;
            if shape.offset == Ticks::ZERO {
                "0♩".to_string()
            } else {
                let digits = format!("{beats:+.3}");
                format!("{}♩", digits.trim_end_matches('0').trim_end_matches('.'))
            }
        }
    }
}

fn signed(value: i32) -> String {
    match value {
        0 => "0".to_string(),
        v => format!("{v:+}"),
    }
}

fn voice_key(voice: LayerVoice) -> Key {
    match voice {
        LayerVoice::All => Key::LayerVoiceAll,
        LayerVoice::Top => Key::LayerVoiceTop,
        LayerVoice::Bottom => Key::LayerVoiceBottom,
    }
}

impl AurisApp {
    /// The one-line summary on the layer section's header.
    pub(crate) fn layer_summary(&self, track: TrackId) -> String {
        let source = self
            .session
            .layer_source(track)
            .and_then(|source| self.project().track(source))
            .map(|source| source.name.clone())
            .unwrap_or_else(|| self.t(Key::LayerSourceMissing).to_string());
        let shaped = match self.session.layer_shape(track) {
            Some(shape) if !shape.is_identity() => self.t(Key::LayerShaped),
            _ => self.t(Key::LayerUnshaped),
        };
        format!("{source} · {shaped}")
    }

    /// The layer section's rows, or none when `track` is no layer.
    pub(crate) fn layer_rows(
        &mut self,
        track: TrackId,
        cx: &mut gpui::Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(shape) = self.session.layer_shape(track).cloned() else {
            return Vec::new();
        };
        let theme = self.theme.clone();
        let source = self
            .session
            .layer_source(track)
            .and_then(|source| self.project().track(source))
            .map(|source| source.name.clone())
            .unwrap_or_else(|| self.t(Key::LayerSourceMissing).to_string());
        let mut rows: Vec<AnyElement> = Vec::new();
        rows.push(
            div()
                .debug_selector(|| "layer-source".into())
                .flex()
                .justify_between()
                .gap_2()
                .text_xs()
                .child(
                    div()
                        .text_color(theme.text_muted)
                        .child(self.t(Key::LayerFollows)),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.text)
                        .child(source),
                )
                .into_any_element(),
        );
        rows.push(
            self.picker_row(
                "layer-voice",
                Key::LayerVoice,
                self.t(voice_key(shape.voice)).to_string(),
                Self::opens_menu(cx, move |this, at| this.layer_voice_menu(at, track)),
            )
            .into_any_element(),
        );
        for dial in LAYER_DIALS {
            let dial = *dial;
            let fraction = layer_fraction(&shape, dial);
            rows.push(
                value_slider(
                    ("layer-dial", dial.element_key()),
                    self.t(dial.label()),
                    layer_text(&shape, dial),
                    fraction,
                    theme.accent,
                    dial.fill(),
                    dial.keyboard_step(),
                    &theme,
                    cx.listener(move |this, event: &MouseDownEvent, _, _| {
                        this.begin_drag(Drag::LayerDial {
                            track,
                            dial,
                            start_fraction: fraction,
                            start_x: event.position.x,
                        });
                    }),
                    cx.listener(move |this, event: &SliderKeyboardEvent, _, cx| {
                        this.set_layer_dial(track, dial, event.fraction);
                        cx.notify();
                    }),
                )
                .debug_selector(move || format!("layer-dial-{}", dial.element_key()))
                .into_any_element(),
            );
        }
        rows.push(divider(&theme).into_any_element());
        rows.push(
            div()
                .flex()
                .gap_2()
                .child(div().flex_1().child(button(
                    "layer-reset",
                    self.t(Key::LayerReset),
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(move |this, _, _, cx| {
                        this.run_menu_command(MenuCommand::ResetLayerShape(track), cx);
                    }),
                )))
                .child(div().flex_1().child(button(
                    "layer-detach",
                    self.t(Key::MenuDetachLayer),
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(move |this, _, _, cx| {
                        this.run_menu_command(MenuCommand::DetachLayer(track), cx);
                    }),
                )))
                .into_any_element(),
        );
        rows
    }

    /// The choice of which voice of a chord a layer keeps.
    fn layer_voice_menu(&self, anchor: Point<Pixels>, track: TrackId) -> ContextMenu {
        let current = self
            .session
            .layer_shape(track)
            .map(|shape| shape.voice)
            .unwrap_or_default();
        let mut menu = ContextMenu::new(anchor, self.t(Key::LayerVoice));
        for voice in [LayerVoice::All, LayerVoice::Top, LayerVoice::Bottom] {
            menu = menu.toggle(
                self.t(voice_key(voice)),
                MenuCommand::SetLayerVoice { track, voice },
                current == voice,
            );
        }
        menu
    }

    /// Moves one of a layer's sliders to `fraction` of its travel.
    pub(crate) fn set_layer_dial(&mut self, track: TrackId, dial: LayerDial, fraction: f32) {
        let Some(shape) = self.session.layer_shape(track) else {
            return;
        };
        let next = with_layer_dial(shape, dial, fraction);
        let _ = self.session.set_layer_shape(track, next);
    }

    /// Applies a layer slider drag, measured in pixels from where it began.
    pub(crate) fn drag_layer_dial(
        &mut self,
        track: TrackId,
        dial: LayerDial,
        start_fraction: f32,
        delta: f32,
    ) {
        self.set_layer_dial(
            track,
            dial,
            crate::ui::widgets::dragged(start_fraction, delta),
        );
    }

    /// Changes which voice of a chord a layer keeps, and nothing else about its shape.
    pub(crate) fn set_layer_voice(&mut self, track: TrackId, voice: LayerVoice) {
        let Some(shape) = self.session.layer_shape(track) else {
            return;
        };
        let next = LayerShape {
            voice,
            ..shape.clone()
        };
        let _ = self.session.set_layer_shape(track, next);
    }

    /// Adds a layer on `source` with the default instrument, and selects it.
    pub(crate) fn add_layer_on(&mut self, source: TrackId) {
        let Some(name) = self
            .project()
            .track(source)
            .map(|track| auris_i18n::messages::layer_track_name(self.language(), &track.name))
        else {
            return;
        };
        match self.session.add_default_layer_track(source, name) {
            Ok(id) => {
                self.select_track(id);
                self.reveal_track(id);
                self.set_status(self.t(Key::AddedLayer));
            }
            Err(error) => self.set_failed_status(self.failure(Key::MenuAddLayer, &error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_slider_reads_the_default_shape_where_its_value_is() {
        let shape = LayerShape::default();
        assert_eq!(layer_fraction(&shape, LayerDial::Lowest), 0.0);
        assert_eq!(layer_fraction(&shape, LayerDial::Highest), 1.0);
        assert_eq!(layer_fraction(&shape, LayerDial::Transpose), 0.5);
        assert_eq!(layer_fraction(&shape, LayerDial::Velocity), 0.5);
        assert_eq!(layer_fraction(&shape, LayerDial::Length), 0.5);
        assert_eq!(layer_fraction(&shape, LayerDial::Offset), 0.5);
        for dial in LAYER_DIALS {
            let back = with_layer_dial(&shape, *dial, layer_fraction(&shape, *dial));
            assert_eq!(back, shape, "{dial:?} round-trips the identity");
        }
    }

    #[test]
    fn sliders_land_on_whole_semitones_percents_and_grid_steps() {
        let shape = LayerShape::default();
        let up = with_layer_dial(&shape, LayerDial::Transpose, 0.5 + 7.2 / 48.0);
        assert_eq!(up.transpose, 7);
        let soft = with_layer_dial(&shape, LayerDial::Velocity, 0.3333);
        assert_eq!(soft.velocity, 0.67);
        let short = with_layer_dial(&shape, LayerDial::Length, 0.0);
        assert_eq!(short.length, 0.25);
        let long = with_layer_dial(&shape, LayerDial::Length, 1.0);
        assert_eq!(long.length, 4.0);
        let late = with_layer_dial(&shape, LayerDial::Offset, 0.5 + 0.0626);
        assert_eq!(late.offset.raw() % OFFSET_STEP, 0);
        assert_eq!(
            late.offset,
            Ticks(TICKS_PER_QUARTER / 2),
            "an eighth note late"
        );
    }

    #[test]
    fn the_ends_of_the_key_range_push_each_other_instead_of_crossing() {
        let shape = LayerShape {
            lowest: 40,
            highest: 60,
            ..LayerShape::default()
        };
        let raised = with_layer_dial(&shape, LayerDial::Lowest, 72.0 / 127.0);
        assert_eq!((raised.lowest, raised.highest), (72, 72));
        let lowered = with_layer_dial(&shape, LayerDial::Highest, 20.0 / 127.0);
        assert_eq!((lowered.lowest, lowered.highest), (20, 20));
    }

    #[test]
    fn slider_text_names_pitches_signs_and_beats() {
        let shape = LayerShape {
            lowest: 60,
            transpose: -12,
            velocity: 0.5,
            offset: Ticks(TICKS_PER_QUARTER / 2),
            ..LayerShape::default()
        };
        assert_eq!(layer_text(&shape, LayerDial::Lowest), "C4 (60)");
        assert_eq!(layer_text(&shape, LayerDial::Transpose), "-12");
        assert_eq!(layer_text(&shape, LayerDial::Velocity), "50%");
        assert_eq!(layer_text(&shape, LayerDial::Offset), "+0.5♩");
        assert_eq!(layer_text(&LayerShape::default(), LayerDial::Offset), "0♩");
    }

    mod window {
        use gpui::{TestAppContext, point, px, size};

        use crate::harness::{choose, drag_to, paint, press, release, resize, with_a_clip};
        use crate::ui::context_menu::MenuCommand;

        use super::super::*;

        #[gpui::test]
        fn the_track_menu_adds_a_layer_and_the_inspector_shapes_it(cx: &mut TestAppContext) {
            let (app, cx, track, clip) = with_a_clip(cx);
            app.update(cx, |this, _| {
                this.session
                    .add_note(clip, Note::new(60, Ticks::ZERO, Ticks(480)))
                    .unwrap();
                this.session.forget_history();
                let menu = this.track_menu(point(px(40.0), px(40.0)), track);
                this.open_menu(menu);
            });
            resize(&app, cx, size(px(1920.0), px(2000.0)));
            choose(&app, cx, &MenuCommand::AddLayer(track));
            paint(&app, cx);

            let layer = app.read_with(cx, |this, _| {
                let layer = this.selected_track.expect("the new layer is selected");
                assert_eq!(this.session.layer_source(layer), Some(track));
                assert_eq!(
                    this.project().track(layer).unwrap().name,
                    auris_i18n::messages::layer_track_name(this.language(), "Test"),
                    "named after its source"
                );
                layer
            });
            assert!(cx.debug_bounds("inspector-layer-section").is_some());
            assert!(cx.debug_bounds("layer-source").is_some());

            // Dragging the transpose slider a quarter of its travel to the right is twelve
            // semitones up, in one undo step, and the source's notes are untouched.
            let at = cx.debug_bounds("layer-dial-2").unwrap().center();
            let travel = crate::ui::widgets::DRAG_RANGE_PIXELS / 4.0;
            press(cx, at);
            drag_to(cx, point(at.x + px(travel / 2.0), at.y));
            drag_to(cx, point(at.x + px(travel), at.y));
            release(cx, point(at.x + px(travel), at.y));
            paint(&app, cx);
            app.update(cx, |this, _| {
                assert_eq!(this.session.layer_shape(layer).unwrap().transpose, 12);
                assert_eq!(this.session.midi_clip(clip).unwrap().notes[0].pitch, 60);
                this.undo();
                assert!(this.session.layer_shape(layer).unwrap().is_identity());
                this.undo();
                assert_eq!(
                    this.session.layer_source(layer),
                    None,
                    "the layer itself undoes next"
                );
            });
        }

        #[gpui::test]
        fn detaching_from_the_inspector_gives_the_layer_its_own_clip(cx: &mut TestAppContext) {
            let (app, cx, track, _) = with_a_clip(cx);
            let layer = app.update(cx, |this, _| {
                this.add_layer_on(track);
                this.selected_track.unwrap()
            });
            resize(&app, cx, size(px(1920.0), px(2000.0)));
            crate::harness::click("layer-detach", cx);
            paint(&app, cx);
            app.read_with(cx, |this, _| {
                assert_eq!(this.session.layer_source(layer), None);
                let clips = this
                    .project()
                    .track(layer)
                    .unwrap()
                    .kind
                    .note_clips()
                    .unwrap();
                assert_eq!(clips.len(), 1);
            });
        }
    }
}
