//! Menus for a clip and for the notes inside it, and the queries that read either kind of clip.
//!
//! The arrangement's clip menu and the piano roll's note menu share a file because they share
//! their subject: the roll is a clip opened up, and both have to answer the same questions about
//! a clip that may be MIDI or audio. Those answers are the small readers at the foot of the file,
//! which is why they are here rather than beside either menu — and why the gain sheet, which only
//! an audio clip has, is here too.

use auris_i18n::{Key, messages};
use auris_session::prelude::*;

use gpui::{Pixels, Point};

use crate::app::AurisApp;
use crate::ui::prompt::{Prompt, PromptTarget};

use super::{ContextMenu, MenuCommand};

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

impl AurisApp {
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
            .item(self.t(Key::MenuCut), MenuCommand::CutClips(clip))
            .item(self.t(Key::MenuCopy), MenuCommand::CopyClips(clip))
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
            )
            // Only where there is a melody to read. A clip with no notes in it has no harmony,
            // and the command would refuse — which is a row that exists to say no.
            .item_if(
                self.session
                    .midi_clip(clip)
                    .is_some_and(|midi| !midi.notes.is_empty()),
                self.t(Key::MenuAccompany),
                MenuCommand::AccompanyClip(clip),
            );
        self.generated_clip_rows(menu, clip)
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
            .item_if(has_selection, self.t(Key::MenuCut), MenuCommand::CutNotes)
            .item_if(has_selection, self.t(Key::MenuCopy), MenuCommand::CopyNotes)
            // Offered whenever there is something to paste, selection or no selection: a paste
            // is aimed at the playhead rather than at whatever happens to be picked out.
            .item_if(
                !self.session.clipboard().is_empty(),
                self.t(Key::MenuPaste),
                MenuCommand::PasteNotes,
            )
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

    /// The clips a menu command should act on.
    ///
    /// A command aimed at a clip inside the selection takes the whole selection with it, which
    /// is what selecting several of them was for; one aimed elsewhere acts alone.
    pub(super) fn clips_for_command(&self, clip: ClipId) -> Vec<ClipId> {
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

    pub(super) fn clip_is_muted(&self, clip: ClipId) -> bool {
        if let Some(midi) = self.session.midi_clip(clip) {
            return midi.muted;
        }
        self.audio_clip(clip).is_some_and(|clip| clip.muted)
    }

    /// Where a clip of either kind starts and ends on the timeline.
    pub(super) fn clip_extent(&self, clip: ClipId) -> Option<(Ticks, Ticks)> {
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
