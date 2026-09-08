//! Drum clips as named kit lanes and beat-aligned hits.
//!
//! Rows follow the instrument's explicit assignments, never a guessed General MIDI kit.
//! Unassigned notes remain editable by MIDI address; a switch exposes every address when
//! programming a new kit. All gestures use the session's ordinary note commands and history.

use std::collections::BTreeSet;

use auris_i18n::{Key, messages};
use auris_session::prelude::*;
use gpui::{
    Bounds, Context, IntoElement, MouseButton, MouseDownEvent, Pixels, Point, Window, canvas, div,
    point, prelude::*, px, size,
};

use crate::app::{AurisApp, BandSurface, Drag};
use crate::theme::{Metrics, Theme};
use crate::ui::paint;
use crate::ui::piano_roll::{RollTool, paint_clip_extent};
use crate::ui::widgets::{ButtonStyle, button};

const ROW_HEIGHT: f32 = 28.0;
const LABEL_WIDTH: f32 = 164.0;
const HIT_WIDTH: f32 = 12.0;

/// Presentation state kept separately from the melodic editor's pitch and zoom.
#[derive(Default)]
pub(crate) struct DrumEditorState {
    scroll: f32,
    all_notes: bool,
    clip: Option<ClipId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DrumRow {
    pitch: u8,
    roles: Vec<DrumRole>,
}

/// One row per physical key, even when multiple musical roles name the same sound.
fn rows_for(map: &DrumMap, pitches: impl IntoIterator<Item = u8>, all: bool) -> Vec<DrumRow> {
    let mut rows: Vec<DrumRow> = Vec::new();
    for role in DrumRole::ALL {
        if let Some(&pitch) = map.voices.get(&role) {
            if let Some(row) = rows.iter_mut().find(|row| row.pitch == pitch) {
                row.roles.push(role);
            } else {
                rows.push(DrumRow {
                    pitch,
                    roles: vec![role],
                });
            }
        }
    }
    let mut extra: BTreeSet<u8> = pitches.into_iter().collect();
    if all || (rows.is_empty() && extra.is_empty()) {
        extra.extend(0..=127);
    }
    for pitch in extra {
        if !rows.iter().any(|row| row.pitch == pitch) {
            rows.push(DrumRow {
                pitch,
                roles: Vec::new(),
            });
        }
    }
    rows
}

fn role_key(role: DrumRole) -> Key {
    match role {
        DrumRole::Kick => Key::RoleKick,
        DrumRole::Snare => Key::RoleSnare,
        DrumRole::ClosedHat => Key::DrumClosedHat,
        DrumRole::OpenHat => Key::DrumOpenHat,
        DrumRole::Crash => Key::RoleCrash,
        DrumRole::Tom => Key::DrumTom,
    }
}

fn row_at(y: Pixels, scroll: f32, count: usize) -> Option<usize> {
    let y = f32::from(y) + scroll;
    (y >= 0.0)
        .then_some((y / ROW_HEIGHT) as usize)
        .filter(|row| *row < count)
}

impl AurisApp {
    /// Whether the selected note clip belongs to a drum track.
    pub(crate) fn editing_a_drum_clip(&self) -> bool {
        self.selected_clip
            .and_then(|clip| self.project().track_of_clip(clip))
            .and_then(|track| self.project().track(track))
            .is_some_and(|track| track.kind.is_drum())
    }

    fn drum_rows(&self) -> Vec<DrumRow> {
        let Some(clip) = self.selected_midi_clip() else {
            return Vec::new();
        };
        let map = self
            .project()
            .track_of_clip(clip.id)
            .and_then(|track| self.project().track(track))
            .and_then(|track| track.kind.as_instrument())
            .and_then(|instrument| DrumMap::load(&instrument.instrument_state))
            .unwrap_or_default();
        let notes = if self.source_score() {
            clip.notes.as_slice()
        } else {
            self.score_preview_notes().unwrap_or(&clip.notes)
        };
        let mut pitches: Vec<u8> = notes.iter().map(|note| note.pitch).collect();
        // Keep an unmapped source row in place for the entire gesture, including after its
        // last note has moved to a different row. Otherwise every following move would be
        // interpreted against a different vertical axis.
        if let Some(Drag::NoteMove { origins, .. }) = &self.drag {
            pitches.extend(origins.iter().map(|(_, _, pitch)| *pitch));
        }
        rows_for(&map, pitches, self.drum_editor.all_notes)
    }

    /// Renders the percussion editor in the shared clip-editor dock.
    pub(crate) fn render_drum_editor(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if self.drum_editor.clip != self.selected_clip {
            self.drum_editor.clip = self.selected_clip;
            self.drum_editor.scroll = 0.0;
        }
        let Some(clip) = self.selected_midi_clip() else {
            return div().into_any_element();
        };
        let name = clip.name.clone();
        let clip_start = clip.start;
        let source = self.source_score();
        let clip_length = if source {
            clip.length
        } else {
            clip.sounding_length()
        };
        let notes = self.score_notes();
        let rows = self.drum_rows();
        if rows.len() == 128 && notes.is_empty() {
            self.drum_editor.all_notes = true;
        }
        let height = self
            .canvas
            .roll
            .get()
            .map_or(ROW_HEIGHT, |bounds| f32::from(bounds.size.height));
        self.drum_editor.scroll = self
            .drum_editor
            .scroll
            .clamp(0.0, (rows.len() as f32 * ROW_HEIGHT - height).max(0.0));
        let scroll = self.drum_editor.scroll;
        let labels: Vec<String> = rows
            .iter()
            .map(|row| {
                if row.roles.is_empty() {
                    format!("MIDI {}", row.pitch)
                } else {
                    format!(
                        "{} · {}",
                        row.roles
                            .iter()
                            .map(|role| self.t(role_key(*role)))
                            .collect::<Vec<_>>()
                            .join(" / "),
                        row.pitch
                    )
                }
            })
            .collect();
        let theme = self.theme.clone();
        let view = self.timeline.clone();
        let signatures = self.project().signatures.spans();
        let playhead = self.playhead_ticks();
        let selected = if source {
            self.selected_notes.clone()
        } else {
            BTreeSet::new()
        };
        let band = source
            .then(|| self.rubber_band(BandSurface::Roll))
            .flatten();
        let recorded = self.canvas.roll.clone();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(80.0))
            .min_w_0()
            .bg(theme.surface_sunken)
            .child(self.score_layer_tabs(cx))
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
                    .min_w_0()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(format!("{} — {name}", self.t(Key::DrumEditor))),
                    )
                    .when(source, |row| row.child(self.tool_strip(cx)))
                    .child(div().flex_1().min_w_0().truncate().child(if !source {
                        self.t(Key::ScorePerformedHint).to_string()
                    } else {
                        match self.tool {
                            RollTool::Pointer => self.t(Key::DrumEditorHint).to_string(),
                            RollTool::Velocity => {
                                messages::piano_roll_velocity_hint(self.language()).to_string()
                            }
                        }
                    }))
                    .child(button(
                        "drum-all-notes",
                        self.t(Key::DrumEditorAllNotes),
                        ButtonStyle::Ghost,
                        self.drum_editor.all_notes,
                        theme.accent_soft,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.drum_editor.all_notes = !this.drum_editor.all_notes;
                            this.drum_editor.scroll = 0.0;
                            cx.notify();
                        }),
                    ))
                    .child(self.zoom_slider("drum-zoom", cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(
                        div()
                            .id("drum-voices")
                            .w(px(LABEL_WIDTH))
                            .flex_shrink_0()
                            .h_full()
                            .overflow_hidden()
                            .child({
                                let theme = theme.clone();
                                canvas(
                                    |_, _, _| (),
                                    move |bounds, _, window, cx| {
                                        paint::clipped(window, bounds, |window| {
                                            paint::rect(window, bounds, theme.surface_raised);
                                            for (index, label) in labels.iter().enumerate() {
                                                let y = bounds.origin.y
                                                    + px(index as f32 * ROW_HEIGHT - scroll);
                                                if y + px(ROW_HEIGHT) < bounds.origin.y
                                                    || y > bounds.bottom()
                                                {
                                                    continue;
                                                }
                                                paint::hline(window, bounds, y, theme.border);
                                                paint::label(
                                                    window,
                                                    cx,
                                                    point(bounds.origin.x + px(8.0), y + px(6.0)),
                                                    label.clone(),
                                                    px(11.0),
                                                    theme.text,
                                                );
                                            }
                                        });
                                    },
                                )
                                .size_full()
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                    if let Some(pitch) =
                                        this.drum_pitch_at(event.position.y - this.roll_origin().y)
                                    {
                                        this.audition(pitch);
                                        cx.notify();
                                    }
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, _| this.stop_audition()),
                            )
                            .on_scroll_wheel(cx.listener(Self::scroll_drums)),
                    )
                    .child(
                        div()
                            .id("drum-grid")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_hidden()
                            .when(source && self.tool == RollTool::Velocity, |this| {
                                this.cursor(gpui::CursorStyle::ResizeUpDown)
                            })
                            .child(
                                canvas(
                                    move |bounds, _, _| {
                                        recorded.set(Some(bounds));
                                    },
                                    move |bounds, _, window, cx| {
                                        paint::clipped(window, bounds, |window| {
                                            paint::rect(window, bounds, theme.surface_sunken);
                                            for index in 0..rows.len() {
                                                let y = bounds.origin.y
                                                    + px(index as f32 * ROW_HEIGHT - scroll);
                                                if index % 2 == 0 {
                                                    paint::rect(
                                                        window,
                                                        Bounds {
                                                            origin: point(bounds.origin.x, y),
                                                            size: size(
                                                                bounds.size.width,
                                                                px(ROW_HEIGHT),
                                                            ),
                                                        },
                                                        Theme::translucent(
                                                            theme.surface_raised,
                                                            0.5,
                                                        ),
                                                    );
                                                }
                                                paint::hline(window, bounds, y, theme.border);
                                            }
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
                                            for (index, note) in notes.iter().enumerate() {
                                                let Some(row) = rows
                                                    .iter()
                                                    .position(|row| row.pitch == note.pitch)
                                                else {
                                                    continue;
                                                };
                                                let x = bounds.origin.x
                                                    + view.tick_to_x(clip_start + note.start);
                                                let y = bounds.origin.y
                                                    + px(row as f32 * ROW_HEIGHT - scroll + 4.0);
                                                let hit = Bounds {
                                                    origin: point(x + px(1.0), y),
                                                    size: size(px(HIT_WIDTH), px(ROW_HEIGHT - 8.0)),
                                                };
                                                let color = if selected.contains(&index) {
                                                    theme.accent
                                                } else {
                                                    Theme::translucent(
                                                        theme.accent,
                                                        0.4 + 0.6 * note.velocity.clamp(0.0, 1.0),
                                                    )
                                                };
                                                paint::rounded_rect(window, hit, px(3.0), color);
                                                let velocity = (ROW_HEIGHT - 12.0)
                                                    * note.velocity.clamp(0.0, 1.0);
                                                paint::rect(
                                                    window,
                                                    Bounds {
                                                        origin: point(
                                                            x + px(5.0),
                                                            y + px(ROW_HEIGHT - 10.0 - velocity),
                                                        ),
                                                        size: size(px(3.0), px(velocity)),
                                                    },
                                                    theme.text,
                                                );
                                                if selected.contains(&index) {
                                                    paint::rounded_outline(
                                                        window,
                                                        hit,
                                                        px(3.0),
                                                        px(1.0),
                                                        theme.text,
                                                    );
                                                    if note.velocity.is_finite() {
                                                        paint::label(
                                                            window,
                                                            cx,
                                                            point(x + px(HIT_WIDTH + 4.0), y),
                                                            format!(
                                                                "{}",
                                                                (note.velocity.clamp(0.0, 1.0)
                                                                    * 127.0)
                                                                    .round()
                                                                    as u8
                                                            ),
                                                            px(10.0),
                                                            theme.text_muted,
                                                        );
                                                    }
                                                }
                                            }
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
                                .size_full(),
                            )
                            .on_mouse_down(MouseButton::Left, cx.listener(Self::press_drum_grid))
                            .on_mouse_down(MouseButton::Right, cx.listener(Self::open_drum_menu))
                            .on_scroll_wheel(cx.listener(Self::scroll_drums)),
                    ),
            )
            .into_any_element()
    }

    fn drum_pitch_at(&self, y: Pixels) -> Option<u8> {
        let rows = self.drum_rows();
        row_at(y, self.drum_editor.scroll, rows.len()).map(|row| rows[row].pitch)
    }

    fn drum_hit_at(&self, at: Point<Pixels>, pitch: u8) -> Option<usize> {
        let clip = self.selected_midi_clip()?;
        let x = at.x - self.roll_origin().x;
        clip.notes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, note)| {
                let start = self.timeline.tick_to_x(clip.start + note.start);
                note.pitch == pitch && x >= start && x <= start + px(HIT_WIDTH + 2.0)
            })
            .map(|(index, _)| index)
    }

    fn press_drum_grid(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.source_score() {
            return;
        }
        let Some(clip) = self.selected_clip else {
            return;
        };
        let origin = self.roll_origin();
        let Some(pitch) = self.drum_pitch_at(event.position.y - origin.y) else {
            return;
        };
        let Some(clip_start) = self.session.midi_clip(clip).map(|clip| clip.start) else {
            return;
        };
        let tick = self.timeline.x_to_tick(event.position.x - origin.x);
        let under = self.drum_hit_at(event.position, pitch);
        if self.tool == RollTool::Velocity {
            if let Some(index) = under {
                self.begin_velocity_drag(clip, index, event.position.y, event.modifiers);
            } else {
                self.begin_rubber_band(BandSurface::Roll, event.position, event.modifiers.shift);
            }
        } else if self.pointer.delete.matches(event) {
            if let Some(index) = under {
                let _ = self.session.remove_notes(clip, &[index]);
                self.selected_notes.clear();
            }
        } else if let Some(index) = under {
            if event.modifiers.shift && self.selected_notes.remove(&index) {
                cx.notify();
                return;
            }
            if !event.modifiers.shift && !self.selected_notes.contains(&index) {
                self.selected_notes.clear();
            }
            self.selected_notes.insert(index);
            self.begin_drag(Drag::NoteMove {
                clip,
                origin_tick: tick - clip_start,
                origin_pitch: pitch,
                origins: self.selected_note_origins(clip),
                pressed_at: Some(event.position),
            });
            self.audition_note(index, pitch);
        } else if event.modifiers.shift {
            self.begin_rubber_band(BandSurface::Roll, event.position, true);
        } else {
            let start = (tick.snap_nearest(self.project().grid) - clip_start).max_zero();
            let length = Ticks(self.project().grid.raw().max(1));
            self.begin_drag(Drag::NoteMove {
                clip,
                origin_tick: tick - clip_start,
                origin_pitch: pitch,
                origins: Vec::new(),
                pressed_at: Some(event.position),
            });
            match self.session.add_note(clip, Note::new(pitch, start, length)) {
                Ok(index) => {
                    self.selected_notes.clear();
                    self.selected_notes.insert(index);
                    self.drag = Some(Drag::NoteMove {
                        clip,
                        origin_tick: tick - clip_start,
                        origin_pitch: pitch,
                        origins: self.selected_note_origins(clip),
                        pressed_at: Some(event.position),
                    });
                    self.audition_note(index, pitch);
                }
                Err(_) => self.abandon_drag(),
            }
        }
        cx.notify();
    }

    fn open_drum_menu(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.source_score() {
            return;
        }
        let Some(pitch) = self.drum_pitch_at(event.position.y - self.roll_origin().y) else {
            return;
        };
        let Some(clip_start) = self.selected_midi_clip().map(|clip| clip.start) else {
            return;
        };
        let under = self.drum_hit_at(event.position, pitch);
        if let Some(index) = under
            && !self.selected_notes.contains(&index)
        {
            self.selected_notes.clear();
            self.selected_notes.insert(index);
        }
        let tick = self
            .timeline
            .x_to_tick(event.position.x - self.roll_origin().x);
        let start = (tick.snap_nearest(self.project().grid) - clip_start).max_zero();
        self.open_menu(self.roll_menu(event.position, under, pitch, start));
        cx.notify();
    }

    fn scroll_drums(
        &mut self,
        event: &gpui::ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = event.delta.pixel_delta(px(ROW_HEIGHT));
        if event.modifiers.alt {
            self.timeline.zoom_by(
                if delta.y > px(0.0) { 1.12 } else { 1.0 / 1.12 },
                event.position.x - self.roll_origin().x,
            );
        } else if event.modifiers.shift {
            self.timeline.scroll_by(-delta.y - delta.x);
        } else {
            let height = self
                .canvas
                .roll
                .get()
                .map_or(ROW_HEIGHT, |bounds| f32::from(bounds.size.height));
            let max = (self.drum_rows().len() as f32 * ROW_HEIGHT - height).max(0.0);
            self.drum_editor.scroll =
                (self.drum_editor.scroll - f32::from(delta.y)).clamp(0.0, max);
            self.timeline.scroll_by(-delta.x);
        }
        cx.notify();
    }

    /// Moves selected hits by kit rows rather than by the numerical distance between MIDI keys.
    pub(crate) fn drag_drum_notes(
        &mut self,
        clip: ClipId,
        origin_pitch: u8,
        origins: &[(usize, Ticks, u8)],
        delta_ticks: Ticks,
        y: Pixels,
    ) {
        let rows = self.drum_rows();
        let Some(source) = rows.iter().position(|row| row.pitch == origin_pitch) else {
            return;
        };
        let target = ((f32::from(y) + self.drum_editor.scroll) / ROW_HEIGHT).floor() as isize;
        let offset = target - source as isize;
        for origin @ (_, _, pitch) in origins {
            let Some(row) = rows.iter().position(|row| row.pitch == *pitch) else {
                continue;
            };
            let target = (row as isize + offset).clamp(0, rows.len() as isize - 1) as usize;
            let _ = self.session.move_notes(
                clip,
                &[*origin],
                delta_ticks,
                i32::from(rows[target].pitch) - i32::from(*pitch),
            );
        }
        let target = target.clamp(0, rows.len() as isize - 1) as usize;
        if !self.is_auditioning(rows[target].pitch) {
            self.audition(rows[target].pitch);
        }
    }

    /// Moves a keyboard or menu selection to neighbouring kit voices in one undo step.
    pub(crate) fn move_drum_selection(&mut self, offset: i32) {
        let Some(clip) = self.selected_clip else {
            return;
        };
        let rows = self.drum_rows();
        let origins = self.selected_note_origins(clip);
        self.session.begin_transaction(Edit::TransposeNotes);
        for origin @ (_, _, pitch) in origins {
            let Some(row) = rows.iter().position(|row| row.pitch == pitch) else {
                continue;
            };
            let target = (row as i32 + offset).clamp(0, rows.len() as i32 - 1) as usize;
            let _ = self.session.move_notes(
                clip,
                &[origin],
                Ticks::ZERO,
                i32::from(rows[target].pitch) - i32::from(pitch),
            );
        }
        self.session.end_transaction();
    }

    /// Selects the drawn hit markers, including non-contiguous MIDI addresses in adjacent rows.
    pub(crate) fn drum_notes_under(&self, band: Bounds<Pixels>) -> BTreeSet<usize> {
        let Some(clip) = self.selected_midi_clip() else {
            return BTreeSet::new();
        };
        if band.size.width <= px(0.0) || band.size.height <= px(0.0) {
            return BTreeSet::new();
        }
        let rows = self.drum_rows();
        let origin = self.roll_origin();
        clip.notes
            .iter()
            .enumerate()
            .filter(|(_, note)| {
                let Some(row) = rows.iter().position(|row| row.pitch == note.pitch) else {
                    return false;
                };
                let x = origin.x + self.timeline.tick_to_x(clip.start + note.start);
                let y = origin.y + px(row as f32 * ROW_HEIGHT - self.drum_editor.scroll);
                x < band.right()
                    && x + px(HIT_WIDTH + 2.0) > band.origin.x
                    && y < band.bottom()
                    && y + px(ROW_HEIGHT) > band.origin.y
            })
            .map(|(index, _)| index)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authored_rows_keep_arbitrary_addresses_and_merge_shared_roles() {
        let map = DrumMap {
            voices: [
                (DrumRole::Kick, 73),
                (DrumRole::Snare, 18),
                (DrumRole::Tom, 18),
            ]
            .into_iter()
            .collect(),
        };
        let rows = rows_for(&map, [91, 73], false);
        assert_eq!(
            rows.iter().map(|row| row.pitch).collect::<Vec<_>>(),
            [73, 18, 91]
        );
        assert_eq!(rows[1].roles, [DrumRole::Snare, DrumRole::Tom]);
        assert!(rows[2].roles.is_empty());
        assert_eq!(rows_for(&map, [], true).len(), 128);
    }

    #[test]
    fn missing_assignments_do_not_invent_kit_roles() {
        let rows = rows_for(&DrumMap::default(), [36, 60], false);
        assert!(rows.iter().all(|row| row.roles.is_empty()));
        assert_eq!(
            rows.iter().map(|row| row.pitch).collect::<Vec<_>>(),
            [36, 60]
        );
        assert_eq!(rows_for(&DrumMap::default(), [], false).len(), 128);
        assert_eq!(row_at(px(-1.0), 0.0, 2), None);
        assert_eq!(row_at(px(0.0), ROW_HEIGHT, 2), Some(1));
        assert_eq!(row_at(px(ROW_HEIGHT * 2.0), 0.0, 2), None);
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;
    use crate::harness::{self, CLIP_LENGTH, click_at, deleting, drag, paint};
    use crate::ui::context_menu::MenuCommand;
    use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};

    fn fixture(
        cx: &mut TestAppContext,
    ) -> (Entity<AurisApp>, &mut VisualTestContext, TrackId, ClipId) {
        let (app, cx) = harness::open(cx);
        let (track, clip) = app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            let track = this.session.add_default_drum_track("Kit").unwrap();
            let clip = this
                .session
                .add_midi_clip(track, "Beat", Ticks::ZERO, CLIP_LENGTH)
                .unwrap();
            this.open_clip_in_editor(clip);
            (track, clip)
        });
        paint(&app, cx);
        (app, cx, track, clip)
    }

    fn hit_point(
        app: &Entity<AurisApp>,
        cx: &VisualTestContext,
        tick: Ticks,
        pitch: u8,
    ) -> Point<Pixels> {
        app.read_with(cx, |this, _| {
            let row = this
                .drum_rows()
                .iter()
                .position(|row| row.pitch == pitch)
                .unwrap();
            let origin = this.roll_origin();
            point(
                origin.x + this.timeline.tick_to_x(tick) + px(5.0),
                origin.y + px(row as f32 * ROW_HEIGHT - this.drum_editor.scroll + ROW_HEIGHT / 2.0),
            )
        })
    }

    #[gpui::test]
    fn performed_drum_rows_follow_transforms_and_reject_note_edits(cx: &mut TestAppContext) {
        let (app, cx, _, clip) = fixture(cx);
        app.update(cx, |this, _| {
            this.session
                .add_note(clip, Note::new(36, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
            this.session
                .set_clip_transforms(
                    clip,
                    vec![
                        NoteTransform::Transpose { semitones: 55 },
                        NoteTransform::Brush { amount: 1.0 },
                    ],
                )
                .unwrap();
        });
        paint(&app, cx);
        harness::click("score-performed", cx);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(this.drum_rows().iter().any(|row| row.pitch == 91));
            assert!(this.score_preview_notes().unwrap().len() > 1);
        });
        let from = hit_point(&app, cx, Ticks::ZERO, 91);
        let to = hit_point(&app, cx, Ticks::QUARTER * 2, 91);
        drag(cx, from, to);
        click_at(cx, to, Modifiers::none());
        harness::right_press(cx, from);
        cx.simulate_keystrokes("backspace");
        app.read_with(cx, |this, _| {
            assert!(this.menu.is_none());
            let notes = &this.session.midi_clip(clip).unwrap().notes;
            assert_eq!(notes.len(), 1);
            assert_eq!(notes[0].pitch, 36);
            assert_eq!(notes[0].start, Ticks::ZERO);
        });
        harness::click("score-source", cx);
        paint(&app, cx);
        let at = hit_point(&app, cx, Ticks::QUARTER * 2, 36);
        click_at(cx, at, Modifiers::none());
        app.read_with(cx, |this, _| {
            assert_eq!(this.session.midi_clip(clip).unwrap().notes.len(), 2)
        });
    }

    #[gpui::test]
    fn plain_click_creates_a_kit_hit_and_one_undo_removes_it(cx: &mut TestAppContext) {
        let (app, cx, _, clip) = fixture(cx);
        let at = hit_point(&app, cx, Ticks::QUARTER, 36);
        click_at(cx, at, Modifiers::none());
        app.update(cx, |this, _| {
            let notes = &this.session.midi_clip(clip).unwrap().notes;
            assert_eq!(notes.len(), 1);
            assert_eq!((notes[0].pitch, notes[0].start), (36, Ticks::QUARTER));
            assert!(this.editing_a_drum_clip());
            this.session.undo();
            assert!(this.session.midi_clip(clip).unwrap().notes.is_empty());
            this.session.redo();
            assert_eq!(this.session.midi_clip(clip).unwrap().notes[0].pitch, 36);
        });
        paint(&app, cx);
        click_at(cx, at, deleting());
        app.read_with(cx, |this, _| {
            assert!(this.session.midi_clip(clip).unwrap().notes.is_empty())
        });
    }

    #[gpui::test]
    fn a_group_drag_moves_by_kit_rows_not_midi_intervals(cx: &mut TestAppContext) {
        let (app, cx, _, clip) = fixture(cx);
        app.update(cx, |this, _| {
            this.session
                .add_note(clip, Note::new(36, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
            this.session
                .add_note(clip, Note::new(38, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
            this.selected_notes.extend([0, 1]);
        });
        paint(&app, cx);
        let from = hit_point(&app, cx, Ticks::ZERO, 36);
        let to = hit_point(&app, cx, Ticks::QUARTER, 38);
        drag(cx, from, to);
        app.update(cx, |this, _| {
            let notes = &this.session.midi_clip(clip).unwrap().notes;
            assert_eq!(
                notes
                    .iter()
                    .map(|note| (note.pitch, note.start))
                    .collect::<Vec<_>>(),
                [(38, Ticks::QUARTER), (42, Ticks::QUARTER)]
            );
            this.session.undo();
            assert_eq!(
                this.session
                    .midi_clip(clip)
                    .unwrap()
                    .notes
                    .iter()
                    .map(|note| note.pitch)
                    .collect::<Vec<_>>(),
                [36, 38]
            );
        });
    }

    #[gpui::test]
    fn an_unmapped_hit_keeps_its_source_row_through_several_drag_moves(cx: &mut TestAppContext) {
        let (app, cx, _, clip) = fixture(cx);
        app.update(cx, |this, _| {
            this.session
                .add_note(clip, Note::new(91, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
            this.drum_editor.scroll = ROW_HEIGHT * 3.0;
        });
        paint(&app, cx);
        let from = hit_point(&app, cx, Ticks::ZERO, 91);
        let middle = hit_point(&app, cx, Ticks::QUARTER, 47);
        let to = hit_point(&app, cx, Ticks::QUARTER * 2, 49);
        harness::press(cx, from);
        harness::drag_to(cx, middle);
        harness::drag_to(cx, to);
        harness::release(cx, to);
        app.update(cx, |this, _| {
            let note = &this.session.midi_clip(clip).unwrap().notes[0];
            assert_eq!((note.pitch, note.start), (49, Ticks::QUARTER * 2));
            this.session.undo();
            assert_eq!(this.session.midi_clip(clip).unwrap().notes[0].pitch, 91);
        });
    }

    #[gpui::test]
    fn a_selection_sweep_follows_named_rows(cx: &mut TestAppContext) {
        let (app, cx, _, clip) = fixture(cx);
        app.update(cx, |this, _| {
            for pitch in [42, 46, 49] {
                this.session
                    .add_note(clip, Note::new(pitch, Ticks::QUARTER, Ticks::QUARTER))
                    .unwrap();
            }
        });
        paint(&app, cx);
        let from = hit_point(&app, cx, Ticks::QUARTER * 2, 42);
        let to = hit_point(&app, cx, Ticks::ZERO, 46);
        harness::drag_with(
            cx,
            from,
            to,
            Modifiers {
                shift: true,
                ..Modifiers::none()
            },
        );
        app.read_with(cx, |this, _| {
            assert_eq!(this.selected_notes, [0, 1].into_iter().collect())
        });
    }

    #[gpui::test]
    fn velocity_and_voice_commands_preserve_time_and_frozen_recipe(cx: &mut TestAppContext) {
        let (app, cx, track, _) = fixture(cx);
        let clip = app.update(cx, |this, _| {
            let clip = this
                .session
                .generate_clip(
                    track,
                    Ticks::ZERO,
                    CLIP_LENGTH,
                    ClipRecipe::new(ClipPreset::Drums, 7),
                )
                .unwrap();
            this.session.freeze_clip(clip).unwrap();
            let indices: Vec<usize> =
                (0..this.session.midi_clip(clip).unwrap().notes.len()).collect();
            this.session.remove_notes(clip, &indices).unwrap();
            this.session
                .add_note(clip, Note::new(38, Ticks::QUARTER, Ticks::QUARTER))
                .unwrap();
            this.open_clip_in_editor(clip);
            this.tool = RollTool::Velocity;
            clip
        });
        paint(&app, cx);
        let at = hit_point(&app, cx, Ticks::QUARTER, 38);
        let original = app.read_with(cx, |this, _| {
            this.session.midi_clip(clip).unwrap().notes[0].velocity
        });
        drag(cx, at, point(at.x, at.y + px(30.0)));
        app.update(cx, |this, cx| {
            let note = &this.session.midi_clip(clip).unwrap().notes[0];
            assert!(note.velocity < original);
            assert_eq!((note.pitch, note.start), (38, Ticks::QUARTER));
            assert!(this.session.midi_clip(clip).unwrap().recipe.is_none());
            this.session.undo();
            assert_eq!(
                this.session.midi_clip(clip).unwrap().notes[0].velocity,
                original
            );
            this.run_menu_command(MenuCommand::TransposeNotes(-1), cx);
            assert_eq!(this.session.midi_clip(clip).unwrap().notes[0].pitch, 42);
            this.run_menu_command(MenuCommand::TransposeNotes(12), cx);
            assert_eq!(this.session.midi_clip(clip).unwrap().notes[0].pitch, 42);
            this.session.undo();
            assert_eq!(this.session.midi_clip(clip).unwrap().notes[0].pitch, 38);
        });
    }

    #[gpui::test]
    fn manual_assignment_changes_the_kit_row_without_rewriting_existing_hits(
        cx: &mut TestAppContext,
    ) {
        let (app, cx, track, clip) = fixture(cx);
        app.update(cx, |this, _| {
            // Opening a clip alone does not select its track; the arrangement's press does
            // that before opening the editor. The inspector needs the same track selection.
            this.select_track(track);
            this.session
                .add_note(clip, Note::new(36, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
            this.show_panel(crate::dock::Panel::Inspector);
        });
        paint(&app, cx);
        for _ in 0..20 {
            let viewport = app.read_with(cx, |this, _| this.inspector_scroll.bounds());
            let control = cx.debug_bounds("drum-assignment-note-0");
            if control.is_some_and(|bounds| viewport.contains(&bounds.center())) {
                break;
            }
            let dy = if control.is_some_and(|bounds| bounds.center().y < viewport.top()) {
                120.0
            } else {
                -120.0
            };
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: viewport.center(),
                delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(dy))),
                ..Default::default()
            });
            paint(&app, cx);
        }
        let control = cx
            .debug_bounds("drum-assignment-note-0")
            .expect("the track has an assignment field");
        let viewport = app.read_with(cx, |this, _| this.inspector_scroll.bounds());
        assert!(
            viewport.contains(&control.center()),
            "the assignment field is reachable inside the inspector"
        );
        harness::click("drum-assignment-note-0", cx);
        paint(&app, cx);
        cx.simulate_input("73");
        cx.simulate_keystrokes("enter");
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            let rows = this.drum_rows();
            assert_eq!((rows[0].pitch, &rows[0].roles), (73, &vec![DrumRole::Kick]));
            assert!(
                rows.iter()
                    .any(|row| row.pitch == 36 && row.roles.is_empty())
            );
            assert_eq!(this.session.midi_clip(clip).unwrap().notes[0].pitch, 36);
        });
        let at = hit_point(&app, cx, Ticks::QUARTER, 73);
        click_at(cx, at, Modifiers::none());
        app.update(cx, |this, _| {
            assert_eq!(
                this.session
                    .midi_clip(clip)
                    .unwrap()
                    .notes
                    .iter()
                    .map(|note| note.pitch)
                    .collect::<Vec<_>>(),
                [36, 73]
            );
            this.session.undo();
            assert_eq!(this.session.midi_clip(clip).unwrap().notes.len(), 1);
            assert_eq!(
                this.session.drum_assignments(track).unwrap().voices[&DrumRole::Kick],
                73
            );
            this.session.undo();
            assert_eq!(
                this.session.drum_assignments(track).unwrap().voices[&DrumRole::Kick],
                36
            );
        });
    }

    #[gpui::test]
    fn opening_drums_preserves_the_melodic_pitch_view_and_singer_routing(cx: &mut TestAppContext) {
        let (app, cx, _, clip) = fixture(cx);
        app.update(cx, |this, _| {
            this.pitch.top_pitch = 87;
            this.pitch.row_height = 19.0;
            this.session
                .add_note(clip, Note::new(36, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
            this.open_clip_in_editor(clip);
            assert_eq!((this.pitch.top_pitch, this.pitch.row_height), (87, 19.0));
            let melody = this.session.add_default_instrument_track("Keys").unwrap();
            let melodic_clip = this
                .session
                .add_midi_clip(melody, "Melody", Ticks::ZERO, CLIP_LENGTH)
                .unwrap();
            this.open_clip_in_editor(melodic_clip);
            assert!(!this.editing_a_drum_clip());
            assert!(!this.editing_a_singer_clip());
            let singer = this.session.add_singer_track("Voice");
            let sung = this
                .session
                .add_midi_clip(singer, "Verse", Ticks::ZERO, CLIP_LENGTH)
                .unwrap();
            this.open_clip_in_editor(sung);
            assert!(!this.editing_a_drum_clip());
            assert!(this.editing_a_singer_clip());
        });
        paint(&app, cx);
    }
}
