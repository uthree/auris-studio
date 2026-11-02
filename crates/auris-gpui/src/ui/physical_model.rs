//! The physical instrument's body, live, beside its parameter editor — and a body to touch.
//!
//! Drawn with Atelier's physical-model plots: one [`StringLane`] per sounding string, bar,
//! shell, bore or vocal tract, a circular [`Heatmap`] per drum head or cymbal, and the modes the
//! body rings in on a stem plot. Every body is also a surface. Pressing it excites the model at
//! that place, the way the instrument itself is played: a string is pulled and let go, a head or
//! a struck string is pressed, a bowed string, a bore or a tract sounds for as long as the press
//! is held. The note is the voice the lane is showing, or the audition note when the lane is at
//! rest, and nothing a touch does is written into the document.

use std::{cell::Cell, rc::Rc};

use super::plugin_window::PluginSubject;
use crate::{app::AurisApp, theme::Theme};
use atelier::plot::{LinePlot, Series};
use atelier::viz::{Heatmap, StringLane};
use atelier::{
    Badge, Button, Card, ControlSize, IconName, Label, LevelMeter, PianoKeyboard, SectionHeader,
    StatusDot, TextColor, TextSize,
};
use auris_i18n::{Key, Language, messages};
use auris_session::{MotionFrame, MotionGeometry, MotionVoice, prelude::*};
use gpui::{
    AnyElement, Bounds, Context, DispatchPhase, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, canvas, div, prelude::*, px,
};

/// Lanes on screen, which is every voice a motion frame carries.
const LANES: usize = auris_session::MOTION_VOICES;
/// The margin [`StringLane`] leaves at each anchor, so a pointer maps onto the drawn string.
const LANE_INSET: f32 = 10.;

#[derive(Debug)]
pub(crate) struct PhysicalView {
    source: Option<(TrackId, String)>,
    pub(crate) frame: Option<MotionFrame>,
    pub(crate) frozen: bool,
    pub(crate) playing: bool,
    pub(crate) pitch: u8,
    drum_preview: bool,
    gain: f32,
    effects_disabled: bool,
    /// Recent captured frames used for short, bounded motion trails.
    history: Vec<MotionFrame>,
    /// A press on a body that has not been let go yet.
    pub(crate) touch: Option<Touch>,
    /// A plucked or struck note left ringing after its touch, released by the next one.
    pub(crate) ringing: Option<u8>,
    /// Where each body was laid out on the last frame, to turn a pointer into a place on it.
    surfaces: Rc<[Cell<Option<Bounds<Pixels>>>; LANES]>,
}
impl Default for PhysicalView {
    fn default() -> Self {
        Self {
            source: None,
            frame: None,
            frozen: false,
            playing: false,
            pitch: 60,
            drum_preview: false,
            gain: 24.0,
            effects_disabled: false,
            history: Vec::new(),
            touch: None,
            ringing: None,
            surfaces: Rc::default(),
        }
    }
}

impl PhysicalView {
    const HISTORY_LIMIT: usize = 12;

    fn accept_frame(&mut self, frame: MotionFrame) -> bool {
        if self.frozen || self.frame == Some(frame) {
            return false;
        }
        self.history.push(frame);
        if self.history.len() > Self::HISTORY_LIMIT {
            self.history.remove(0);
        }
        self.frame = Some(frame);
        true
    }
}

/// How a model is played, which is what a press on its body does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gesture {
    /// Pulled aside and let go: the note starts on release.
    Pluck,
    /// Struck where pressed: the note starts on the press and rings on.
    Strike,
    /// A key: struck on the press and damped on the release, as a piano's is.
    Key,
    /// Bowed, blown or sung for as long as the press is held.
    Sustain,
}

impl Gesture {
    fn of(id: &str) -> Self {
        match id {
            "auris.physical.guitar" | "auris.physical.electric_guitar" | "auris.physical.bass" => {
                Self::Pluck
            }
            "auris.physical.piano" => Self::Key,
            "auris.physical.violin"
            | "auris.physical.clarinet"
            | "auris.physical.tin_whistle"
            | "auris.physical.choir" => Self::Sustain,
            _ => Self::Strike,
        }
    }
}

/// One press on a body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Touch {
    lane: usize,
    pitch: u8,
    /// Where along the body, from zero to one; on a head, from the centre to the rim.
    at: f32,
    /// How far from the body at rest, from zero on it to one at the edge of its lane.
    pull: f32,
    /// Whether the note has started, which a pluck's has not until it is let go.
    sounding: bool,
}

/// The place along a lane and the distance from its rest line that a pointer marks.
fn lane_touch(bounds: Bounds<Pixels>, position: Point<Pixels>) -> (f32, f32) {
    let inset = px(LANE_INSET);
    let width = (bounds.size.width - inset * 2.).max(px(1.));
    let at = ((position.x - bounds.left() - inset) / width).clamp(0., 1.);
    let half = (bounds.size.height / 2.).max(px(1.));
    let pull = ((bounds.center().y - position.y) / half).clamp(-1., 1.);
    (at, pull)
}

/// The radius on a circular head that a pointer marks, matching how [`Heatmap`] fits its
/// circle into the square at the centre of its bounds. Outside the head, `None`.
fn head_touch(bounds: Bounds<Pixels>, position: Point<Pixels>) -> Option<f32> {
    let radius = bounds.size.width.min(bounds.size.height) / 2.;
    if radius <= px(0.) {
        return None;
    }
    let offset = position - bounds.center();
    let distance = (offset.x / radius).hypot(offset.y / radius);
    (distance <= 1.).then_some(distance)
}

/// How hard a touch plays: never silent, and as hard as the instrument goes at the lane's edge.
fn touch_velocity(pull: f32) -> f32 {
    (0.3 + 0.7 * pull.abs()).clamp(0.3, 1.)
}

/// A string held aside at `at`: two straight segments from the anchors to the pointer.
fn pulled_string(at: f32, pull: f32) -> Vec<f32> {
    let at = at.clamp(0.02, 0.98);
    (0..auris_session::MOTION_POINTS)
        .map(|index| {
            let x = index as f32 / (auris_session::MOTION_POINTS - 1) as f32;
            let shape = if x <= at {
                x / at
            } else {
                (1. - x) / (1. - at)
            };
            shape * pull * 0.8
        })
        .collect()
}

fn active(voice: &MotionVoice) -> bool {
    voice.level > 0. && voice.pitch.is_finite()
}

/// Which voice each lane shows: the sounding ones from the highest pitch down, so a chord keeps
/// its order however the instrument ranks its voices by recency, and then the silent slots.
fn lane_voices(frame: &MotionFrame) -> [Option<usize>; LANES] {
    let mut order: Vec<usize> = (0..LANES).filter(|i| active(&frame.voices[*i])).collect();
    order.sort_by(|a, b| frame.voices[*b].pitch.total_cmp(&frame.voices[*a].pitch));
    std::array::from_fn(|lane| order.get(lane).copied())
}

/// Pitch and geometry are the available identity hints; never reuse a historical body.
fn match_history(current: &MotionFrame, past: &MotionFrame) -> [Option<usize>; LANES] {
    let mut used = [false; LANES];
    std::array::from_fn(|index| {
        let voice = &current.voices[index];
        if !active(voice) {
            return None;
        }
        let found = past
            .voices
            .iter()
            .enumerate()
            .filter(|(candidate, previous)| {
                !used[*candidate]
                    && previous.level > 0.
                    && previous.geometry.unwrap_or(past.geometry)
                        == voice.geometry.unwrap_or(current.geometry)
                    && (previous.pitch - voice.pitch).abs() <= 0.5
            })
            .min_by(|(_, a), (_, b)| {
                (a.pitch - voice.pitch)
                    .abs()
                    .total_cmp(&(b.pitch - voice.pitch).abs())
            })
            .map(|(candidate, _)| candidate);
        if let Some(candidate) = found {
            used[candidate] = true;
        }
        found
    })
}

fn is_surface(geometry: MotionGeometry) -> bool {
    matches!(geometry, MotionGeometry::Membrane | MotionGeometry::Plate)
}

/// The colour of a voice in every plot it appears in, from the pitch it plays.
fn voice_color(theme: &Theme, pitch: f32) -> Hsla {
    let series = &theme.atelier.colors.series;
    series[pitch.round().clamp(0., 127.) as usize % series.len()]
}

impl AurisApp {
    pub(crate) fn close_physical_view(&mut self) {
        self.release_physical_preview();
        self.session.watch_instrument_motion(None);
        self.physical_view = PhysicalView {
            pitch: self.physical_view.pitch,
            drum_preview: self.physical_view.drum_preview,
            gain: self.physical_view.gain,
            effects_disabled: self.physical_view.effects_disabled,
            ..Default::default()
        };
    }
    pub(crate) fn release_physical_preview(&mut self) {
        let Some((track, _)) = self.physical_view.source.as_ref().map(|(t, id)| (*t, id)) else {
            self.physical_view.playing = false;
            self.physical_view.touch = None;
            self.physical_view.ringing = None;
            return;
        };
        if self.physical_view.playing {
            self.session.note_off(track, self.physical_view.pitch);
            self.physical_view.playing = false;
        }
        if let Some(touch) = self.physical_view.touch.take()
            && touch.sounding
        {
            self.session.note_off(track, touch.pitch);
        }
        if let Some(pitch) = self.physical_view.ringing.take() {
            self.session.note_off(track, pitch);
        }
    }
    pub(crate) fn poll_physical_view(&mut self) {
        let source = self.plugin_window.and_then(|editor| {
            let PluginSubject::Instrument(track) = editor.subject else {
                return None;
            };
            let (id, _) = self.resolve_plugin(editor.subject)?;
            self.session
                .has_instrument_motion(track)
                .then_some((track, id))
        });
        if source != self.physical_view.source {
            self.close_physical_view();
            if let Some((_, id)) = &source {
                let drums = is_drum_model(id);
                if drums != self.physical_view.drum_preview {
                    self.physical_view.pitch = if drums { 36 } else { 60 };
                }
                self.physical_view.drum_preview = drums;
            }
            self.physical_view.source = source;
        }
        let track = self.physical_view.source.as_ref().map(|(track, _)| *track);
        self.session
            .watch_instrument_motion(track.filter(|_| !self.physical_view.frozen));
        if let Some(frame) = track.and_then(|track| self.session.instrument_motion(track)) {
            self.physical_view.accept_frame(frame);
        }
    }
    pub(crate) fn toggle_physical_freeze(&mut self) {
        if self
            .physical_view
            .frame
            .is_some_and(|frame| frame.active > 0)
        {
            self.physical_view.frozen = !self.physical_view.frozen;
            self.poll_physical_view();
        }
    }
    fn toggle_physical_preview(&mut self, cx: &mut Context<Self>) {
        if self.physical_view.playing {
            self.release_physical_preview();
        } else if self.acquire_audio(cx)
            && let Some((track, _)) = &self.physical_view.source
        {
            let track = *track;
            self.release_physical_preview();
            self.session.note_on(track, self.physical_view.pitch, 0.75);
            if self.physical_view.drum_preview {
                // A drum preview is a strike, so the same button can immediately strike again.
                self.session.note_off(track, self.physical_view.pitch);
            } else {
                self.physical_view.playing = true;
            }
        }
    }
    fn transpose_physical_preview(&mut self, delta: i16, cx: &mut Context<Self>) {
        let playing = self.physical_view.playing;
        self.release_physical_preview();
        self.physical_view.pitch = if self.physical_view.drum_preview {
            next_drum_preview(self.physical_view.pitch, delta > 0)
        } else {
            (i16::from(self.physical_view.pitch) + delta).clamp(24, 96) as u8
        };
        if playing {
            self.toggle_physical_preview(cx);
        }
    }
    /// A key on the audition keyboard: the note a touch plays from now on, sounded once.
    fn audition_physical_key(&mut self, pitch: u8, cx: &mut Context<Self>) {
        self.release_physical_preview();
        self.physical_view.pitch = pitch;
        self.toggle_physical_preview(cx);
    }

    /// The note a press on `lane` plays: the voice it shows, or the audition note at rest.
    fn lane_pitch(&self, lane: usize) -> u8 {
        let frame = self.physical_view.frame.unwrap_or_default();
        lane_voices(&frame)[lane]
            .map(|index| frame.voices[index].pitch.round().clamp(0., 127.) as u8)
            .unwrap_or(self.physical_view.pitch)
    }

    /// The geometry `lane` is drawn as, which decides how a pointer on it is read.
    fn lane_geometry(&self, lane: usize) -> MotionGeometry {
        let frame = self.physical_view.frame.unwrap_or_default();
        match lane_voices(&frame)[lane] {
            Some(index) => frame.voices[index].geometry.unwrap_or(frame.geometry),
            None if self.physical_view.drum_preview => drum_geometry(self.physical_view.pitch),
            None => resting_geometry(
                self.physical_view
                    .source
                    .as_ref()
                    .map_or("", |(_, id)| id.as_str()),
                frame.geometry,
            ),
        }
    }

    /// A press on a body. Everything but a pluck sounds at once.
    pub(crate) fn touch_physical_body(
        &mut self,
        lane: usize,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some((track, id)) = self.physical_view.source.clone() else {
            return;
        };
        let Some(bounds) = self.physical_view.surfaces.get(lane).and_then(Cell::get) else {
            return;
        };
        let (at, pull) = if is_surface(self.lane_geometry(lane)) {
            let Some(radius) = head_touch(bounds, position) else {
                return;
            };
            // A stick lands as hard wherever it lands on the head; how hard is the audition's.
            (radius, 0.65)
        } else {
            lane_touch(bounds, position)
        };
        if !self.acquire_audio(cx) {
            return;
        }
        let pitch = self.lane_pitch(lane);
        self.release_physical_preview();
        let gesture = Gesture::of(&id);
        let sounding = gesture != Gesture::Pluck;
        if sounding {
            self.session
                .touch_instrument(track, pitch, at, touch_velocity(pull));
        }
        self.physical_view.touch = Some(Touch {
            lane,
            pitch,
            at,
            pull,
            sounding,
        });
        cx.notify();
    }

    /// The pointer moving with the press held: a pulled string follows it.
    pub(crate) fn drag_physical_body(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(touch) = self.physical_view.touch.as_mut() else {
            return;
        };
        if touch.sounding {
            return;
        }
        let Some(bounds) = self
            .physical_view
            .surfaces
            .get(touch.lane)
            .and_then(Cell::get)
        else {
            return;
        };
        (touch.at, touch.pull) = lane_touch(bounds, position);
        cx.notify();
    }

    /// The press let go: a pulled string is released into its note, a key is damped, a bow
    /// lifts, and a struck body rings on until the next touch.
    pub(crate) fn release_physical_body(&mut self, cx: &mut Context<Self>) {
        let Some(touch) = self.physical_view.touch.take() else {
            return;
        };
        let Some((track, id)) = self.physical_view.source.clone() else {
            return;
        };
        match Gesture::of(&id) {
            Gesture::Pluck => {
                self.session.touch_instrument(
                    track,
                    touch.pitch,
                    touch.at,
                    touch_velocity(touch.pull),
                );
                self.physical_view.ringing = Some(touch.pitch);
            }
            Gesture::Strike => self.physical_view.ringing = Some(touch.pitch),
            Gesture::Key | Gesture::Sustain => self.session.note_off(track, touch.pitch),
        }
        cx.notify();
    }

    pub(crate) fn physical_model_display(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.poll_physical_view();
        let (track, id) = self.physical_view.source.clone()?;
        let piano = id == "auris.physical.piano";
        let drums = is_drum_model(&id);
        let gesture = Gesture::of(&id);
        let frame = self.physical_view.frame.unwrap_or_default();
        let theme = self.theme.clone();
        let tokens = &theme.atelier;
        let spacing = tokens.spacing;
        let frozen = self.physical_view.frozen;
        let playing = self.physical_view.playing;
        let effects_disabled = self.physical_view.effects_disabled;
        let gain = self.physical_view.gain;
        let language = self.language;
        let note = if drums {
            drum_name(self.physical_view.pitch, language).to_owned()
        } else {
            midi_name(i32::from(self.physical_view.pitch))
        };
        let resting_contact = self
            .session
            .instrument_descriptors(track)
            .iter()
            .find(|descriptor| descriptor.key == "position")
            .map(|descriptor| {
                self.session.param_value(
                    ParamTarget::Instrument {
                        track,
                        param: descriptor.id,
                    },
                    descriptor,
                )
            });
        let caption = if frame.active == 0 {
            self.t(Key::PhysicalWaiting).to_owned()
        } else {
            messages::physical_activity(
                language,
                frame.active,
                frame.expression,
                (id == "auris.physical.violin").then_some(frame.pressure),
                piano.then_some(frame.pedal),
            )
        };
        let hint = self.t(if drums {
            Key::PhysicalTouchHead
        } else {
            match (gesture, frame.geometry, id.as_str()) {
                (Gesture::Pluck, ..) => Key::PhysicalTouchPluck,
                (Gesture::Sustain, _, "auris.physical.violin") => Key::PhysicalTouchBow,
                (Gesture::Sustain, _, "auris.physical.choir") => Key::PhysicalTouchSing,
                (Gesture::Sustain, ..) => Key::PhysicalTouchBlow,
                _ => Key::PhysicalTouchStrike,
            }
        });
        let bodies = Bodies {
            frame,
            history: if effects_disabled {
                Vec::new()
            } else {
                self.physical_view.history.clone()
            },
            gain,
            touch: self.physical_view.touch,
            resting: if drums {
                drum_geometry(self.physical_view.pitch)
            } else {
                resting_geometry(&id, frame.geometry)
            },
            resting_contact,
            resting_pitch: self.physical_view.pitch,
            drums,
            language,
        };
        let motion = self.physical_bodies(&bodies, &theme, cx);
        let toolbar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(spacing.sm)
            .child(
                Button::new(
                    "physical-gain",
                    format!("{}: {gain:.0}×", self.t(Key::VisualizerDisplayGain)),
                )
                .ghost()
                .size(ControlSize::Small)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.physical_view.gain = match this.physical_view.gain as u32 {
                        6 => 24.,
                        24 => 96.,
                        _ => 6.,
                    };
                    cx.notify();
                })),
            )
            .child(
                Button::new("physical-effects", self.t(Key::VisualizerEffects))
                    .ghost()
                    .size(ControlSize::Small)
                    .selected(!effects_disabled)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.physical_view.effects_disabled = !this.physical_view.effects_disabled;
                        cx.notify();
                    })),
            )
            .child(
                Button::new(
                    "physical-freeze",
                    self.t(if frozen {
                        Key::PhysicalResume
                    } else {
                        Key::VisualizerFreeze
                    }),
                )
                .icon(IconName::Pause)
                .size(ControlSize::Small)
                .selected(frozen)
                .disabled(frame.active == 0)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_physical_freeze();
                    if this
                        .physical_view
                        .frame
                        .is_none_or(|frame| frame.active == 0)
                    {
                        window.focus(&this.focus);
                    }
                    cx.notify();
                })),
            )
            .children((piano && frame.pedal).then(|| Badge::new(self.t(Key::PhysicalPedal))));
        let motion_card = Card::new()
            .id("physical-model")
            .debug_selector(|| "physical-model".into())
            .w_full()
            .flex_shrink_0()
            .p(spacing.lg)
            .gap(spacing.md)
            .child(
                SectionHeader::new(self.t(Key::PhysicalMotion))
                    .description(self.t(Key::PhysicalObserved))
                    .action(Badge::new(self.t(if frozen {
                        Key::PhysicalFrozen
                    } else if frame.active > 0 {
                        Key::PhysicalLive
                    } else {
                        Key::VisualizerWaiting
                    }))),
            )
            .child(toolbar)
            .child(motion)
            .child(
                Label::new(hint)
                    .size(TextSize::XSmall)
                    .color(TextColor::Faint),
            )
            .child(
                Label::new(caption)
                    .size(TextSize::Small)
                    .color(TextColor::Muted),
            );
        let analysis = self.physical_analysis(&id, &frame, &theme);
        let audition = self.physical_audition(drums, playing, &note, cx);
        Some(
            div()
                .id("physical-view")
                .flex()
                .flex_col()
                .w_full()
                .gap(spacing.lg)
                .child(motion_card)
                .children(analysis)
                .child(audition)
                .into_any_element(),
        )
    }

    /// The bodies themselves: a lane per string-like body, a row of heads for a kit.
    fn physical_bodies(
        &mut self,
        bodies: &Bodies,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = &theme.atelier;
        let spacing = tokens.spacing;
        let lanes = lane_voices(&bodies.frame);
        let mut container = div()
            .id("physical-motion")
            .debug_selector(|| "physical-motion".into())
            .w_full()
            .flex()
            .gap(spacing.sm)
            .child(self.touch_tracker(cx));
        container = if bodies.drums {
            container.flex_row().flex_wrap()
        } else {
            container.flex_col()
        };
        for (lane, voice) in lanes.iter().enumerate() {
            let shown = voice.map(|index| bodies.frame.voices[index]);
            let geometry = shown.map_or(bodies.resting, |voice| {
                voice.geometry.unwrap_or(bodies.frame.geometry)
            });
            let plot = bodies.plot(lane, *voice, geometry, theme);
            let surface = self.touch_surface(lane, plot, cx);
            let (label, state) = match shown {
                Some(voice) => (
                    if bodies.drums {
                        drum_name(voice.pitch.round() as u8, bodies.language).to_owned()
                    } else {
                        let cents = (voice.pitch - voice.pitch.round()) * 100.;
                        let name = midi_name(voice.pitch.round().clamp(0., 127.) as i32);
                        if cents.abs() >= 1. {
                            format!("{name} {cents:+.0}¢")
                        } else {
                            name
                        }
                    },
                    if voice.held {
                        Key::PhysicalHeld
                    } else {
                        Key::PhysicalReleased
                    }
                    .get(bodies.language),
                ),
                None if lane == 0 => (
                    if bodies.drums {
                        drum_name(bodies.resting_pitch, bodies.language).to_owned()
                    } else {
                        midi_name(i32::from(bodies.resting_pitch))
                    },
                    "",
                ),
                None => ("—".to_owned(), ""),
            };
            let color = shown.map_or(tokens.colors.text_faint, |voice| {
                voice_color(theme, voice.pitch)
            });
            let caption = div()
                .flex()
                .flex_col()
                .gap(spacing.xxs)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(spacing.xs)
                        .child(StatusDot::new(color))
                        .child(Label::new(label).mono()),
                )
                .child(
                    Label::new(state)
                        .size(TextSize::XSmall)
                        .color(TextColor::Faint),
                );
            let level = shown.map_or(0., |voice| voice.level.clamp(0., 1.));
            container = container.child(if bodies.drums {
                div()
                    .flex()
                    .flex_col()
                    .gap(spacing.xs)
                    .flex_1()
                    .min_w(gpui::rems(7.))
                    .child(
                        div()
                            .h(gpui::rems(8.))
                            .rounded(tokens.shape.radius_sm)
                            .bg(tokens.colors.sunken)
                            .p(spacing.xs)
                            .child(surface),
                    )
                    .child(caption)
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .items_center()
                    .gap(spacing.md)
                    .h(gpui::rems(4.))
                    .child(div().w(gpui::rems(5.)).flex_shrink_0().child(caption))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .rounded(tokens.shape.radius_sm)
                            .bg(tokens.colors.sunken)
                            .child(surface),
                    )
                    .child(
                        div()
                            .h_full()
                            .py(spacing.sm)
                            .child(LevelMeter::new(level).thickness(px(4.))),
                    )
                    .into_any_element()
            });
        }
        container.into_any_element()
    }

    /// A body that answers a press, and remembers where it was laid out so the press can be
    /// read as a place on it.
    fn touch_surface(&self, lane: usize, plot: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let surfaces = Rc::clone(&self.physical_view.surfaces);
        div()
            .id(("physical-body", lane))
            .debug_selector(move || format!("physical-body-{lane}"))
            .relative()
            .size_full()
            .child(plot)
            .child(
                canvas(
                    move |bounds, _, _| {
                        if let Some(cell) = surfaces.get(lane) {
                            cell.set(Some(bounds));
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.touch_physical_body(lane, event.position, cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| this.release_physical_body(cx)),
            )
            .into_any_element()
    }

    /// While a press is held, follows the pointer wherever it goes in the window and catches
    /// the release even off the body, so a string pulled past its lane still lets go.
    fn touch_tracker(&self, cx: &mut Context<Self>) -> AnyElement {
        let held = self.physical_view.touch.is_some();
        let entity = cx.entity().downgrade();
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                if !held {
                    return;
                }
                let moved = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && event.pressed_button.is_some() {
                        moved
                            .update(cx, |this, cx| this.drag_physical_body(event.position, cx))
                            .ok();
                    }
                });
                let released = entity.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                        released
                            .update(cx, |this, cx| this.release_physical_body(cx))
                            .ok();
                    }
                });
            },
        )
        .absolute()
        .size_0()
        .into_any_element()
    }

    /// The modes a modal body rings in, or the shape of a singer's tract.
    fn physical_analysis(
        &self,
        id: &str,
        frame: &MotionFrame,
        theme: &Theme,
    ) -> Option<AnyElement> {
        let tokens = &theme.atelier;
        let spacing = tokens.spacing;
        let tract = id == "auris.physical.choir";
        let modal = matches!(
            id,
            "auris.physical.piano"
                | "auris.physical.bell"
                | "auris.physical.mallet"
                | "auris.physical.hammered_dulcimer"
        ) || is_drum_model(id);
        if !tract && !modal {
            return None;
        }
        let gain = self.physical_view.gain;
        let mut plot = LinePlot::new().x_range(1., 8.).y_range(0., 1.);
        for index in lane_voices(frame).into_iter().flatten() {
            let voice = &frame.voices[index];
            let color = voice_color(theme, voice.pitch);
            let values = voice.modes.iter().enumerate().map(|(n, value)| {
                let value = if tract {
                    value.clamp(0., 1.)
                } else {
                    displayed_motion(*value, gain).abs()
                };
                (n as f32 + 1., value)
            });
            plot = plot.series(if tract {
                Series::line(values.collect()).color(color)
            } else {
                Series::stems(values.collect()).color(color)
            });
            if tract {
                // Every singer of a note shares the vowel; one profile says it.
                break;
            }
        }
        Some(
            Card::new()
                .id("physical-analysis")
                .debug_selector(|| "physical-analysis".into())
                .w_full()
                .flex_shrink_0()
                .p(spacing.lg)
                .gap(spacing.md)
                .child(SectionHeader::new(self.t(if tract {
                    Key::PhysicalTract
                } else {
                    Key::PhysicalModes
                })))
                .child(
                    div()
                        .h(gpui::rems(7.))
                        .rounded(tokens.shape.radius_sm)
                        .bg(tokens.colors.sunken)
                        .overflow_hidden()
                        .p(spacing.xs)
                        .child(plot),
                )
                .into_any_element(),
        )
    }

    /// The note a touch plays, chosen on a keyboard or, for a kit, by pad.
    fn physical_audition(
        &self,
        drums: bool,
        playing: bool,
        note: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = &self.theme.atelier;
        let spacing = tokens.spacing;
        let pitch = self.physical_view.pitch;
        let controls = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(spacing.sm)
            .child(
                Button::new(
                    "physical-octave-down",
                    self.t(if drums {
                        Key::PhysicalPreviousPad
                    } else {
                        Key::PhysicalLower
                    }),
                )
                .ghost()
                .size(ControlSize::Small)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.transpose_physical_preview(-12, cx);
                    cx.notify();
                })),
            )
            .child(
                Button::new(
                    "physical-play",
                    messages::physical_preview(self.language, note, playing),
                )
                .icon(if playing {
                    IconName::Pause
                } else {
                    IconName::Play
                })
                .size(ControlSize::Small)
                .selected(playing)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toggle_physical_preview(cx);
                    cx.notify();
                })),
            )
            .child(
                Button::new(
                    "physical-octave-up",
                    self.t(if drums {
                        Key::PhysicalNextPad
                    } else {
                        Key::PhysicalHigher
                    }),
                )
                .ghost()
                .size(ControlSize::Small)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.transpose_physical_preview(12, cx);
                    cx.notify();
                })),
            );
        let low = (pitch / 12 * 12).clamp(24, 84);
        Card::new()
            .id("physical-audition")
            .debug_selector(|| "physical-audition".into())
            .w_full()
            .flex_shrink_0()
            .p(spacing.lg)
            .gap(spacing.md)
            .child(
                SectionHeader::new(self.t(Key::PhysicalAudition))
                    .description(self.t(Key::PhysicalAuditionHint)),
            )
            .child(controls)
            .children((!drums).then(|| {
                PianoKeyboard::new("physical-keys", low, low + 24)
                    .pressed(playing.then_some(pitch))
                    .on_press(cx.listener(|this, pitch: &u8, _, cx| {
                        this.audition_physical_key(*pitch, cx);
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }
}

/// What the bodies are drawn from, gathered once per frame.
struct Bodies {
    frame: MotionFrame,
    history: Vec<MotionFrame>,
    gain: f32,
    touch: Option<Touch>,
    /// What an empty lane is drawn as.
    resting: MotionGeometry,
    /// Where the instrument's own position control excites it, shown on the resting body.
    resting_contact: Option<f32>,
    resting_pitch: u8,
    drums: bool,
    language: Language,
}

impl Bodies {
    /// One body: a lane for anything with a length, a head for a surface.
    fn plot(
        &self,
        lane: usize,
        voice: Option<usize>,
        geometry: MotionGeometry,
        theme: &Theme,
    ) -> AnyElement {
        let shown = voice.map(|index| &self.frame.voices[index]);
        if is_surface(geometry) {
            let values = shown.map_or_else(
                || vec![0.; auris_session::MOTION_POINTS],
                |voice| {
                    voice
                        .points
                        .iter()
                        .map(|value| displayed_motion(*value, self.gain))
                        .collect()
                },
            );
            return Heatmap::new(8, 8, values)
                .signed()
                .circular()
                .into_any_element();
        }
        let touch = self.touch.filter(|touch| touch.lane == lane);
        let color = match (shown, touch) {
            (Some(voice), _) => voice_color(theme, voice.pitch),
            (None, Some(touch)) => voice_color(theme, f32::from(touch.pitch)),
            (None, None) => theme.atelier.colors.text_faint,
        };
        let points = match (touch, shown) {
            // A string held aside shows where it is held, until it is let go.
            (Some(touch), _) if !touch.sounding => pulled_string(touch.at, touch.pull),
            (_, Some(voice)) => body_points(voice, geometry, self.gain),
            (_, None) => vec![0.; auris_session::MOTION_POINTS],
        };
        let mut plot = StringLane::new(points).color(color);
        let contact = match (touch, shown) {
            (Some(touch), _) => Some(touch.at),
            (_, Some(voice)) => Some(voice.contact),
            (_, None) => self.resting_contact,
        };
        match geometry {
            // A shell is a ring opened out; it has no one place it was struck.
            MotionGeometry::Shell => {}
            // Air is set going at the mouthpiece or the glottis, the first point.
            MotionGeometry::Bore | MotionGeometry::Tract => plot = plot.contact(0.),
            _ => {
                if let Some(contact) = contact.filter(|contact| contact.is_finite()) {
                    plot = plot.contact(contact);
                }
            }
        }
        if let (Some(index), Some(voice)) = (voice, shown) {
            let trails = self
                .history
                .iter()
                .rev()
                .skip(1)
                .take(5)
                .rev()
                .filter_map(|past| {
                    let matched = match_history(&self.frame, past)[index]?;
                    Some(body_points(&past.voices[matched], geometry, self.gain))
                })
                .collect();
            plot = plot.trails(trails);
            if !voice.held {
                plot = plot.inactive();
            }
        } else if touch.is_none() {
            plot = plot.inactive();
        }
        plot.into_any_element()
    }
}

fn is_drum_model(id: &str) -> bool {
    id == "auris.synth.drumkit"
}

/// What a model's body is when nothing is sounding, before any frame has said so.
fn resting_geometry(id: &str, observed: MotionGeometry) -> MotionGeometry {
    match id {
        "auris.physical.bell" => MotionGeometry::Shell,
        "auris.physical.mallet" => MotionGeometry::Bar,
        "auris.physical.clarinet" | "auris.physical.tin_whistle" => MotionGeometry::Bore,
        "auris.physical.choir" => MotionGeometry::Tract,
        "auris.synth.drumkit" => MotionGeometry::Membrane,
        id if id.starts_with("auris.physical.") => MotionGeometry::String,
        _ => observed,
    }
}

/// One track's model on its overview tile: the loudest body, without labels or a surface.
pub(crate) fn model_thumbnail(
    id: String,
    frame: Option<MotionFrame>,
    _language: Language,
    theme: Theme,
) -> AnyElement {
    let frame = frame.unwrap_or_default();
    let loudest = (0..LANES)
        .filter(|index| active(&frame.voices[*index]))
        .max_by(|a, b| frame.voices[*a].level.total_cmp(&frame.voices[*b].level));
    let geometry = loudest.map_or_else(
        || resting_geometry(&id, frame.geometry),
        |index| frame.voices[index].geometry.unwrap_or(frame.geometry),
    );
    Bodies {
        frame,
        history: Vec::new(),
        gain: 24.,
        touch: None,
        resting: geometry,
        resting_contact: None,
        resting_pitch: 60,
        drums: is_drum_model(&id),
        language: Language::English,
    }
    .plot(0, loudest, geometry, &theme)
}

fn next_drum_preview(pitch: u8, forward: bool) -> u8 {
    const KEYS: [u8; 12] = [36, 38, 42, 46, 49, 51, 41, 43, 45, 47, 48, 50];
    let current = KEYS.iter().position(|key| *key == pitch).unwrap_or(0);
    KEYS[(current + if forward { 1 } else { KEYS.len() - 1 }) % KEYS.len()]
}

fn drum_geometry(pitch: u8) -> MotionGeometry {
    if matches!(pitch, 42 | 44 | 46 | 49 | 51 | 52 | 53 | 55 | 57 | 59) {
        MotionGeometry::Plate
    } else {
        MotionGeometry::Membrane
    }
}

fn drum_name(pitch: u8, language: Language) -> &'static str {
    let key = match pitch {
        35 | 36 => Key::RoleKick,
        37..=40 => Key::RoleSnare,
        42 | 44 => Key::DrumClosedHat,
        46 => Key::DrumOpenHat,
        49 | 52 | 55 | 57 => Key::RoleCrash,
        51 | 53 | 59 => Key::DrumRide,
        _ => Key::DrumTom,
    };
    key.get(language)
}

fn displayed_motion(value: f32, gain: f32) -> f32 {
    if value.is_finite() {
        (value * gain).clamp(-1., 1.)
    } else {
        0.0
    }
}

/// A body's motion as a lane draws it, from -1 to 1. A string's ends are fixed and drawn so.
fn body_points(voice: &MotionVoice, geometry: MotionGeometry, gain: f32) -> Vec<f32> {
    let last = voice.points.len() - 1;
    voice
        .points
        .iter()
        .enumerate()
        .map(|(index, value)| {
            if geometry == MotionGeometry::String && (index == 0 || index == last) {
                0.0
            } else {
                displayed_motion(*value, gain)
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "physical_model_tests.rs"]
mod tests;
