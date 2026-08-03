//! Right-click menus: what each component offers, where the menu lands, and what a choice does.
//!
//! Items carry a [`MenuCommand`] rather than a closure. A menu is then plain data — it can be
//! built where the component knows what was clicked, placed and drawn somewhere else entirely,
//! and checked by a test without a window.

use auris_i18n::{Key, messages};
use auris_session::prelude::*;

use gpui::{
    AnyElement, Context, IntoElement, MouseButton, MouseDownEvent, Pixels, Point, SharedString,
    Size, Window, div, point, prelude::*, px, size,
};

use crate::app::AurisApp;
use crate::theme::Metrics;
use crate::ui::icons::{Icon, icon};
use crate::ui::prompt::{Prompt, PromptTarget};

/// Height of one row.
const ITEM_HEIGHT: Pixels = px(22.0);
/// Height taken by the rule between two groups, including the space either side of it.
const SEPARATOR_HEIGHT: Pixels = px(7.0);
/// Height of the heading naming what the menu acts on.
const TITLE_HEIGHT: Pixels = px(20.0);
/// Padding above the first row and below the last.
const PADDING: Pixels = px(4.0);
/// The menu's own border, which sits inside the width and height it is given.
const BORDER: Pixels = px(1.0);
/// Width of the column holding the tick on a latched item.
const MARK_WIDTH: f32 = 18.0;
/// Narrowest a menu may be.
const MIN_WIDTH: f32 = 168.0;
/// Widest a menu may be.
const MAX_WIDTH: f32 = 300.0;
/// Rough advance width of one character at the menu's text size.
///
/// Only used to pick a width — the labels themselves are truncated, so an over- or
/// under-estimate costs a little whitespace rather than a clipped word.
const CHARACTER_WIDTH: f32 = 6.6;

/// What choosing a menu item does.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuCommand {
    /// Copy a track, its clips and its effects.
    DuplicateTrack(TrackId),
    /// Rename a track.
    RenameTrack(TrackId),
    /// Delete a track.
    DeleteTrack(TrackId),
    /// Silence or unsilence a track.
    ToggleTrackMute(TrackId),
    /// Solo or unsolo a track.
    ToggleTrackSolo(TrackId),
    /// Append an instrument track.
    NewInstrumentTrack,
    /// Append an audio track.
    NewAudioTrack,

    /// Copy a clip, immediately after the original.
    DuplicateClip(ClipId),
    /// Rename a clip.
    RenameClip(ClipId),
    /// Delete a clip.
    DeleteClip(ClipId),
    /// Silence or unsilence a clip.
    ToggleClipMute(ClipId),
    /// Divide a clip at the playhead.
    SplitClipAtPlayhead(ClipId),
    /// Set the cycle region to a clip's extent.
    LoopOverClip(ClipId),
    /// Open a clip in the piano roll.
    EditClip(ClipId),
    /// Create an empty clip on a track.
    NewClip {
        /// Track to create it on.
        track: TrackId,
        /// Where it starts.
        start: Ticks,
    },

    /// Copy the selected notes.
    DuplicateNotes,
    /// Delete the selected notes.
    DeleteNotes,
    /// Shift the selected notes in pitch.
    TransposeNotes(i32),
    /// Select every note in the clip being edited.
    SelectAllNotes,
    /// Add one note.
    NewNote {
        /// Pitch to add it at.
        pitch: u8,
        /// Where it starts, relative to the clip.
        start: Ticks,
    },

    /// Bypass or re-enable an effect.
    ToggleEffect {
        /// Track owning the chain, or `None` for the master bus.
        track: Option<TrackId>,
        /// Slot to bypass.
        slot: EffectSlotId,
    },
    /// Move an effect along its chain.
    MoveEffect {
        /// Track owning the chain, or `None` for the master bus.
        track: Option<TrackId>,
        /// Slot to move.
        slot: EffectSlotId,
        /// How far, in positions.
        delta: isize,
    },
    /// Remove an effect.
    RemoveEffect(EffectSlotId),
    /// Show the plugin browser, aimed at a chain.
    BrowsePlugins {
        /// Track to add to, or `None` for the master bus.
        track: Option<TrackId>,
    },

    /// Move the cycle region's start.
    SetLoopStart(Ticks),
    /// Move the cycle region's end.
    SetLoopEnd(Ticks),
    /// Turn cycling on or off.
    ToggleLoop,
    /// Remove the cycle region.
    ClearLoop,
}

/// One row in a menu.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    /// Text shown in the row.
    pub label: SharedString,
    /// What choosing it does.
    pub command: MenuCommand,
    /// A greyed-out row explains that an action exists but does not apply right now, which is
    /// more use than hiding it and leaving the user wondering where it went.
    pub enabled: bool,
    /// Whether the row shows a tick, for the items that latch.
    pub checked: bool,
}

/// A row in a menu.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuEntry {
    /// A choice.
    Item(MenuItem),
    /// The rule between two groups.
    Separator,
}

/// An open context menu.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextMenu {
    /// Where the pointer was when it opened, in window coordinates.
    pub anchor: Point<Pixels>,
    /// What the menu acts on, shown as a heading.
    pub title: SharedString,
    /// The rows.
    pub entries: Vec<MenuEntry>,
}

impl ContextMenu {
    /// An empty menu anchored at `anchor`.
    pub fn new(anchor: Point<Pixels>, title: impl Into<SharedString>) -> Self {
        Self {
            anchor,
            title: title.into(),
            entries: Vec::new(),
        }
    }

    /// Adds a row.
    pub fn item(self, label: impl Into<SharedString>, command: MenuCommand) -> Self {
        self.push(label, command, true, false)
    }

    /// Adds a row that is only usable when `enabled`.
    pub fn item_if(
        self,
        enabled: bool,
        label: impl Into<SharedString>,
        command: MenuCommand,
    ) -> Self {
        self.push(label, command, enabled, false)
    }

    /// Adds a row that shows a tick when `checked`.
    pub fn toggle(
        self,
        label: impl Into<SharedString>,
        command: MenuCommand,
        checked: bool,
    ) -> Self {
        self.push(label, command, true, checked)
    }

    /// Adds the rule between two groups.
    pub fn separator(mut self) -> Self {
        // Never leads, never doubles: a menu built from conditional groups would otherwise show
        // a rule against its own top edge or two rules in a row.
        if matches!(self.entries.last(), Some(MenuEntry::Item(_))) {
            self.entries.push(MenuEntry::Separator);
        }
        self
    }

    fn push(
        mut self,
        label: impl Into<SharedString>,
        command: MenuCommand,
        enabled: bool,
        checked: bool,
    ) -> Self {
        self.entries.push(MenuEntry::Item(MenuItem {
            label: label.into(),
            command,
            enabled,
            checked,
        }));
        self
    }

    /// `true` when the menu has nothing to show.
    pub fn is_empty(&self) -> bool {
        !self
            .entries
            .iter()
            .any(|entry| matches!(entry, MenuEntry::Item(_)))
    }

    /// How large the menu will be drawn.
    pub fn size(&self) -> Size<Pixels> {
        let widest = self
            .entries
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Item(item) => Some(item.label.chars().count()),
                MenuEntry::Separator => None,
            })
            .max()
            .unwrap_or(0);
        let width =
            (widest as f32 * CHARACTER_WIDTH + MARK_WIDTH + 24.0).clamp(MIN_WIDTH, MAX_WIDTH);
        let height = self.entries.iter().fold(
            TITLE_HEIGHT + PADDING * 2.0 + BORDER * 2.0,
            |total, entry| {
                total
                    + match entry {
                        MenuEntry::Item(_) => ITEM_HEIGHT,
                        MenuEntry::Separator => SEPARATOR_HEIGHT,
                    }
            },
        );
        size(px(width), height)
    }

    /// Where the menu's top-left corner goes inside a window of `viewport`.
    ///
    /// A menu that would overflow is flipped to the other side of the pointer rather than merely
    /// pushed back inside: pushing it back leaves it under the pointer, where it swallows the
    /// click the user is about to make.
    pub fn origin(&self, viewport: Size<Pixels>) -> Point<Pixels> {
        let size = self.size();
        let x = if self.anchor.x + size.width > viewport.width && self.anchor.x >= size.width {
            self.anchor.x - size.width
        } else {
            self.anchor
                .x
                .min((viewport.width - size.width).max(px(0.0)))
        };
        let y = if self.anchor.y + size.height > viewport.height && self.anchor.y >= size.height {
            self.anchor.y - size.height
        } else {
            self.anchor
                .y
                .min((viewport.height - size.height).max(px(0.0)))
        };
        point(x, y)
    }
}

impl AurisApp {
    /// Shows a menu, unless it has nothing to offer.
    pub(crate) fn open_menu(&mut self, menu: ContextMenu) {
        if !menu.is_empty() {
            self.menu = Some(menu);
        }
    }

    /// Closes any open menu, reporting whether there was one.
    pub(crate) fn close_menu(&mut self) -> bool {
        self.menu.take().is_some()
    }

    /// Draws the open menu over everything else.
    pub(crate) fn render_context_menu(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let theme = self.theme.clone();
        let size = menu.size();
        let origin = menu.origin(window.viewport_size());

        let rows: Vec<AnyElement> = menu
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| match entry {
                MenuEntry::Separator => div()
                    .my(px(3.0))
                    .h(px(1.0))
                    .w_full()
                    .flex_shrink_0()
                    // The subtle border is a shade off the raised surface the menu is drawn on,
                    // which makes the rule invisible exactly where it has a job to do.
                    .bg(theme.border)
                    .into_any_element(),
                MenuEntry::Item(item) => {
                    let command = item.command.clone();
                    let enabled = item.enabled;
                    div()
                        .id(("menu-item", index))
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .h(ITEM_HEIGHT)
                        .px_1p5()
                        .rounded(Metrics::RADIUS_SM)
                        .text_xs()
                        .text_color(if enabled {
                            theme.text
                        } else {
                            theme.text_faint
                        })
                        .when(enabled, |this| {
                            this.cursor_pointer().hover(|this| {
                                this.bg(theme.accent).text_color(theme.text_on_accent)
                            })
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(MARK_WIDTH))
                                .flex_shrink_0()
                                .when(item.checked, |this| {
                                    // Not the accent colour: the hover state fills the row with
                                    // it, and a tick would vanish exactly when it is pointed at.
                                    this.child(icon(Icon::Check, px(10.0), theme.text))
                                }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(item.label.clone()),
                        )
                        .when(enabled, |this| {
                            this.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                    let command = command.clone();
                                    this.close_menu();
                                    this.run_menu_command(command, cx);
                                    cx.notify();
                                }),
                            )
                        })
                        .into_any_element()
                }
            })
            .collect();

        Some(
            // A full-window backdrop, so a click anywhere else dismisses the menu the way a
            // native one does. It is transparent: this is a menu, not a modal.
            div()
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.close_menu();
                        cx.notify();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.close_menu();
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .left(origin.x)
                        .top(origin.y)
                        .w(size.width)
                        .h(size.height)
                        .flex()
                        .flex_col()
                        .p(PADDING)
                        .rounded(Metrics::RADIUS_MD)
                        .bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border)
                        .shadow_lg()
                        // Clicks inside the menu must not reach the backdrop behind it, or the
                        // menu would close before the row underneath the pointer could act.
                        .on_mouse_down(
                            MouseButton::Left,
                            |_: &MouseDownEvent, _, cx: &mut gpui::App| cx.stop_propagation(),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .h(TITLE_HEIGHT)
                                .px_1p5()
                                .text_xs()
                                .text_color(theme.text_faint)
                                .truncate()
                                .child(menu.title.clone()),
                        )
                        .children(rows),
                )
                .into_any_element(),
        )
    }

    /// Carries out a menu choice.
    pub(crate) fn run_menu_command(&mut self, command: MenuCommand, cx: &mut Context<Self>) {
        match command {
            MenuCommand::DuplicateTrack(track) => match self.session.duplicate_track(track) {
                Ok(copy) => {
                    self.select_track(copy);
                    self.set_status(self.t(Key::DuplicatedTrack));
                }
                Err(error) => self.set_status(self.failure(Key::MenuDuplicate, &error)),
            },
            MenuCommand::RenameTrack(track) => {
                let name = self
                    .project()
                    .track(track)
                    .map(|track| track.name.clone())
                    .unwrap_or_default();
                let title = self.t(Key::RenameTrackTitle);
                self.open_prompt(Prompt::new(title, PromptTarget::Track(track), name));
            }
            MenuCommand::DeleteTrack(track) => {
                self.select_track(track);
                self.delete_selected_track();
            }
            MenuCommand::ToggleTrackMute(track) => self.toggle_mute(track),
            MenuCommand::ToggleTrackSolo(track) => self.toggle_solo(track),
            MenuCommand::NewInstrumentTrack => self.add_instrument_track(),
            MenuCommand::NewAudioTrack => self.add_audio_track(),

            MenuCommand::DuplicateClip(clip) => {
                let mut copies = std::collections::BTreeSet::new();
                let mut failure = None;
                for source in self.clips_for_command(clip) {
                    match self.session.duplicate_clip(source) {
                        Ok(copy) => {
                            copies.insert(copy);
                        }
                        Err(error) => failure = Some(error),
                    }
                }
                match failure {
                    Some(error) => self.set_status(self.failure(Key::MenuDuplicate, &error)),
                    None => {
                        // The copies become the selection, so dragging straight afterwards moves
                        // the new material rather than the original.
                        self.select_clips(copies, None);
                        self.selected_notes.clear();
                        self.set_status(self.t(Key::DuplicatedClip));
                    }
                }
            }
            MenuCommand::RenameClip(clip) => {
                let name = self
                    .clip_name(clip)
                    .map(|name| name.to_string())
                    .unwrap_or_default();
                let title = self.t(Key::RenameClipTitle);
                self.open_prompt(Prompt::new(title, PromptTarget::Clip(clip), name));
            }
            MenuCommand::DeleteClip(clip) => {
                let doomed = self.clips_for_command(clip);
                if self.session.remove_clips(&doomed).is_ok() {
                    self.select_clip(None);
                    self.selected_notes.clear();
                }
            }
            MenuCommand::ToggleClipMute(clip) => {
                let muted = self.clip_is_muted(clip);
                let _ = self.session.set_clip_muted(clip, !muted);
            }
            MenuCommand::SplitClipAtPlayhead(clip) => {
                let at = self.playhead_ticks();
                match self.session.split_clip(clip, at) {
                    Ok(right) => {
                        self.select_clip(Some(right));
                        self.selected_notes.clear();
                        self.set_status(self.t(Key::SplitClipStatus));
                    }
                    Err(error) => self.set_status(self.failure(Key::MenuSplitAtPlayhead, &error)),
                }
            }
            MenuCommand::LoopOverClip(clip) => {
                if let Some((start, end)) = self.clip_extent(clip) {
                    self.session.set_loop_region(start, end);
                    self.session.set_loop_enabled(true);
                }
            }
            MenuCommand::EditClip(clip) => self.open_clip_in_editor(clip),
            MenuCommand::NewClip { track, start } => self.create_clip_at(track, start),

            MenuCommand::DuplicateNotes => {
                let Some(clip) = self.selected_clip else {
                    return;
                };
                let chosen: Vec<usize> = self.selected_notes.iter().copied().collect();
                if let Ok(copies) = self.session.duplicate_notes(clip, &chosen) {
                    // The copies become the selection, so the same command can be run again to
                    // lay out a third and a fourth.
                    self.selected_notes = copies.into_iter().collect();
                }
            }
            MenuCommand::DeleteNotes => self.delete_selection(),
            MenuCommand::TransposeNotes(semitones) => {
                let Some(clip) = self.selected_clip else {
                    return;
                };
                let chosen: Vec<usize> = self.selected_notes.iter().copied().collect();
                let _ = self.session.transpose_notes(clip, &chosen, semitones);
            }
            MenuCommand::SelectAllNotes => {
                let count = self
                    .selected_midi_clip()
                    .map(|clip| clip.notes.len())
                    .unwrap_or(0);
                self.selected_notes = (0..count).collect();
            }
            MenuCommand::NewNote { pitch, start } => {
                let Some(clip) = self.selected_clip else {
                    return;
                };
                let length = Ticks(self.project().grid.raw().max(1));
                if let Ok(index) = self.session.add_note(clip, Note::new(pitch, start, length)) {
                    self.selected_notes.clear();
                    self.selected_notes.insert(index);
                }
            }

            MenuCommand::ToggleEffect { track, slot } => self.toggle_effect(track, slot),
            MenuCommand::MoveEffect { track, slot, delta } => self.move_effect(track, slot, delta),
            MenuCommand::RemoveEffect(slot) => self.remove_effect(slot),
            MenuCommand::BrowsePlugins { track } => {
                // The library adds to whatever is selected, so aim the selection first.
                self.selected_track = track;
                self.panels.library_visible = true;
            }

            MenuCommand::SetLoopStart(tick) => {
                let end = self
                    .project()
                    .loop_region
                    .map(|(_, end)| end)
                    .unwrap_or(tick);
                self.session.set_loop_region(tick, end.max(tick));
                self.session.set_loop_enabled(true);
            }
            MenuCommand::SetLoopEnd(tick) => {
                let start = self
                    .project()
                    .loop_region
                    .map(|(start, _)| start)
                    .unwrap_or(Ticks::ZERO);
                self.session.set_loop_region(start.min(tick), tick);
                self.session.set_loop_enabled(true);
            }
            MenuCommand::ToggleLoop => self.toggle_loop(),
            MenuCommand::ClearLoop => {
                self.session.set_loop_enabled(false);
                self.session.set_loop_region(Ticks::ZERO, Ticks::ZERO);
            }
        }
        cx.notify();
    }

    /// The menu for a track, shown by its header and its mixer strip.
    pub(crate) fn track_menu(&self, anchor: Point<Pixels>, track: TrackId) -> ContextMenu {
        let Some(entry) = self.project().track(track) else {
            return self.arrangement_menu(anchor);
        };
        ContextMenu::new(anchor, entry.name.clone())
            .item(
                self.t(Key::MenuDuplicateTrack),
                MenuCommand::DuplicateTrack(track),
            )
            .item(self.t(Key::MenuRename), MenuCommand::RenameTrack(track))
            .item(self.t(Key::CmdDeleteTrack), MenuCommand::DeleteTrack(track))
            .separator()
            .toggle(
                self.t(Key::Mute),
                MenuCommand::ToggleTrackMute(track),
                entry.mixer.mute,
            )
            .toggle(
                self.t(Key::Solo),
                MenuCommand::ToggleTrackSolo(track),
                entry.mixer.solo,
            )
            .item(
                self.t(Key::MenuAddEffect),
                MenuCommand::BrowsePlugins { track: Some(track) },
            )
            .separator()
            .item(
                self.t(Key::MenuNewInstrumentTrack),
                MenuCommand::NewInstrumentTrack,
            )
            .item(self.t(Key::MenuNewAudioTrack), MenuCommand::NewAudioTrack)
    }

    /// The menu for a clip in the arrangement.
    pub(crate) fn clip_menu(&self, anchor: Point<Pixels>, clip: ClipId) -> ContextMenu {
        // With several clips selected the menu acts on all of them, so it says so rather than
        // naming one and quietly taking the rest with it.
        let name = if self.selected_clips.len() > 1 && self.selected_clips.contains(&clip) {
            messages::clip_count(self.language(), self.selected_clips.len())
        } else {
            self.clip_name(clip)
                .unwrap_or_else(|| self.t(Key::PianoRoll))
                .to_string()
        };
        let playhead = self.playhead_ticks();
        let splittable = self
            .clip_extent(clip)
            .is_some_and(|(start, end)| playhead > start && playhead < end);
        let is_midi = self.session.midi_clip(clip).is_some();

        ContextMenu::new(anchor, name)
            .item(self.t(Key::MenuDuplicate), MenuCommand::DuplicateClip(clip))
            .item(self.t(Key::MenuRename), MenuCommand::RenameClip(clip))
            .item(self.t(Key::MenuDelete), MenuCommand::DeleteClip(clip))
            .separator()
            .item_if(
                splittable,
                self.t(Key::MenuSplitAtPlayhead),
                MenuCommand::SplitClipAtPlayhead(clip),
            )
            .toggle(
                self.t(Key::MenuMuteClip),
                MenuCommand::ToggleClipMute(clip),
                self.clip_is_muted(clip),
            )
            .item(
                self.t(Key::MenuCycleOverClip),
                MenuCommand::LoopOverClip(clip),
            )
            .separator()
            .item_if(
                is_midi,
                self.t(Key::MenuEditInPianoRoll),
                MenuCommand::EditClip(clip),
            )
    }

    /// The menu for an empty spot in a track's lane.
    pub(crate) fn lane_menu(
        &self,
        anchor: Point<Pixels>,
        track: TrackId,
        start: Ticks,
    ) -> ContextMenu {
        let Some(entry) = self.project().track(track) else {
            return self.arrangement_menu(anchor);
        };
        let is_instrument = entry.kind.as_instrument().is_some();
        ContextMenu::new(anchor, entry.name.clone())
            .item_if(
                is_instrument,
                self.t(Key::MenuNewClipHere),
                MenuCommand::NewClip { track, start },
            )
            .separator()
            .item(
                self.t(Key::MenuDuplicateTrack),
                MenuCommand::DuplicateTrack(track),
            )
            .item(
                self.t(Key::MenuRenameTrack),
                MenuCommand::RenameTrack(track),
            )
            .item(self.t(Key::CmdDeleteTrack), MenuCommand::DeleteTrack(track))
            .separator()
            .item(
                self.t(Key::MenuNewInstrumentTrack),
                MenuCommand::NewInstrumentTrack,
            )
            .item(self.t(Key::MenuNewAudioTrack), MenuCommand::NewAudioTrack)
    }

    /// The menu for the arrangement below the last track.
    pub(crate) fn arrangement_menu(&self, anchor: Point<Pixels>) -> ContextMenu {
        ContextMenu::new(anchor, self.t(Key::MenuArrangement))
            .item(
                self.t(Key::MenuNewInstrumentTrack),
                MenuCommand::NewInstrumentTrack,
            )
            .item(self.t(Key::MenuNewAudioTrack), MenuCommand::NewAudioTrack)
    }

    /// The menu for the bar ruler.
    pub(crate) fn ruler_menu(&self, anchor: Point<Pixels>, tick: Ticks) -> ContextMenu {
        ContextMenu::new(anchor, self.t(Key::MenuCycleTitle))
            .item(
                self.t(Key::MenuCycleStartHere),
                MenuCommand::SetLoopStart(tick),
            )
            .item(self.t(Key::MenuCycleEndHere), MenuCommand::SetLoopEnd(tick))
            .separator()
            .toggle(
                self.t(Key::MenuCycleTitle),
                MenuCommand::ToggleLoop,
                self.project().loop_enabled,
            )
            .item_if(
                self.project().loop_region.is_some(),
                self.t(Key::MenuClearCycle),
                MenuCommand::ClearLoop,
            )
    }

    /// The menu for a note, or for empty space in the piano roll.
    pub(crate) fn roll_menu(
        &self,
        anchor: Point<Pixels>,
        under_pointer: Option<usize>,
        pitch: u8,
        start: Ticks,
    ) -> ContextMenu {
        let Some(clip) = self.selected_clip else {
            return ContextMenu::new(anchor, self.t(Key::PianoRoll));
        };
        let selected = self.selected_notes.len();
        let title = match (under_pointer, selected) {
            (Some(_), 0 | 1) => self.t(Key::MenuNote).to_string(),
            (_, count) if count > 1 => messages::note_count(self.language(), count),
            _ => self
                .clip_name(clip)
                .unwrap_or_else(|| self.t(Key::PianoRoll))
                .to_string(),
        };
        let has_selection = selected > 0;

        ContextMenu::new(anchor, title)
            .item_if(
                has_selection,
                self.t(Key::MenuDuplicate),
                MenuCommand::DuplicateNotes,
            )
            .item_if(
                has_selection,
                self.t(Key::MenuDelete),
                MenuCommand::DeleteNotes,
            )
            .separator()
            .item_if(
                has_selection,
                self.t(Key::MenuOctaveUp),
                MenuCommand::TransposeNotes(12),
            )
            .item_if(
                has_selection,
                self.t(Key::MenuOctaveDown),
                MenuCommand::TransposeNotes(-12),
            )
            .item_if(
                has_selection,
                self.t(Key::MenuSemitoneUp),
                MenuCommand::TransposeNotes(1),
            )
            .item_if(
                has_selection,
                self.t(Key::MenuSemitoneDown),
                MenuCommand::TransposeNotes(-1),
            )
            .separator()
            .item_if(
                under_pointer.is_none(),
                self.t(Key::MenuAddNoteHere),
                MenuCommand::NewNote { pitch, start },
            )
            .item(self.t(Key::MenuSelectAllNotes), MenuCommand::SelectAllNotes)
            .separator()
            .item(self.t(Key::MenuRenameClip), MenuCommand::RenameClip(clip))
    }

    /// The menu for one effect in a chain.
    pub(crate) fn effect_menu(
        &self,
        anchor: Point<Pixels>,
        track: Option<TrackId>,
        slot: EffectSlotId,
        name: impl Into<SharedString>,
    ) -> ContextMenu {
        let enabled = self.session.effect_enabled(track, slot).unwrap_or(true);
        ContextMenu::new(anchor, name)
            .toggle(
                self.t(Key::MenuEnabled),
                MenuCommand::ToggleEffect { track, slot },
                enabled,
            )
            .separator()
            .item(
                self.t(Key::MenuMoveUp),
                MenuCommand::MoveEffect {
                    track,
                    slot,
                    delta: -1,
                },
            )
            .item(
                self.t(Key::MenuMoveDown),
                MenuCommand::MoveEffect {
                    track,
                    slot,
                    delta: 1,
                },
            )
            .item(self.t(Key::MenuRemove), MenuCommand::RemoveEffect(slot))
            .separator()
            .item(
                self.t(Key::MenuAddEffect),
                MenuCommand::BrowsePlugins { track },
            )
    }

    /// The clips a menu command should act on.
    ///
    /// A command aimed at a clip inside the selection takes the whole selection with it, which
    /// is what selecting several of them was for; one aimed elsewhere acts alone.
    fn clips_for_command(&self, clip: ClipId) -> Vec<ClipId> {
        if self.selected_clips.contains(&clip) {
            self.selected_clips.iter().copied().collect()
        } else {
            vec![clip]
        }
    }

    /// The name of a clip of either kind.
    pub(crate) fn clip_name(&self, clip: ClipId) -> Option<&str> {
        if let Some(midi) = self.session.midi_clip(clip) {
            return Some(&midi.name);
        }
        self.audio_clip(clip).map(|clip| clip.name.as_str())
    }

    fn clip_is_muted(&self, clip: ClipId) -> bool {
        if let Some(midi) = self.session.midi_clip(clip) {
            return midi.muted;
        }
        self.audio_clip(clip).is_some_and(|clip| clip.muted)
    }

    /// Where a clip of either kind starts and ends on the timeline.
    fn clip_extent(&self, clip: ClipId) -> Option<(Ticks, Ticks)> {
        if let Some(midi) = self.session.midi_clip(clip) {
            return Some((midi.start, midi.end()));
        }
        let audio = self.audio_clip(clip)?;
        Some((
            audio.start,
            audio.start + self.audio_clip_length_ticks(audio),
        ))
    }

    fn audio_clip(&self, clip: ClipId) -> Option<&AudioClip> {
        self.project().tracks.iter().find_map(|track| {
            track
                .kind
                .as_audio()?
                .clips
                .iter()
                .find(|candidate| candidate.id == clip)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu(anchor: Point<Pixels>, items: usize) -> ContextMenu {
        (0..items).fold(ContextMenu::new(anchor, "Track 1"), |menu, index| {
            menu.item(format!("Item {index}"), MenuCommand::NewAudioTrack)
        })
    }

    #[test]
    fn a_menu_that_fits_opens_at_the_pointer() {
        let anchor = point(px(100.0), px(80.0));
        let menu = menu(anchor, 4);
        assert_eq!(menu.origin(size(px(1200.0), px(800.0))), anchor);
    }

    #[test]
    fn a_menu_near_an_edge_flips_to_the_other_side_of_the_pointer() {
        let viewport = size(px(400.0), px(300.0));
        let menu = menu(point(px(390.0), px(290.0)), 6);
        let size = menu.size();
        let origin = menu.origin(viewport);

        assert_eq!(origin.x, px(390.0) - size.width);
        assert_eq!(origin.y, px(290.0) - size.height);
        assert!(
            origin.x + size.width <= px(390.0) && origin.y + size.height <= px(290.0),
            "a flipped menu must clear the pointer, or it swallows the next click"
        );
    }

    #[test]
    fn a_menu_larger_than_the_window_still_starts_on_screen() {
        // Too tall to flip and too tall to fit: the top edge is what matters, because that is
        // where the first item is.
        let menu = menu(point(px(10.0), px(20.0)), 40);
        let origin = menu.origin(size(px(400.0), px(200.0)));
        assert_eq!(origin, point(px(10.0), px(0.0)));
    }

    #[test]
    fn separators_never_lead_or_double_up() {
        let built = ContextMenu::new(point(px(0.0), px(0.0)), "Track")
            .separator()
            .item("One", MenuCommand::NewAudioTrack)
            .separator()
            .separator()
            .item("Two", MenuCommand::NewAudioTrack);
        assert_eq!(
            built.entries,
            vec![
                MenuEntry::Item(MenuItem {
                    label: "One".into(),
                    command: MenuCommand::NewAudioTrack,
                    enabled: true,
                    checked: false,
                }),
                MenuEntry::Separator,
                MenuEntry::Item(MenuItem {
                    label: "Two".into(),
                    command: MenuCommand::NewAudioTrack,
                    enabled: true,
                    checked: false,
                }),
            ]
        );
    }

    #[test]
    fn a_menu_of_separators_alone_counts_as_empty() {
        let empty = ContextMenu::new(point(px(0.0), px(0.0)), "Nothing").separator();
        assert!(empty.is_empty());
        assert!(!menu(point(px(0.0), px(0.0)), 1).is_empty());
    }

    #[test]
    fn the_height_matches_what_gets_drawn() {
        let built = ContextMenu::new(point(px(0.0), px(0.0)), "Track")
            .item("One", MenuCommand::NewAudioTrack)
            .separator()
            .item("Two", MenuCommand::NewAudioTrack);
        assert_eq!(
            built.size().height,
            TITLE_HEIGHT + PADDING * 2.0 + BORDER * 2.0 + ITEM_HEIGHT * 2.0 + SEPARATOR_HEIGHT
        );
    }
}
