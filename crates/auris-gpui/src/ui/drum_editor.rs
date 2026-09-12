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
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::paint;
use crate::ui::piano_roll::{RollTool, paint_clip_extent};
use crate::ui::prompt::{Prompt, PromptTarget};
use crate::ui::tooltip::keyed_tip;
use crate::ui::widgets::{ButtonStyle, button};

const ROW_HEIGHT: f32 = 28.0;
const LABEL_WIDTH: f32 = 220.0;
const HIT_WIDTH: f32 = 12.0;

/// Which lanes the drum editor presents.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum DrumRowsMode {
    /// The authored map, plus otherwise hidden notes already used by the clip.
    #[default]
    Map,
    /// Only MIDI keys used by this clip.
    Used,
    /// Every physical MIDI key.
    All,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct DrumLearn {
    track: TrackId,
    lane: Option<u64>,
}

/// Presentation state kept separately from the melodic editor's pitch and zoom.
#[derive(Default)]
pub(crate) struct DrumEditorState {
    scroll: f32,
    mode: DrumRowsMode,
    clip: Option<ClipId>,
    pub(crate) selected_lane: Option<u64>,
    learn: Option<DrumLearn>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DrumRow {
    pitch: u8,
    lane: Option<u64>,
    name: String,
    roles: Vec<DrumRole>,
}

fn row_for(map: &DrumMap, pitch: u8) -> DrumRow {
    match map.lanes.iter().find(|lane| lane.note == pitch) {
        Some(lane) => DrumRow {
            pitch,
            lane: Some(lane.id),
            name: lane.name.clone(),
            roles: lane.roles.iter().copied().collect(),
        },
        None => DrumRow {
            pitch,
            lane: None,
            name: String::new(),
            roles: Vec::new(),
        },
    }
}

/// One row per physical key, even when multiple musical roles name the same sound.
fn rows_for(
    map: &DrumMap,
    pitches: impl IntoIterator<Item = u8>,
    mode: DrumRowsMode,
) -> Vec<DrumRow> {
    let mut used: BTreeSet<u8> = pitches.into_iter().collect();
    if mode == DrumRowsMode::All {
        return (0..=127).map(|pitch| row_for(map, pitch)).collect();
    }
    let mut rows = Vec::new();
    for lane in &map.lanes {
        if mode == DrumRowsMode::Map || used.contains(&lane.note) {
            rows.push(row_for(map, lane.note));
            used.remove(&lane.note);
        }
    }
    for pitch in used {
        rows.push(row_for(map, pitch));
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

    pub(crate) fn selected_drum_track(&self) -> Option<TrackId> {
        let track = self
            .selected_clip
            .and_then(|clip| self.project().track_of_clip(clip))
            .or(self.selected_track)?;
        self.project()
            .track(track)
            .is_some_and(|entry| entry.kind.is_drum())
            .then_some(track)
    }

    fn general_midi_drum_name(&self, pitch: u8) -> Option<String> {
        let source = self
            .selected_drum_track()
            .and_then(|track| self.session.drum_map_source(track))?;
        let font = source.soundfont?;
        let known = font.bank == 128
            && font.path.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .eq_ignore_ascii_case("MuseScore_General.sf2")
            });
        if !known {
            return None;
        }
        static GM: std::sync::OnceLock<DrumMap> = std::sync::OnceLock::new();
        GM.get_or_init(DrumMap::general_midi)
            .lanes
            .iter()
            .find(|lane| lane.note == pitch)
            .map(|lane| lane.name.clone())
    }

    fn drum_map_actions(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        button(
            "drum-map-actions",
            self.t(Key::DrumEditorMappedNotes),
            ButtonStyle::Normal,
            false,
            theme.accent,
            &theme,
            cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                let menu = this.drum_map_menu(event.position());
                this.open_menu(menu);
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .tooltip(keyed_tip(self.t(Key::DrumEditorAutoSaved), "", &theme))
        .into_any_element()
    }

    fn drum_empty_state(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(track) = self.selected_drum_track() else {
            return div().into_any_element();
        };
        let theme = self.theme.clone();
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .p_3()
                    .rounded(Metrics::RADIUS_MD)
                    .bg(theme.surface_raised)
                    .border_1()
                    .border_color(theme.border)
                    .text_sm()
                    .text_color(theme.text)
                    .child(self.t(Key::DrumEditorEmpty))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(button(
                                "drum-empty-gm",
                                self.t(Key::DrumEditorGmTemplate),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, _, _, cx| {
                                    this.apply_drum_map_template(track, DrumMap::general_midi());
                                    cx.notify();
                                }),
                            ))
                            .when(!self.drum_maps.entries().is_empty(), |row| {
                                row.child(button(
                                    "drum-empty-saved",
                                    self.t(Key::DrumEditorChooseSaved),
                                    ButtonStyle::Normal,
                                    false,
                                    theme.accent,
                                    &theme,
                                    cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                                        let menu = this.drum_map_menu(event.position());
                                        this.open_menu(menu);
                                        cx.notify();
                                    }),
                                ))
                            })
                            .child(button(
                                "drum-empty-learn",
                                self.t(Key::DrumEditorMidiLearn),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, _, _, cx| {
                                    this.begin_drum_learn(track, None);
                                    cx.notify();
                                }),
                            ))
                            .child(button(
                                "drum-empty-add",
                                self.t(Key::DrumEditorAddLane),
                                ButtonStyle::Primary,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(move |this, _, _, cx| {
                                    this.prompt_for_new_drum_lane(track);
                                    cx.notify();
                                }),
                            )),
                    ),
            )
            .into_any_element()
    }

    fn drum_map_menu(&self, at: Point<Pixels>) -> ContextMenu {
        let Some(track) = self.selected_drum_track() else {
            return ContextMenu::new(at, self.t(Key::DrumEditorMap));
        };
        let mut menu = ContextMenu::new(at, self.t(Key::DrumEditorMap))
            .item(
                self.t(Key::DrumEditorAddLane),
                MenuCommand::NewDrumLane(track),
            )
            .item(
                self.t(Key::DrumEditorMidiLearn),
                MenuCommand::LearnDrumLane { track, lane: None },
            )
            .item(
                self.t(Key::DrumEditorGmTemplate),
                MenuCommand::ApplyGeneralMidiDrumMap(track),
            );
        if !self.drum_maps.entries().is_empty() {
            menu = menu.separator();
            for (index, saved) in self.drum_maps.entries().iter().enumerate() {
                menu = menu.item(
                    saved.name.clone(),
                    MenuCommand::ApplySavedDrumMap { track, index },
                );
            }
        }

        let selected = self.drum_editor.selected_lane.and_then(|id| {
            self.session
                .drum_assignments(track)?
                .lanes
                .into_iter()
                .find(|lane| lane.id == id)
        });
        if let Some(lane) = selected {
            menu = menu
                .separator()
                .item(
                    self.t(Key::DrumLaneName),
                    MenuCommand::RenameDrumLane {
                        track,
                        lane: lane.id,
                    },
                )
                .item(
                    self.t(Key::DrumLaneMidi),
                    MenuCommand::SetDrumLaneNote {
                        track,
                        lane: lane.id,
                        move_existing_hits: false,
                    },
                )
                .item(
                    self.t(Key::DrumLaneMidiMove),
                    MenuCommand::SetDrumLaneNote {
                        track,
                        lane: lane.id,
                        move_existing_hits: true,
                    },
                )
                .item(
                    self.t(Key::DrumEditorMidiLearn),
                    MenuCommand::LearnDrumLane {
                        track,
                        lane: Some(lane.id),
                    },
                )
                .item(
                    self.t(Key::MenuMoveUp),
                    MenuCommand::MoveDrumLane {
                        track,
                        lane: lane.id,
                        offset: -1,
                    },
                )
                .item(
                    self.t(Key::MenuMoveDown),
                    MenuCommand::MoveDrumLane {
                        track,
                        lane: lane.id,
                        offset: 1,
                    },
                )
                .separator();
            for role in DrumRole::ALL {
                menu = menu.toggle(
                    format!("{}: {}", self.t(Key::DrumLaneRoles), self.t(role_key(role))),
                    MenuCommand::ToggleDrumLaneRole {
                        track,
                        lane: lane.id,
                        role,
                    },
                    lane.roles.contains(&role),
                );
            }
            menu = menu.separator().item(
                self.t(Key::DrumLaneRemove),
                MenuCommand::RemoveDrumLane {
                    track,
                    lane: lane.id,
                },
            );
        }
        menu
    }

    pub(crate) fn prompt_for_new_drum_lane(&mut self, track: TrackId) {
        let used: BTreeSet<_> = self
            .session
            .drum_lanes(track)
            .unwrap_or_default()
            .into_iter()
            .map(|lane| lane.note)
            .collect();
        let Some(note) = (0..=127).find(|note| !used.contains(note)) else {
            self.set_failed_status("Every MIDI note already has a lane".to_string());
            return;
        };
        self.open_prompt(Prompt::new(
            self.t(Key::DrumEditorAddLane),
            PromptTarget::NewDrumLane(track),
            note.to_string(),
        ));
    }

    pub(crate) fn prompt_to_rename_drum_lane(&mut self, track: TrackId, lane: u64) {
        let current = self
            .session
            .drum_lanes(track)
            .unwrap_or_default()
            .into_iter()
            .find(|candidate| candidate.id == lane)
            .map(|lane| lane.name)
            .unwrap_or_default();
        self.open_prompt(Prompt::new(
            self.t(Key::DrumLaneName),
            PromptTarget::DrumLaneName { track, lane },
            current,
        ));
    }

    pub(crate) fn prompt_for_drum_lane_note(
        &mut self,
        track: TrackId,
        lane: u64,
        move_existing_hits: bool,
    ) {
        let current = self
            .session
            .drum_lanes(track)
            .unwrap_or_default()
            .into_iter()
            .find(|candidate| candidate.id == lane)
            .map(|lane| lane.note.to_string())
            .unwrap_or_default();
        self.open_prompt(Prompt::new(
            self.t(if move_existing_hits {
                Key::DrumLaneMidiMove
            } else {
                Key::DrumLaneMidi
            }),
            PromptTarget::DrumLaneNote {
                track,
                lane,
                move_existing_hits,
            },
            current,
        ));
    }

    pub(crate) fn begin_drum_learn(&mut self, track: TrackId, lane: Option<u64>) {
        self.drum_editor.learn = Some(DrumLearn { track, lane });
        self.drum_editor.mode = DrumRowsMode::All;
        self.session.set_musical_typing(true);
        self.set_status(self.t(Key::DrumEditorLearning));
    }

    pub(crate) fn accept_drum_learn(&mut self, pitch: u8) -> bool {
        let Some(target) = self.drum_editor.learn else {
            return false;
        };
        let result = match target.lane {
            Some(lane) => self
                .session
                .set_drum_lane_note(target.track, lane, pitch, false)
                .map(|_| lane),
            None => {
                if let Some(lane) = self
                    .session
                    .drum_lanes(target.track)
                    .unwrap_or_default()
                    .into_iter()
                    .find(|lane| lane.note == pitch)
                {
                    Ok(lane.id)
                } else {
                    self.session
                        .add_drum_lane(target.track, pitch, String::new())
                }
            }
        };
        match result {
            Ok(lane) => {
                self.drum_editor.selected_lane = Some(lane);
                self.drum_editor.learn = None;
                self.remember_drum_map(target.track);
                self.set_status(self.t(Key::DrumEditorAutoSaved));
            }
            Err(error) => self.set_failed_status(error.to_string()),
        }
        true
    }

    pub(crate) fn cancel_drum_learn(&mut self) -> bool {
        self.drum_editor.learn.take().is_some()
    }

    pub(crate) fn remember_drum_map(&mut self, track: TrackId) {
        let Some(source) = self.session.drum_map_source(track) else {
            return;
        };
        let Some(map) = self.session.drum_assignments(track) else {
            return;
        };
        let name = self
            .project()
            .track(track)
            .map(|entry| entry.name.clone())
            .unwrap_or_else(|| self.t(Key::DrumEditorMap).to_string());
        if !self
            .drum_maps
            .keep(name.clone(), source.clone(), map.clone())
            || cfg!(test)
        {
            return;
        }
        match auris_session::DrumMapBook::keep_saved(name, source, map) {
            Ok(_) => self.drum_maps = auris_session::DrumMapBook::load(),
            Err(error) => self.set_failed_status(error.to_string()),
        }
    }

    /// Restores the user-level map for a newly selected sound source, when one exists.
    pub(crate) fn restore_drum_map_for_source(&mut self, track: TrackId) {
        let Some(source) = self.session.drum_map_source(track) else {
            return;
        };
        let map = self
            .drum_maps
            .map_for(&source)
            .map(|saved| saved.map.clone())
            .or_else(|| self.session.suggested_drum_map(track));
        let Some(map) = map else {
            return;
        };
        if let Err(error) = self.session.set_drum_source_map(track, map) {
            self.set_failed_status(error.to_string());
        }
    }

    pub(crate) fn apply_drum_map_template(&mut self, track: TrackId, map: DrumMap) {
        match self.session.set_drum_map(track, map) {
            Ok(_) => {
                self.drum_editor.mode = DrumRowsMode::Map;
                self.drum_editor.selected_lane = None;
                self.remember_drum_map(track);
                self.set_status(self.t(Key::DrumEditorAutoSaved));
            }
            Err(error) => self.set_failed_status(error.to_string()),
        }
    }

    pub(crate) fn apply_saved_drum_map(&mut self, track: TrackId, index: usize) {
        let Some(saved) = self.drum_maps.entries().get(index).cloned() else {
            return;
        };
        self.apply_drum_map_template(track, saved.map);
    }

    pub(crate) fn move_drum_lane(&mut self, track: TrackId, lane: u64, offset: i32) {
        match self.session.move_drum_lane(track, lane, offset) {
            Ok(_) => self.remember_drum_map(track),
            Err(error) => self.set_failed_status(error.to_string()),
        }
    }

    pub(crate) fn toggle_drum_lane_role(&mut self, track: TrackId, lane: u64, role: DrumRole) {
        let enabled = self
            .session
            .drum_lanes(track)
            .unwrap_or_default()
            .into_iter()
            .find(|candidate| candidate.id == lane)
            .is_none_or(|candidate| !candidate.roles.contains(&role));
        match self.session.set_drum_lane_role(track, lane, role, enabled) {
            Ok(_) => self.remember_drum_map(track),
            Err(error) => self.set_failed_status(error.to_string()),
        }
    }

    pub(crate) fn remove_drum_lane(&mut self, track: TrackId, lane: u64) {
        match self.session.remove_drum_lane(track, lane) {
            Ok(_) => {
                self.drum_editor.selected_lane = None;
                self.remember_drum_map(track);
            }
            Err(error) => self.set_failed_status(error.to_string()),
        }
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
        rows_for(&map, pitches, self.drum_editor.mode)
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
            self.drum_editor.selected_lane = None;
            self.drum_editor.learn = None;
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
        let theme = self.theme.clone();
        let labels: Vec<String> = rows
            .iter()
            .map(|row| {
                if !row.name.is_empty() {
                    format!("{} · {}", row.name, row.pitch)
                } else if row.roles.is_empty() {
                    self.general_midi_drum_name(row.pitch).map_or_else(
                        || format!("MIDI {}", row.pitch),
                        |name| format!("{name} · {}", row.pitch),
                    )
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
        let selected_lane = self.drum_editor.selected_lane;
        let track = self.selected_drum_track();
        let label_elements = rows
            .iter()
            .zip(labels.iter())
            .enumerate()
            .filter_map(|(index, (row, label))| {
                let top = index as f32 * ROW_HEIGHT - scroll;
                if top + ROW_HEIGHT < 0.0 || top > height {
                    return None;
                }
                let pitch = row.pitch;
                let lane = row.lane;
                let label = label.clone();
                let selected = lane.is_some() && lane == selected_lane;
                let handle = match (track, lane) {
                    (Some(track), Some(lane)) => Some(
                        div()
                            .id(("drum-lane-handle", lane))
                            .w(px(14.0))
                            .flex_shrink_0()
                            .text_color(theme.text_muted)
                            .child("≡")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.drum_editor.selected_lane = Some(lane);
                                    this.begin_drag(Drag::DrumLaneReorder { track, lane });
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            )
                            .into_any_element(),
                    ),
                    _ => None,
                };
                let mut element = div()
                    .id(("drum-row", u64::from(pitch)))
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(top))
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .px_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.text)
                    .when(selected, |row| row.bg(theme.accent_soft))
                    .children(handle)
                    .child(div().flex_1().min_w_0().truncate().child(label.clone()))
                    .tooltip(keyed_tip(label, "", &theme));
                if let Some(track) = track {
                    element = element
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                if this.accept_drum_learn(pitch) {
                                    cx.stop_propagation();
                                    cx.notify();
                                    return;
                                }
                                this.drum_editor.selected_lane = lane;
                                if event.click_count >= 2 {
                                    match lane {
                                        Some(lane) => this.prompt_to_rename_drum_lane(track, lane),
                                        None => this.open_prompt(Prompt::new(
                                            this.t(Key::DrumEditorAddLane),
                                            PromptTarget::NewDrumLane(track),
                                            pitch.to_string(),
                                        )),
                                    }
                                } else {
                                    this.audition(pitch);
                                }
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                this.drum_editor.selected_lane = lane;
                                if lane.is_some() {
                                    let menu = this.drum_map_menu(event.position);
                                    this.open_menu(menu);
                                } else {
                                    this.open_prompt(Prompt::new(
                                        this.t(Key::DrumEditorAddLane),
                                        PromptTarget::NewDrumLane(track),
                                        pitch.to_string(),
                                    ));
                                }
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        );
                }
                Some(element.into_any_element())
            })
            .collect::<Vec<_>>();
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
        let empty_state = (source && rows.is_empty()).then(|| self.drum_empty_state(cx));

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
                        "drum-map-notes",
                        self.t(Key::DrumEditorMap),
                        ButtonStyle::Ghost,
                        self.drum_editor.mode == DrumRowsMode::Map,
                        theme.accent_soft,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.drum_editor.mode = DrumRowsMode::Map;
                            this.drum_editor.scroll = 0.0;
                            cx.notify();
                        }),
                    ))
                    .child(button(
                        "drum-used-notes",
                        self.t(Key::DrumEditorUsed),
                        ButtonStyle::Ghost,
                        self.drum_editor.mode == DrumRowsMode::Used,
                        theme.accent_soft,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.drum_editor.mode = DrumRowsMode::Used;
                            this.drum_editor.scroll = 0.0;
                            cx.notify();
                        }),
                    ))
                    .child(button(
                        "drum-all-notes",
                        self.t(Key::DrumEditorAllNotes),
                        ButtonStyle::Ghost,
                        self.drum_editor.mode == DrumRowsMode::All,
                        theme.accent_soft,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.drum_editor.mode = DrumRowsMode::All;
                            this.drum_editor.scroll = 0.0;
                            cx.notify();
                        }),
                    ))
                    .when(source, |row| row.child(self.drum_map_actions(cx)))
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
                            .relative()
                            .bg(theme.surface_raised)
                            .children(label_elements)
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
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_hidden()
                            .tooltip(keyed_tip(self.t(Key::DrumRepeatPaint), "", &theme))
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
                            .children(empty_state)
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
            let mut visited = BTreeSet::new();
            if let Some(index) = under
                && let Some(note) = self
                    .session
                    .midi_clip(clip)
                    .and_then(|clip| clip.notes.get(index))
                    .cloned()
            {
                visited.insert((note.pitch, note.start));
            }
            self.begin_drag(Drag::DrumPaint {
                clip,
                erase: true,
                visited,
                last: None,
            });
            if let Some(index) = under {
                let _ = self.session.remove_notes(clip, &[index]);
                self.selected_notes.clear();
            } else {
                self.paint_drum_hits(event.position);
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
                grabbed: index,
                origin_tick: tick - clip_start,
                origin_pitch: pitch,
                origins: self.selected_note_origins(clip),
                pressed_at: Some(event.position),
            });
            self.audition_note(index, pitch);
        } else if event.modifiers.shift {
            self.begin_rubber_band(BandSurface::Roll, event.position, true);
        } else {
            self.selected_notes.clear();
            self.begin_drag(Drag::DrumPaint {
                clip,
                erase: false,
                visited: BTreeSet::new(),
                last: None,
            });
            self.paint_drum_hits(event.position);
        }
        cx.notify();
    }

    /// Fills every grid cell crossed by the active drum paint stroke.
    pub(crate) fn paint_drum_hits(&mut self, at: Point<Pixels>) {
        let Some(Drag::DrumPaint {
            clip, erase, last, ..
        }) = self.drag.clone()
        else {
            return;
        };
        let origin = self.roll_origin();
        let Some(pitch) = self.drum_pitch_at(at.y - origin.y) else {
            return;
        };
        let Some(clip_start) = self.session.midi_clip(clip).map(|clip| clip.start) else {
            return;
        };
        let grid = Ticks(self.project().grid.raw().max(1));
        let tick = self.timeline.x_to_tick(at.x - origin.x);
        let current = (tick.snap_nearest(grid) - clip_start).max_zero();
        let mut cells = Vec::new();
        if let Some((last_pitch, previous)) = last
            && last_pitch == pitch
        {
            let (from, to) = if previous <= current {
                (previous, current)
            } else {
                (current, previous)
            };
            let mut at = from;
            while at <= to {
                cells.push(at);
                at += grid;
            }
        } else {
            cells.push(current);
        }

        for start in cells {
            let seen = matches!(
                &self.drag,
                Some(Drag::DrumPaint { visited, .. }) if visited.contains(&(pitch, start))
            );
            if seen {
                continue;
            }
            if erase {
                let indices = self
                    .session
                    .midi_clip(clip)
                    .map(|clip| {
                        clip.notes
                            .iter()
                            .enumerate()
                            .filter(|(_, note)| note.pitch == pitch && note.start == start)
                            .map(|(index, _)| index)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if !indices.is_empty() {
                    let _ = self.session.remove_notes(clip, &indices);
                    self.selected_notes.clear();
                }
            } else {
                let exists = self.session.midi_clip(clip).is_some_and(|clip| {
                    clip.notes
                        .iter()
                        .any(|note| note.pitch == pitch && note.start == start)
                });
                if !exists
                    && let Ok(index) = self.session.add_note(clip, Note::new(pitch, start, grid))
                {
                    self.selected_notes.insert(index);
                    self.audition_note(index, pitch);
                }
            }
            if let Some(Drag::DrumPaint { visited, .. }) = &mut self.drag {
                visited.insert((pitch, start));
            }
        }
        if let Some(Drag::DrumPaint { last, .. }) = &mut self.drag {
            *last = Some((pitch, current));
        }
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

    /// Reorders the dragged lane to the mapped row currently under the pointer.
    pub(crate) fn reorder_drum_lane_at(&mut self, track: TrackId, lane: u64, y: Pixels) {
        let rows = self.drum_rows();
        let Some(row) = row_at(y, self.drum_editor.scroll, rows.len()) else {
            return;
        };
        let Some(target) = rows[row].lane else {
            return;
        };
        let Some(map) = self.session.drum_assignments(track) else {
            return;
        };
        let Some(from) = map.lanes.iter().position(|candidate| candidate.id == lane) else {
            return;
        };
        let Some(to) = map
            .lanes
            .iter()
            .position(|candidate| candidate.id == target)
        else {
            return;
        };
        let _ = self
            .session
            .move_drum_lane(track, lane, to as i32 - from as i32);
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
        let map = DrumMap::from_voices([
            (DrumRole::Kick, 73),
            (DrumRole::Snare, 18),
            (DrumRole::Tom, 18),
        ]);
        let rows = rows_for(&map, [91, 73], DrumRowsMode::Map);
        assert_eq!(
            rows.iter().map(|row| row.pitch).collect::<Vec<_>>(),
            [73, 18, 91]
        );
        assert_eq!(rows[1].roles, [DrumRole::Snare, DrumRole::Tom]);
        assert!(rows[2].roles.is_empty());
        assert_eq!(rows_for(&map, [], DrumRowsMode::All).len(), 128);
    }

    #[test]
    fn map_and_used_views_preserve_authored_names_and_order() {
        let mut map = DrumMap::default();
        map.add_lane(73, "Short Guiro");
        map.add_lane(18, "Machine rim");
        let mapped = rows_for(&map, [91, 18], DrumRowsMode::Map);
        assert_eq!(
            mapped
                .iter()
                .map(|row| (row.pitch, row.name.as_str()))
                .collect::<Vec<_>>(),
            [(73, "Short Guiro"), (18, "Machine rim"), (91, "")]
        );
        let used = rows_for(&map, [91, 18], DrumRowsMode::Used);
        assert_eq!(
            used.iter().map(|row| row.pitch).collect::<Vec<_>>(),
            [18, 91]
        );
    }

    #[test]
    fn missing_assignments_do_not_invent_kit_roles() {
        let rows = rows_for(&DrumMap::default(), [36, 60], DrumRowsMode::Map);
        assert!(rows.iter().all(|row| row.roles.is_empty()));
        assert_eq!(
            rows.iter().map(|row| row.pitch).collect::<Vec<_>>(),
            [36, 60]
        );
        assert!(rows_for(&DrumMap::default(), [], DrumRowsMode::Map).is_empty());
        assert_eq!(
            rows_for(&DrumMap::default(), [36], DrumRowsMode::Used).len(),
            1
        );
        assert_eq!(
            rows_for(&DrumMap::default(), [], DrumRowsMode::All).len(),
            128
        );
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
    fn an_empty_grid_drag_paints_repeated_hits_as_one_edit(cx: &mut TestAppContext) {
        let (app, cx, _, clip) = fixture(cx);
        app.update(cx, |this, _| this.session.set_grid(Ticks::QUARTER));
        paint(&app, cx);
        let from = hit_point(&app, cx, Ticks::QUARTER, 36);
        let to = hit_point(&app, cx, Ticks::QUARTER * 3, 36);
        drag(cx, from, to);
        app.update(cx, |this, _| {
            assert_eq!(
                this.session
                    .midi_clip(clip)
                    .unwrap()
                    .notes
                    .iter()
                    .map(|note| (note.pitch, note.start, note.length))
                    .collect::<Vec<_>>(),
                [
                    (36, Ticks::QUARTER, Ticks::QUARTER),
                    (36, Ticks::QUARTER * 2, Ticks::QUARTER),
                    (36, Ticks::QUARTER * 3, Ticks::QUARTER),
                ]
            );
            this.session.undo();
            assert!(this.session.midi_clip(clip).unwrap().notes.is_empty());
            this.session.redo();
            assert_eq!(this.session.midi_clip(clip).unwrap().notes.len(), 3);
        });
        paint(&app, cx);
        harness::drag_with(cx, from, to, deleting());
        app.update(cx, |this, _| {
            assert!(this.session.midi_clip(clip).unwrap().notes.is_empty());
            this.session.undo();
            assert_eq!(this.session.midi_clip(clip).unwrap().notes.len(), 3);
        });
    }

    #[gpui::test]
    fn midi_learn_adds_an_unassigned_lane_and_can_be_cancelled(cx: &mut TestAppContext) {
        let (app, cx, track, _) = fixture(cx);
        app.update(cx, |this, _| {
            this.begin_drum_learn(track, None);
            assert_eq!(this.drum_editor.mode, DrumRowsMode::All);
            assert!(this.accept_drum_learn(73));
            let lane = this
                .session
                .drum_lanes(track)
                .unwrap()
                .into_iter()
                .find(|lane| lane.note == 73)
                .expect("the learned key becomes a lane");
            assert_eq!(this.drum_editor.selected_lane, Some(lane.id));
            assert!(lane.roles.is_empty());
            this.begin_drum_learn(track, Some(lane.id));
            assert!(this.cancel_drum_learn());
            assert!(!this.cancel_drum_learn());
        });
    }

    #[gpui::test]
    fn an_empty_map_offers_starters_and_all_three_row_views(cx: &mut TestAppContext) {
        let (app, cx, track, _) = fixture(cx);
        app.update(cx, |this, _| {
            this.session
                .set_drum_map(track, DrumMap::default())
                .unwrap();
        });
        paint(&app, cx);
        assert!(cx.debug_bounds("drum-empty-gm").is_some());
        assert!(cx.debug_bounds("drum-empty-learn").is_some());
        assert!(cx.debug_bounds("drum-empty-add").is_some());
        app.read_with(cx, |this, _| assert!(this.drum_rows().is_empty()));

        harness::click("drum-all-notes", cx);
        app.read_with(cx, |this, _| assert_eq!(this.drum_rows().len(), 128));
        harness::click("drum-used-notes", cx);
        app.read_with(cx, |this, _| assert!(this.drum_rows().is_empty()));
        harness::click("drum-map-notes", cx);
        app.read_with(cx, |this, _| assert!(this.drum_rows().is_empty()));
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
            assert!(
                rows.iter()
                    .any(|row| { row.pitch == 73 && row.roles == vec![DrumRole::Kick] })
            );
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
