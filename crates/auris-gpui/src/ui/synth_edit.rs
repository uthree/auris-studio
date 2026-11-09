//! The oscillator patches' pictures as controls: drag a plot and the patch follows.
//!
//! [`super::synth_view`] draws each built-in synthesiser's patch — the unison stack across its
//! detune, the filter over the note's harmonics, the FM pair's sidebands, the chiptune cycle — and
//! the knobs beside them were the only way to change any of it. A plot is a better handle than a
//! knob for the two numbers it is drawn from: the picture is what a person is listening for, and
//! the hand moves along the same axes the eye reads. So each plot takes a drag, its horizontal
//! axis moving the number the x axis is drawn from and its vertical axis the one the height is,
//! and the knobs keep turning the same parameters: both read the document, so either one moved
//! is the other one moved.
//!
//! | Picture | Sideways | Up and down | Wheel |
//! | --- | --- | --- | --- |
//! | Unison stack (subtractive) | detune | side copies' level (mix) | copies |
//! | Unison stack (chiptune) | spread | — | copies |
//! | Filter · harmonics | cutoff | resonance | — |
//! | FM sidebands | ratio | depth (index) | — |
//! | Chiptune cycle | duty cycle, square only | bit depth | — |
//!
//! A drag is measured from where it began, as every other drag in the application is, rather
//! than putting the value under the pointer: the press can land anywhere on the plot, and nothing
//! jumps when it does. The horizontal axis moves in the plot's own units — one harmonic of
//! travel moves the cutoff by one harmonic, a stack's outermost copy follows the pointer — and
//! the vertical one sweeps its parameter's whole range across the plot's height, because no plot
//! here draws its second parameter as a height it could be read back from. Shift slows both, and
//! a double click puts both back to their defaults.
//!
//! The whole drag is one undo step, filed under the horizontal parameter as the envelope's
//! two-parameter corner is.

use std::{cell::Cell, rc::Rc};

use crate::app::{AurisApp, Drag};
use atelier::plot::LinePlot;
use auris_session::prelude::*;
use gpui::{
    Bounds, Context, MouseButton, MouseDownEvent, Pixels, Point, ScrollWheelEvent, canvas, div,
    prelude::*, px,
};

/// An oscillator patch's picture that a drag can take hold of.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum SynthPlot {
    /// The subtractive patches' unison stack.
    Stack,
    /// The subtractive patches' filter over the note's harmonics.
    Filter,
    /// The FM pair's sidebands.
    Sidebands,
    /// The chiptune voice's one cycle.
    Cycle,
    /// The chiptune voice's unison stack.
    ChipStack,
}

/// How many [`SynthPlot`]s there are, for the table of where each was drawn.
const PLOTS: usize = 5;

/// Where each plot was drawn on the last frame, by [`SynthPlot::index`].
pub(crate) type PlotBounds = Rc<[Cell<Option<Bounds<Pixels>>>; PLOTS]>;

impl SynthPlot {
    /// The plot's slot in [`PlotBounds`].
    fn index(self) -> usize {
        match self {
            SynthPlot::Stack => 0,
            SynthPlot::Filter => 1,
            SynthPlot::Sidebands => 2,
            SynthPlot::Cycle => 3,
            SynthPlot::ChipStack => 4,
        }
    }

    /// The parameter the horizontal axis moves, by its stable key.
    pub(crate) fn horizontal(self) -> &'static str {
        match self {
            SynthPlot::Stack => "detune",
            SynthPlot::Filter => "cutoff",
            SynthPlot::Sidebands => "ratio",
            SynthPlot::Cycle => "pulse_width",
            SynthPlot::ChipStack => "spread",
        }
    }

    /// The parameter the vertical axis moves, if any.
    pub(crate) fn vertical(self) -> Option<&'static str> {
        match self {
            SynthPlot::Stack => Some("mix"),
            SynthPlot::Filter => Some("resonance"),
            SynthPlot::Sidebands => Some("index"),
            SynthPlot::Cycle => Some("bits"),
            SynthPlot::ChipStack => None,
        }
    }

    /// The count the wheel steps, if any: how many copies a stack has.
    pub(crate) fn wheel(self) -> Option<&'static str> {
        match self {
            SynthPlot::Stack | SynthPlot::ChipStack => Some("unison"),
            _ => None,
        }
    }

    /// Whether the plot is drawn either side of a centre, so that dragging away from it on
    /// either side widens it — a stack, whose copies sit mirrored around the note.
    fn mirrored(self) -> bool {
        matches!(self, SynthPlot::Stack | SynthPlot::ChipStack)
    }
}

/// How one plot's horizontal axis turns into its parameter.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct PlotScale {
    /// How far the horizontal parameter moves when the pointer crosses the whole plot.
    pub across: f32,
    /// The x axis's half width, held still while a drag on a stack is under way.
    pub reach: f32,
}

/// The pixels the plot's axes take from its bounds: the y tick labels and the margin on the
/// right across, the margin above and the x tick labels below.
///
/// Atelier's `LinePlot` lays its data area out from the width of its widest tick label, which
/// only the painter knows. These are what that comes to for the short labels every one of these
/// plots carries; being a few pixels out changes how fast a drag moves by a per cent or two, and
/// never where it starts, since a drag is measured from its anchor.
const GUTTER: (f32, f32) = (48.0, 34.0);

/// The size of the data area inside a plot's bounds, in pixels.
pub(crate) fn data_area(bounds: Bounds<Pixels>) -> (f32, f32) {
    (
        (f32::from(bounds.size.width) - GUTTER.0).max(1.0),
        (f32::from(bounds.size.height) - GUTTER.1).max(1.0),
    )
}

/// How far the pointer has moved from `anchor` as fractions of the data area: rightwards and
/// *upwards*, since up is more on every plot here.
pub(crate) fn travel(anchor: Point<Pixels>, at: Point<Pixels>, area: (f32, f32)) -> (f32, f32) {
    (
        f32::from(at.x - anchor.x) / area.0.max(1.0),
        f32::from(anchor.y - at.y) / area.1.max(1.0),
    )
}

/// Half the width of a stack's axis in cents, for a detune or spread in semitones.
///
/// A little wider than the outermost copy, so it never sits on the edge, and never narrower than
/// a quarter-tone, so a stack with no detune is still a stack and not one line filling the plot.
pub(crate) fn stack_reach(semitones: f32) -> f32 {
    (semitones * 110.0).max(25.0)
}

/// How far a stack's detune moves across the whole plot: its axis is in cents and the
/// parameter in semitones.
pub(crate) fn stack_across(reach: f32) -> f32 {
    2.0 * reach / 100.0
}

/// A parameter after a drag: where it was at the anchor, moved by `fraction` of `across`.
pub(crate) fn dragged_value(
    descriptor: &ParamDescriptor,
    start: f32,
    fraction: f32,
    across: f32,
) -> f32 {
    descriptor.clamp(start + fraction * across)
}

/// The whole range a vertical drag sweeps across the plot's height.
pub(crate) fn vertical_across(descriptor: &ParamDescriptor) -> f32 {
    descriptor.max - descriptor.min
}

impl AurisApp {
    /// A patch's parameter by key, with its target.
    fn synth_param(&mut self, track: TrackId, key: &str) -> Option<(ParamTarget, ParamDescriptor)> {
        let descriptor = self
            .session
            .instrument_descriptors(track)
            .iter()
            .find(|descriptor| descriptor.key == key)
            .cloned()?;
        let target = ParamTarget::Instrument {
            track,
            param: descriptor.id,
        };
        Some((target, descriptor))
    }

    /// The value one of a patch's parameters stands at.
    fn synth_value(&mut self, track: TrackId, key: &str) -> Option<f32> {
        let (target, descriptor) = self.synth_param(track, key)?;
        Some(self.session.param_value(target, &descriptor))
    }

    /// The half width a stack's axis is held at while it is being dragged, so the axis does not
    /// rescale under the pointer moving it.
    pub(crate) fn held_reach(&self, track: TrackId, plot: SynthPlot) -> Option<f32> {
        match &self.drag {
            Some(Drag::SynthPlot {
                track: dragged,
                plot: held,
                scale,
                ..
            }) if *dragged == track && *held == plot => Some(scale.reach),
            _ => None,
        }
    }

    /// A plot drawn on its well that a drag, a double click and (for a stack) the wheel edit.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn synth_plot_well(
        &self,
        id: &'static str,
        track: TrackId,
        plot: SynthPlot,
        scale: PlotScale,
        chart: LinePlot,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let tokens = &self.theme.atelier;
        let recorded = self.physical_view.plots.clone();
        let mut well = div()
            .id(id)
            .debug_selector(move || id.into())
            .h(gpui::rems(8.))
            .rounded(tokens.shape.radius_sm)
            .bg(tokens.colors.sunken)
            .overflow_hidden()
            .p(tokens.spacing.xs)
            .cursor_crosshair()
            .child(
                div().size_full().relative().child(chart).child(
                    canvas(
                        move |bounds, _, _| recorded[plot.index()].set(Some(bounds)),
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                ),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    if event.click_count >= 2 {
                        this.reset_synth_plot(track, plot);
                    } else {
                        this.press_synth_plot(track, plot, scale, event.position, event.modifiers);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            );
        if plot.wheel().is_some() {
            well =
                well.on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, _, cx| {
                    let notches = f32::from(event.delta.pixel_delta(px(16.0)).y) / 16.0;
                    if this.scroll_synth_plot(track, plot, notches) {
                        cx.notify();
                    }
                }));
        }
        well
    }

    /// Takes hold of a plot: notes where the drag starts and what the two parameters stood at.
    fn press_synth_plot(
        &mut self,
        track: TrackId,
        plot: SynthPlot,
        scale: PlotScale,
        at: Point<Pixels>,
        modifiers: gpui::Modifiers,
    ) {
        let Some((target, _)) = self.synth_param(track, plot.horizontal()) else {
            return;
        };
        let bounds = self.physical_view.plots[plot.index()].get();
        // Pressed left of a stack's centre, dragging further left is what widens it.
        let side = match bounds {
            Some(bounds) if plot.mirrored() && at.x < bounds.center().x => -1.0,
            _ => 1.0,
        };
        let start = (
            self.synth_value(track, plot.horizontal())
                .unwrap_or_default(),
            plot.vertical()
                .and_then(|key| self.synth_value(track, key))
                .unwrap_or_default(),
        );
        self.begin_drag(Drag::SynthPlot {
            track,
            plot,
            target,
            scale: PlotScale {
                across: scale.across * side,
                ..scale
            },
            anchor: at,
            start,
            fine: modifiers.shift,
        });
    }

    /// Moves a plot's parameters with the pointer.
    pub(crate) fn drag_synth_plot(&mut self, at: Point<Pixels>, modifiers: gpui::Modifiers) {
        let Some(Drag::SynthPlot {
            track,
            plot,
            scale,
            anchor,
            start,
            fine,
            ..
        }) = self.drag.clone()
        else {
            return;
        };
        // Shift pressed or let go half way: carry on from here at the new rate, rather than
        // rescaling the travel so far and jumping.
        if modifiers.shift != fine {
            let start = (
                self.synth_value(track, plot.horizontal())
                    .unwrap_or(start.0),
                plot.vertical()
                    .and_then(|key| self.synth_value(track, key))
                    .unwrap_or(start.1),
            );
            if let Some(Drag::SynthPlot {
                anchor,
                start: held,
                fine,
                ..
            }) = self.drag.as_mut()
            {
                *anchor = at;
                *held = start;
                *fine = modifiers.shift;
            }
            return;
        }
        let Some(bounds) = self.physical_view.plots[plot.index()].get() else {
            return;
        };
        let (dx, dy) = travel(anchor, at, data_area(bounds));
        let rate = crate::ui::widgets::fine_scaled(1.0, modifiers);
        if let Some((target, descriptor)) = self.synth_param(track, plot.horizontal())
            && scale.across != 0.0
        {
            let value = dragged_value(&descriptor, start.0, dx * rate, scale.across);
            self.session.set_param(target, value);
        }
        if let Some((target, descriptor)) =
            plot.vertical().and_then(|key| self.synth_param(track, key))
        {
            let across = vertical_across(&descriptor);
            let value = dragged_value(&descriptor, start.1, dy * rate, across);
            self.session.set_param(target, value);
        }
    }

    /// Puts both of a plot's parameters back to their defaults.
    fn reset_synth_plot(&mut self, track: TrackId, plot: SynthPlot) {
        let Some((target, _)) = self.synth_param(track, plot.horizontal()) else {
            return;
        };
        self.session
            .begin_transaction(auris_session::Edit::AdjustParameter(target));
        for key in std::iter::once(plot.horizontal()).chain(plot.vertical()) {
            if let Some((target, descriptor)) = self.synth_param(track, key) {
                self.session.set_param(target, descriptor.default);
            }
        }
        self.session.end_transaction();
    }

    /// Adds or takes away a stack's copies with the wheel, a whole copy per notch.
    ///
    /// Returns whether a step was taken.
    pub(crate) fn scroll_synth_plot(
        &mut self,
        track: TrackId,
        plot: SynthPlot,
        notches: f32,
    ) -> bool {
        let Some((target, descriptor)) = plot.wheel().and_then(|key| self.synth_param(track, key))
        else {
            return false;
        };
        self.physical_view.wheel += notches;
        let steps = self.physical_view.wheel.trunc();
        if steps == 0.0 {
            return false;
        }
        self.physical_view.wheel -= steps;
        let now = self.session.param_value(target, &descriptor);
        let next = descriptor.clamp(now.round() + steps);
        if next == now {
            return false;
        }
        self.session.set_param(target, next);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, size};

    fn descriptor(min: f32, max: f32) -> ParamDescriptor {
        ParamDescriptor::new(ParamId(0), "x", "X", min, max, min)
    }

    #[test]
    fn travel_is_rightwards_and_upwards_over_the_data_area() {
        let bounds = Bounds::new(point(px(10.), px(20.)), size(px(448.), px(134.)));
        let area = data_area(bounds);
        assert_eq!(area, (400.0, 100.0));
        let anchor = point(px(100.), px(100.));
        let (dx, dy) = travel(anchor, point(px(200.), px(75.)), area);
        assert!(
            (dx - 0.25).abs() < 1e-6,
            "a quarter of the width rightwards: {dx}"
        );
        assert!(
            (dy - 0.25).abs() < 1e-6,
            "a quarter of the height upwards: {dy}"
        );
    }

    #[test]
    fn a_stack_follows_its_outermost_copy_and_holds_its_axis() {
        // A narrow stack is drawn a quarter-tone either side at least.
        let reach = stack_reach(0.18);
        assert_eq!(reach, 25.0);
        assert!((stack_reach(0.4) - 44.0).abs() < 1e-4);
        // Crossing the whole ±25-cent axis is 50 cents, half a semitone of detune.
        let across = stack_across(reach);
        assert!((across - 0.5).abs() < 1e-6);
        let detune = descriptor(0.0, 1.0);
        // The outermost copy is under the pointer: a tenth of the width is five cents further
        // out, so the detune grows by 0.05 semitones…
        let wider = dragged_value(&detune, 0.18, 0.1, across);
        assert!((wider - 0.23).abs() < 1e-6, "{wider}");
        // …and on the left-hand copies the same hand movement leftwards does the same.
        let mirrored = dragged_value(&detune, 0.18, -0.1, -across);
        assert!((mirrored - wider).abs() < 1e-6);
        // Past the middle it stops at no detune rather than going negative.
        assert_eq!(dragged_value(&detune, 0.18, -1.0, across), 0.0);
    }

    #[test]
    fn the_filter_knee_moves_one_harmonic_per_harmonic_of_travel() {
        // At C4 with no key tracking, one harmonic of the x axis is 261.6 Hz of cutoff.
        let note = 440.0 * ((60.0f32 - 69.0) / 12.0).exp2();
        let across = 16.0 * note;
        let cutoff = ParamDescriptor::new(ParamId(0), "cutoff", "Cutoff", 20.0, 20_000.0, 900.0);
        let moved = dragged_value(&cutoff, 900.0, 1.0 / 16.0, across);
        assert!((moved - (900.0 + note)).abs() < 0.01, "{moved}");
        // It never leaves the parameter's range, however far the pointer goes.
        assert_eq!(dragged_value(&cutoff, 900.0, -2.0, across), 20.0);
    }

    #[test]
    fn up_and_down_sweeps_the_whole_range_across_the_height() {
        let resonance = descriptor(0.0, 1.0);
        let across = vertical_across(&resonance);
        assert!((dragged_value(&resonance, 0.15, 0.5, across) - 0.65).abs() < 1e-6);
        assert_eq!(dragged_value(&resonance, 0.15, 2.0, across), 1.0);
        // A stepped parameter lands on its steps: bit depth is a whole number of bits.
        let bits = ParamDescriptor::new(ParamId(0), "bits", "Bits", 1.0, 16.0, 16.0).with_steps(16);
        let crushed = dragged_value(&bits, 16.0, -0.43, vertical_across(&bits));
        assert_eq!(crushed, 10.0);
    }
}
