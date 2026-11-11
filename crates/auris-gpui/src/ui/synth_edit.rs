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
//! | Kick bass punch | punch time | punch | — |
//! | Kick bass drive | drive | — | — |
//! | Electronic kit kick | kick decay | kick punch | — |
//! | Electronic kit hats | hat tone | metal | — |
//!
//! The gestures are Atelier's: [`LinePlot::on_drag`] measures a drag from where it began, in the
//! plot's own units on the data area the plot really drew, so the press can land anywhere and
//! nothing jumps; Shift slows it without a jump, a double click is [`LinePlot::on_reset`], the
//! wheel comes in whole notches, and a focused plot nudges with the arrows. This file only maps
//! that travel to parameters. The horizontal axis moves in the plot's own units — one harmonic
//! of travel moves the cutoff by one harmonic, a stack's outermost copy follows the pointer — and
//! the vertical one sweeps its parameter's whole range across the plot's height, because no plot
//! here draws its second parameter as a height it could be read back from. A double click puts
//! both back to their defaults.
//!
//! The whole drag is one undo step, filed under the horizontal parameter as the envelope's
//! two-parameter corner is.

use crate::app::{AurisApp, Drag};
use atelier::plot::{DragPhase, LinePlot, PlotDrag};
use auris_session::prelude::*;
use gpui::{Context, div, prelude::*};

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
    /// A kick bass's opening: the punch falling onto the note.
    Punch,
    /// A kick bass's soft clipper.
    Drive,
    /// An electronic kit's kick pad.
    Kick,
    /// An electronic kit's hats: the metal oscillators under their filter.
    Metal,
}

impl SynthPlot {
    /// The parameter the horizontal axis moves, by its stable key.
    pub(crate) fn horizontal(self) -> &'static str {
        match self {
            SynthPlot::Stack => "detune",
            SynthPlot::Filter => "cutoff",
            SynthPlot::Sidebands => "ratio",
            SynthPlot::Cycle => "pulse_width",
            SynthPlot::ChipStack => "spread",
            SynthPlot::Punch => "punch_time",
            SynthPlot::Drive => "drive",
            SynthPlot::Kick => "kick_decay",
            SynthPlot::Metal => "hat_tone",
        }
    }

    /// The parameter the vertical axis moves, if any.
    pub(crate) fn vertical(self) -> Option<&'static str> {
        match self {
            SynthPlot::Stack => Some("mix"),
            SynthPlot::Filter => Some("resonance"),
            SynthPlot::Sidebands => Some("index"),
            SynthPlot::Cycle => Some("bits"),
            SynthPlot::ChipStack | SynthPlot::Drive => None,
            SynthPlot::Punch => Some("punch"),
            SynthPlot::Kick => Some("kick_punch"),
            SynthPlot::Metal => Some("metal"),
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
    /// How wide and how tall the plot's ranges are, in its own units: the travel Atelier
    /// reports is divided by these to give the share of the plot crossed.
    pub span: (f32, f32),
}

/// How far a drag has gone, in plot units, as shares of the plot's width and height:
/// rightwards and *upwards*, since up is more on every plot here.
pub(crate) fn travel(delta: (f32, f32), span: (f32, f32)) -> (f32, f32) {
    (
        delta.0 / span.0.abs().max(f32::MIN_POSITIVE),
        delta.1 / span.1.abs().max(f32::MIN_POSITIVE),
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

    /// A plot drawn on its well, which Atelier makes the control for what it draws: a drag, a
    /// double click, the arrows and (for a stack) the wheel edit it.
    pub(crate) fn synth_plot(
        &self,
        id: &'static str,
        track: TrackId,
        plot: SynthPlot,
        scale: PlotScale,
        chart: LinePlot,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let tokens = &self.theme.atelier;
        let mut chart = chart
            .id(id)
            .on_drag(cx.listener(move |this, drag: &PlotDrag, window, cx| {
                this.drag_synth_plot(track, plot, scale, drag, window, cx);
                cx.notify();
            }))
            .on_reset(cx.listener(move |this, _: &(), _, cx| {
                this.reset_synth_plot(track, plot);
                cx.notify();
            }));
        if plot.wheel().is_some() {
            chart = chart.on_wheel(cx.listener(move |this, notches: &i32, _, cx| {
                if this.scroll_synth_plot(track, plot, *notches) {
                    cx.notify();
                }
            }));
        }
        div()
            .h(gpui::rems(8.))
            .rounded(tokens.shape.radius_sm)
            .bg(tokens.colors.sunken)
            .overflow_hidden()
            .p(tokens.spacing.xs)
            .child(chart)
    }

    /// Follows one step of a drag on a plot: takes hold of it on the press, moves its
    /// parameters by the travel since, and lets go on the release.
    fn drag_synth_plot(
        &mut self,
        track: TrackId,
        plot: SynthPlot,
        scale: PlotScale,
        drag: &PlotDrag,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        match drag.phase {
            DragPhase::Start => {
                let Some((target, _)) = self.synth_param(track, plot.horizontal()) else {
                    return;
                };
                // Pressed left of a stack's centre, dragging further left is what widens it.
                let side = if plot.mirrored() && drag.from.0 < 0.0 {
                    -1.0
                } else {
                    1.0
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
                    start,
                });
            }
            DragPhase::Move | DragPhase::End => {
                let Some(Drag::SynthPlot {
                    track: held_track,
                    plot: held_plot,
                    scale,
                    start,
                    ..
                }) = self.drag.clone()
                else {
                    return;
                };
                if held_track != track || held_plot != plot {
                    return;
                }
                let (dx, dy) = travel(drag.delta, scale.span);
                if let Some((target, descriptor)) = self.synth_param(track, plot.horizontal())
                    && scale.across != 0.0
                {
                    let value = dragged_value(&descriptor, start.0, dx, scale.across);
                    self.session.set_param(target, value);
                }
                if let Some((target, descriptor)) =
                    plot.vertical().and_then(|key| self.synth_param(track, key))
                {
                    let across = vertical_across(&descriptor);
                    let value = dragged_value(&descriptor, start.1, dy, across);
                    self.session.set_param(target, value);
                }
                if drag.phase == DragPhase::End {
                    self.end_drag(window, cx);
                }
            }
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
        steps: i32,
    ) -> bool {
        let Some((target, descriptor)) = plot.wheel().and_then(|key| self.synth_param(track, key))
        else {
            return false;
        };
        if steps == 0 {
            return false;
        }
        let steps = steps as f32;
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

    fn descriptor(min: f32, max: f32) -> ParamDescriptor {
        ParamDescriptor::new(ParamId(0), "x", "X", min, max, min)
    }

    #[test]
    fn travel_is_a_share_of_the_plot_rightwards_and_upwards() {
        // A quarter of a 0..16 harmonic axis across, and a quarter of a -1..1 one up.
        let (dx, dy) = travel((4.0, 0.5), (16.0, 2.0));
        assert!((dx - 0.25).abs() < 1e-6, "{dx}");
        assert!((dy - 0.25).abs() < 1e-6, "{dy}");
        // A plot with nothing to cross never divides by nothing.
        assert!(travel((1.0, 1.0), (0.0, 0.0)).0.is_finite());
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
