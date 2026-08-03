//! The piano roll: note editing for the selected MIDI clip.

use auris_i18n::messages;
use auris_session::prelude::*;

use gpui::{
    Bounds, IntoElement, MouseButton, MouseDownEvent, Pixels, Point, Window, canvas, div, point,
    prelude::*, px, size,
};

use crate::app::{AurisApp, Drag};
use crate::theme::{Metrics, Theme};
use crate::ui::paint;
use crate::ui::timeline::{PitchView, TimelineView};

/// Width of the grab zone on a note's right edge, in pixels.
const RESIZE_HANDLE: f32 = 5.0;

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
                    .child(div().flex_1())
                    .child(messages::piano_roll_hint(
                        self.language(),
                        self.t(self.pointer.create.label()),
                        self.t(self.pointer.delete.label()),
                    ))
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
                            .child({
                                let theme = theme.clone();
                                let view = view.clone();
                                let pitch_view = pitch_view.clone();
                                let recorded = self.canvas.roll.clone();
                                canvas(
                                    move |bounds, _, _| recorded.set(Some(bounds)),
                                    move |bounds, _, window, _| {
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
                                                bounds,
                                                &notes,
                                                &selected,
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

#[allow(clippy::too_many_arguments)]
fn paint_notes(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    notes: &[Note],
    selected: &[usize],
    clip_start: Ticks,
    view: &TimelineView,
    pitch_view: &PitchView,
    theme: &Theme,
) {
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
}
