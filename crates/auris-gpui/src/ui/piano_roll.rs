//! The piano roll: note editing for the selected MIDI clip.

use auris_i18n::{Key, messages};
use auris_session::prelude::*;

use gpui::{
    App, Bounds, IntoElement, MouseButton, MouseDownEvent, Pixels, Point, Window, canvas, div,
    point, prelude::*, px, size,
};

use crate::app::{AurisApp, Drag};
use crate::dock::Dock;
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

    /// The next tool along, wrapping round at the end.
    ///
    /// One bindable command rather than one per tool. Logic's tool key swaps back to the tool
    /// before it when pressed twice, which with two tools is the same gesture as cycling — and
    /// this way the keymap grows by one entry rather than by one for every tool there will ever
    /// be.
    pub fn next(self) -> Self {
        let at = RollTool::ALL
            .iter()
            .position(|tool| *tool == self)
            .unwrap_or(0);
        RollTool::ALL[(at + 1) % RollTool::ALL.len()]
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

/// The horizontal span of a note's resize grab, as an origin and a width.
///
/// The inner half of the zone the press measures, which is the half a press can reach: the
/// resize check is only arrived at once [`AurisApp::note_at`] has found a note under the
/// pointer's tick, and the outer half is past the note's end. `None` for a note too narrow to
/// grab, which is one drawn in less than three pixels.
fn note_end_span(start_x: Pixels, end_x: Pixels) -> Option<(Pixels, Pixels)> {
    let grab = px(resize_grab(end_x - start_x));
    (grab > px(0.0)).then_some((end_x - grab, grab))
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
        let signatures = self.project().signatures.spans();
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
        let note_ends = self.note_end_zones(clip_start, &notes);
        let selected: Vec<usize> = self.selected_notes.iter().copied().collect();
        let clip_name = clip.name.clone();
        let band = self.rubber_band(crate::app::BandSurface::Roll);
        let velocity_tag = self.velocity_tag();
        // Built before the chain rather than inside it: each one needs `&mut self`, and the
        // builder below is already holding a borrow of it.
        let lanes: Vec<gpui::AnyElement> = ClipCurve::ALL
            .into_iter()
            .filter(|which| self.panels.curve_lane(*which))
            .collect::<Vec<_>>()
            .into_iter()
            .map(|which| self.render_curve_lane(which, cx))
            .collect();

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
                    .h(Metrics::PANEL_HEADER_HEIGHT)
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
                                    move |bounds, window, _| {
                                        recorded.set(Some(bounds));
                                        note_ends
                                            .iter()
                                            .map(|zone| {
                                                window.insert_hitbox(
                                                    Bounds {
                                                        origin: bounds.origin + zone.origin,
                                                        size: zone.size,
                                                    },
                                                    gpui::HitboxBehavior::Normal,
                                                )
                                            })
                                            .collect::<Vec<_>>()
                                    },
                                    move |bounds, ends: Vec<gpui::Hitbox>, window, cx| {
                                        // What the lanes do for a clip's edges: the grab is a few
                                        // pixels of a canvas with nothing behind it, so the arrow
                                        // is the only thing that can say it is there.
                                        for end in &ends {
                                            window.set_cursor_style(
                                                gpui::CursorStyle::ResizeLeftRight,
                                                end,
                                            );
                                        }
                                        paint::clipped(window, bounds, |window| {
                                            paint::rect(window, bounds, theme.surface_sunken);
                                            paint::pitch_rows(window, bounds, &pitch_view, &theme);
                                            paint::time_grid(
                                                window,
                                                bounds,
                                                &view,
                                                &signatures,
                                                &theme,
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
            .children(lanes)
            .into_any_element()
    }

    /// One of the strips under the notes: the pitch bend, or the modulation.
    ///
    /// Under rather than beside, and spanning the same timeline: both are things that happen *at a
    /// moment in the phrase*, so the only useful way to look at one is with the notes it is
    /// shaping directly above it. The gutter on the left is the keyboard's width, for the same
    /// reason the track headers reserve what the ruler spends — a strip that started at the panel
    /// edge would put every point a keyboard's width away from the note it belongs to.
    ///
    /// One function for both, because they differ in exactly two ways — what the vertical axis
    /// means and what the gutter says — and a second copy would be a second set of gestures to
    /// keep in step with the first.
    fn render_curve_lane(
        &mut self,
        which: ClipCurve,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let view = self.timeline.clone();
        let playhead = self.playhead_ticks();
        let Some(clip) = self.selected_midi_clip() else {
            return div().into_any_element();
        };
        let (start, length) = (clip.start, clip.length);
        let points = clip.curve(which).to_vec();
        let recorded = self.canvas.curve(which).clone();

        div()
            .flex()
            .h(px(CURVE_LANE_HEIGHT))
            .flex_shrink_0()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.surface_sunken)
            .child(
                div()
                    .w(Metrics::KEYBOARD_WIDTH)
                    .flex_shrink_0()
                    .h_full()
                    .px_1()
                    .pt_1()
                    .bg(theme.surface)
                    .border_r_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(curve_label(which))),
            )
            .child(
                div()
                    .id(match which {
                        ClipCurve::Bend => "bend-lane",
                        ClipCurve::Modulation => "modulation-lane",
                    })
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .cursor_pointer()
                    .child({
                        let theme = theme.clone();
                        canvas(
                            move |bounds, _, _| recorded.set(Some(bounds)),
                            move |bounds, _, window, cx| {
                                paint::clipped(window, bounds, |window| {
                                    paint_curve(
                                        window, cx, bounds, &view, which, start, length, &points,
                                        playhead, &theme,
                                    );
                                });
                            },
                        )
                        .size_full()
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.press_curve_lane(which, event, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        Self::opens_menu(cx, move |this, at| {
                            // Taking points off one at a time is the ⌥-click; this is the way
                            // back from a curve that got away from somebody. With nothing
                            // selected there is nothing to straighten, and an empty menu is a
                            // menu `open_menu` declines to show.
                            let menu = crate::ui::context_menu::ContextMenu::new(
                                at,
                                this.t(curve_label(which)),
                            );
                            match this.selected_clip {
                                Some(clip) => menu.item(
                                    this.t(Key::StraightenCurve),
                                    crate::ui::context_menu::MenuCommand::ClearCurve {
                                        clip,
                                        which,
                                    },
                                ),
                                None => menu,
                            }
                        }),
                    )
                    .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                        this.scroll_roll(event, cx);
                    })),
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
    /// It used to be derived from the window height and the bottom panel's fixed height, which was
    /// correct until that panel became resizable — after that, every note the user clicked was off
    /// by however far they had dragged the divider. The fallback below is only reached before the
    /// first paint, and reads the dock's *current* height for the same reason. It assumes the roll
    /// is in the bottom dock, which is where it starts and where it usually is; a roll parked down
    /// one side is one frame out of place and right from the next paint onwards.
    pub(crate) fn roll_origin(&self) -> Point<Pixels> {
        self.canvas.roll.get().map_or_else(
            || {
                point(
                    Metrics::KEYBOARD_WIDTH,
                    self.viewport_height - Metrics::STATUS_HEIGHT - self.panels.size(Dock::Bottom)
                        + Metrics::PANEL_HEADER_HEIGHT,
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
                        pressed_at: Some(event.position),
                    });
                } else {
                    let origins = self.selected_note_origins(clip_id);
                    self.begin_drag(Drag::NoteMove {
                        clip: clip_id,
                        origin_tick: local_tick,
                        origin_pitch: pitch,
                        origins,
                        pressed_at: Some(event.position),
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
                        pressed_at: None,
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
                        pressed_at: None,
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

    /// Where the pointer would take hold of a note's end, relative to the note grid.
    ///
    /// The arrangement's [`clip_edge_zones`](super::arrangement) for notes, and the same rule
    /// keeps it honest: the cursor lights up exactly what the press acts on. Only the *inner*
    /// half of the grab, because [`Self::note_at`] has to find a note under the pointer's tick
    /// before the resize check is reached — the half hanging past the end is a zone no press can
    /// land in. Only the end, too, because a note has no front trim.
    ///
    /// Empty while the velocity tool is in hand. That tool drags a note's velocity rather than
    /// its length, and the grid already says so with a cursor of its own.
    fn note_end_zones(&self, clip_start: Ticks, notes: &[Note]) -> Vec<Bounds<Pixels>> {
        if self.tool != RollTool::Pointer {
            return Vec::new();
        }
        let row = px(self.pitch.row_height);
        notes
            .iter()
            .filter_map(|note| {
                let start_x = self.timeline.tick_to_x(clip_start + note.start);
                let end_x = self.timeline.tick_to_x(clip_start + note.end());
                let (x, width) = note_end_span(start_x, end_x)?;
                // The rows the note is drawn in, not the row it is binned into: a pixel inside at
                // the top and bottom, which is where the note the eye sees actually is.
                Some(Bounds {
                    origin: point(x, self.pitch.pitch_to_y(note.pitch) + px(1.0)),
                    size: size(width, (row - px(2.0)).max(px(2.0))),
                })
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

// ------------------------------------------------------------- the curve lanes

/// How tall a curve strip is drawn.
const CURVE_LANE_HEIGHT: f32 = 76.0;

/// How near a point a press has to land to take hold of it, in pixels.
const CURVE_GRAB: f32 = 7.0;

/// How large a point is drawn.
const CURVE_POINT_RADIUS: f32 = 3.0;

/// What a strip is called in the gutter.
fn curve_label(which: ClipCurve) -> Key {
    match which {
        ClipCurve::Bend => Key::BendLane,
        ClipCurve::Modulation => Key::ModulationLane,
    }
}

/// Where `value` sits in the strip, from 0 at the top to 1 at the bottom.
///
/// A bend gets the whole of its limit either way with nothing at the middle — not the two
/// semitones MIDI assumes, because the document works in semitones and can hold an octave, and a
/// strip that only reached a tone would make a dive of a fifth undrawable and, worse, unreadable
/// once written. The wheel gets the bottom of the strip for nothing and the top for all the way
/// up, because that is the whole of its travel: half a strip drawn under a control that cannot go
/// there would be half a strip of nothing.
pub fn curve_row(which: ClipCurve, value: f32) -> f32 {
    let (low, high) = which.range();
    let value = value.clamp(low, high);
    match which.is_bipolar() {
        true => 0.5 - (value / high) * 0.5,
        false => 1.0 - value / high,
    }
}

/// The value a row in the strip stands for. The inverse of [`curve_row`].
pub fn curve_of_row(which: ClipCurve, row: f32) -> f32 {
    let (_, high) = which.range();
    let row = row.clamp(0.0, 1.0);
    match which.is_bipolar() {
        true => (0.5 - row) * 2.0 * high,
        false => (1.0 - row) * high,
    }
}

/// `CURVE_GRAB` as a duration, so the zone is the same handful of pixels at any zoom.
///
/// A radius is a *length*, which is why this is `width_to_duration` and can never be `x_to_tick`:
/// the latter answers where a pixel column is, and so adds the scroll. Five bars along, seven
/// pixels came back as some nineteen thousand ticks — a press on empty strip took hold of a point
/// a bar or more away and the first move of the pointer flung it across the clip, an alt-click
/// deleted a point nowhere near it, and a curve that had one point could never be given a second
/// anywhere on screen. Never less than a tick, so that zoomed far enough out the zone is at least
/// the point itself.
fn curve_grab_radius(view: &TimelineView) -> Ticks {
    Ticks(view.width_to_duration(px(CURVE_GRAB)).raw().abs().max(1))
}

/// The point within `radius` ticks of `at`, nearest first.
///
/// In ticks rather than pixels so the answer does not change with the zoom in a way the caller has
/// to compensate for; the caller turns its pixels into ticks, which it has to do anyway.
pub fn curve_point_at(points: &[CurvePoint], at: Ticks, radius: Ticks) -> Option<Ticks> {
    points
        .iter()
        .map(|point| (point.at, (point.at - at).raw().abs()))
        .filter(|(_, distance)| *distance <= radius.raw().abs())
        .min_by_key(|(_, distance)| *distance)
        .map(|(at, _)| at)
}

/// Draws a curve: its zero line, the curve itself, and a handle on each point.
#[allow(clippy::too_many_arguments)]
fn paint_curve(
    window: &mut Window,
    cx: &mut App,
    bounds: Bounds<Pixels>,
    view: &TimelineView,
    which: ClipCurve,
    clip_start: Ticks,
    clip_length: Ticks,
    points: &[CurvePoint],
    playhead: Ticks,
    theme: &Theme,
) {
    let top = f32::from(bounds.origin.y);
    let height = f32::from(bounds.size.height);
    let at = |tick: Ticks, value: f32| {
        point(
            bounds.origin.x + view.tick_to_x(clip_start + tick),
            px(top + height * curve_row(which, value)),
        )
    };

    // The stretch the clip covers, so a curve is read against the notes rather than against the
    // whole song — the same tint the grid above uses for the same purpose.
    paint::rect(
        window,
        Bounds {
            origin: point(
                bounds.origin.x + view.tick_to_x(clip_start),
                bounds.origin.y,
            ),
            size: size(
                view.tick_to_x(clip_start + clip_length) - view.tick_to_x(clip_start),
                bounds.size.height,
            ),
        },
        Theme::translucent(theme.surface, 0.5),
    );
    // Nothing, drawn: without it a flat curve at rest and an empty strip look the same, and the
    // one number a person needs to find again is where they started from. On a bend that line is
    // across the middle; on the wheel it is the floor, which is where nothing is.
    paint::rect(
        window,
        Bounds {
            origin: point(bounds.origin.x, px(top + height * curve_row(which, 0.0))),
            size: size(bounds.size.width, px(1.0)),
        },
        theme.border,
    );
    paint::label(
        window,
        cx,
        point(bounds.origin.x + px(3.0), px(top + 1.0)),
        match which {
            ClipCurve::Bend => format!("+{:.0}", which.limit()),
            ClipCurve::Modulation => "127".to_string(),
        },
        px(9.0),
        theme.text_faint,
    );

    if !points.is_empty() {
        // Held flat outside the points, which is what `curve_at` says and therefore what is heard.
        // A curve drawn only between its ends would show a slide starting somewhere it does not.
        let mut drawn: Vec<Point<Pixels>> = Vec::with_capacity(points.len() + 2);
        let first = points[0];
        let last = points[points.len() - 1];
        drawn.push(at(Ticks::ZERO, first.value));
        drawn.extend(points.iter().map(|point| at(point.at, point.value)));
        if last.at < clip_length {
            drawn.push(at(clip_length, last.value));
        }
        paint::polyline(window, &drawn, px(1.5), theme.accent);
        for held in points {
            let centre = at(held.at, held.value);
            paint::rounded_rect(
                window,
                Bounds {
                    origin: point(
                        centre.x - px(CURVE_POINT_RADIUS),
                        centre.y - px(CURVE_POINT_RADIUS),
                    ),
                    size: size(px(CURVE_POINT_RADIUS * 2.0), px(CURVE_POINT_RADIUS * 2.0)),
                },
                px(CURVE_POINT_RADIUS),
                theme.accent,
            );
        }
    }

    paint::playhead(
        window,
        bounds,
        bounds.origin.x + view.tick_to_x(playhead),
        theme,
    );
}

impl AurisApp {
    /// A press in a curve strip: take a point off, take hold of one, or write one and drag it.
    ///
    /// The three cases in the order a hand expects them, which is the order the automation lane
    /// put them in — delete first so the gesture bound to deleting takes a point off rather than
    /// adding one on top of it, and a press on empty strip writes the point it is about to drag,
    /// so placing a bend and shaping it is one gesture rather than click, look, click again.
    fn press_curve_lane(
        &mut self,
        which: ClipCurve,
        event: &MouseDownEvent,
        cx: &mut gpui::Context<Self>,
    ) {
        let (Some(bounds), Some(clip)) = (self.canvas.curve(which).get(), self.selected_clip)
        else {
            return;
        };
        let Some(held) = self.session.midi_clip(clip) else {
            return;
        };
        let (clip_start, points) = (held.start, held.curve(which).to_vec());
        let at = (self.snap_unless_held(
            self.timeline.x_to_tick(event.position.x - bounds.origin.x),
            event.modifiers,
        ) - clip_start)
            .max_zero();
        let grabbed = curve_point_at(&points, at, curve_grab_radius(&self.timeline));

        if let Some(existing) = grabbed
            && self.pointer.delete.matches(event)
        {
            self.session.remove_curve_point(clip, which, existing);
            cx.notify();
            return;
        }

        let row =
            f32::from(event.position.y - bounds.origin.y) / f32::from(bounds.size.height).max(1.0);
        let from = match grabbed {
            Some(existing) => existing,
            None => {
                if !self
                    .session
                    .set_curve_point(clip, which, at, curve_of_row(which, row))
                {
                    return;
                }
                at
            }
        };
        self.begin_drag(Drag::CurvePoint {
            clip,
            which,
            at: from,
        });
        cx.notify();
    }

    /// Moves the point in hand to where the pointer is, and says where it landed.
    ///
    /// The point is looked up by where it currently sits rather than by where the drag began, the
    /// way the automation lane's is: a point dropped onto another replaces it, and the drag has to
    /// go on holding whatever survived.
    pub(crate) fn drag_curve_point(
        &mut self,
        clip: ClipId,
        which: ClipCurve,
        at: Ticks,
        event: &gpui::MouseMoveEvent,
    ) -> Option<Ticks> {
        let bounds = self.canvas.curve(which).get()?;
        let clip_start = self.session.midi_clip(clip)?.start;
        let to = (self.snap_unless_held(
            self.timeline.x_to_tick(event.position.x - bounds.origin.x),
            event.modifiers,
        ) - clip_start)
            .max_zero();
        let row =
            f32::from(event.position.y - bounds.origin.y) / f32::from(bounds.size.height).max(1.0);
        self.session
            .move_curve_point(clip, which, at, to, curve_of_row(which, row))
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
    fn the_resize_cursor_covers_what_a_press_would_actually_grab() {
        // The arrow and the press read the same number, and this is what keeps them reading it
        // the same way: every pixel the arrow lights up has to be one where the press takes the
        // resize branch *and* one where `note_at` still finds the note. A zone that ran past the
        // end would promise a grab on empty grid.
        for width in [3.0f32, 8.0, 24.0, 96.0, 400.0] {
            let (start_x, end_x) = (px(200.0), px(200.0 + width));
            let Some((x, zone)) = note_end_span(start_x, end_x) else {
                continue;
            };
            let grab = resize_grab(end_x - start_x);
            assert!(x >= start_x, "a {width}px note lit up grid to its left");
            assert_eq!(x + zone, end_x, "the zone stops at the note's end");
            for step in 0..=10 {
                let at = x + zone * (step as f32 / 10.0);
                // A thousandth of a pixel of slack, and only at the boundary: `end_x - grab`
                // and then `end_x` minus that do not round-trip exactly in `f32`, so the zone's
                // own left edge can miss the press rule by an ulp. No pointer position lands
                // there, and widening the zone to hide it would be arithmetic dressed as design.
                assert!(
                    f32::from(end_x - at).abs() <= grab + 1e-3,
                    "{at:?} on a {width}px note is lit but would not resize",
                );
                assert!(at <= end_x, "{at:?} is past the note");
            }
        }
        // A note drawn thinner than three pixels has no room for a zone that is not the whole
        // note, and offers none — it can still be moved, which is the gesture left.
        assert_eq!(note_end_span(px(100.0), px(100.0)), None);
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
    fn the_tool_key_reaches_every_tool_and_comes_back() {
        // One binding for the lot, so the cycle has to reach all of them and return. A tool the
        // key could get into and not out of would be a mode with no way back.
        let mut tool = RollTool::default();
        let mut seen = vec![tool];
        for _ in 1..RollTool::ALL.len() {
            tool = tool.next();
            assert!(!seen.contains(&tool), "{tool:?} came round twice");
            seen.push(tool);
        }
        assert_eq!(seen.len(), RollTool::ALL.len());
        assert_eq!(tool.next(), RollTool::default(), "and then it wraps");
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

#[cfg(test)]
mod curve_tests {
    use super::*;

    #[test]
    fn a_value_survives_the_trip_to_the_strip_and_back() {
        // A curve is drawn from `curve_row` and dragged through `curve_of_row`, so a value that
        // did not round-trip would make the point slide out from under the pointer holding it.
        for which in ClipCurve::ALL {
            let (low, high) = which.range();
            for value in [low, low / 2.0, 0.0, high / 2.0, high] {
                let back = curve_of_row(which, curve_row(which, value));
                assert!(
                    (back - value).abs() < 0.001,
                    "{which:?} took {value} to {} and gave back {back}",
                    curve_row(which, value)
                );
            }
            // Past either end clamps rather than drawing off the strip.
            assert_eq!(curve_row(which, high * 99.0), 0.0);
            assert_eq!(curve_of_row(which, -2.0), high);
            assert_eq!(curve_of_row(which, 3.0), low);
        }
    }

    #[test]
    fn the_zero_line_is_where_each_curve_rests() {
        // The rule drawn across a strip is what makes an empty one different from a flat one, so
        // it has to be *at* the value the instrument holds when nothing is written.
        assert_eq!(
            curve_row(ClipCurve::Bend, 0.0),
            0.5,
            "a bend rests in the middle, because it goes both ways"
        );
        assert_eq!(
            curve_row(ClipCurve::Modulation, 0.0),
            1.0,
            "and a wheel rests on the floor, because there is nothing below it"
        );
        assert_eq!(curve_row(ClipCurve::Bend, BEND_LIMIT), 0.0, "the top is up");
        assert_eq!(curve_row(ClipCurve::Bend, -BEND_LIMIT), 1.0);
        assert_eq!(curve_row(ClipCurve::Modulation, MODULATION_LIMIT), 0.0);
    }

    #[test]
    fn the_wheel_never_goes_below_nothing() {
        // Half a strip drawn under a control that cannot reach it would be half a strip of
        // nothing — and a drag into it would write a negative wheel position, which is not a
        // thing.
        for row in [0.0, 0.5, 1.0, 2.0] {
            assert!(curve_of_row(ClipCurve::Modulation, row) >= 0.0, "{row}");
        }
        assert!(curve_of_row(ClipCurve::Bend, 1.0) < 0.0, "a bend does");
    }

    #[test]
    fn a_press_takes_the_point_it_landed_on_and_nothing_else() {
        let points = vec![
            CurvePoint {
                at: Ticks(0),
                value: 0.0,
            },
            CurvePoint {
                at: Ticks(480),
                value: 2.0,
            },
            CurvePoint {
                at: Ticks(960),
                value: 0.0,
            },
        ];
        let radius = Ticks(40);
        assert_eq!(
            curve_point_at(&points, Ticks(480), radius),
            Some(Ticks(480))
        );
        assert_eq!(
            curve_point_at(&points, Ticks(500), radius),
            Some(Ticks(480)),
            "just inside the zone"
        );
        assert_eq!(
            curve_point_at(&points, Ticks(600), radius),
            None,
            "the line between two points belongs to neither"
        );
        // Nearest rather than first, so two points dragged close together still resolve to the
        // one under the pointer.
        assert_eq!(
            curve_point_at(&points, Ticks(700), Ticks(400)),
            Some(Ticks(480))
        );
        assert_eq!(curve_point_at(&[], Ticks(0), radius), None);
    }

    #[test]
    fn the_grab_zone_is_a_length_and_stays_one_however_far_the_view_has_scrolled() {
        // Every other test in this file sits at the start of the song, which is the one place a
        // length and a position are the same number — and so the one place this could not be
        // seen. The radius was read out of `x_to_tick`, which adds the scroll, so five bars in
        // the seven-pixel zone had swollen to the better part of twenty thousand ticks.
        let per_beat = 48.0;
        let at_start = TimelineView {
            pixels_per_beat: per_beat,
            scroll_ticks: Ticks::ZERO,
        };
        let five_bars_in = TimelineView {
            pixels_per_beat: per_beat,
            scroll_ticks: Ticks(TICKS_PER_QUARTER * 20),
        };
        assert_eq!(
            curve_grab_radius(&five_bars_in),
            curve_grab_radius(&at_start),
            "scrolling the view must not widen the zone",
        );
        // And it is the seven pixels it claims to be: a quarter note is drawn 48 across, so the
        // zone reaches seven forty-eighths of one either way.
        assert_eq!(
            curve_grab_radius(&five_bars_in),
            Ticks((TICKS_PER_QUARTER as f32 * CURVE_GRAB / per_beat) as i64),
        );

        // Which is what keeps a second point writable. One point at the clip's start, a press a
        // beat later: near enough to reach only if the zone has grown, and if it has, the press
        // grabs that point and drags it instead of writing a new one.
        let points = vec![CurvePoint {
            at: Ticks::ZERO,
            value: 0.0,
        }];
        let radius = curve_grab_radius(&five_bars_in);
        assert_eq!(
            curve_point_at(&points, Ticks::ZERO, radius),
            Some(Ticks::ZERO),
            "a press on the point itself still takes hold of it",
        );
        assert_eq!(
            curve_point_at(&points, Ticks(TICKS_PER_QUARTER), radius),
            None,
            "a beat away is empty strip, and a press there writes a point of its own",
        );

        // Zoomed all the way out a tick is far narrower than a pixel, and the zone must not round
        // down to nothing: a zero radius is a point that can be drawn and never grabbed again.
        let far_out = TimelineView {
            pixels_per_beat: TimelineView::MIN_PIXELS_PER_BEAT,
            scroll_ticks: Ticks(TICKS_PER_QUARTER * 20),
        };
        assert!(curve_grab_radius(&far_out) >= Ticks(1));
    }

    #[test]
    fn each_strip_says_which_one_it_is() {
        // Two strips of the same size stacked under the notes, and the gutter is the only thing
        // that tells them apart.
        let labels: Vec<Key> = ClipCurve::ALL.into_iter().map(curve_label).collect();
        assert_eq!(labels[0], Key::BendLane);
        assert_eq!(labels[1], Key::ModulationLane);
        assert_ne!(labels[0], labels[1]);
    }
}
