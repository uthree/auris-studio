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
use crate::dock::{Dock, Panel};
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
    /// Tint a track with a palette entry.
    SetTrackColor(TrackId, Color),
    /// Drop every recipe on a track, so nothing on it is written again.
    FreezeTrack(TrackId),
    /// Show a track's automation lane on a parameter, or hide it when it is already showing that
    /// one.
    ///
    /// One lane per track rather than a stack of them: choosing a second parameter swaps what the
    /// row draws. That is the whole difference between this and Logic, and it is the difference
    /// between a row and a panel — the room a stack needs is room the arrangement does not have
    /// until a track can be folded.
    ShowAutomation(TrackId, ParamTarget),
    /// Take a parameter's lane away, giving it back its stored value.
    ClearAutomation(ParamTarget),
    /// Append an instrument track.
    NewInstrumentTrack,
    /// Append an audio track.
    NewAudioTrack,
    /// Append a bus.
    NewBusTrack,
    /// Set the song sheet's time signature.
    ///
    /// These eight turn the *sheet* rather than the document: nothing they set has been written
    /// until Write is pressed, so none of them records an undo step.
    SongMeter(u32, u32),
    /// Set the song sheet's mood from a named feeling.
    SongMood(&'static str),
    /// Set what one section of the song sheet plays, by chart name or catalogue name.
    SongSectionChords {
        /// Which section, by position in the sheet's list.
        section: usize,
        /// A progression the song already carries, or one the catalogue knows.
        name: String,
    },
    /// Write out the progression one section of the song sheet plays.
    SongWriteProgression(usize),
    /// Keep the progression one section plays, in the book that outlives this song.
    SongKeepProgression(usize),
    /// Move one section of the song sheet away from the key.
    SongSectionTranspose {
        /// Which section, by position in the sheet's list.
        section: usize,
        /// How far, in semitones.
        steps: i32,
    },
    /// Point one place in the song sheet's form at a section.
    SongFormName {
        /// Which place in the order.
        place: usize,
        /// The section it should play.
        name: String,
    },
    /// Add a playing of a section to the song sheet's form.
    SongAddSection {
        /// After which place in the order.
        place: usize,
        /// The section to play.
        name: String,
    },
    /// Set the song sheet's drum groove.
    SongGroove(&'static str),
    /// Set the role of one part on the song sheet.
    SongPartRole {
        /// Which part, by position in the roster.
        part: usize,
        /// What it plays.
        role: Role,
    },
    /// Set the instrument of one part on the song sheet.
    SongPartInstrument {
        /// Which part, by position in the roster.
        part: usize,
        /// The plugin's registry id.
        id: String,
    },
    /// Set the octave of one part on the song sheet.
    SongPartOctave {
        /// Which part, by position in the roster.
        part: usize,
        /// Which octave it sits in.
        octave: i32,
    },
    /// Set which MIDI note one drum part of the song sheet strikes.
    SongPartNote {
        /// Which part, by position in the roster.
        part: usize,
        /// The note it strikes.
        note: u8,
    },
    /// Add a part of this role to the song sheet's roster.
    SongAddPart(Role),
    /// Open the list of places a track's output could go.
    ShowOutputPicker {
        /// Track being routed.
        track: TrackId,
        /// Where the list is dropped.
        at: Point<Pixels>,
    },
    /// Point a track's output at a bus, or back at the master.
    SetTrackOutput(TrackId, Output),
    /// Open the list of buses a track could send to.
    ShowSendPicker {
        /// Track the send would come from.
        track: TrackId,
        /// Where the list is dropped.
        at: Point<Pixels>,
    },
    /// Add a send from a track to a bus.
    AddSend {
        /// Track the copy is taken from.
        track: TrackId,
        /// Bus it feeds.
        bus: TrackId,
    },
    /// Remove a send from a track.
    RemoveSend {
        /// Track the send is on.
        track: TrackId,
        /// Which send.
        send: SendId,
    },
    /// Move a send's tap before or after the fader.
    ToggleSendPreFader {
        /// Track the send is on.
        track: TrackId,
        /// Which send.
        send: SendId,
    },

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
    /// Sets how hard the selected notes are struck, as a dynamic marking's MIDI velocity.
    SetNoteVelocity(u8),
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
    /// Open the list of effects, aimed at one particular strip.
    ShowEffectPicker {
        /// Strip to add to, or `None` for the master bus.
        track: Option<TrackId>,
        /// Where to put the menu.
        at: Point<Pixels>,
    },
    /// Add one effect to one strip.
    AddEffect {
        /// Strip to add to, or `None` for the master bus.
        track: Option<TrackId>,
        /// Registry id of the effect.
        effect_id: String,
    },

    /// Move the cycle region's start.
    SetLoopStart(Ticks),
    /// Move the cycle region's end.
    SetLoopEnd(Ticks),
    /// Turn cycling on or off.
    ToggleLoop,
    /// Remove the cycle region.
    ClearLoop,

    /// Open the list of named progressions, aimed at a position.
    ShowProgressionPicker {
        /// Where the first bar of the chosen progression goes.
        at: Ticks,
        /// Where to put the menu.
        anchor: Point<Pixels>,
    },
    /// Write a catalogue progression onto the timeline.
    StampProgression {
        /// Catalogue name, such as `axis` or `marusa`.
        name: &'static str,
        /// Where the first bar of it goes.
        at: Ticks,
    },
    /// Type the key that takes effect at a position.
    SetKeyAt(Ticks),
    /// Remove the key change at a position.
    RemoveKeyAt(Ticks),
    /// Type the tempo that runs from a position.
    SetTempoAt(Ticks),
    /// Remove the tempo change in force at a position.
    RemoveTempoAt(Ticks),
    /// Turn the meter of the stretch a position falls in.
    SetSignature(Ticks, TimeSignature),
    /// Type a meter the list does not offer, for the stretch a position falls in.
    TypeSignature,
    /// Write a signature change at the bar a position falls in.
    SetSignatureAt(Ticks),
    /// Remove the signature change in force at a position.
    RemoveSignatureAt(Ticks),
    /// Type an audio clip's own gain.
    ClipGain(ClipId),
    /// Remove an audio clip's fades.
    ClearFades(ClipId),
    /// Type the chord that sounds from a position.
    SetChordAt(Ticks),
    /// Remove the chord change at a position.
    RemoveChordAt(Ticks),
    /// Name the song section in force at a position.
    SetSectionAt(Ticks),
    /// Remove the section change in force at a position.
    RemoveSectionAt(Ticks),
    /// Leave the song unnamed from a position onwards — how a structure ends.
    EndSectionsAt(Ticks),
    /// Empty the chords over a range.
    ClearHarmony {
        /// Where the cleared stretch begins.
        from: Ticks,
        /// Where it ends, and the music resumes.
        to: Ticks,
    },

    /// Open the list of presets, aimed at a place on a track.
    ShowPresetPicker {
        /// Track the clip would go on.
        track: TrackId,
        /// Where it would start.
        start: Ticks,
        /// Where to put the menu.
        anchor: Point<Pixels>,
    },
    /// Write a clip from the harmony under it.
    GenerateClip {
        /// Track to write it on.
        track: TrackId,
        /// Where it starts.
        start: Ticks,
        /// What the clip should be.
        preset: ClipPreset,
    },
    /// Write a generated clip's notes again, from the harmony as it now stands.
    RegenerateClip(ClipId),
    /// Write another take of a generated clip.
    RerollClip(ClipId),
    /// Keep a generated clip's notes and forget how they got there.
    FreezeClip(ClipId),
    /// Make a generated clip a different kind of part.
    SetClipPreset {
        /// Clip to rewrite.
        clip: ClipId,
        /// What it should be instead.
        preset: ClipPreset,
    },
    /// Give a generated drum clip a different groove.
    SetClipGroove {
        /// Clip to rewrite.
        clip: ClipId,
        /// Catalogue name, such as `basic-rock` or `shuffle`.
        groove: &'static str,
    },
    /// Divide a generated clip's beats a different way.
    SetClipSubdivision {
        /// Clip to rewrite.
        clip: ClipId,
        /// How finely the beat should divide.
        subdivision: Subdivision,
    },
    /// Move a generated clip's register.
    SetClipOctave {
        /// Clip to rewrite.
        clip: ClipId,
        /// Octaves from where the preset sits.
        octave: i32,
    },

    /// Move a panel to one of the window's edges.
    DockPanel {
        /// Panel to move.
        panel: Panel,
        /// Where it should live.
        dock: Dock,
    },
    /// Show or hide a panel.
    TogglePanel(Panel),

    /// Put a discrete plugin parameter on one of its named positions.
    ///
    /// The value rather than the index: a choice parameter stores its position as the number the
    /// plugin reads, and the menu is built where the descriptor is in hand.
    SetParamChoice {
        /// Which parameter of which plugin.
        target: ParamTarget,
        /// The position to take.
        value: f32,
    },
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
    /// A colour shown at the end of the row, for choices that *are* a colour.
    ///
    /// The row still carries text, so the keyboard and the eye have something to land on — but
    /// the swatch is what the choice actually is, and it is the same in every language. The
    /// alternative was naming eight palette entries twice over, in a set where two of them are
    /// both fairly called orange.
    pub swatch: Option<gpui::Hsla>,
}

/// A row in a menu.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuEntry {
    /// A choice.
    Item(MenuItem),
    /// The rule between two groups.
    Separator,
}

/// The dynamic markings the note menu offers, and the MIDI velocity each one means.
///
/// The usual mapping — six steps of about 20, centred so mf is a little above the middle. Not
/// translated: pp and ff are the notation, and a Japanese score prints them the same way.
const DYNAMICS: [(&str, u8); 6] = [
    ("pp", 24),
    ("p", 48),
    ("mp", 64),
    ("mf", 80),
    ("f", 100),
    ("ff", 120),
];

/// An open context menu.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextMenu {
    /// Where the pointer was when it opened, in window coordinates.
    pub anchor: Point<Pixels>,
    /// What the menu acts on, shown as a heading.
    pub title: SharedString,
    /// The rows.
    pub entries: Vec<MenuEntry>,
    /// The row the keyboard is on, once the keyboard has been used.
    ///
    /// `None` until then, so a menu opened with the mouse does not draw a highlight nobody asked
    /// for — and so the first arrow key lands on the first row rather than the second.
    pub highlighted: Option<usize>,
}

impl ContextMenu {
    /// An empty menu anchored at `anchor`.
    pub fn new(anchor: Point<Pixels>, title: impl Into<SharedString>) -> Self {
        Self {
            anchor,
            title: title.into(),
            entries: Vec::new(),
            highlighted: None,
        }
    }

    /// Moves the highlight `delta` rows, skipping separators and anything disabled.
    ///
    /// Wraps, and starts from whichever end the direction implies, so the first Down lands on the
    /// first row and the first Up on the last. A menu with nothing to choose leaves it alone.
    pub fn step(&mut self, delta: isize) {
        let choosable: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| matches!(entry, MenuEntry::Item(item) if item.enabled))
            .map(|(index, _)| index)
            .collect();
        let Some(&first) = choosable.first() else {
            return;
        };
        let Some(&last) = choosable.last() else {
            return;
        };
        let Some(current) = self.highlighted else {
            self.highlighted = Some(if delta >= 0 { first } else { last });
            return;
        };
        // Where the current row sits among the choosable ones, or just before the next one down
        // when it is a separator that somehow held the highlight.
        let at = choosable
            .iter()
            .position(|index| *index == current)
            .unwrap_or(0) as isize;
        let count = choosable.len() as isize;
        let next = (at + delta).rem_euclid(count) as usize;
        self.highlighted = Some(choosable[next]);
    }

    /// The command the highlighted row would run.
    pub fn highlighted_command(&self) -> Option<MenuCommand> {
        match self.entries.get(self.highlighted?) {
            Some(MenuEntry::Item(item)) if item.enabled => Some(item.command.clone()),
            _ => None,
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

    /// Adds a run of rows that share one enabled condition.
    pub fn items_if<L: Into<SharedString>>(
        self,
        enabled: bool,
        rows: impl IntoIterator<Item = (L, MenuCommand)>,
    ) -> Self {
        rows.into_iter().fold(self, |menu, (label, command)| {
            menu.item_if(enabled, label, command)
        })
    }

    /// Adds a row that shows a tick when `checked` and is only usable when `enabled`.
    pub fn toggle_if(
        self,
        enabled: bool,
        label: impl Into<SharedString>,
        command: MenuCommand,
        checked: bool,
    ) -> Self {
        self.push(label, command, enabled, checked)
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
            swatch: None,
        }));
        self
    }

    /// Adds a row whose choice is a colour, shown as a swatch and ticked when it is the one in use.
    pub fn colour(
        mut self,
        label: impl Into<SharedString>,
        command: MenuCommand,
        swatch: gpui::Hsla,
        checked: bool,
    ) -> Self {
        self.entries.push(MenuEntry::Item(MenuItem {
            label: label.into(),
            command,
            enabled: true,
            checked,
            swatch: Some(swatch),
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
                        // The keyboard's row is drawn as though the pointer were on it, so Down
                        // and a hover mean the same thing on screen as they do to the menu.
                        .when(menu.highlighted == Some(index), |this| {
                            this.bg(theme.accent).text_color(theme.text_on_accent)
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
                        .children(item.swatch.map(|colour| {
                            div()
                                .flex_shrink_0()
                                .ml_2()
                                .w(px(22.0))
                                .h(px(10.0))
                                .rounded(Metrics::RADIUS_XS)
                                .bg(colour)
                        }))
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
                Err(error) => self.set_failed_status(self.failure(Key::MenuDuplicate, &error)),
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
            MenuCommand::SetTrackColor(track, color) => {
                let _ = self.session.set_track_color(track, color);
            }
            MenuCommand::FreezeTrack(track) => self.freeze_track(track),
            MenuCommand::ShowAutomation(track, target) => self.show_automation(track, target),
            MenuCommand::ClearAutomation(target) => {
                self.session.clear_automation(target);
            }
            MenuCommand::NewInstrumentTrack => self.add_instrument_track(),
            MenuCommand::NewAudioTrack => self.add_audio_track(),
            MenuCommand::NewBusTrack => self.add_bus_track(),
            MenuCommand::SongMeter(numerator, denominator) => {
                if let Some(dials) = self.song_sheet.as_mut() {
                    dials.meter = TimeSignature::new(numerator, denominator);
                }
            }
            MenuCommand::SongMood(name) => {
                if let (Some(dials), Some(mood)) = (self.song_sheet.as_mut(), Mood::named(name)) {
                    dials.mood = mood;
                }
            }
            MenuCommand::SongSectionChords { section, name } => {
                // The song's own charts and the catalogue first, then the book. Read before the
                // sheet is borrowed, because both live on `self`.
                let kept = self.progressions.chart(&name);
                if let Some(dials) = self.song_sheet.as_mut()
                    && !crate::ui::compose_sheet::set_section_chart(dials, section, &name)
                    && let Some(chart) = kept
                {
                    crate::ui::compose_sheet::give_section_chart(dials, section, &name, chart);
                }
            }
            MenuCommand::SongWriteProgression(section) => {
                let title = self.t(Key::SongChords);
                let current = self
                    .song_sheet
                    .as_ref()
                    .and_then(|dials| {
                        let held = dials.sections.get(section)?;
                        let (_, chart) =
                            dials.charts.iter().find(|(name, _)| name == &held.chords)?;
                        Some(chart.to_string())
                    })
                    .unwrap_or_default();
                self.open_prompt(crate::ui::prompt::Prompt::new(
                    title,
                    crate::ui::prompt::PromptTarget::SongSectionChart(section),
                    current,
                ));
            }
            MenuCommand::SongKeepProgression(section) => {
                let title = self.t(Key::SongKeepProgression);
                self.open_prompt(crate::ui::prompt::Prompt::new(
                    title,
                    crate::ui::prompt::PromptTarget::KeepProgression(section),
                    String::new(),
                ));
            }
            MenuCommand::SongSectionTranspose { section, steps } => {
                if let Some(section) = self
                    .song_sheet
                    .as_mut()
                    .and_then(|dials| dials.sections.get_mut(section))
                {
                    section.transpose = steps;
                }
            }
            MenuCommand::SongFormName { place, name } => {
                if let Some(dials) = self.song_sheet.as_mut() {
                    crate::ui::compose_sheet::set_form_entry(dials, place, &name);
                }
            }
            MenuCommand::SongAddSection { place, name } => {
                if let Some(dials) = self.song_sheet.as_mut() {
                    crate::ui::compose_sheet::add_to_form(dials, place, &name);
                }
            }
            MenuCommand::SongGroove(name) => {
                if let Some(dials) = self.song_sheet.as_mut() {
                    dials.groove = name.to_string();
                }
            }
            MenuCommand::SongPartRole { part, role } => {
                if let Some(part) = self
                    .song_sheet
                    .as_mut()
                    .and_then(|dials| dials.parts.get_mut(part))
                {
                    // Everything the role implies comes with it, and the name does not: the name
                    // is what the document keys its material by, and changing it under somebody
                    // would rewrite the part they were listening to.
                    let name = part.name.clone();
                    *part = PartSpec::of_role(name, role);
                }
            }
            MenuCommand::SongPartInstrument { part, id } => {
                if let Some(part) = self
                    .song_sheet
                    .as_mut()
                    .and_then(|dials| dials.parts.get_mut(part))
                {
                    part.instrument = id;
                }
            }
            MenuCommand::SongPartOctave { part, octave } => {
                if let Some(part) = self
                    .song_sheet
                    .as_mut()
                    .and_then(|dials| dials.parts.get_mut(part))
                {
                    part.octave = octave;
                }
            }
            MenuCommand::SongPartNote { part, note } => {
                if let Some(part) = self
                    .song_sheet
                    .as_mut()
                    .and_then(|dials| dials.parts.get_mut(part))
                {
                    part.note = Some(note);
                }
            }
            MenuCommand::SongAddPart(role) => {
                if let Some(dials) = self.song_sheet.as_mut() {
                    crate::ui::compose_sheet::add_part(dials, role);
                }
            }
            MenuCommand::ShowOutputPicker { track, at } => {
                let menu = self.output_menu(at, track);
                self.open_menu(menu);
            }
            MenuCommand::SetTrackOutput(track, output) => self.set_track_output(track, output),
            MenuCommand::ShowSendPicker { track, at } => {
                let menu = self.send_picker_menu(at, track);
                self.open_menu(menu);
            }
            MenuCommand::AddSend { track, bus } => self.add_send(track, bus),
            MenuCommand::RemoveSend { track, send } => self.remove_send(track, send),
            MenuCommand::ToggleSendPreFader { track, send } => {
                self.toggle_send_pre_fader(track, send)
            }

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
                    Some(error) => self.set_failed_status(self.failure(Key::MenuDuplicate, &error)),
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
                    Err(error) => {
                        self.set_failed_status(self.failure(Key::MenuSplitAtPlayhead, &error))
                    }
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
            MenuCommand::SetNoteVelocity(midi) => {
                let Some(clip) = self.selected_clip else {
                    return;
                };
                let chosen: Vec<usize> = self.selected_notes.iter().copied().collect();
                let _ = self
                    .session
                    .set_note_velocity(clip, &chosen, f32::from(midi) / 127.0);
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
            MenuCommand::ShowEffectPicker { track, at } => {
                let menu = self.effect_picker_menu(at, track);
                self.open_menu(menu);
            }
            MenuCommand::AddEffect { track, effect_id } => self.add_effect_to(track, &effect_id),

            MenuCommand::ShowProgressionPicker { at, anchor } => {
                let menu = self.progression_picker_menu(anchor, at);
                self.open_menu(menu);
            }
            MenuCommand::StampProgression { name, at } => {
                let bars =
                    progression_target(self.project().loop_region, at, &self.project().signatures)
                        .1;
                match self.session.stamp_named_progression(name, at, bars) {
                    Ok(chords) => self.set_status(messages::progression_written(
                        self.language(),
                        name,
                        chords,
                    )),
                    Err(error) => self.set_failed_status(self.failure(Key::MenuHarmony, &error)),
                }
            }
            MenuCommand::ShowPresetPicker {
                track,
                start,
                anchor,
            } => {
                let menu = self.preset_picker_menu(anchor, track, start);
                self.open_menu(menu);
            }
            MenuCommand::GenerateClip {
                track,
                start,
                preset,
            } => {
                let (start, length) = generation_range(
                    self.project().loop_region,
                    start,
                    &self.project().signatures,
                );
                let recipe = ClipRecipe::new(preset, self.next_seed());
                match self.session.generate_clip(track, start, length, recipe) {
                    Ok(clip) => {
                        self.select_clip(Some(clip));
                        self.report_clip(preset, clip);
                    }
                    Err(error) => {
                        self.set_failed_status(self.failure(Key::MenuGenerateClip, &error))
                    }
                }
            }
            MenuCommand::RegenerateClip(clip) => match self.session.regenerate_clip(clip) {
                Ok(_) => {
                    self.forget_rewritten_notes(clip);
                    self.report_clip_preset(clip);
                }
                Err(error) => self.set_failed_status(self.failure(Key::MenuRegenerateClip, &error)),
            },
            MenuCommand::RerollClip(clip) => self.reroll_clip(clip),
            MenuCommand::FreezeClip(clip) => self.freeze_clip(clip),
            MenuCommand::SetClipPreset { clip, preset } => self.set_clip_preset(clip, preset),
            MenuCommand::SetClipGroove { clip, groove } => self.set_clip_groove(clip, groove),
            MenuCommand::SetClipSubdivision { clip, subdivision } => {
                self.set_clip_subdivision(clip, subdivision)
            }
            MenuCommand::SetClipOctave { clip, octave } => self.set_clip_octave(clip, octave),
            MenuCommand::SetParamChoice { target, value } => self.session.set_param(target, value),

            MenuCommand::DockPanel { panel, dock } => self.dock_panel(panel, dock),
            MenuCommand::TogglePanel(panel) => self.toggle_panel(panel),

            MenuCommand::SetKeyAt(tick) => {
                let current = self.project().harmony.key_at(tick).to_text();
                let title = self.t(Key::SetKeyTitle);
                self.open_prompt(Prompt::new(title, PromptTarget::Key(tick), current));
            }
            MenuCommand::RemoveKeyAt(tick) => self.session.remove_key(tick),
            MenuCommand::SetTempoAt(tick) => self.prompt_for_tempo_from(tick),
            MenuCommand::RemoveTempoAt(tick) => self.session.remove_tempo_point(tick),
            MenuCommand::SetSignature(tick, signature) => {
                self.session.set_signature_at(tick, signature)
            }
            MenuCommand::TypeSignature => self.prompt_for_signature(),
            MenuCommand::SetSignatureAt(tick) => self.prompt_for_signature_from(tick),
            MenuCommand::RemoveSignatureAt(tick) => self.session.remove_signature_point(tick),
            MenuCommand::ClipGain(clip) => self.prompt_for_clip_gain(clip),
            MenuCommand::ClearFades(clip) => {
                let _ = self.session.set_clip_fades(clip, 0, 0);
            }
            MenuCommand::SetChordAt(tick) => {
                let current = self
                    .project()
                    .harmony
                    .numeral_at(tick)
                    .map(|numeral| numeral.to_string())
                    .unwrap_or_default();
                let title = self.t(Key::SetChordTitle);
                self.open_prompt(Prompt::new(title, PromptTarget::Chord(tick), current));
            }
            MenuCommand::RemoveChordAt(tick) => self.session.remove_chord(tick),
            MenuCommand::ClearHarmony { from, to } => self.session.clear_harmony(from, to),
            MenuCommand::SetSectionAt(tick) => self.prompt_for_section(tick),
            MenuCommand::RemoveSectionAt(tick) => self.session.remove_section(tick),
            MenuCommand::EndSectionsAt(tick) => self.session.set_section(tick, None),

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
        let showing = self.automation_lanes.get(&track).copied();
        // Every clip on this track that is still written from a recipe. Freezing is only offered
        // when there is something to freeze, and the count is what the status line reports back.
        let generated = entry
            .kind
            .as_instrument()
            .map(|inner| {
                inner
                    .clips
                    .iter()
                    .filter(|clip| clip.is_generated())
                    .count()
            })
            .unwrap_or(0);
        let current_color = entry.color;
        let menu = ContextMenu::new(anchor, entry.name.clone())
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
                MenuCommand::ShowEffectPicker {
                    track: Some(track),
                    at: anchor,
                },
            )
            .separator()
            .item(
                self.t(Key::MenuRouteTo),
                MenuCommand::ShowOutputPicker { track, at: anchor },
            )
            // Only where there is a bus at all: with none in the project the item can do nothing
            // but open an empty list. One that exists and cannot be sent to *is* worth opening,
            // because the greyed row is the answer to why.
            .item_if(
                self.session.buses().next().is_some(),
                self.t(Key::MenuAddSend),
                MenuCommand::ShowSendPicker { track, at: anchor },
            )
            .separator()
            .toggle(
                self.t(Key::MenuAutomateVolume),
                MenuCommand::ShowAutomation(track, ParamTarget::TrackGain(track)),
                showing == Some(ParamTarget::TrackGain(track)),
            )
            .toggle(
                self.t(Key::MenuAutomatePan),
                MenuCommand::ShowAutomation(track, ParamTarget::TrackPan(track)),
                showing == Some(ParamTarget::TrackPan(track)),
            );

        // Clearing is offered only for a lane that exists: on a parameter nobody has automated it
        // is an item that can only do nothing, and a menu full of those is a menu people stop
        // reading.
        let menu = match showing.filter(|target| self.session.is_automated(*target)) {
            Some(target) => menu.item(
                self.t(Key::MenuClearAutomation),
                MenuCommand::ClearAutomation(target),
            ),
            None => menu,
        };

        // Only for a track that has something to freeze. On one with no generated clips it is a
        // row that can only report zero.
        let menu = match generated > 0 {
            true => menu.separator().item(
                self.t(Key::MenuFreezeTrack),
                MenuCommand::FreezeTrack(track),
            ),
            false => menu,
        };

        // The palette, as swatches. The colours carry the meaning and the rows are numbered
        // rather than named: the set holds two entries a reasonable person would call orange, and
        // naming those twice over in two languages is an argument nobody needs to have. The word
        // is on every row because this menu has no section headings and one row saying what the
        // run below it is would have to be a disabled item pretending to be a heading.
        let colour = self.t(Key::MenuTrackColor);
        let menu =
            Color::PALETTE
                .iter()
                .enumerate()
                .fold(menu.separator(), |menu, (index, color)| {
                    menu.colour(
                        format!("{colour} {}", index + 1),
                        MenuCommand::SetTrackColor(track, *color),
                        self.theme.track_color(color.0),
                        *color == current_color,
                    )
                });

        menu.separator()
            .item(
                self.t(Key::MenuNewInstrumentTrack),
                MenuCommand::NewInstrumentTrack,
            )
            .item(self.t(Key::MenuNewAudioTrack), MenuCommand::NewAudioTrack)
            .item(self.t(Key::MenuNewBusTrack), MenuCommand::NewBusTrack)
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

        let menu = ContextMenu::new(anchor, name)
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
            // What only an audio clip has: its own gain, and fades to take back off.
            .item_if(
                !is_midi,
                self.t(Key::MenuClipGain),
                MenuCommand::ClipGain(clip),
            )
            .item_if(
                self.audio_clip_shape(clip)
                    .is_some_and(|(_, _, _, _, fade_in, fade_out)| fade_in > 0 || fade_out > 0),
                self.t(Key::MenuClearFades),
                MenuCommand::ClearFades(clip),
            )
            .item_if(
                is_midi,
                self.t(Key::MenuEditInPianoRoll),
                MenuCommand::EditClip(clip),
            );
        self.generated_clip_rows(menu, clip)
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
            .item_if(
                is_instrument,
                self.t(Key::MenuGenerateClip),
                MenuCommand::ShowPresetPicker {
                    track,
                    start,
                    anchor,
                },
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
            .item(self.t(Key::MenuNewBusTrack), MenuCommand::NewBusTrack)
    }

    /// The menu for the arrangement below the last track.
    pub(crate) fn arrangement_menu(&self, anchor: Point<Pixels>) -> ContextMenu {
        ContextMenu::new(anchor, self.t(Key::MenuArrangement))
            .item(
                self.t(Key::MenuNewInstrumentTrack),
                MenuCommand::NewInstrumentTrack,
            )
            .item(self.t(Key::MenuNewAudioTrack), MenuCommand::NewAudioTrack)
            .item(self.t(Key::MenuNewBusTrack), MenuCommand::NewBusTrack)
    }

    /// The menu for the bar ruler: the cycle above, the tempo below.
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
            .separator()
            .item(self.t(Key::MenuSetTempoHere), MenuCommand::SetTempoAt(tick))
            // Offered only where a change governs: the anchor at tick zero is the song's own
            // tempo, not a change, and cannot be removed.
            .item_if(
                self.project().tempo_map.change_at(tick) != Ticks::ZERO,
                self.t(Key::MenuRemoveTempoHere),
                MenuCommand::RemoveTempoAt(tick),
            )
            .separator()
            .item(
                self.t(Key::MenuSetSignatureHere),
                MenuCommand::SetSignatureAt(tick),
            )
            .item_if(
                self.project().signatures.change_at(tick) != Ticks::ZERO,
                self.t(Key::MenuRemoveSignatureHere),
                MenuCommand::RemoveSignatureAt(tick),
            )
    }

    /// The list of meters the transport's signature field drops.
    ///
    /// The common ones with a tick beside whichever is in force, then a way to type one the list
    /// does not hold, then — where a change governs rather than the song's own meter — a way to
    /// take it away. Turning one of these *replaces* the meter of the stretch the playhead is in;
    /// writing a new change part way through a song is the ruler's job, which is where a person
    /// can see the bar they are aiming at.
    pub(crate) fn signature_menu(&self, anchor: Point<Pixels>, at: Ticks) -> ContextMenu {
        let current = self.session.signature_at(at);
        let mut menu = ContextMenu::new(anchor, self.t(Key::Signature));
        for signature in TimeSignature::COMMON {
            menu = menu.toggle(
                signature.to_string(),
                MenuCommand::SetSignature(at, signature),
                signature == current,
            );
        }
        menu.separator()
            .item(self.t(Key::MenuOtherSignature), MenuCommand::TypeSignature)
            .item_if(
                self.project().signatures.change_at(at) != Ticks::ZERO,
                self.t(Key::MenuRemoveSignatureHere),
                MenuCommand::RemoveSignatureAt(at),
            )
    }

    /// Opens the signature sheet aimed at the bar `at` rounds to.
    ///
    /// The ruler's counterpart to [`Self::prompt_for_signature`], and the same shape as
    /// [`Self::prompt_for_tempo_from`]: the field comes up holding the meter already in force
    /// there, so the sheet reads as "the meter from here is —".
    pub(crate) fn prompt_for_signature_from(&mut self, at: Ticks) {
        let title = self.t(Key::SetSignatureTitle);
        let current = self.session.signature_at(at).to_string();
        self.open_prompt(Prompt::new(title, PromptTarget::SignatureFrom(at), current));
    }

    /// Opens the tempo sheet aimed at the beat `at` rounds to.
    ///
    /// The field comes up holding the tempo already in force there, so the sheet reads as "the
    /// tempo from here is —" whether the answer confirms it or changes it.
    pub(crate) fn prompt_for_tempo_from(&mut self, at: Ticks) {
        let title = self.t(Key::SetTempoTitle);
        let current = format!("{:.2}", self.project().tempo_map.bpm_at(at));
        self.open_prompt(Prompt::new(title, PromptTarget::TempoFrom(at), current));
    }

    /// Opens the sheet that takes an audio clip's gain in decibels.
    ///
    /// The field comes up holding the gain the clip already has, so nudging by a decibel is
    /// an edit of the number on screen rather than an act of memory.
    pub(crate) fn prompt_for_clip_gain(&mut self, clip: ClipId) {
        let Some((_, _, _, gain_db, _, _)) = self.audio_clip_shape(clip) else {
            return;
        };
        let title = self.t(Key::SetClipGainTitle);
        let current = format!("{gain_db:.1}");
        self.open_prompt(Prompt::new(title, PromptTarget::ClipGain(clip), current));
    }

    /// Opens the naming sheet for the section in force at `tick`.
    ///
    /// The field comes up holding the current name, so double-clicking a section renames it
    /// and double-clicking an empty stretch gives it its first one.
    pub(crate) fn prompt_for_section(&mut self, tick: Ticks) {
        let current = self
            .project()
            .sections
            .label_at(tick)
            .map(str::to_string)
            .unwrap_or_default();
        let title = self.t(Key::SetSectionTitle);
        self.open_prompt(Prompt::new(title, PromptTarget::Section(tick), current));
    }

    /// The menu for the structure lane.
    ///
    /// Headed by the section under the pointer when there is one — numbered the way the lane
    /// draws it — because that is what the items act on, wherever inside it the press landed.
    pub(crate) fn structure_menu(&self, anchor: Point<Pixels>, tick: Ticks) -> ContextMenu {
        let sections = &self.project().sections;
        let named = sections.label_at(tick).is_some();
        let title = match sections.section_at(tick) {
            Some((label, instance)) => {
                if sections.repeats(label) > 1 {
                    format!("{label} {instance}")
                } else {
                    label.to_string()
                }
            }
            None => self.t(Key::SetSectionTitle).to_string(),
        };

        ContextMenu::new(anchor, title)
            .item(
                self.t(Key::MenuSetSectionHere),
                MenuCommand::SetSectionAt(tick),
            )
            .item_if(
                named,
                self.t(Key::MenuRemoveSectionHere),
                MenuCommand::RemoveSectionAt(tick),
            )
            .item_if(
                named,
                self.t(Key::MenuEndSectionsHere),
                MenuCommand::EndSectionsAt(tick),
            )
    }

    /// The menu for the harmony lane, aimed at whatever the pointer is over.
    ///
    /// `tick` is where the pointer is, unrounded. What the chord items act on is the chord *in
    /// force* there rather than that position: a chord occupies everything up to the next change,
    /// so retyping or removing "the chord here" has to mean the one you can see, not one that
    /// happens to begin under the pixel. Only writing a chord where there is none falls back to
    /// the rounded position, which is what [`Session::snap_harmony`] decides.
    ///
    /// The range a progression is written across is the cycle region when there is one, and the
    /// chart's own length otherwise. That is the rule everywhere else in the application — set the
    /// cycle over the chorus, then act on it — and it saves inventing a "how many bars" field
    /// nothing else would use.
    pub(crate) fn harmony_menu(&self, anchor: Point<Pixels>, tick: Ticks) -> ContextMenu {
        let signatures = &self.project().signatures;
        let placed = self.session.snap_harmony(tick);
        let (from, bars) = progression_target(self.project().loop_region, placed, signatures);
        let harmony = &self.project().harmony;
        let target = harmony_target(harmony, tick, placed);

        // Naming the chord rather than the bar when there is one: this menu can retype or remove
        // it, and a heading reading "Harmony · bar 5" over an item that acts on the chord that
        // started in bar 3 would be pointing somewhere else entirely.
        let title = match harmony.chord_at(tick) {
            Some(chord) => messages::harmony_chord(
                self.language(),
                &harmony
                    .numeral_at(tick)
                    .map(|numeral| numeral.to_string())
                    .unwrap_or_default(),
                &chord.to_string(),
            ),
            None => messages::harmony_at_bar(self.language(), signatures.bar_of(placed)),
        };

        ContextMenu::new(anchor, title)
            .item(
                self.t(Key::MenuSetChordHere),
                MenuCommand::SetChordAt(target.chord),
            )
            .item_if(
                target.sounding,
                self.t(Key::MenuRemoveChordHere),
                MenuCommand::RemoveChordAt(tick),
            )
            .item(self.t(Key::MenuSetKeyHere), MenuCommand::SetKeyAt(placed))
            .item_if(
                target.removable_key,
                self.t(Key::MenuRemoveKeyHere),
                MenuCommand::RemoveKeyAt(tick),
            )
            .separator()
            .item(
                self.t(Key::MenuWriteProgression),
                MenuCommand::ShowProgressionPicker { at: from, anchor },
            )
            .item_if(
                !harmony.is_empty(),
                self.t(Key::MenuClearHarmony),
                MenuCommand::ClearHarmony {
                    from,
                    // Counted off the ruler, so clearing "four bars from here" clears the four
                    // the ruler shows even where one of them is in a different meter.
                    to: signatures.bar_start(signatures.bar_of(from) + bars.max(1) as u32),
                },
            )
    }

    /// Every preset, aimed at one place on one track.
    pub(crate) fn preset_picker_menu(
        &self,
        anchor: Point<Pixels>,
        track: TrackId,
        start: Ticks,
    ) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::MenuGenerateClip));
        for preset in ClipPreset::ALL {
            menu = menu.item(
                self.t(preset_key(preset)),
                MenuCommand::GenerateClip {
                    track,
                    start,
                    preset,
                },
            );
        }
        menu
    }

    /// Every preset, aimed at a clip that already has one.
    ///
    /// Ticks the one it is now: this menu is opened from a button showing that same name, and a
    /// list of six with nothing marked would leave the reader checking the button behind it.
    pub(crate) fn clip_preset_menu(&self, anchor: Point<Pixels>, clip: ClipId) -> ContextMenu {
        let current = self.session.clip_recipe(clip).map(|recipe| recipe.preset);
        let mut menu = ContextMenu::new(anchor, self.t(Key::PartPreset));
        for preset in ClipPreset::ALL {
            menu = menu.toggle(
                self.t(preset_key(preset)),
                MenuCommand::SetClipPreset { clip, preset },
                current == Some(preset),
            );
        }
        menu
    }

    /// The named positions of a discrete parameter, aimed at one plugin's copy of it.
    ///
    /// A choice used to cycle on click, which is the right control for two positions and the wrong
    /// one for eight: reaching the pulse wave meant counting, and overshooting meant going round
    /// again. A menu names them all and ticks the one in force, which is what the preset and groove
    /// pickers already do with the same kind of question.
    pub(crate) fn param_choice_menu(
        &self,
        anchor: Point<Pixels>,
        target: ParamTarget,
        descriptor: &ParamDescriptor,
    ) -> ContextMenu {
        let current = self.session.param_value(target, descriptor);
        let mut menu = ContextMenu::new(anchor, self.param_label(&descriptor.name));
        for index in 0..descriptor.choices.len() {
            let value = crate::ui::plugin_editor::discrete_value(descriptor, index);
            menu = menu.toggle(
                self.format_param(descriptor, value),
                MenuCommand::SetParamChoice { target, value },
                // Compared as positions rather than as floats: a value read back out of a saved
                // document is whatever survived the round trip, not necessarily the integer that
                // went in.
                current.round() as i64 == value.round() as i64,
            );
        }
        menu
    }

    /// Every way of dividing a beat, aimed at one generated clip.
    pub(crate) fn clip_subdivision_menu(&self, anchor: Point<Pixels>, clip: ClipId) -> ContextMenu {
        let current = self
            .session
            .clip_recipe(clip)
            .map(|recipe| recipe.subdivision);
        let mut menu = ContextMenu::new(anchor, self.t(Key::PartSubdivision));
        for subdivision in Subdivision::ALL {
            menu = menu.toggle(
                self.t(subdivision_key(subdivision)),
                MenuCommand::SetClipSubdivision { clip, subdivision },
                current == Some(subdivision),
            );
        }
        menu
    }

    /// Every register a generated clip can be moved to.
    pub(crate) fn clip_octave_menu(&self, anchor: Point<Pixels>, clip: ClipId) -> ContextMenu {
        let current = self.session.clip_recipe(clip).map(|recipe| recipe.octave);
        let mut menu = ContextMenu::new(anchor, self.t(Key::PartOctave));
        // Highest first, because that is the way a register reads on a keyboard and on every
        // stave: going down the menu should go down in pitch.
        for octave in crate::ui::part::octave_choices().rev() {
            menu = menu.toggle(
                crate::ui::part::octave_text(octave),
                MenuCommand::SetClipOctave { clip, octave },
                current == Some(octave),
            );
        }
        menu
    }

    /// Every groove the composer knows by name, aimed at one drum clip.
    pub(crate) fn clip_groove_menu(&self, anchor: Point<Pixels>, clip: ClipId) -> ContextMenu {
        let current = self
            .session
            .clip_recipe(clip)
            .map(|recipe| recipe.groove.clone());
        let mut menu = ContextMenu::new(anchor, self.t(Key::PartGroove));
        for groove in groove_catalog() {
            menu = menu.toggle(
                // The hyphenated identifier is what a specification writes; a menu row is a
                // place for what the groove sounds like.
                auris_i18n::audio::theory_description(groove.description, self.language()),
                MenuCommand::SetClipGroove {
                    clip,
                    groove: groove.name,
                },
                current.as_deref() == Some(groove.name),
            );
        }
        menu
    }

    /// Writes another take of a generated clip, and says what came out.
    pub(crate) fn reroll_clip(&mut self, clip: ClipId) {
        match self.session.reroll_clip(clip) {
            Ok(_) => {
                self.forget_rewritten_notes(clip);
                self.report_clip_preset(clip);
            }
            Err(error) => self.set_failed_status(self.failure(Key::MenuRerollClip, &error)),
        }
    }

    /// Keeps a generated clip's notes and forgets how they got there.
    pub(crate) fn freeze_clip(&mut self, clip: ClipId) {
        match self.session.freeze_clip(clip) {
            Ok(()) => self.set_status(self.t(Key::ClipKept)),
            Err(error) => self.set_failed_status(self.failure(Key::MenuFreezeClip, &error)),
        }
    }

    /// Stops every clip on a track from being written again.
    ///
    /// The count is reported rather than a bare confirmation, because this acts on clips the user
    /// is not necessarily looking at — a track scrolled past the bottom of the panel has clips on
    /// it too, and "kept 6" is the difference between believing that and checking.
    pub(crate) fn freeze_track(&mut self, track: TrackId) {
        match self.session.freeze_track(track) {
            Ok(count) => {
                let language = self.language();
                self.set_status(messages::track_kept(language, count));
            }
            Err(error) => self.set_failed_status(self.failure(Key::MenuFreezeTrack, &error)),
        }
    }

    /// Makes a generated clip a different kind of part, keeping its seed and its dials.
    ///
    /// The seed is deliberately kept. A recipe's dials mean the same thing whichever part reads
    /// them, so trying the same idea as a bass line and then as an arpeggio is one click either
    /// way rather than a click and then four dials set again from memory.
    pub(crate) fn set_clip_preset(&mut self, clip: ClipId, preset: ClipPreset) {
        let Some(recipe) = self.session.clip_recipe(clip) else {
            return;
        };
        if recipe.preset == preset {
            return;
        }
        let recipe = crate::ui::part::with_preset(recipe, preset);
        match self.session.set_clip_recipe(clip, recipe) {
            Ok(_) => {
                self.forget_rewritten_notes(clip);
                self.report_clip(preset, clip);
            }
            Err(error) => self.set_failed_status(self.failure(Key::MenuGenerateClip, &error)),
        }
    }

    /// Writes a generated clip from a seed somebody typed.
    pub(crate) fn set_clip_seed(&mut self, clip: ClipId, seed: u64) {
        let Some(recipe) = self.session.clip_recipe(clip) else {
            return;
        };
        if recipe.seed == seed {
            return;
        }
        let recipe = recipe.with_seed(seed);
        match self.session.set_clip_recipe(clip, recipe) {
            Ok(_) => {
                self.forget_rewritten_notes(clip);
                self.report_clip_preset(clip);
            }
            Err(error) => self.set_failed_status(self.failure(Key::MenuGenerateClip, &error)),
        }
    }

    /// Gives a generated clip a different groove.
    pub(crate) fn set_clip_groove(&mut self, clip: ClipId, groove: &str) {
        let Some(recipe) = self.session.clip_recipe(clip) else {
            return;
        };
        if recipe.groove == groove {
            return;
        }
        let recipe = ClipRecipe {
            groove: groove.to_string(),
            ..recipe.clone()
        };
        match self.session.set_clip_recipe(clip, recipe) {
            Ok(_) => {
                self.forget_rewritten_notes(clip);
                self.report_clip_preset(clip);
            }
            Err(error) => self.set_failed_status(self.failure(Key::MenuGenerateClip, &error)),
        }
    }

    /// Writes a generated clip over a beat divided a different way.
    pub(crate) fn set_clip_subdivision(&mut self, clip: ClipId, subdivision: Subdivision) {
        let Some(recipe) = self.session.clip_recipe(clip) else {
            return;
        };
        if recipe.subdivision == subdivision {
            return;
        }
        let recipe = ClipRecipe {
            subdivision,
            ..recipe.clone()
        };
        match self.session.set_clip_recipe(clip, recipe) {
            Ok(_) => {
                self.forget_rewritten_notes(clip);
                self.report_clip_preset(clip);
            }
            Err(error) => self.set_failed_status(self.failure(Key::MenuGenerateClip, &error)),
        }
    }

    /// Writes a generated clip in a different register.
    pub(crate) fn set_clip_octave(&mut self, clip: ClipId, octave: i32) {
        let Some(recipe) = self.session.clip_recipe(clip) else {
            return;
        };
        if recipe.octave == octave {
            return;
        }
        let recipe = ClipRecipe {
            octave,
            ..recipe.clone()
        };
        match self.session.set_clip_recipe(clip, recipe) {
            Ok(_) => {
                self.forget_rewritten_notes(clip);
                self.report_clip_preset(clip);
            }
            Err(error) => self.set_failed_status(self.failure(Key::MenuGenerateClip, &error)),
        }
    }

    /// The rows a generated clip adds to its own menu.
    fn generated_clip_rows(&self, menu: ContextMenu, clip: ClipId) -> ContextMenu {
        generated_clip_rows(
            menu,
            clip,
            self.session.clip_recipe(clip).is_some(),
            self.t(Key::MenuRerollClip),
            self.t(Key::MenuRegenerateClip),
            self.t(Key::MenuFreezeClip),
        )
    }

    /// A seed nobody has used yet, for the next clip that needs one.
    fn next_seed(&self) -> u64 {
        next_seed(self.project())
    }

    /// Says what was written, in the words the preset picker used.
    fn report_clip(&mut self, preset: ClipPreset, clip: ClipId) {
        let notes = self
            .session
            .project()
            .midi_clip(clip)
            .map_or(0, |(_, midi)| midi.notes.len());
        let name = self.t(preset_key(preset)).to_string();
        self.set_status(messages::clip_written(self.language(), &name, notes));
    }

    /// The same, for a clip that already knows which preset it is.
    fn report_clip_preset(&mut self, clip: ClipId) {
        let Some(preset) = self.session.clip_recipe(clip).map(|recipe| recipe.preset) else {
            return;
        };
        self.report_clip(preset, clip);
    }

    /// Every progression the composer knows by name, aimed at one position.
    pub(crate) fn progression_picker_menu(&self, anchor: Point<Pixels>, at: Ticks) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::MenuWriteProgression));
        for entry in progression_catalog() {
            menu = menu.item(
                // What it is, not what the parser calls it. The rows read `axis-minor` and
                // `doo-wop`, which is the vocabulary of a specification file rather than of
                // somebody deciding what the next four bars should do — and the catalogue has
                // carried a description for each of them all along.
                auris_i18n::audio::theory_description(entry.description, self.language()),
                MenuCommand::StampProgression {
                    name: entry.name,
                    at,
                },
            );
        }
        menu
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
            // Dynamics rather than a number, because that is what a musician means by "softer".
            // The roll has coloured notes by velocity since it was written and nothing could
            // change one; six markings cover the range a part is actually written in.
            .items_if(
                has_selection,
                DYNAMICS
                    .iter()
                    .map(|(label, velocity)| (*label, MenuCommand::SetNoteVelocity(*velocity))),
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
                MenuCommand::ShowEffectPicker { track, at: anchor },
            )
    }

    /// Every effect the registry knows, aimed at one particular strip.
    ///
    /// Aimed, rather than added "to whatever is selected". Two call sites used to reach the
    /// browser by moving the selection first — a track menu set it so the pick would land on that
    /// track, and the mixer's master strip *cleared* it so the pick would land on master. The
    /// second silently deselected whatever the user was working on, and the two pulled in
    /// opposite directions on the same piece of state. Carrying the target in the command
    /// removes the question.
    pub(crate) fn effect_picker_menu(
        &self,
        anchor: Point<Pixels>,
        track: Option<TrackId>,
    ) -> ContextMenu {
        let effects: Vec<(String, String)> = self
            .registry()
            .effects()
            .map(|descriptor| (descriptor.id.to_string(), descriptor.name.to_string()))
            .collect();
        let mut menu = ContextMenu::new(anchor, self.t(Key::MenuAddEffect));
        for (id, name) in effects {
            let label = crate::ui::inspector::audio_name(self, &name);
            menu = menu.item(
                label,
                MenuCommand::AddEffect {
                    track,
                    effect_id: id,
                },
            );
        }
        menu
    }

    /// Where a track's output could go: the master, then every bus in the project.
    ///
    /// *Every* bus, not only the legal ones. A destination that would send a signal back into
    /// itself is greyed out rather than left out, because the two look identical to a person
    /// reading the list and mean different things — a bus that is missing reads as a bus that was
    /// never made. Which of the two a row is comes from
    /// [`Session::can_route`](auris_session::Session::can_route), the same rule the command that
    /// enforces it uses, so the list can never offer something the session would refuse.
    pub(crate) fn output_menu(&self, anchor: Point<Pixels>, track: TrackId) -> ContextMenu {
        let current = self
            .project()
            .track(track)
            .map(|track| track.output)
            .unwrap_or_default();
        let mut menu = ContextMenu::new(anchor, self.t(Key::MenuRouteTo)).toggle(
            self.t(Key::Master),
            MenuCommand::SetTrackOutput(track, Output::Master),
            current == Output::Master,
        );
        for (bus, name) in self.bus_names() {
            menu = menu.toggle_if(
                self.session.can_route(track, bus),
                name,
                MenuCommand::SetTrackOutput(track, Output::Bus(bus)),
                current == Output::Bus(bus),
            );
        }
        menu
    }

    /// Every bus a track could send to, with the ones that would loop greyed out for the reason
    /// [`Self::output_menu`] gives.
    ///
    /// A bus it already sends to is still offered: a second send to the same bus is unusual rather
    /// than wrong, and hiding it would mean the list changed shape as it was used.
    pub(crate) fn send_picker_menu(&self, anchor: Point<Pixels>, track: TrackId) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::MenuAddSend));
        for (bus, name) in self.bus_names() {
            menu = menu.item_if(
                self.session.can_route(track, bus),
                name,
                MenuCommand::AddSend { track, bus },
            );
        }
        menu
    }

    /// Every bus in the project, by id and name.
    ///
    /// Collected rather than borrowed: a menu row owns its label, and the walk over the track list
    /// has to finish before the menu is handed back.
    fn bus_names(&self) -> Vec<(TrackId, SharedString)> {
        self.session
            .buses()
            .map(|bus| (bus.id, SharedString::from(bus.name.clone())))
            .collect()
    }

    /// The menu for one send row in the mixer.
    pub(crate) fn send_menu(
        &self,
        anchor: Point<Pixels>,
        track: TrackId,
        send: SendId,
    ) -> ContextMenu {
        let pre_fader = self
            .project()
            .track(track)
            .and_then(|track| track.sends.iter().find(|existing| existing.id == send))
            .is_some_and(|send| send.pre_fader);
        ContextMenu::new(anchor, self.t(Key::Sends))
            .toggle(
                self.t(Key::MenuSendPreFader),
                MenuCommand::ToggleSendPreFader { track, send },
                pre_fader,
            )
            .item(
                self.t(Key::MenuRemoveSend),
                MenuCommand::RemoveSend { track, send },
            )
            .separator()
            .item(
                self.t(Key::MenuAddSend),
                MenuCommand::ShowSendPicker { track, at: anchor },
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

/// The name a preset goes by on screen.
pub(crate) fn preset_key(preset: ClipPreset) -> Key {
    match preset {
        ClipPreset::Lead => Key::PresetLead,
        ClipPreset::Chords => Key::PresetChords,
        ClipPreset::Pad => Key::PresetPad,
        ClipPreset::Arp => Key::PresetArp,
        ClipPreset::Bass => Key::PresetBass,
        ClipPreset::Stab => Key::PresetStab,
        ClipPreset::Drums => Key::PresetDrums,
        ClipPreset::Kick => Key::PresetKick,
        ClipPreset::Snare => Key::PresetSnare,
        ClipPreset::Hat => Key::PresetHat,
    }
}

/// The note value a subdivision goes by on screen.
pub(crate) fn subdivision_key(subdivision: Subdivision) -> Key {
    match subdivision {
        Subdivision::Eighth => Key::SubdivisionEighth,
        Subdivision::Sixteenth => Key::SubdivisionSixteenth,
        Subdivision::EighthTriplet => Key::SubdivisionEighthTriplet,
        Subdivision::SixteenthTriplet => Key::SubdivisionSixteenthTriplet,
    }
}

/// Adds the rows that only mean something on a clip the composer wrote.
///
/// Nothing is added to a clip somebody played: every one of these commands would refuse it, and a
/// row that can only say no is worse than no row at all.
///
/// A free function taking its own labels, rather than a method reaching into the application, so
/// that what it decides can be checked without a window — which is the whole reason a menu is
/// plain data here.
fn generated_clip_rows(
    menu: ContextMenu,
    clip: ClipId,
    generated: bool,
    reroll: &str,
    regenerate: &str,
    freeze: &str,
) -> ContextMenu {
    if !generated {
        return menu;
    }
    menu.separator()
        .item(reroll.to_string(), MenuCommand::RerollClip(clip))
        .item(regenerate.to_string(), MenuCommand::RegenerateClip(clip))
        .item(freeze.to_string(), MenuCommand::FreezeClip(clip))
}

/// A seed no clip in the project is using, for the next one that needs one.
///
/// Counted up from the highest in use rather than drawn at random, so a session writes the same
/// run of clips twice and a phrase somebody wants back can be reached again.
fn next_seed(project: &Project) -> u64 {
    project
        .tracks
        .iter()
        .filter_map(|track| track.kind.as_instrument())
        .flat_map(|track| &track.clips)
        .filter_map(|clip| clip.recipe.as_ref())
        .map(|recipe| recipe.seed)
        .max()
        .map_or(1, |highest| highest.wrapping_add(1))
}

/// Where a generated clip goes, and how long it is.
///
/// The cycle region when there is one, for the same reason a progression uses it: setting the
/// cycle over the part of the song being worked on and then acting on it is how the rest of the
/// application already behaves. Four bars from the pointer otherwise, which is enough of a phrase
/// to judge and short enough to throw away.
fn generation_range(
    loop_region: Option<(Ticks, Ticks)>,
    tick: Ticks,
    signatures: &SignatureMap,
) -> (Ticks, Ticks) {
    match loop_region {
        Some((from, to)) if to > from => (from.max_zero(), to - from),
        _ => {
            // Four *bars*, not four bar lengths: across a meter change those differ, and what
            // "four bars" means is the four the ruler counts.
            let tick = tick.max_zero();
            let first = signatures.bar_of(tick);
            let length = signatures.bar_start(first + 4) - signatures.bar_start(first);
            (tick, length)
        }
    }
}

/// Where a progression goes, and across how many bars.
///
/// The cycle region when there is one, because "set the cycle over the chorus, then act on it" is
/// how the rest of the application already works, and it saves inventing a how-many-bars field
/// that nothing else would use. Otherwise it starts where the pointer was, and zero bars means
/// the chart's own length — which is what [`Session::stamp_named_progression`] reads it as.
fn progression_target(
    loop_region: Option<(Ticks, Ticks)>,
    tick: Ticks,
    signatures: &SignatureMap,
) -> (Ticks, usize) {
    match loop_region {
        Some((start, end)) if end > start => {
            // Counted off the ruler rather than divided out of a length, so a cycle spanning a
            // meter change reports the bars a person would count across it.
            let bars = signatures.bar_of(end) - signatures.bar_of(start.max_zero());
            // A cycle shorter than a bar still means one bar: the user asked for *there*, and
            // writing nothing would look like the command had failed.
            (start.max_zero(), bars.max(1) as usize)
        }
        _ => (tick, 0),
    }
}

/// What the harmony lane's menu acts on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct HarmonyTarget {
    /// Where a chord typed from this menu goes.
    chord: Ticks,
    /// Whether a chord sounds here, which is what makes removing one worth offering.
    sounding: bool,
    /// Whether the key here came from a change that can be removed — the anchor at tick zero
    /// cannot, since a song is always in some key.
    removable_key: bool,
}

/// What a right-click at `tick` aims the harmony menu at, `placed` being that position rounded.
///
/// Editing a chord means editing the one you can see, and a chord occupies everything from where
/// it starts to the next change — so a menu that acted on the rounded pointer position would write
/// a *second* chord a beat later instead of retyping the one that is there. Only where nothing
/// sounds does the rounded position win, because then there is nothing to edit and the menu is
/// placing something new.
///
/// Free rather than a method so the aiming can be tested: everything else it would take is a whole
/// session and a window.
fn harmony_target(harmony: &Harmony, tick: Ticks, placed: Ticks) -> HarmonyTarget {
    let sounding = harmony.chord_at(tick).is_some();
    HarmonyTarget {
        chord: harmony
            .chords
            .change_at(tick)
            .filter(|_| sounding)
            .unwrap_or(placed),
        sounding,
        removable_key: harmony.keys.change_at(tick) != Ticks::ZERO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4/4 for the whole timeline, which is what the bar arithmetic below is counted in.
    fn meters() -> SignatureMap {
        SignatureMap::default()
    }

    /// The commands a menu offers, ignoring its labels and its separators.
    fn commands(menu: &ContextMenu) -> Vec<MenuCommand> {
        menu.entries
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Item(item) => Some(item.command.clone()),
                MenuEntry::Separator => None,
            })
            .collect()
    }

    #[test]
    fn only_a_clip_the_composer_wrote_is_offered_another_take() {
        let clip = ClipId(7);
        let base = || ContextMenu::new(point(px(0.0), px(0.0)), "Clip");

        let played = generated_clip_rows(base(), clip, false, "again", "rewrite", "keep");
        assert!(
            commands(&played).is_empty(),
            "a clip somebody played was offered a command that would refuse it"
        );

        let written = generated_clip_rows(base(), clip, true, "again", "rewrite", "keep");
        assert_eq!(
            commands(&written),
            vec![
                MenuCommand::RerollClip(clip),
                MenuCommand::RegenerateClip(clip),
                MenuCommand::FreezeClip(clip),
            ]
        );
    }

    #[test]
    fn a_new_clip_takes_a_seed_no_other_clip_is_using() {
        let mut project = Project::new("Song", 48_000.0);
        assert_eq!(
            next_seed(&project),
            1,
            "the first clip has to start somewhere"
        );

        let track = project.add_instrument_track("Keys", "auris.synth.chiptune");
        let clip = project
            .add_midi_clip(track, "One", Ticks::ZERO, Ticks(3840))
            .unwrap();
        if let Some(midi) = project.midi_clip_mut(clip) {
            midi.recipe = Some(ClipRecipe::new(ClipPreset::Lead, 41));
        }
        assert_eq!(next_seed(&project), 42);

        // A clip somebody played holds no seed and must not be counted as holding zero.
        let played = project
            .add_midi_clip(track, "Two", Ticks(3840), Ticks(3840))
            .unwrap();
        assert!(project.midi_clip(played).unwrap().1.recipe.is_none());
        assert_eq!(next_seed(&project), 42);
    }

    #[test]
    fn a_generated_clip_goes_where_the_cycle_is_when_there_is_one() {
        let bar = TimeSignature::new(4, 4).ticks_per_bar();

        // No cycle: four bars from where the pointer was — enough of a phrase to judge, and
        // short enough to throw away.
        assert_eq!(
            generation_range(None, bar * 2, &meters()),
            (bar * 2, bar * 4)
        );

        // A cycle wins, and the clip is exactly as long as it.
        assert_eq!(
            generation_range(Some((bar * 8, bar * 16)), bar, &meters()),
            (bar * 8, bar * 8)
        );

        // An empty cycle is not a range, so the pointer decides again.
        assert_eq!(
            generation_range(Some((bar * 4, bar * 4)), Ticks::ZERO, &meters()),
            (Ticks::ZERO, bar * 4)
        );
    }

    #[test]
    fn a_progression_goes_where_the_cycle_is_when_there_is_one() {
        let bar = TimeSignature::new(4, 4).ticks_per_bar();

        // No cycle: it starts where the pointer was, for the chart's own length.
        assert_eq!(
            progression_target(None, bar * 3, &meters()),
            (bar * 3, 0),
            "zero bars means the chart decides"
        );

        // A cycle over bars 5..9 wins over wherever the pointer happened to be.
        assert_eq!(
            progression_target(Some((bar * 4, bar * 8)), bar * 99, &meters()),
            (bar * 4, 4)
        );

        // A cycle shorter than a bar still writes something.
        assert_eq!(
            progression_target(Some((Ticks::ZERO, Ticks(480))), bar, &meters()),
            (Ticks::ZERO, 1)
        );

        // An empty or inverted cycle is not a range, so the pointer decides again.
        assert_eq!(
            progression_target(Some((bar * 4, bar * 4)), bar, &meters()),
            (bar, 0)
        );
    }

    fn menu(anchor: Point<Pixels>, items: usize) -> ContextMenu {
        (0..items).fold(ContextMenu::new(anchor, "Track 1"), |menu, index| {
            menu.item(format!("Item {index}"), MenuCommand::NewAudioTrack)
        })
    }

    #[test]
    fn the_keyboard_walks_the_rows_and_wraps() {
        let mut menu = menu(gpui::point(px(0.0), px(0.0)), 3);
        assert_eq!(
            menu.highlighted, None,
            "a menu opened with the mouse draws no highlight"
        );

        menu.step(1);
        assert_eq!(
            menu.highlighted,
            Some(0),
            "the first Down lands on the first row"
        );
        menu.step(1);
        menu.step(1);
        assert_eq!(menu.highlighted, Some(2));
        menu.step(1);
        assert_eq!(menu.highlighted, Some(0), "and wraps round the end");
        menu.step(-1);
        assert_eq!(menu.highlighted, Some(2), "and back the other way");
    }

    #[test]
    fn the_first_up_lands_on_the_last_row() {
        let mut menu = menu(gpui::point(px(0.0), px(0.0)), 3);
        menu.step(-1);
        assert_eq!(menu.highlighted, Some(2));
    }

    #[test]
    fn separators_and_disabled_rows_are_stepped_over() {
        // Return on a separator would run nothing, and stopping on a disabled row is a keypress
        // the user has to make twice.
        let menu = ContextMenu::new(gpui::point(px(0.0), px(0.0)), "Track 1")
            .item("Rename", MenuCommand::NewAudioTrack)
            .separator()
            .item_if(false, "Delete", MenuCommand::NewAudioTrack)
            .item("Duplicate", MenuCommand::NewAudioTrack);

        let mut walking = menu.clone();
        walking.step(1);
        assert_eq!(walking.highlighted, Some(0));
        walking.step(1);
        assert_eq!(
            walking.highlighted,
            Some(3),
            "past the rule and the dead row"
        );
        assert!(walking.highlighted_command().is_some());

        // A menu with nothing choosable in it leaves the highlight alone rather than pointing at
        // a rule.
        let mut rules = ContextMenu::new(gpui::point(px(0.0), px(0.0)), "Nothing").separator();
        rules.step(1);
        assert_eq!(rules.highlighted, None);
        assert!(rules.highlighted_command().is_none());
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
                    swatch: None,
                }),
                MenuEntry::Separator,
                MenuEntry::Item(MenuItem {
                    label: "Two".into(),
                    command: MenuCommand::NewAudioTrack,
                    enabled: true,
                    checked: false,
                    swatch: None,
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

    /// One bar of 4/4.
    const BAR: Ticks = Ticks(3840);

    fn numeral(text: &str) -> Option<Numeral> {
        Some(Numeral::parse(text).expect("a numeral the test wrote itself"))
    }

    #[test]
    fn the_harmony_menu_edits_the_chord_you_can_see() {
        // A chord runs until the next change, so pointing part-way into one must retype *it*
        // rather than write a second chord where the pointer rounded to. The positions matter: a
        // progression written three chords to a bar sits on thirds of one, which no editing grid
        // rounds to, so an aim that went through the grid would miss every chord in it.
        let mut harmony = Harmony::default();
        harmony.chords.set_point(BAR, numeral("I"));
        harmony.chords.set_point(BAR + Ticks(1280), numeral("IV"));

        let target = harmony_target(&harmony, BAR + Ticks(700), BAR + Ticks(960));
        assert_eq!(target.chord, BAR, "aimed at the chord, not at the grid");
        assert!(target.sounding);

        let later = harmony_target(&harmony, BAR * 4, BAR * 4);
        assert_eq!(later.chord, BAR + Ticks(1280), "the last one runs on");

        // Before anything is written there is nothing to edit, so a new chord goes where the
        // pointer rounded to.
        let empty = harmony_target(&harmony, Ticks::ZERO, Ticks::ZERO);
        assert_eq!(empty.chord, Ticks::ZERO);
        assert!(
            !empty.sounding,
            "removing a chord that is not there was offered"
        );
    }

    #[test]
    fn a_cleared_stretch_offers_a_chord_rather_than_removing_a_silence() {
        // Clearing writes a `None` marker, which is a change like any other. It is not a chord,
        // though, so the menu must offer to write one there rather than to remove one.
        let mut harmony = Harmony::default();
        harmony.chords.set_point(Ticks::ZERO, numeral("I"));
        harmony.clear(BAR, BAR * 2);

        let target = harmony_target(&harmony, BAR + Ticks(100), BAR);
        assert!(!target.sounding);
        assert_eq!(target.chord, BAR, "the rounded position: nothing to edit");
    }

    #[test]
    fn only_a_key_change_can_be_removed_and_never_the_song_s_own_key() {
        let mut harmony = Harmony::default();
        assert!(
            !harmony_target(&harmony, BAR * 9, BAR * 9).removable_key,
            "a song in one key throughout has no key change to remove"
        );

        harmony
            .keys
            .set_point(BAR * 4, MusicalKey::parse("Eb major").unwrap());
        assert!(!harmony_target(&harmony, BAR * 2, BAR * 2).removable_key);
        assert!(
            harmony_target(&harmony, BAR * 9, BAR * 9).removable_key,
            "pointing anywhere inside the E flat finds the change that started it"
        );
    }
}
