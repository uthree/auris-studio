//! The vocabulary of a menu choice, and the one place every choice is carried out.
//!
//! [`MenuCommand`] and `AurisApp::run_menu_command` live in one file on purpose. They are an
//! enum and its exhaustive match, and the compiler is what keeps them in step: a variant added
//! here fails to build until the match answers it. Separating them would turn that compile error
//! into a menu item that quietly does nothing.

use auris_i18n::{Key, messages};
use auris_session::prelude::*;

use gpui::{Context, Pixels, Point};

use crate::app::AurisApp;
use crate::dock::{Dock, Panel};
use crate::ui::compose_sheet::song_dials;
use crate::ui::prompt::{Prompt, PromptTarget};

use super::recipe::generation_range;
use super::timeline::progression_target;

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
    /// Pin one section of the song sheet to a tempo, or let it follow the song's.
    SongSectionTempo {
        /// Which section, by position in the sheet's list.
        section: usize,
        /// The tempo it plays at, or `None` to follow the song.
        bpm: Option<f64>,
    },
    /// Turn one part of the roster on or off for one section of the song sheet.
    SongSectionPart {
        /// Which section, by position in the sheet's list.
        section: usize,
        /// The part, by name — which is what a section stores rather than a position.
        part: String,
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
    /// Open the General MIDI programs of one family, for one part on the song sheet.
    ///
    /// The anchor travels with the command because the menu that offered it is already closed by
    /// the time this runs, and the second menu has to open where the first one was rather than
    /// wherever the pointer has wandered since.
    SongPartFamily {
        /// Which part, by position in the roster.
        part: usize,
        /// Which family, by position in [`gm::FAMILIES`].
        family: usize,
        /// Where to put the menu.
        anchor: gpui::Point<gpui::Pixels>,
    },
    /// Set which General MIDI sound one part on the song sheet plays.
    SongPartProgram {
        /// Which part, by position in the roster.
        part: usize,
        /// The program, or the kit on a drum part.
        program: u8,
    },
    /// Replace everything on the song sheet with one of the composer's whole-song presets.
    SongPreset(&'static str),
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
    /// Straighten a clip out, taking one of its curves off entirely.
    ClearCurve {
        /// Whose curve.
        clip: ClipId,
        /// Which one.
        which: ClipCurve,
    },
    /// Stretch an audio clip to follow the piece's tempo, or stop.
    FollowTempo {
        /// Which clip.
        clip: ClipId,
        /// Whether it should be following afterwards.
        follows: bool,
    },
    /// Ask what tempo an audio clip's material was recorded at.
    ClipSourceTempo(ClipId),
    /// Show or hide one of the piano roll's curve strips.
    ShowCurveLane {
        /// Which strip.
        which: ClipCurve,
        /// Whether it should be showing afterwards.
        shown: bool,
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
    /// Put a clip — or the whole selection it belongs to — on the clipboard, and remove it.
    CutClips(ClipId),
    /// Put a clip, or the selection it belongs to, on the clipboard.
    CopyClips(ClipId),
    /// Lay the clipboard's clips onto a track, starting at a position.
    PasteClips {
        /// Where the topmost copied clip lands.
        track: TrackId,
        /// Where it starts.
        at: Ticks,
    },
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
    /// Repeat a clip out to the next one on its lane, or stop it repeating.
    ToggleClipLoop(ClipId),
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
    /// Put the selected notes on the clipboard and take them out of the clip.
    CutNotes,
    /// Put the selected notes on the clipboard.
    CopyNotes,
    /// Lay the clipboard's notes into the clip being edited, at the playhead.
    PasteNotes,
    /// Delete the selected notes.
    DeleteNotes,
    /// Shift the selected notes in pitch.
    TransposeNotes(i32),
    /// Sets how hard the selected notes are struck, as a dynamic marking's MIDI velocity.
    SetNoteVelocity(u8),
    /// Snap the selected notes onto the editing grid.
    QuantizeNotes(Quantize),
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
    /// Open the list of tracks an effect could be keyed from.
    ShowSidechainPicker {
        /// Strip the effect sits in, or `None` for the master bus.
        track: Option<TrackId>,
        /// Which slot in it.
        slot: EffectSlotId,
        /// Where to put the menu.
        at: Point<Pixels>,
    },
    /// Open the list of device inputs a track could be recorded from.
    ShowInputPicker {
        /// The track to arm.
        track: TrackId,
        /// Where to put the menu.
        at: Point<Pixels>,
    },
    /// Set how many bars are counted in front of a take, or none.
    SetCountIn {
        /// Bars to count, or zero for no count-in.
        bars: u32,
    },
    /// Arm a track on particular input channels, or disarm it.
    SetTrackInput {
        /// The track.
        track: TrackId,
        /// The channels to read, or `None` to disarm it.
        input: Option<InputChannels>,
    },
    /// Point one effect at a track to key from, or stop it listening to one.
    SetEffectSidechain {
        /// Strip the effect sits in, or `None` for the master bus.
        track: Option<TrackId>,
        /// Which slot in it.
        slot: EffectSlotId,
        /// The track to key from, or `None` for nothing.
        source: Option<TrackId>,
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
    /// Move the punch-in to this position.
    SetPunchStart(Ticks),
    /// Move the punch-out to this position.
    SetPunchEnd(Ticks),
    /// Turn punch recording on or off.
    TogglePunch,
    /// Set the punch region to whatever the cycle is set to.
    PunchFromCycle,
    /// Forget the punch region.
    ClearPunch,

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
    /// Crossfade an audio clip with the clip it overlaps.
    Crossfade(ClipId),
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
    /// Read a melody's harmony and write a band behind it.
    AccompanyClip(ClipId),
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

    /// Put a parameter back on whatever its descriptor calls the default.
    ///
    /// The same thing double-clicking the control does. It is in the menu as well because the
    /// menu is now where a parameter is asked about at all, and a list that offers to automate a
    /// fader and not to straighten it is a list with a hole in it.
    ResetParam(ParamTarget),

    /// Ask for a parameter's value as a number.
    SetParamValue(ParamTarget),

    /// Open a project from the recent list.
    OpenRecent(std::path::PathBuf),
    /// Empty the recent list.
    ForgetRecent,

    /// Change how an existing lane gets from one point to the next.
    ///
    /// A new lane is given the shape its parameter implies — a chooser holds, a fader runs
    /// straight — but the implication is not always right. A volume that should drop on the bar
    /// line rather than slide into it wants Step, and a cutoff written as a sequence of settings
    /// wants it too. Only offered on a lane that exists, because the shape is the lane's and
    /// there is nothing to shape before the first point.
    SetAutomationCurve {
        /// Which lane.
        target: ParamTarget,
        /// The shape to give it.
        curve: AutomationCurve,
    },
}

impl AurisApp {
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
            MenuCommand::RenameTrack(track) => self.prompt_to_rename_track(track),
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
            MenuCommand::SongSectionTempo { section, bpm } => {
                if let Some(section) = self
                    .song_sheet
                    .as_mut()
                    .and_then(|dials| dials.sections.get_mut(section))
                {
                    section.tempo = bpm;
                }
            }
            MenuCommand::SongSectionPart { section, part } => {
                if let Some(dials) = self.song_sheet.as_mut() {
                    crate::ui::compose_sheet::toggle_part_in_section(dials, section, &part);
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
                    // Choosing a plugin is choosing *that* sound, and a program left behind
                    // would go on winning — the row would say one thing and the piece play
                    // another.
                    part.program = None;
                }
            }
            MenuCommand::SongPartFamily {
                part,
                family,
                anchor,
            } => {
                let menu = self.song_program_menu(anchor, part, family);
                self.open_menu(menu);
            }
            MenuCommand::SongPartProgram { part, program } => {
                if let Some(part) = self
                    .song_sheet
                    .as_mut()
                    .and_then(|dials| dials.parts.get_mut(part))
                {
                    part.program = Some(gm::Program(program));
                }
            }
            MenuCommand::SongPreset(name) => {
                if let Some(preset) = preset(name) {
                    // The whole sheet, title and all. Half a preset is the arrangement of one
                    // style at the tempo of another, which is not a style at all.
                    self.song_sheet = Some(song_dials(&preset.spec()));
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
            MenuCommand::ClearCurve { clip, which } => {
                self.session.clear_curve(clip, which);
            }
            MenuCommand::FollowTempo { clip, follows } => {
                let _ = self.session.set_clip_follows_tempo(clip, follows);
            }
            MenuCommand::ClipSourceTempo(clip) => self.prompt_for_clip_source_tempo(clip),
            MenuCommand::ShowCurveLane { which, shown } => {
                self.panels.set_curve_lane(which, shown);
                self.remember_layout();
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
                // One click, one step. `duplicate_clip` records a step per call, so duplicating a
                // selection of three left three of them: one Undo took back the last copy and left
                // the other two sitting there, which is a document the user never asked for. The
                // copies that succeeded stay copied even when a later one fails — same as every
                // other command here that works through a selection — and now one Undo reverses
                // the whole click rather than the tail of it.
                self.session.begin_transaction(Edit::DuplicateClip);
                for source in self.clips_for_command(clip) {
                    match self.session.duplicate_clip(source) {
                        Ok(copy) => {
                            copies.insert(copy);
                        }
                        Err(error) => failure = Some(error),
                    }
                }
                self.session.end_transaction();
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
            MenuCommand::CutClips(clip) | MenuCommand::CopyClips(clip) => {
                let cutting = matches!(command, MenuCommand::CutClips(_));
                let chosen = self.clips_for_command(clip);
                let taken = match cutting {
                    true => self.session.cut_clips(&chosen).unwrap_or(0),
                    false => self.session.copy_clips(&chosen),
                };
                if taken == 0 {
                    return;
                }
                if cutting {
                    self.select_clip(None);
                    self.selected_notes.clear();
                }
                self.set_status(self.t(match cutting {
                    true => Key::CutToClipboard,
                    false => Key::CopiedToClipboard,
                }));
            }
            MenuCommand::PasteClips { track, at } => {
                if self.session.clipboard().is_empty() {
                    self.set_status(self.t(Key::NothingToPaste));
                    return;
                }
                match self.session.paste_clips(track, at) {
                    Ok(pasted) if !pasted.is_empty() => {
                        // The arrivals become the selection, for the reason a duplicate's copies
                        // do: dragging straight afterwards should move what was just laid down.
                        self.select_clips(pasted.into_iter().collect(), None);
                        self.selected_notes.clear();
                        self.set_status(self.t(Key::PastedFromClipboard));
                    }
                    // A paste that fits nowhere — every copied clip is the wrong kind for the
                    // tracks under it, or the rows below the target have run out.
                    Ok(_) => self.set_status(self.t(Key::NothingToPaste)),
                    Err(error) => self.set_failed_status(self.failure(Key::CmdPasteClips, &error)),
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
                // The *sounding* extent, repeats included: cycling over a looped clip and
                // hearing only its first bar would be a cycle region nobody could explain.
                if let (Some(start), Some(length)) = (
                    self.session.clip_start(clip),
                    self.session.clip_sounding_length(clip),
                ) {
                    self.session.set_loop_region(start, start + length);
                    self.session.set_loop_enabled(true);
                }
            }
            MenuCommand::ToggleClipLoop(clip) => self.toggle_clip_loop(clip),
            MenuCommand::EditClip(clip) => self.open_clip_in_editor(clip),
            MenuCommand::NewClip { track, start } => self.create_clip_at(track, start),

            MenuCommand::QuantizeNotes(what) => self.quantize_selected_notes(what),
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
            MenuCommand::CutNotes | MenuCommand::CopyNotes => {
                let cutting = command == MenuCommand::CutNotes;
                let Some(clip) = self.selected_clip else {
                    return;
                };
                let chosen: Vec<usize> = self.selected_notes.iter().copied().collect();
                let taken = match cutting {
                    true => self.session.cut_notes(clip, &chosen).unwrap_or(0),
                    false => self.session.copy_notes(clip, &chosen),
                };
                if taken == 0 {
                    return;
                }
                if cutting {
                    // The indices are gone with the notes, and a selection still naming them
                    // would point at whatever slid into their places.
                    self.selected_notes.clear();
                }
                self.set_status(self.t(match cutting {
                    true => Key::CutToClipboard,
                    false => Key::CopiedToClipboard,
                }));
            }
            MenuCommand::PasteNotes => {
                let Some(clip) = self.selected_clip else {
                    self.set_status(self.t(Key::NoClipToPasteInto));
                    return;
                };
                if self.session.clipboard().is_empty() {
                    self.set_status(self.t(Key::NothingToPaste));
                    return;
                }
                // Clip-relative, because a note position is. A playhead parked before the clip
                // pastes at its beginning, which `paste_notes` clamps for us.
                let start = self.session.midi_clip(clip).map(|midi| midi.start);
                let at = self.playhead_ticks() - start.unwrap_or_default();
                match self.session.paste_notes(clip, at) {
                    Ok(pasted) if !pasted.is_empty() => {
                        // The arrivals become the selection, so they can be dragged straight
                        // away without hunting for them among what was already there.
                        self.selected_notes = pasted.into_iter().collect();
                        self.set_status(self.t(Key::PastedFromClipboard));
                    }
                    Ok(_) => self.set_status(self.t(Key::NothingToPaste)),
                    Err(error) => self.set_failed_status(self.failure(Key::CmdPasteNotes, &error)),
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
            MenuCommand::ShowInputPicker { track, at } => {
                let menu = self.input_menu(at, track);
                self.open_menu(menu);
            }
            MenuCommand::SetTrackInput { track, input } => self.set_track_input(track, input),
            MenuCommand::SetCountIn { bars } => self.set_count_in(bars),
            MenuCommand::ShowSidechainPicker { track, slot, at } => {
                let menu = self.sidechain_menu(at, track, slot);
                self.open_menu(menu);
            }
            MenuCommand::SetEffectSidechain {
                track,
                slot,
                source,
            } => self.set_effect_sidechain(track, slot, source),
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
            MenuCommand::AccompanyClip(clip) => {
                let seed = self.next_seed();
                match self.session.accompany(clip, &DEFAULT_PARTS, seed) {
                    Ok(report) => {
                        // The melody stays selected. What was added is around it, and pointing the
                        // editors at a bass line somebody did not ask to look at would be the
                        // application deciding it knows better than the person who was mid-phrase.
                        self.set_status(messages::accompaniment_written(
                            self.language(),
                            &report.key.to_text(),
                            report.parts.len(),
                            report.chords,
                        ));
                    }
                    Err(error) => self.set_failed_status(self.failure(Key::MenuAccompany, &error)),
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
            MenuCommand::ResetParam(target) => self.reset_param(target),
            MenuCommand::SetParamValue(target) => self.prompt_for_param(target),
            MenuCommand::OpenRecent(path) => {
                // The same guard a dropped project gets, and for the same reason: the document
                // on screen may be unsaved, and the sheet has to be able to answer with "save,
                // then open *that* one" rather than reopening a dialog.
                if self.confirm_discard(crate::ui::prompt::PendingAction::OpenDropped(path.clone()))
                {
                    self.open_project_at(path, cx);
                }
            }
            MenuCommand::ForgetRecent => self.forget_recent(),
            MenuCommand::SetAutomationCurve { target, curve } => {
                self.session.set_automation_curve(target, curve);
            }

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
            MenuCommand::Crossfade(clip) => self.crossfade_clip(clip),
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

            // The same pair as the cycle's, and switching punch on as a side effect for the same
            // reason: somebody who has just said where the punch-in goes has said they want one.
            MenuCommand::SetPunchStart(tick) => {
                let end = self.session.punch_region().map_or(tick, |(_, end)| end);
                self.session.set_punch_region(tick, end.max(tick));
                self.session.set_punch_enabled(true);
            }
            MenuCommand::SetPunchEnd(tick) => {
                let start = self
                    .session
                    .punch_region()
                    .map_or(Ticks::ZERO, |(start, _)| start);
                self.session.set_punch_region(start.min(tick), tick);
                self.session.set_punch_enabled(true);
            }
            MenuCommand::TogglePunch => self.toggle_punch(),
            // The two are usually the same bars — loop to find the bad one, punch to replace it —
            // and dragging the second region to match the first by hand is a minute's work for a
            // thing the application already knows.
            MenuCommand::PunchFromCycle => {
                if let Some((start, end)) = self.project().loop_region {
                    self.session.set_punch_region(start, end);
                    self.session.set_punch_enabled(true);
                }
            }
            MenuCommand::ClearPunch => {
                self.session.set_punch_enabled(false);
                self.session.set_punch_region(Ticks::ZERO, Ticks::ZERO);
            }
        }
        cx.notify();
    }
}
