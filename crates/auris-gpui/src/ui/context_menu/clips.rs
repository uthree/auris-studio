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

/// What the row that sets a clip's recorded tempo says.
///
/// A number where there is one, and plainly *not set* where there is not — the row is what tells
/// somebody why the switch above it is doing nothing, so it has to say that rather than show a
/// default that would look like an answer.
pub fn source_tempo_label(bpm: Option<f64>, language: auris_i18n::Language) -> String {
    match bpm {
        Some(bpm) => messages::clip_source_tempo(language, bpm),
        None => messages::clip_source_tempo_unknown(language),
    }
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
            // Beside Mute rather than beside Cycle, because that is what it is: a switch on the
            // clip. "Cycle over Clip" a row below moves the *transport's* loop, and the two
            // reading as versions of one another is exactly the confusion to avoid.
            .toggle(
                self.t(Key::MenuLoopClip),
                MenuCommand::ToggleClipLoop(clip),
                self.session.clip_is_looped(clip),
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
            // Only where there is a join to shape. A row that answered "those clips do not
            // overlap" would be a row offering to do nothing, on every clip in the project.
            .item_if(
                !is_midi && self.session.crossfade_partner(clip).is_some(),
                self.t(Key::MenuCrossfade),
                MenuCommand::Crossfade(clip),
            )
            .item_if(
                self.audio_clip_shape(clip)
                    .is_some_and(|(_, _, _, _, fade_in, fade_out)| fade_in > 0 || fade_out > 0),
                self.t(Key::MenuClearFades),
                MenuCommand::ClearFades(clip),
            )
            // What tempo the material was played at, and whether it is stretched to keep its
            // place when the piece is played at another. The two rows are next to each other
            // because the switch means nothing without the number under it.
            .toggle_if(
                !is_midi,
                self.t(Key::MenuFollowTempo),
                MenuCommand::FollowTempo {
                    clip,
                    follows: !self.session.clip_follows_tempo(clip),
                },
                self.session.clip_follows_tempo(clip),
            )
            .item_if(
                !is_midi,
                source_tempo_label(self.session.clip_source_bpm(clip), self.language()),
                MenuCommand::ClipSourceTempo(clip),
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

    /// Crossfades a clip with the one it overlaps, and says how long the join came out.
    ///
    /// The partner is worked out here rather than chosen, because there is nothing to choose: a
    /// clip overlaps at most one neighbour in any arrangement somebody meant, and the nearest is
    /// the join in every arrangement they did not.
    pub(crate) fn crossfade_clip(&mut self, clip: ClipId) {
        let Some((partner, _)) = self.session.crossfade_partner(clip) else {
            self.set_failed_status(self.t(Key::ErrorNotOverlapping));
            return;
        };
        match self.session.crossfade_clips(clip, partner) {
            Ok(overlap) => {
                let seconds = self.project().tempo_map.ticks_to_seconds(overlap).0;
                let status = messages::crossfaded(self.language, seconds);
                self.set_status(status);
            }
            Err(error) => {
                let text = self.failure(Key::CmdCrossfade, &error);
                self.set_failed_status(text);
            }
        }
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

    /// Opens the sheet that takes the tempo an audio clip was recorded at.
    ///
    /// It comes up holding whatever the clip believes now, or the tempo it sits at when it
    /// believes nothing: the answer for material dropped into the piece it was made for is the
    /// piece's own tempo, and typing over a plausible number beats typing into an empty box.
    pub(crate) fn prompt_for_clip_source_tempo(&mut self, clip: ClipId) {
        let Some(audio) = self.audio_clip(clip) else {
            return;
        };
        let anchor = audio.anchored_at();
        let current = self
            .session
            .clip_source_bpm(clip)
            .unwrap_or_else(|| self.session.project().tempo_map.bpm_at(anchor));
        let title = self.t(Key::SetClipSourceTempoTitle);
        self.open_prompt(Prompt::new(
            title,
            PromptTarget::ClipSourceTempo(clip),
            format!("{current:.1}"),
        ));
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
            // The three quantise passes, spelt out rather than hidden behind one row that moves
            // whichever number the last person chose. They snap to the editing grid, which is on
            // screen above the notes being snapped.
            .item_if(
                has_selection,
                self.t(Key::MenuQuantizeStarts),
                MenuCommand::QuantizeNotes(Quantize::Starts),
            )
            .item_if(
                has_selection,
                self.t(Key::MenuQuantizeLengths),
                MenuCommand::QuantizeNotes(Quantize::Lengths),
            )
            .item_if(
                has_selection,
                self.t(Key::MenuQuantizeBoth),
                MenuCommand::QuantizeNotes(Quantize::Both),
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
    pub(crate) fn clips_for_command(&self, clip: ClipId) -> Vec<ClipId> {
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

    pub(crate) fn audio_clip(&self, clip: ClipId) -> Option<&AudioClip> {
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
