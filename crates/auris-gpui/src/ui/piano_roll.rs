//! The piano roll: note editing for the selected MIDI clip.

use auris_i18n::{Key, messages};
use auris_session::prelude::*;

use gpui::{
    App, Bounds, IntoElement, MouseButton, MouseDownEvent, Pixels, Point, Window, canvas, div,
    point, prelude::*, px, size,
};

use crate::app::{AurisApp, Drag};
use crate::theme::{Metrics, Theme};
use crate::ui::paint;
use crate::ui::timeline::{PitchView, TimelineView};
use crate::ui::widgets::{ButtonStyle, button};

/// What the pointer does in the note grid.
///
/// Logic Pro's tool menu, reduced to the two tools this editor has. A tool rather than a modifier
/// because there is no modifier left to give it: ⌘ creates a note and suspends the grid, ⌥
/// deletes, ⇧ extends the selection, and Logic's own ⌃⌥-drag cannot arrive at all — gpui rewrites
/// a ⌃-left-click into a right-click on macOS, and strips the ⌃ off it on the way, so the gesture
/// reaches the window as a request for the context menu.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum RollTool {
    /// Select, move, resize and create — everything the roll did before there were tools.
    #[default]
    Pointer,
    /// Drag a note up or down to say how hard it is struck.
    Velocity,
}

impl RollTool {
    /// Every tool, in the order the strip shows them.
    pub const ALL: [RollTool; 2] = [RollTool::Pointer, RollTool::Velocity];

    /// What the tool is called.
    pub fn label(self) -> Key {
        match self {
            RollTool::Pointer => Key::ToolPointer,
            RollTool::Velocity => Key::ToolVelocity,
        }
    }
}

/// Width of the grab zone on a note's right edge, in pixels.
const RESIZE_HANDLE: f32 = 5.0;

/// How far the pointer travels for one step of MIDI velocity.
///
/// The whole range is then about 190 pixels — a comfortable drag, short enough to reach either
/// end without letting go, and long enough that a single step is still deliberate.
const PIXELS_PER_VELOCITY_STEP: f32 = 1.5;

/// The softest a note may be struck.
///
/// MIDI spends velocity 0 on "this note has stopped", so nothing is written at it. A note dragged
/// down to nothing would otherwise still be drawn, still be selected and still be movable, and
/// never once be heard.
const MIN_VELOCITY: u8 = 1;

/// A stored velocity as the number a musician and the MIDI cable both use.
fn midi_velocity(velocity: f32) -> u8 {
    (velocity.clamp(0.0, 1.0) * 127.0).round() as u8
}

/// Where a note struck at `origin` ends up after the pointer has travelled `dy` from where the
/// drag began.
///
/// Up is louder. Screen y grows downward, which is the negation — but every fader in the
/// application, every fader in every mixing desk, and Logic's own velocity tool all move that
/// way, and a velocity drag that went the other way would be wrong however consistent the
/// arithmetic was.
///
/// Measured from `origin` rather than from wherever the note is now, so the whole gesture is one
/// idempotent rewrite: a drag past either end and back leaves the selection exactly as it was
/// rather than collapsed against the limit it was pushed into.
fn dragged_velocity(origin: u8, dy: Pixels) -> u8 {
    let steps = -f32::from(dy) / PIXELS_PER_VELOCITY_STEP;
    (i32::from(origin) + steps.round() as i32).clamp(i32::from(MIN_VELOCITY), 127) as u8
}

/// How wide the grab zone on a note's right edge is, for a note drawn `width` across.
///
/// Never more than a third of the note. At a 1/32 grid a note is about eight pixels wide, and a
/// fixed five-pixel handle either side of its end swallowed the whole thing: the note could be
/// stretched and never moved, which reads as the roll refusing to let go of it.
fn resize_grab(width: Pixels) -> f32 {
    RESIZE_HANDLE.min(f32::from(width) / 3.0)
}

/// Length given to a note drawn with a single click.
fn default_note_length(grid: Ticks) -> Ticks {
    Ticks(grid.raw().max(1))
}

impl AurisApp {
    /// Renders the piano roll panel.
    pub(crate) fn render_piano_roll(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let pitch_view = self.pitch.clone();
        let view = self.timeline.clone();
        let signature = self.project().time_signature;
        let playhead = self.playhead_ticks();

        let Some(clip) = self.selected_midi_clip() else {
            return div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .bg(theme.surface_sunken)
                .text_color(theme.text_muted)
                .text_xs()
                .child(messages::piano_roll_empty(
                    self.language(),
                    self.t(self.pointer.create.label()),
                ))
                .into_any_element();
        };

        let clip_start = clip.start;
        let clip_length = clip.length;
        let notes = clip.notes.clone();
        let selected: Vec<usize> = self.selected_notes.iter().copied().collect();
        let clip_name = clip.name.clone();
        let band = self.rubber_band(crate::app::BandSurface::Roll);
        let velocity_tag = self.velocity_tag();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(80.0))
            .bg(theme.surface_sunken)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(Metrics::EDITOR_HEADER_HEIGHT)
                    .px_2()
                    .bg(theme.surface_raised)
                    .border_b_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(messages::piano_roll_title(self.language(), &clip_name))
                    .child(self.tool_strip(cx))
                    .child(div().flex_1())
                    // The hint describes the tool in hand. It named the create and delete
                    // gestures unconditionally, and holding the velocity tool while being told
                    // how to add a note is being told about a different editor.
                    .child(match self.tool {
                        RollTool::Pointer => messages::piano_roll_hint(
                            self.language(),
                            self.t(self.pointer.create.label()),
                            self.t(self.pointer.delete.label()),
                        ),
                        RollTool::Velocity => messages::piano_roll_velocity_hint(self.language()),
                    })
                    .child(self.zoom_slider("roll-zoom", cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("keyboard")
                            .w(Metrics::KEYBOARD_WIDTH)
                            .flex_shrink_0()
                            .h_full()
                            .child({
                                let theme = theme.clone();
                                let pitch_view = pitch_view.clone();
                                canvas(
                                    |_, _, _| (),
                                    move |bounds, _, window, cx| {
                                        paint::clipped(window, bounds, |window| {
                                            paint::keyboard(
                                                window,
                                                cx,
                                                bounds,
                                                &pitch_view,
                                                &theme,
                                            );
                                        });
                                    },
                                )
                                .size_full()
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                    this.audition_from_keyboard(event, cx);
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, _| this.stop_audition()),
                            )
                            // The keyboard scrolls with the rows beside it. It is the strip a
                            // user reaches for when they want to see a different octave, and the
                            // wheel did nothing there while working on the grid an inch away.
                            .on_scroll_wheel(cx.listener(
                                |this, event: &gpui::ScrollWheelEvent, _, cx| {
                                    this.scroll_roll(event, cx);
                                },
                            )),
                    )
                    .child(
                        div()
                            .id("roll")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_hidden()
                            // The grid says which tool is in hand under the pointer as well as in
                            // the header. A mode is only dangerous while it is invisible, and the
                            // header is the one place the eye is not while editing notes.
                            .when(self.tool == RollTool::Velocity, |this| {
                                this.cursor(gpui::CursorStyle::ResizeUpDown)
                            })
                            .child({
                                let theme = theme.clone();
                                let view = view.clone();
                                let pitch_view = pitch_view.clone();
                                let recorded = self.canvas.roll.clone();
                                canvas(
                                    move |bounds, _, _| recorded.set(Some(bounds)),
                                    move |bounds, _, window, cx| {
                                        paint::clipped(window, bounds, |window| {
                                            paint::rect(window, bounds, theme.surface_sunken);
                                            paint::pitch_rows(window, bounds, &pitch_view, &theme);
                                            paint::time_grid(
                                                window, bounds, &view, signature, &theme,
                                            );
                                            paint_clip_extent(
                                                window,
                                                bounds,
                                                &view,
                                                clip_start,
                                                clip_length,
                                                &theme,
                                            );
                                            paint_notes(
                                                window,
                                                cx,
                                                bounds,
                                                &notes,
                                                &selected,
                                                velocity_tag,
                                                clip_start,
                                                &view,
                                                &pitch_view,
                                                &theme,
                                            );
                                            paint::playhead(
                                                window,
                                                bounds,
                                                bounds.origin.x + view.tick_to_x(playhead),
                                                &theme,
                                            );
                                            if let Some(band) = band {
                                                paint::selection_band(window, band, &theme);
                                            }
                                        });
                                    },
                                )
                                .size_full()
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                    this.begin_note_drag(event, cx);
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                    this.open_roll_menu(event, cx);
                                }),
                            )
                            .on_scroll_wheel(cx.listener(
                                |this, event: &gpui::ScrollWheelEvent, _, cx| {
                                    this.scroll_roll(event, cx);
                                },
                            )),
                    ),
            )
            .into_any_element()
    }

    /// The strip in the header saying which tool the roll has in hand.
    ///
    /// Latched buttons rather than a menu, because a mode has to be visible from across the room:
    /// the whole hazard of a tool is reaching for it, being interrupted, and coming back to a
    /// pointer that no longer does what the hand expects.
    fn tool_strip(&self, cx: &mut gpui::Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let current = self.tool;
        div()
            .flex()
            .items_center()
            .gap_1()
            .children(RollTool::ALL.map(|tool| {
                button(
                    ("roll-tool", tool as usize),
                    self.t(tool.label()),
                    ButtonStyle::Ghost,
                    tool == current,
                    theme.accent_soft,
                    &theme,
                    cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                        this.tool = tool;
                        cx.notify();
                    }),
                )
            }))
            .into_any_element()
    }

    /// Window origin of the note grid, taken from where it was last painted.
    ///
    /// It used to be derived from the window height and `Metrics::EDITOR_HEIGHT`, which was
    /// correct until the editor panel became resizable — after that, every note the user
    /// clicked was off by however far they had dragged the divider. The fallback below is only
    /// reached before the first paint, and uses the panel's *current* height for the same
    /// reason.
    pub(crate) fn roll_origin(&self) -> Point<Pixels> {
        self.canvas.roll.get().map_or_else(
            || {
                point(
                    Metrics::KEYBOARD_WIDTH,
                    self.viewport_height - Metrics::STATUS_HEIGHT - self.panels.editor_height
                        + Metrics::EDITOR_HEADER_HEIGHT,
                )
            },
            |bounds| bounds.origin,
        )
    }

    /// Starts a note drag, creating a note when alt is held on empty space.
    fn begin_note_drag(&mut self, event: &MouseDownEvent, cx: &mut gpui::Context<Self>) {
        let Some(clip_id) = self.selected_clip else {
            return;
        };
        let origin = self.roll_origin();
        let tick = self.timeline.x_to_tick(event.position.x - origin.x);
        // Below MIDI 0 the grid is unpainted, so a click there must not act on pitch 0.
        let Some(pitch) = self.pitch.pitch_at(event.position.y - origin.y) else {
            return;
        };
        let Some(clip_start) = self.session.midi_clip(clip_id).map(|c| c.start) else {
            return;
        };
        let local_tick = tick - clip_start;

        let under_pointer = self.note_at(clip_id, local_tick, pitch);

        // The velocity tool claims a press on a note outright, ahead of the create and delete
        // gestures: a tool that sometimes removed the note instead is not a tool anyone can trust
        // to be in hand. Empty grid still sweeps a selection, as it does under every tool, so the
        // notes to work on can be gathered without putting the tool down.
        if self.tool == RollTool::Velocity {
            match under_pointer {
                Some(index) => {
                    self.begin_velocity_drag(clip_id, index, event.position.y, event.modifiers)
                }
                None => self.begin_rubber_band(
                    crate::app::BandSurface::Roll,
                    event.position,
                    event.modifiers.shift,
                ),
            }
            cx.notify();
            return;
        }

        // Delete first: it is the only gesture that acts on what is already there, so letting
        // anything else claim the press would make it unreachable.
        if let Some(index) = under_pointer
            && self.pointer.delete.matches(event)
        {
            let _ = self.session.remove_notes(clip_id, &[index]);
            self.selected_notes.clear();
            cx.notify();
            return;
        }

        match under_pointer {
            Some(index) => {
                if !event.modifiers.shift {
                    if !self.selected_notes.contains(&index) {
                        self.selected_notes.clear();
                    }
                } else if self.selected_notes.contains(&index) {
                    self.selected_notes.remove(&index);
                    cx.notify();
                    return;
                }
                self.selected_notes.insert(index);

                let note = self
                    .project()
                    .midi_clip(clip_id)
                    .and_then(|(_, c)| c.notes.get(index).copied());
                let Some(note) = note else { return };
                let start_x = self.timeline.tick_to_x(clip_start + note.start);
                let end_x = self.timeline.tick_to_x(clip_start + note.end());
                let grab = resize_grab(end_x - start_x);
                if f32::from(end_x - (event.position.x - origin.x)).abs() <= grab {
                    self.begin_drag(Drag::NoteResize {
                        clip: clip_id,
                        index,
                    });
                } else {
                    let origins = self.selected_note_origins(clip_id);
                    self.begin_drag(Drag::NoteMove {
                        clip: clip_id,
                        origin_tick: local_tick,
                        origin_pitch: pitch,
                        origins,
                    });
                    self.audition(pitch);
                }
            }
            None => {
                if self.pointer.create.matches(event) {
                    let start = self.snap(local_tick).max_zero();
                    let length = default_note_length(self.project().grid);
                    // The new note and the resize that follows it are one gesture, so the
                    // transaction opens first and the note lands inside it.
                    self.begin_drag(Drag::NoteResize {
                        clip: clip_id,
                        index: 0,
                    });
                    let Ok(index) = self
                        .session
                        .add_note(clip_id, Note::new(pitch, start, length))
                    else {
                        self.drag = None;
                        return;
                    };
                    self.drag = Some(Drag::NoteResize {
                        clip: clip_id,
                        index,
                    });
                    self.selected_notes.clear();
                    self.selected_notes.insert(index);
                    self.audition(pitch);
                } else {
                    // A drag on empty grid sweeps a selection; a press that never moves ends up
                    // selecting nothing, which is the deselect it looks like.
                    self.begin_rubber_band(
                        crate::app::BandSurface::Roll,
                        event.position,
                        event.modifiers.shift,
                    );
                }
            }
        }
        cx.notify();
    }

    /// Takes hold of a note's dynamics, and of every note selected along with it.
    ///
    /// Pressing a note that is not in the selection makes it the selection, which is what the
    /// pointer tool does and what Logic does: the gesture then acts on what was aimed at rather
    /// than on a chord left selected somewhere off-screen.
    fn begin_velocity_drag(
        &mut self,
        clip: ClipId,
        index: usize,
        y: Pixels,
        modifiers: gpui::Modifiers,
    ) {
        if modifiers.shift {
            self.selected_notes.insert(index);
        } else if !self.selected_notes.contains(&index) {
            self.selected_notes.clear();
            self.selected_notes.insert(index);
        }

        let Some(target) = self.session.midi_clip(clip) else {
            return;
        };
        let origins: Vec<(usize, u8)> = self
            .selected_notes
            .iter()
            .filter_map(|index| {
                target
                    .notes
                    .get(*index)
                    .map(|note| (*index, midi_velocity(note.velocity)))
            })
            .collect();
        let struck = target
            .notes
            .get(index)
            .map(|note| (note.pitch, note.velocity));

        self.begin_drag(Drag::NoteVelocity {
            clip,
            start_y: y,
            origins,
            grabbed: index,
        });
        // Heard at the level it is written at, so the drag starts from something the ear has a
        // reference for rather than from a number.
        if let Some((pitch, velocity)) = struck {
            self.audition_at(pitch, velocity);
        }
    }

    /// Moves a velocity drag to wherever the pointer has reached.
    pub(crate) fn drag_velocity(
        &mut self,
        clip: ClipId,
        start_y: Pixels,
        origins: &[(usize, u8)],
        y: Pixels,
    ) {
        let dy = y - start_y;
        let changes: Vec<(usize, f32)> = origins
            .iter()
            .map(|(index, origin)| (*index, f32::from(dragged_velocity(*origin, dy)) / 127.0))
            .collect();
        let _ = self.session.set_note_velocities(clip, &changes);
    }

    /// The note a velocity drag has hold of and what it now says, for the tag drawn beside it.
    ///
    /// Read back out of the document rather than recomputed from the drag, so the tag reports
    /// what was actually written — including the clamp at either end, which is the moment a
    /// number is worth having.
    fn velocity_tag(&self) -> Option<(usize, u8)> {
        let Some(Drag::NoteVelocity { clip, grabbed, .. }) = &self.drag else {
            return None;
        };
        let note = self.session.midi_clip(*clip)?.notes.get(*grabbed)?;
        Some((*grabbed, midi_velocity(note.velocity)))
    }

    /// Positions of every selected note, captured at the start of a move.
    fn selected_note_origins(&self, clip: ClipId) -> Vec<(usize, Ticks, u8)> {
        let Some(clip) = self.session.midi_clip(clip) else {
            return Vec::new();
        };
        self.selected_notes
            .iter()
            .filter_map(|index| {
                clip.notes
                    .get(*index)
                    .map(|note| (*index, note.start, note.pitch))
            })
            .collect()
    }

    /// Index of the note at a clip-relative position.
    fn note_at(&self, clip: ClipId, tick: Ticks, pitch: u8) -> Option<usize> {
        let clip = self.session.midi_clip(clip)?;
        // Search backwards so the most recently added note wins when notes overlap, which is
        // what the user just drew and therefore what they expect to grab.
        clip.notes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, note)| note.pitch == pitch && note.contains(tick))
            .map(|(index, _)| index)
    }

    /// Opens the menu for whatever is under the pointer in the note grid.
    fn open_roll_menu(&mut self, event: &MouseDownEvent, cx: &mut gpui::Context<Self>) {
        let origin = self.roll_origin();
        let tick = self.timeline.x_to_tick(event.position.x - origin.x);
        let Some(pitch) = self.pitch.pitch_at(event.position.y - origin.y) else {
            return;
        };
        let clip_start = self
            .selected_clip
            .and_then(|clip| self.session.midi_clip(clip))
            .map(|clip| clip.start)
            .unwrap_or(Ticks::ZERO);
        let local_tick = tick - clip_start;

        let under_pointer = self
            .selected_clip
            .and_then(|clip| self.note_at(clip, local_tick, pitch));
        // Right-clicking a note that is not part of the selection makes it the selection, so
        // Delete and Transpose act on what was pointed at rather than on something off-screen.
        if let Some(index) = under_pointer
            && !self.selected_notes.contains(&index)
        {
            self.selected_notes.clear();
            self.selected_notes.insert(index);
        }

        let menu = self.roll_menu(
            event.position,
            under_pointer,
            pitch,
            self.snap(local_tick).max_zero(),
        );
        self.open_menu(menu);
        cx.notify();
    }

    /// Wheel handling: plain scrolls pitch, shift scrolls time, alt zooms time,
    /// control zooms the pitch axis.
    fn scroll_roll(&mut self, event: &gpui::ScrollWheelEvent, cx: &mut gpui::Context<Self>) {
        let delta = event.delta.pixel_delta(px(24.0));
        if event.modifiers.control {
            let anchor = event.position.y - self.roll_origin().y;
            let factor = if delta.y > px(0.0) { 1.12 } else { 1.0 / 1.12 };
            self.pitch.zoom_by(factor, anchor);
        } else if event.modifiers.alt {
            // The same origin the vertical branch above uses, and the same one the painter does.
            // `KEYBOARD_WIDTH` is only half of it — the roll starts after the panel's own padding
            // as well — so the anchor was a constant off, and the notes slid sideways on every
            // zoom notch instead of staying put under the pointer.
            let anchor = event.position.x - self.roll_origin().x;
            let factor = if delta.y > px(0.0) { 1.12 } else { 1.0 / 1.12 };
            self.timeline.zoom_by(factor, anchor);
        } else if event.modifiers.shift {
            self.timeline.scroll_by(-delta.y - delta.x);
        } else {
            self.pitch.scroll_by(-delta.y);
            self.timeline.scroll_by(-delta.x);
        }
        cx.notify();
    }

    /// Plays the key the pointer landed on, so the keyboard strip is playable.
    fn audition_from_keyboard(&mut self, event: &MouseDownEvent, cx: &mut gpui::Context<Self>) {
        let origin = self.roll_origin();
        let Some(pitch) = self.pitch.pitch_at(event.position.y - origin.y) else {
            return;
        };
        self.audition(pitch);
        cx.notify();
    }
}

fn paint_clip_extent(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    view: &TimelineView,
    clip_start: Ticks,
    clip_length: Ticks,
    theme: &Theme,
) {
    // Dim everything outside the clip: notes there exist but are never played.
    let start_x = bounds.origin.x + view.tick_to_x(clip_start);
    let end_x = bounds.origin.x + view.tick_to_x(clip_start + clip_length);
    let shade = Theme::translucent(theme.background, 0.45);
    if start_x > bounds.origin.x {
        paint::rect(
            window,
            Bounds {
                origin: bounds.origin,
                size: size(start_x - bounds.origin.x, bounds.size.height),
            },
            shade,
        );
    }
    let right_edge = bounds.origin.x + bounds.size.width;
    if end_x < right_edge {
        paint::rect(
            window,
            Bounds {
                origin: point(end_x.max(bounds.origin.x), bounds.origin.y),
                size: size(right_edge - end_x.max(bounds.origin.x), bounds.size.height),
            },
            shade,
        );
    }
    paint::vline(window, bounds, start_x, px(1.0), theme.accent);
    paint::vline(window, bounds, end_x, px(1.0), theme.accent);
}

/// Distance from a note's ends to the velocity bar inside it.
const VELOCITY_BAR_INSET: f32 = 2.0;

/// Narrowest a note may be and still carry a velocity bar.
///
/// Below this the bar is a smudge that says nothing about the value and everything about the
/// zoom, and the note's own colour is left to do the talking.
const VELOCITY_BAR_MIN_WIDTH: f32 = 12.0;

/// Shortest a row may be and still carry a velocity bar.
const VELOCITY_BAR_MIN_ROW: f32 = 7.0;

/// Width of the tag that reports the value during a drag.
///
/// Fixed at three digits rather than measured, so the tag does not change width — and so the
/// number does not shuffle sideways — as the value crosses 9 and 99.
const VELOCITY_TAG_WIDTH: f32 = 30.0;

/// Whether a note drawn this size has room for a legible velocity bar.
///
/// Zoomed out far enough, every note is a few pixels of colour and a bar inside one is a smudge
/// that says more about the zoom than about the value. The colour is left to do the talking
/// there, which it can, because it does not depend on how much room there is.
fn bar_fits(note_bounds: Bounds<Pixels>) -> bool {
    f32::from(note_bounds.size.width) - VELOCITY_BAR_INSET * 2.0 >= VELOCITY_BAR_MIN_WIDTH
        && f32::from(note_bounds.size.height) >= VELOCITY_BAR_MIN_ROW
}

/// Draws the bar inside a note that says how hard it is struck.
///
/// Logic's marking, and the reason it is worth having on top of the colour: a hue ramp says
/// roughly where in the range a note sits, and cannot show the difference between 96 and 100,
/// which is exactly the difference a velocity drag is being made to find.
fn paint_velocity_bar(
    window: &mut Window,
    note_bounds: Bounds<Pixels>,
    velocity: f32,
    theme: &Theme,
) {
    if !bar_fits(note_bounds) {
        return;
    }
    let span = f32::from(note_bounds.size.width) - VELOCITY_BAR_INSET * 2.0;
    let thickness = (note_bounds.size.height / 5.0).clamp(px(1.0), px(2.0));
    let filled = (span * velocity.clamp(0.0, 1.0)).max(1.0);
    paint::rect(
        window,
        Bounds {
            origin: point(
                note_bounds.origin.x + px(VELOCITY_BAR_INSET),
                note_bounds.origin.y + (note_bounds.size.height - thickness) / 2.0,
            ),
            size: size(px(filled), thickness),
        },
        // Against the note's own fill, which runs from blue to red: a fixed colour would vanish
        // into one end of the ramp or the other.
        Theme::translucent(theme.text_on(theme.velocity_color(velocity)), 0.8),
    );
}

/// Draws the number a velocity drag has reached, beside the note it has hold of.
///
/// Logic's help tag. The value is the one thing a continuous drag cannot say by itself, and it is
/// wanted most at the ends, where the note stops changing and only the number can explain why.
fn paint_velocity_tag(
    window: &mut Window,
    cx: &mut App,
    bounds: Bounds<Pixels>,
    note_bounds: Bounds<Pixels>,
    midi: u8,
    theme: &Theme,
) {
    let height = px(16.0);
    let width = px(VELOCITY_TAG_WIDTH);
    // Beside the note by preference, and on its other side when that would leave the canvas:
    // the tag is worth nothing clipped in half against the right-hand edge.
    let right = note_bounds.origin.x + note_bounds.size.width + px(6.0);
    let x = if right + width <= bounds.origin.x + bounds.size.width {
        right
    } else {
        (note_bounds.origin.x - width - px(6.0)).max(bounds.origin.x)
    };
    let y = (note_bounds.origin.y + (note_bounds.size.height - height) / 2.0).clamp(
        bounds.origin.y,
        bounds.origin.y + bounds.size.height - height,
    );
    let plate = Bounds {
        origin: point(x, y),
        size: size(width, height),
    };
    paint::rounded_rect(window, plate, Metrics::RADIUS_XS, theme.surface_raised);
    paint::rounded_outline(window, plate, Metrics::RADIUS_XS, px(1.0), theme.border);
    paint::label(
        window,
        cx,
        point(x + px(6.0), y + px(2.0)),
        midi.to_string(),
        px(10.0),
        theme.text,
    );
}

#[allow(clippy::too_many_arguments)]
fn paint_notes(
    window: &mut Window,
    cx: &mut App,
    bounds: Bounds<Pixels>,
    notes: &[Note],
    selected: &[usize],
    tag: Option<(usize, u8)>,
    clip_start: Ticks,
    view: &TimelineView,
    pitch_view: &PitchView,
    theme: &Theme,
) {
    // Where the tag goes, decided in the loop and drawn after it, so a note painted later cannot
    // land on top of the one thing on the canvas that is only there to be read.
    let mut tag_at = None;
    for (index, note) in notes.iter().enumerate() {
        let x = bounds.origin.x + view.tick_to_x(clip_start + note.start);
        let width = view.duration_to_width(note.length).max(px(2.0));
        if x + width < bounds.origin.x || x > bounds.origin.x + bounds.size.width {
            continue;
        }
        let y = bounds.origin.y + pitch_view.pitch_to_y(note.pitch);
        if y + px(pitch_view.row_height) < bounds.origin.y
            || y > bounds.origin.y + bounds.size.height
        {
            continue;
        }
        // The fill says how hard the note was struck and nothing else, so the dynamics of a part
        // are readable at a glance rather than one note at a time.
        let note_bounds = Bounds {
            origin: point(x, y + px(1.0)),
            size: size(width, px((pitch_view.row_height - 2.0).max(2.0))),
        };
        paint::rounded_rect(
            window,
            note_bounds,
            Metrics::RADIUS_XS,
            theme.velocity_color(note.velocity),
        );
        paint_velocity_bar(window, note_bounds, note.velocity, theme);
        if tag.is_some_and(|(grabbed, _)| grabbed == index) {
            tag_at = Some(note_bounds);
        }
        // Which leaves selection to the outline alone. It used to share the fill, and the two
        // cannot both have it: a selected note and a loud one would be the same rectangle.
        if selected.contains(&index) {
            paint::rounded_outline(
                window,
                note_bounds,
                Metrics::RADIUS_XS,
                px(1.5),
                // Against the note's own colour, not against the accent. The selection colour is
                // a shade of the accent, and the velocity ramp runs through blue, green, yellow
                // and red: at mid velocity the outline landed on green about one and a tenth to
                // one from it, and a selected note in the middle of a phrase simply did not look
                // selected. Deciding per note also stops the whole thing resting on hue, which
                // is the one channel a red-green-deficient reader does not have.
                theme.text_on(theme.velocity_color(note.velocity)),
            );
        }
    }

    if let (Some(note_bounds), Some((_, midi))) = (tag_at, tag) {
        paint_velocity_tag(window, cx, bounds, note_bounds, midi, theme);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_can_always_be_moved_however_short_it_is() {
        // At a 1/32 grid a note is about eight pixels across. A fixed five-pixel handle either
        // side of its end covered the whole note, so it could be stretched and never dragged.
        assert_eq!(resize_grab(px(120.0)), RESIZE_HANDLE);
        let short = resize_grab(px(8.0));
        assert!(short < RESIZE_HANDLE);
        assert!(
            short * 2.0 < 8.0,
            "there is a middle left over to take hold of",
        );
    }

    #[test]
    fn the_roll_opens_holding_the_pointer() {
        // A tool is a mode, and a mode that outlives the moment is one the user comes back to
        // having forgotten. Nothing persists it, and the strip lists every tool there is, so
        // none of them can be reached and then not left.
        assert_eq!(RollTool::default(), RollTool::Pointer);
        assert!(RollTool::ALL.contains(&RollTool::default()));
        assert_ne!(
            RollTool::Pointer.label(),
            RollTool::Velocity.label(),
            "two buttons under one name is a strip nobody can read",
        );
    }

    #[test]
    fn a_velocity_drag_goes_up_for_louder_and_never_reaches_silence() {
        let mid = 64;
        assert!(dragged_velocity(mid, px(-30.0)) > mid, "up is louder");
        assert!(dragged_velocity(mid, px(30.0)) < mid, "down is softer");
        assert_eq!(dragged_velocity(mid, px(0.0)), mid);

        // Both ends clamp, and the soft end stops at 1 rather than 0: MIDI spends 0 on "this
        // note has stopped", so a note dragged to nothing would still be drawn, still be
        // selected, still be movable, and never once be heard.
        assert_eq!(dragged_velocity(mid, px(-9999.0)), 127);
        assert_eq!(dragged_velocity(mid, px(9999.0)), MIN_VELOCITY);
    }

    #[test]
    fn a_step_is_deliberate_and_the_whole_range_is_one_movement() {
        assert_eq!(dragged_velocity(64, px(-PIXELS_PER_VELOCITY_STEP)), 65);
        assert_eq!(
            dragged_velocity(64, px(-PIXELS_PER_VELOCITY_STEP / 3.0)),
            64,
            "less than half a step is not a step — a press that wobbles must not rewrite the note",
        );

        let whole_range = 127.0 * PIXELS_PER_VELOCITY_STEP;
        assert!(
            whole_range < 240.0,
            "the range takes {whole_range} pixels, which is further than the roll is tall",
        );
        assert_eq!(dragged_velocity(MIN_VELOCITY, px(-whole_range)), 127);
    }

    #[test]
    fn a_drag_past_the_end_and_back_leaves_the_selection_as_it_was() {
        // Measured from where the notes started rather than from where they now are, which is
        // the whole reason the origins are captured when the button goes down: a chord pushed
        // into the ceiling and brought back down gets its shape returned rather than flattened.
        let chord = [40u8, 80, 120];
        let against = |dy| -> Vec<u8> {
            chord
                .iter()
                .map(|origin| dragged_velocity(*origin, dy))
                .collect()
        };
        assert_eq!(against(px(-400.0)), vec![127, 127, 127]);
        assert_eq!(against(px(0.0)), chord.to_vec());
        assert_eq!(
            against(px(-15.0)),
            vec![50, 90, 127],
            "and each note moves by the same amount until it runs out of room",
        );
    }

    #[test]
    fn what_is_stored_and_what_is_shown_are_the_same_number() {
        // The document holds a fraction and the tag reports MIDI. A round trip that lost a step
        // would make the drag feel as though it were sticking.
        for midi in 0..=127u8 {
            assert_eq!(midi_velocity(f32::from(midi) / 127.0), midi);
        }
        assert_eq!(midi_velocity(-1.0), 0);
        assert_eq!(midi_velocity(2.0), 127);
    }

    #[test]
    fn a_note_too_small_to_carry_a_bar_does_not_get_one() {
        let note = |width: f32, height: f32| Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(width), px(height)),
        };
        assert!(bar_fits(note(80.0, 14.0)));
        assert!(
            !bar_fits(note(8.0, 14.0)),
            "a 1/32 note at a normal zoom is narrower than the bar would need",
        );
        assert!(
            !bar_fits(note(80.0, 5.0)),
            "and the rows go down to five pixels, which is thinner than the bar",
        );
    }
}
