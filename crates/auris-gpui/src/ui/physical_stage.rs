//! Inside a physical instrument, beside the bodies a person can touch.
//!
//! Three pictures, each of something the lanes cannot show. The soundboard or box the strings
//! drive, as a surface seen from above its far corner: where each of its lowest modes is in
//! its swing, laid out the way a plate's modes lie. How the partials have died away over the
//! last second and a half, which is where a piano's two decays and a bell's beating are seen
//! rather than heard. And what excites the instrument — a hammer's force through the few
//! milliseconds of its blow, a string's speed under the bow through one period, a reed
//! opening and shutting — next to the spectrum it leaves.
//!
//! The surface is a [`Plot3d`], which shades on the GPU where there is one and paints the
//! same picture without it, so the view is the same on a machine with neither and in a test.
//! Everything a picture decides — which plate shape a mode is drawn as, how loud a partial
//! has to be to show, what an exciter's trace is measured against — is a free function here,
//! because the view cannot be asserted on and those can.

use std::f32::consts::PI;

use atelier::plot::{LinePlot, Plot3d, Series, Shading};
use atelier::{
    Card, ControlSize, Label, SectionHeader, Segment, SegmentedControl, TextColor, TextSize,
};
use auris_i18n::Key;
use auris_session::{MOTION_BODY, MOTION_PARTIALS, MOTION_SCOPE, MotionFrame, MotionVoice};
use gpui::{AnyElement, Context, div, prelude::*};

use super::physical_model::{lane_voices, voice_color};
use crate::{app::AurisApp, theme::Theme};

/// What the turning picture shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Resonance {
    /// The soundboard or box, as a surface.
    #[default]
    Body,
    /// The partials over the last moments, as a surface.
    Decay,
}

/// Columns and rows of the grid a body is drawn on.
pub(crate) const PLATE: (usize, usize) = (28, 18);
/// Frames the decay picture looks back over: a second and a third at thirty a second.
pub(crate) const DECAY_FRAMES: usize = 40;
/// The quietest a partial is drawn, in decibels under the loudest in the picture.
const FLOOR_DB: f32 = -60.0;

/// How much wider than deep the body a model's strings drive is, for laying its modes out;
/// `None` for a model with no such body.
pub(crate) fn body_plate(id: &str) -> Option<f32> {
    match id {
        "auris.physical.piano" => Some(1.6),
        "auris.physical.hammered_dulcimer" => Some(2.2),
        "auris.physical.guitar" | "auris.physical.bass" => Some(1.25),
        "auris.physical.violin" => Some(1.7),
        _ => None,
    }
}

/// The half-waves along and across a plate in each of its modes, lowest mode first, for a
/// plate `aspect` times as long as it is deep and held at its edges.
///
/// A soundboard is not that plate: it is ribbed, bridged and cut to a harp's shape. But its
/// modes come in the same order for the same reason — the lowest moves the whole board as one
/// piece, the next split it along its length, and each one up adds a line that stays still —
/// and that order is what the picture needs.
pub(crate) fn plate_modes(aspect: f32) -> [(u8, u8); MOTION_BODY] {
    let aspect = aspect.max(0.1);
    let mut all: Vec<(f32, u8, u8)> = (1..=8_u8)
        .flat_map(|along| {
            (1..=8_u8).map(move |across| {
                let pitch = (f32::from(along) / aspect).powi(2) + f32::from(across).powi(2);
                (pitch, along, across)
            })
        })
        .collect();
    all.sort_by(|a, b| a.0.total_cmp(&b.0));
    std::array::from_fn(|index| (all[index].1, all[index].2))
}

/// A value as it is drawn, from -1 to 1, at the display's `gain`.
fn displayed(value: f32, gain: f32) -> f32 {
    if value.is_finite() {
        (value * gain).clamp(-1., 1.)
    } else {
        0.
    }
}

/// The height of a body's surface over [`PLATE`], row by row from the near edge, from where
/// each of its modes is in its swing.
pub(crate) fn body_surface(body: &[f32; MOTION_BODY], aspect: f32, gain: f32) -> Vec<f32> {
    let (columns, rows) = PLATE;
    let modes = plate_modes(aspect);
    // A half-wave count's shape along one edge, worked out once for every row or column.
    let shape = |count: usize| -> Vec<[f32; 8]> {
        (0..count)
            .map(|index| {
                let at = index as f32 / (count - 1).max(1) as f32;
                std::array::from_fn(|waves| (PI * (waves + 1) as f32 * at).sin())
            })
            .collect()
    };
    let (along, across) = (shape(columns), shape(rows));
    let mut values = Vec::with_capacity(columns * rows);
    for row in &across {
        for column in &along {
            let height: f32 = body
                .iter()
                .zip(modes)
                .map(|(swing, (m, n))| swing * column[usize::from(m) - 1] * row[usize::from(n) - 1])
                .sum();
            values.push(displayed(height, gain));
        }
    }
    values
}

/// A partial's size as a height from zero at the floor to one at `loudest`, in decibels.
fn level(size: f32, loudest: f32) -> f32 {
    if size <= 0. || loudest <= 0. || !size.is_finite() {
        return 0.;
    }
    (1. - 20. * (size / loudest).log10() / FLOOR_DB).clamp(0., 1.)
}

/// The partials of the voice at `pitch` as the picture's stems: its number and its height.
pub(crate) fn partial_stems(voice: &MotionVoice, loudest: f32) -> Vec<(f32, f32)> {
    voice
        .partials
        .iter()
        .enumerate()
        .map(|(number, size)| (number as f32 + 1., level(*size, loudest)))
        .collect()
}

/// The loudest partial among the voices of `frames`, which is what every height is against.
pub(crate) fn loudest_partial<'a>(frames: impl IntoIterator<Item = &'a MotionFrame>) -> f32 {
    frames
        .into_iter()
        .flat_map(|frame| frame.voices.iter())
        .filter(|voice| voice.level > 0.)
        .flat_map(|voice| voice.partials.iter())
        .fold(0.0_f32, |loudest, size| loudest.max(*size))
}

/// How the partials of the note at `pitch` have died away: [`DECAY_FRAMES`] rows from the
/// oldest to the newest, each [`MOTION_PARTIALS`] heights. A frame the note is not in is a
/// row at rest, and so is the time before there were frames.
pub(crate) fn decay_surface(history: &[MotionFrame], pitch: f32) -> Vec<f32> {
    let recent = &history[history.len().saturating_sub(DECAY_FRAMES)..];
    let loudest = loudest_partial(recent);
    let mut values = vec![0.; (DECAY_FRAMES - recent.len()) * MOTION_PARTIALS];
    for frame in recent {
        let voice = frame
            .voices
            .iter()
            .find(|voice| voice.level > 0. && (voice.pitch - pitch).abs() <= 0.5);
        values.extend(match voice {
            Some(voice) => voice.partials.map(|size| level(size, loudest)),
            None => [0.; MOTION_PARTIALS],
        });
    }
    values
}

/// What excites a model, which decides what its trace is of and what it is read against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Exciter {
    /// A felt hammer: its force through the blow.
    Hammer,
    /// A mallet or a clapper: its force through the blow.
    Mallet,
    /// A bow: the string's speed under the hair through one period, in bow speeds.
    Bow,
    /// A reed: how far open it stands through one period, one being at rest.
    Reed,
    /// An air jet: where it meets the edge through one period.
    Jet,
}

impl Exciter {
    /// The exciter of the model `id`, or `None` for a model with nothing of the kind to show:
    /// a finger lets a string go and is gone.
    pub(crate) fn of(id: &str) -> Option<Self> {
        match id {
            "auris.physical.piano" | "auris.physical.hammered_dulcimer" => Some(Self::Hammer),
            "auris.physical.mallet" | "auris.physical.bell" => Some(Self::Mallet),
            "auris.physical.violin" => Some(Self::Bow),
            "auris.physical.clarinet" => Some(Self::Reed),
            "auris.physical.tin_whistle" => Some(Self::Jet),
            _ => None,
        }
    }

    /// The level worth a line across the trace: the bow's own speed, which a string that is
    /// gripped moves at; a reed at rest; the edge itself.
    pub(crate) fn rest(self) -> Option<f32> {
        match self {
            Self::Hammer | Self::Mallet => None,
            Self::Bow | Self::Reed => Some(1.),
            Self::Jet => Some(0.),
        }
    }

    /// The range the trace `scope` is drawn in: wide enough for it, and never so narrow that
    /// a quiet note fills the plot.
    pub(crate) fn range(self, scope: &[f32; MOTION_SCOPE]) -> (f32, f32) {
        let (low, high) = scope
            .iter()
            .filter(|sample| sample.is_finite())
            .fold((0.0_f32, 0.0_f32), |(low, high), sample| {
                (low.min(*sample), high.max(*sample))
            });
        match self {
            Self::Hammer | Self::Mallet => (0., high.max(0.5) * 1.1),
            // A string flies back under the bow many times faster than it is carried forward.
            Self::Bow => (low.min(-4.) * 1.1, 2.),
            Self::Reed => (0., 2.),
            Self::Jet => (-1.1, 1.1),
        }
    }

    /// What the trace is, in words.
    fn caption(self) -> Key {
        match self {
            Self::Hammer => Key::PhysicalExciterHammer,
            Self::Mallet => Key::PhysicalExciterMallet,
            Self::Bow => Key::PhysicalExciterBow,
            Self::Reed => Key::PhysicalExciterReed,
            Self::Jet => Key::PhysicalExciterJet,
        }
    }
}

/// A voice's trace as points across the plot, oldest at the left.
pub(crate) fn scope_points(scope: &[f32; MOTION_SCOPE]) -> Vec<(f32, f32)> {
    scope
        .iter()
        .enumerate()
        .map(|(index, sample)| {
            let at = index as f32 / (MOTION_SCOPE - 1) as f32;
            (at, if sample.is_finite() { *sample } else { 0. })
        })
        .collect()
}

/// The voice the single-voice pictures follow: the one most lately excited that still sounds.
pub(crate) fn newest_voice(frame: &MotionFrame) -> Option<&MotionVoice> {
    frame
        .voices
        .iter()
        .find(|voice| voice.level > 0. && voice.pitch.is_finite())
}

impl AurisApp {
    /// The inside of the physical instrument `id`, or `None` for anything else.
    pub(crate) fn physical_resonance(
        &self,
        id: &str,
        frame: &MotionFrame,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !id.starts_with("auris.physical.") || id == "auris.physical.choir" {
            return None;
        }
        let tokens = &theme.atelier;
        let spacing = tokens.spacing;
        let gain = self.physical_view.gain;
        let plate = body_plate(id);
        // A model with no body has only the one picture to turn.
        let shown = match plate {
            Some(_) => self.physical_view.resonance,
            None => Resonance::Decay,
        };
        let newest = newest_voice(frame);
        let (azimuth, elevation) = self.physical_view.orbit;
        let turning = Plot3d::new()
            .id("physical-resonance-plot")
            .shading(Shading::Smooth)
            .view(azimuth, elevation)
            .on_orbit({
                let app = cx.entity().downgrade();
                move |angles, _, cx| {
                    if let Some(app) = app.upgrade() {
                        app.update(cx, |this, cx| {
                            this.physical_view.orbit = *angles;
                            cx.notify();
                        });
                    }
                }
            });
        let (turning, caption) = match (shown, plate) {
            (Resonance::Body, Some(aspect)) => {
                let (columns, rows) = PLATE;
                (
                    turning
                        .surface(
                            columns,
                            rows,
                            body_surface(&frame.body, aspect, gain),
                            (0., aspect),
                            (0., 1.),
                        )
                        .z_range(-1., 1.),
                    Key::PhysicalBodyCaption,
                )
            }
            _ => {
                let pitch = newest.map_or(f32::from(self.physical_view.pitch), |voice| voice.pitch);
                (
                    turning
                        .surface(
                            MOTION_PARTIALS,
                            DECAY_FRAMES,
                            decay_surface(&self.physical_view.history, pitch),
                            (1., MOTION_PARTIALS as f32),
                            (-(DECAY_FRAMES as f32) / 30., 0.),
                        )
                        .z_range(0., 1.)
                        .labels(
                            self.t(Key::PhysicalAxisPartial),
                            self.t(Key::PhysicalAxisSeconds),
                            "",
                        ),
                    Key::PhysicalDecayCaption,
                )
            }
        };
        let loudest = loudest_partial([frame]);
        let mut spectrum = LinePlot::new()
            .x_range(1., MOTION_PARTIALS as f32)
            .y_range(0., 1.);
        for index in lane_voices(frame).into_iter().flatten() {
            let voice = &frame.voices[index];
            spectrum = spectrum.series(
                Series::stems(partial_stems(voice, loudest)).color(voice_color(theme, voice.pitch)),
            );
        }
        let well = |height: f32, plot: AnyElement| {
            div()
                .h(gpui::rems(height))
                .rounded(tokens.shape.radius_sm)
                .bg(tokens.colors.sunken)
                .overflow_hidden()
                .p(spacing.xs)
                .child(plot)
        };
        let titled = |title: &str, plot: AnyElement| {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(gpui::rems(12.))
                .gap(spacing.xs)
                .child(
                    Label::new(title.to_owned())
                        .size(TextSize::XSmall)
                        .color(TextColor::Muted),
                )
                .child(well(7., plot))
        };
        let mut plots = div().flex().flex_wrap().gap(spacing.md).child(titled(
            self.t(Key::PhysicalSpectrum),
            spectrum.into_any_element(),
        ));
        if let Some(exciter) = Exciter::of(id) {
            let scope = newest.map_or([0.; MOTION_SCOPE], |voice| voice.scope);
            let (low, high) = exciter.range(&scope);
            let mut trace = LinePlot::new().x_range(0., 1.).y_range(low, high);
            if let Some(rest) = exciter.rest() {
                trace = trace.series(
                    Series::line(vec![(0., rest), (1., rest)])
                        .color(tokens.colors.text_faint)
                        .faint(),
                );
            }
            if let Some(voice) = newest {
                trace = trace.series(
                    Series::line(scope_points(&scope)).color(voice_color(theme, voice.pitch)),
                );
            }
            plots = plots.child(
                div()
                    .id("physical-exciter")
                    .debug_selector(|| "physical-exciter".into())
                    .flex()
                    .flex_1()
                    .min_w(gpui::rems(12.))
                    .child(titled(self.t(exciter.caption()), trace.into_any_element())),
            );
        }
        let mut header = SectionHeader::new(self.t(Key::PhysicalResonance))
            .description(self.t(Key::PhysicalResonanceHint));
        if plate.is_some() {
            header = header.action(
                SegmentedControl::new(
                    "physical-resonance-view",
                    [Key::PhysicalViewBody, Key::PhysicalViewDecay]
                        .map(|key| Segment::label(self.t(key))),
                    usize::from(shown == Resonance::Decay),
                )
                .size(ControlSize::Small)
                .on_select(cx.listener(|this, index: &usize, _, cx| {
                    this.physical_view.resonance = if *index == 0 {
                        Resonance::Body
                    } else {
                        Resonance::Decay
                    };
                    cx.notify();
                })),
            );
        }
        Some(
            Card::new()
                .id("physical-resonance")
                .debug_selector(|| "physical-resonance".into())
                .w_full()
                .flex_shrink_0()
                .p(spacing.lg)
                .gap(spacing.md)
                .child(header)
                .child(well(
                    15.,
                    div()
                        .size_full()
                        .cursor_grab()
                        .child(turning)
                        .into_any_element(),
                ))
                .child(
                    Label::new(self.t(caption))
                        .size(TextSize::XSmall)
                        .color(TextColor::Faint),
                )
                .child(plots)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plates_modes_start_with_the_whole_board_and_split_its_length_first() {
        let modes = plate_modes(1.6);
        assert_eq!(modes[0], (1, 1));
        // A long plate bends along its length before it bends across.
        assert_eq!(modes[1], (2, 1));
        // A square one cannot tell the two apart, and has them next to each other.
        let square = plate_modes(1.);
        assert_eq!(
            {
                let mut pair = [square[1], square[2]];
                pair.sort();
                pair
            },
            [(1, 2), (2, 1)]
        );
        let mut seen = modes.to_vec();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), MOTION_BODY, "no shape is used twice");
    }

    #[test]
    fn a_body_is_drawn_from_its_modes_still_at_its_edges() {
        let (columns, rows) = PLATE;
        let mut body = [0.; MOTION_BODY];
        assert!(
            body_surface(&body, 1.6, 24.)
                .iter()
                .all(|height| *height == 0.)
        );
        // The lowest mode lifts the whole board, most in the middle.
        body[0] = 0.02;
        let lifted = body_surface(&body, 1.6, 24.);
        assert_eq!(lifted.len(), columns * rows);
        let at = |surface: &[f32], column: usize, row: usize| surface[row * columns + column];
        assert!(at(&lifted, columns / 2, rows / 2) > 0.4);
        assert!(lifted.iter().all(|height| *height >= -1e-6));
        for column in 0..columns {
            assert!(at(&lifted, column, 0).abs() < 1e-5);
            assert!(at(&lifted, column, rows - 1).abs() < 1e-5);
        }
        // The next rocks it end to end about a line that stays still.
        body = [0.; MOTION_BODY];
        body[1] = 0.02;
        let rocked = body_surface(&body, 1.6, 24.);
        let (left, right) = (
            at(&rocked, columns / 4, rows / 2),
            at(&rocked, 3 * columns / 4, rows / 2),
        );
        assert!(left > 0.2 && right < -0.2, "{left} and {right}");
        // And nothing is drawn past the top of the box, however hard the board is driven.
        body[1] = 50.;
        assert!(
            body_surface(&body, 1.6, 24.)
                .iter()
                .all(|height| height.abs() <= 1.)
        );
        body[1] = f32::NAN;
        assert!(
            body_surface(&body, 1.6, 24.)
                .iter()
                .all(|height| height.is_finite())
        );
    }

    #[test]
    fn partials_are_drawn_in_decibels_against_the_loudest() {
        let mut voice = MotionVoice {
            level: 1.,
            pitch: 60.,
            ..Default::default()
        };
        voice.partials[0] = 0.1;
        voice.partials[1] = 0.01;
        voice.partials[2] = 0.000_05;
        let stems = partial_stems(&voice, 0.1);
        assert_eq!(stems.len(), MOTION_PARTIALS);
        assert_eq!(stems[0], (1., 1.));
        // Twenty decibels down is a third of the way to a floor sixty down.
        assert!((stems[1].1 - 2. / 3.).abs() < 1e-4, "{}", stems[1].1);
        assert_eq!(stems[2].1, 0., "under the floor is not drawn");
        assert_eq!(stems[3].1, 0.);
        assert_eq!(level(0.1, 0.), 0.);
        assert_eq!(level(f32::NAN, 0.1), 0.);
    }

    #[test]
    fn the_decay_picture_follows_one_note_and_rests_where_it_is_not() {
        let frame = |pitch: f32, size: f32| {
            let mut frame = MotionFrame {
                active: 1,
                ..Default::default()
            };
            frame.voices[0].pitch = pitch;
            frame.voices[0].level = 0.5;
            frame.voices[0].partials[0] = size;
            frame.voices[0].partials[3] = size * 0.1;
            frame
        };
        // A note that halves every frame, with another note in the way for one of them.
        let mut history: Vec<MotionFrame> = (0..6).map(|n| frame(60., 0.5_f32.powi(n))).collect();
        history[2] = frame(67., 1.);
        let surface = decay_surface(&history, 60.);
        assert_eq!(surface.len(), DECAY_FRAMES * MOTION_PARTIALS);
        let row = |index: usize| &surface[index * MOTION_PARTIALS..(index + 1) * MOTION_PARTIALS];
        // Before there were frames, and where the note was not sounding: at rest.
        assert!(row(0).iter().all(|height| *height == 0.));
        let first = DECAY_FRAMES - 6;
        assert!(row(first + 2).iter().all(|height| *height == 0.));
        assert_eq!(row(first)[0], 1.);
        // Six decibels a frame is a tenth of the way to the floor each time.
        assert!((row(first + 1)[0] - 0.9).abs() < 5e-3);
        assert!((row(first + 5)[0] - 0.5).abs() < 5e-3);
        assert!(row(first)[3] < row(first)[0]);
        // A history longer than the picture shows its newest frames.
        let long: Vec<MotionFrame> = (0..100).map(|_| frame(60., 0.2)).collect();
        assert!(decay_surface(&long, 60.)[..MOTION_PARTIALS][0] > 0.);
    }

    #[test]
    fn every_model_with_an_exciter_names_it_and_draws_it_in_a_range_that_holds_it() {
        for (id, exciter) in [
            ("auris.physical.piano", Some(Exciter::Hammer)),
            ("auris.physical.hammered_dulcimer", Some(Exciter::Hammer)),
            ("auris.physical.mallet", Some(Exciter::Mallet)),
            ("auris.physical.bell", Some(Exciter::Mallet)),
            ("auris.physical.violin", Some(Exciter::Bow)),
            ("auris.physical.clarinet", Some(Exciter::Reed)),
            ("auris.physical.tin_whistle", Some(Exciter::Jet)),
            ("auris.physical.guitar", None),
            ("auris.physical.electric_guitar", None),
            ("auris.physical.bass", None),
        ] {
            assert_eq!(Exciter::of(id), exciter, "{id}");
        }
        // A string under a bow flies back far faster than it is carried: the range follows.
        let mut scope = [1.; MOTION_SCOPE];
        scope[40] = -11.;
        let (low, high) = Exciter::Bow.range(&scope);
        assert!(low < -11. && high > 1.);
        assert!(Exciter::Bow.range(&[1.; MOTION_SCOPE]).0 <= -4.);
        // A soft blow does not fill the plot, and a hard one is not cut off.
        assert!(Exciter::Hammer.range(&[0.05; MOTION_SCOPE]).1 >= 0.5);
        assert!(Exciter::Hammer.range(&[1.4; MOTION_SCOPE]).1 > 1.4);
        assert_eq!(Exciter::Bow.rest(), Some(1.));
        assert_eq!(Exciter::Jet.rest(), Some(0.));
        assert_eq!(Exciter::Hammer.rest(), None);
        let points = scope_points(&scope);
        assert_eq!(points.len(), MOTION_SCOPE);
        assert_eq!((points[0].0, points[MOTION_SCOPE - 1].0), (0., 1.));
        assert_eq!(points[40].1, -11.);
    }

    #[test]
    fn which_models_have_a_body_to_turn() {
        for id in [
            "auris.physical.piano",
            "auris.physical.hammered_dulcimer",
            "auris.physical.guitar",
            "auris.physical.bass",
            "auris.physical.violin",
        ] {
            assert!(body_plate(id).is_some_and(|aspect| aspect >= 1.), "{id}");
        }
        for id in [
            "auris.physical.electric_guitar",
            "auris.physical.mallet",
            "auris.physical.bell",
            "auris.physical.clarinet",
            "auris.physical.tin_whistle",
            "auris.synth.fm2",
        ] {
            assert!(body_plate(id).is_none(), "{id}");
        }
    }

    #[test]
    fn the_pictures_follow_the_voice_most_lately_excited() {
        let mut frame = MotionFrame::default();
        assert!(newest_voice(&frame).is_none());
        frame.voices[1].pitch = 64.;
        frame.voices[1].level = 0.3;
        frame.voices[2].pitch = 67.;
        frame.voices[2].level = 0.3;
        assert_eq!(newest_voice(&frame).unwrap().pitch, 64.);
        assert_eq!(loudest_partial([&frame]), 0.);
        frame.voices[2].partials[5] = 0.25;
        // A slot that has stopped sounding keeps its last numbers and is not counted.
        frame.voices[3].partials[0] = 9.;
        assert_eq!(loudest_partial([&frame]), 0.25);
    }
}
