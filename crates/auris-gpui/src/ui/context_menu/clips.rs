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

use crate::app::{AurisApp, FadeEdge};
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

/// Whether a clip running from `start` to `end` can be cut at `playhead`.
///
/// Strictly inside, both ends: a cut on either edge makes an empty clip and leaves the other
/// exactly as it was, which is a command that appears to do nothing.
///
/// Free-standing because the window cannot check it. A headless session's playhead is an atomic
/// the *audio thread* writes and there is no audio thread, so a seek is invisible to a test that
/// drives the window — see the harness in `src/harness.rs`, which is a test build only.
/// The rule is worth more than the line it takes.
pub fn splittable(playhead: Ticks, start: Ticks, end: Ticks) -> bool {
    playhead > start && playhead < end
}

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
            .is_some_and(|(start, end)| splittable(playhead, start, end));
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
            // A shape each, and only for an edge that has a fade to shape. What a crossfade sets
            // for itself, offered by hand for a join somebody made by dragging a fade instead.
            .item_if(
                self.audio_clip_shape(clip)
                    .is_some_and(|(_, _, _, _, fade_in, _)| fade_in > 0),
                self.t(Key::MenuFadeInShape),
                MenuCommand::ShowFadeShapePicker {
                    clip,
                    edge: FadeEdge::In,
                    at: anchor,
                },
            )
            .item_if(
                self.audio_clip_shape(clip)
                    .is_some_and(|(_, _, _, _, _, fade_out)| fade_out > 0),
                self.t(Key::MenuFadeOutShape),
                MenuCommand::ShowFadeShapePicker {
                    clip,
                    edge: FadeEdge::Out,
                    at: anchor,
                },
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
            )
            // The other direction: this clip's line becomes the tune the composer restates.
            // Gated the same way, because a clip with no notes has no line to take.
            .item_if(
                self.session
                    .midi_clip(clip)
                    .is_some_and(|midi| !midi.notes.is_empty()),
                self.t(Key::MenuComposeFromMotif),
                MenuCommand::TakeClipAsMotif(clip),
            );
        self.generated_clip_rows(menu, clip)
    }

    /// The shapes one of a clip's fades can take.
    ///
    /// Two rows, because there are two shapes and each is right for a different job — a fade to
    /// silence wants the straight one and a join wants the other. Which edge is in the row that
    /// opened this, so the picker says only what the shape is.
    pub(crate) fn fade_shape_menu(
        &self,
        anchor: Point<Pixels>,
        clip: ClipId,
        edge: FadeEdge,
    ) -> ContextMenu {
        let current = self
            .session
            .fade_curves(clip)
            .map(|(into, out)| match edge {
                FadeEdge::In => into,
                FadeEdge::Out => out,
            });
        let title = match edge {
            FadeEdge::In => Key::MenuFadeInShape,
            FadeEdge::Out => Key::MenuFadeOutShape,
        };
        let mut menu = ContextMenu::new(anchor, self.t(title));
        for (curve, label) in [
            (FadeCurve::Linear, Key::MenuFadeLinear),
            (FadeCurve::EqualPower, Key::MenuFadeEqualPower),
        ] {
            menu = menu.toggle(
                self.t(label),
                MenuCommand::SetFadeCurve { clip, edge, curve },
                current == Some(curve),
            );
        }
        menu
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
        // The lyric rows, only where there are words to edit: on an instrument track they would
        // be three rows about a feature the track does not have.
        let singing = self.editing_a_singer_clip();
        // Which ornaments the note under the pointer wears, for the toggle rows below.
        let worn = under_pointer
            .and_then(|index| {
                self.session
                    .midi_clip(clip)
                    .and_then(|target| target.notes.get(index))
            })
            .map(|note| {
                (
                    note.scoop.is_some(),
                    note.fall.is_some(),
                    note.vibrato.is_some(),
                )
            })
            .unwrap_or((false, false, false));

        ContextMenu::new(anchor, title)
            .item_if(
                singing && under_pointer.is_some(),
                self.t(Key::MenuEditLyric),
                MenuCommand::EditLyric {
                    clip,
                    index: under_pointer.unwrap_or(0),
                },
            )
            .item_if(
                singing && under_pointer.is_some(),
                self.t(Key::MenuEditPhonemes),
                MenuCommand::EditPhonemes {
                    clip,
                    index: under_pointer.unwrap_or(0),
                },
            )
            .item_if(
                singing && has_selection,
                self.t(Key::MenuWriteLyrics),
                MenuCommand::WriteLyrics { clip },
            )
            // Only where a pin actually stands: a reset over nothing is a row that lies.
            .item_if(
                singing
                    && under_pointer.is_some_and(|index| {
                        self.session
                            .midi_clip(clip)
                            .and_then(|target| target.notes.get(index))
                            .is_some_and(|note| !note.phoneme_seconds.is_empty())
                    }),
                self.t(Key::MenuResetPhonemeTiming),
                MenuCommand::ResetPhonemeTiming {
                    clip,
                    index: under_pointer.unwrap_or(0),
                },
            )
            // Each ornament row reads the note and toggles, and the label says which way. A
            // full reset appears only over two or more, where it is shorter than the removes
            // it stands for — over one it would be a remove wearing a longer name.
            .item_if(
                singing && under_pointer.is_some(),
                self.t(match worn.0 {
                    true => Key::MenuRemoveScoop,
                    false => Key::MenuAddScoop,
                }),
                MenuCommand::SetScoop {
                    clip,
                    index: under_pointer.unwrap_or(0),
                    on: !worn.0,
                },
            )
            .item_if(
                singing && under_pointer.is_some(),
                self.t(match worn.1 {
                    true => Key::MenuRemoveFall,
                    false => Key::MenuAddFall,
                }),
                MenuCommand::SetFall {
                    clip,
                    index: under_pointer.unwrap_or(0),
                    on: !worn.1,
                },
            )
            .item_if(
                singing && under_pointer.is_some(),
                self.t(match worn.2 {
                    true => Key::MenuRemoveVibrato,
                    false => Key::MenuAddVibrato,
                }),
                MenuCommand::SetVibrato {
                    clip,
                    index: under_pointer.unwrap_or(0),
                    on: !worn.2,
                },
            )
            .item_if(
                singing && [worn.0, worn.1, worn.2].iter().filter(|on| **on).count() >= 2,
                self.t(Key::MenuResetOrnaments),
                MenuCommand::ResetOrnaments {
                    clip,
                    index: under_pointer.unwrap_or(0),
                },
            )
            .separator()
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

/// What a right-press over the clip lanes opens, and what choosing a row then does.
///
/// The menu is built from the document, so which rows exist is a question about the clip under
/// the pointer — and a row that quietly stops being offered is invisible until somebody goes
/// What a right-press over the clip lanes opens, and what choosing a row then does.
///
/// The menu is built from the document, so which rows it *offers* — as against which it shows
/// greyed — is a question about the clip under the pointer. A row that quietly stops being
/// offered, or starts being, is invisible until somebody goes looking for it.
#[cfg(test)]
mod window_tests {
    use gpui::TestAppContext;

    use auris_session::prelude::*;

    use crate::harness::{CLIP_LENGTH, choose, lane_point, paint, right_press, with_a_clip};
    use crate::ui::context_menu::{MenuCommand, MenuEntry};

    /// Halfway along the fixture's clip.
    const HALF_CLIP: Ticks = Ticks(CLIP_LENGTH.0 / 2);

    /// Every row of the open menu, with whether it can be chosen.
    ///
    /// Both halves, because a row can be on screen and not choosable —
    /// `ContextMenu::item_greyed_unless` is what makes one, and the bus pickers use it. Everything
    /// else conditional is simply not there, which is what `item_if` now means.
    fn rows(
        app: &gpui::Entity<crate::app::AurisApp>,
        cx: &gpui::TestAppContext,
    ) -> Vec<(MenuCommand, bool)> {
        app.read_with(cx, |this, _| {
            this.menu
                .as_ref()
                .expect("a menu is open")
                .entries
                .iter()
                .filter_map(|entry| match entry {
                    MenuEntry::Item(item) => Some((item.command.clone(), item.enabled)),
                    MenuEntry::Separator => None,
                })
                .collect()
        })
    }

    /// Whether the open menu is offering `command` — present *and* choosable.
    fn offers(
        app: &gpui::Entity<crate::app::AurisApp>,
        cx: &gpui::TestAppContext,
        command: &MenuCommand,
    ) -> bool {
        rows(app, cx)
            .into_iter()
            .any(|(row, enabled)| &row == command && enabled)
    }

    #[gpui::test]
    fn a_right_press_on_a_clip_opens_that_clips_menu(cx: &mut TestAppContext) {
        let (app, cx, track, clip) = with_a_clip(cx);
        let at = lane_point(&app, cx, track, HALF_CLIP);

        right_press(cx, at);

        assert!(offers(&app, cx, &MenuCommand::DeleteClip(clip)));
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.selected_clip,
                Some(clip),
                "the menu is titled after a clip it has also selected"
            );
        });
    }

    /// A clip's tune can be taken as the motif, and the sheet opens holding its line.
    ///
    /// The line and not the notes: what lands on the sheet is scale steps around the first
    /// note, which is exactly the text `motif = "0 2 4 2"` would put in a specification.
    #[gpui::test]
    fn taking_a_clip_as_the_motif_opens_the_sheet_holding_its_line(cx: &mut TestAppContext) {
        let (app, cx, track, clip) = with_a_clip(cx);
        // C E G E in the document's C major: the line `0 2 4 2`.
        app.update(cx, |this, _| {
            for (index, pitch) in [60u8, 64, 67, 64].into_iter().enumerate() {
                this.session
                    .add_note(
                        clip,
                        Note::new(pitch, Ticks(index as i64 * 960), Ticks(960)),
                    )
                    .expect("the clip takes a note");
            }
        });
        paint(&app, cx);
        let at = lane_point(&app, cx, track, HALF_CLIP);

        right_press(cx, at);
        assert!(offers(&app, cx, &MenuCommand::TakeClipAsMotif(clip)));
        choose(&app, cx, &MenuCommand::TakeClipAsMotif(clip));

        app.read_with(cx, |this, _| {
            let dials = this.song_sheet.as_ref().expect("the song sheet opened");
            assert_eq!(dials.motif, [0, 2, 4, 2]);
        });
    }

    /// An empty clip has no line, and its menu does not offer to take one.
    #[gpui::test]
    fn an_empty_clip_offers_no_motif_to_take(cx: &mut TestAppContext) {
        let (app, cx, track, clip) = with_a_clip(cx);
        let at = lane_point(&app, cx, track, HALF_CLIP);

        right_press(cx, at);

        assert!(!offers(&app, cx, &MenuCommand::TakeClipAsMotif(clip)));
    }

    /// Empty lane is a different menu, and getting this wrong offers commands about a clip that
    /// is not there.
    #[gpui::test]
    fn a_right_press_on_empty_lane_offers_nothing_about_a_clip(cx: &mut TestAppContext) {
        let (app, cx, track, clip) = with_a_clip(cx);
        let at = lane_point(&app, cx, track, CLIP_LENGTH * 3);

        right_press(cx, at);

        let rows = rows(&app, cx);
        assert!(
            !rows
                .iter()
                .any(|(row, _)| *row == MenuCommand::DeleteClip(clip)),
            "the lane's menu does not act on a clip elsewhere on it: {rows:?}"
        );
    }

    /// A MIDI clip has no gain, no fades and no source tempo, and its menu does not name them.
    ///
    /// Not greyed — *absent*. A menu is titled after one object and its rows are the things that
    /// can be done to that object, so a row offering to clear the fades of a clip that has none
    /// is not saying "not now", it is saying something untrue about the clip it is named after.
    ///
    /// Four rows, which is the case for checking it at all: each is a separate `is_midi` somebody
    /// could get the sense of backwards, and the only sign of that would be a row that refuses
    /// when it is chosen.
    #[gpui::test]
    fn a_midi_clips_menu_does_not_name_anything_that_belongs_to_audio(cx: &mut TestAppContext) {
        let (app, cx, track, clip) = with_a_clip(cx);
        let at = lane_point(&app, cx, track, HALF_CLIP);

        right_press(cx, at);

        let rows = rows(&app, cx);
        for command in [
            MenuCommand::ClipGain(clip),
            MenuCommand::Crossfade(clip),
            MenuCommand::ClearFades(clip),
            MenuCommand::ClipSourceTempo(clip),
        ] {
            assert!(
                !rows.iter().any(|(row, _)| *row == command),
                "{command:?} is not something a MIDI clip can do, so it is not in its menu"
            );
        }
        // And the one that only a MIDI clip can do is offered.
        assert!(offers(&app, cx, &MenuCommand::EditClip(clip)));
    }

    /// The rows that depend on a selection are not there when there is no selection.
    ///
    /// The note menu is nearly all of them — cut, copy, transpose, quantise, the dynamics — and
    /// with nothing picked out it used to open as a wall of grey with one live row at the bottom.
    #[gpui::test]
    fn the_note_menu_with_nothing_selected_offers_only_what_needs_no_selection(
        cx: &mut TestAppContext,
    ) {
        let (app, cx, _, clip) = with_a_clip(cx);
        let commands = app.update(cx, |this, _| {
            this.open_clip_in_editor(clip);
            this.selected_notes.clear();
            this.roll_menu(
                gpui::point(gpui::px(10.0), gpui::px(10.0)),
                None,
                60,
                Ticks::ZERO,
            )
            .entries
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Item(item) => Some(item.command.clone()),
                MenuEntry::Separator => None,
            })
            .collect::<Vec<_>>()
        });

        assert!(
            !commands.contains(&MenuCommand::CutNotes)
                && !commands.contains(&MenuCommand::DeleteNotes),
            "nothing is selected, so there is nothing to cut or delete: {commands:?}"
        );
        assert!(
            commands.contains(&MenuCommand::NewNote {
                pitch: 60,
                start: Ticks::ZERO
            }),
            "and what is left is the row that needs no selection: {commands:?}"
        );
    }

    /// A cut on the clip's own edge would make an empty clip and leave the other exactly as it
    /// was, so the row is shown but cannot be chosen.
    ///
    /// Only this direction: the playhead a headless session reads is an atomic the audio thread
    /// writes, so a seek is invisible here and there is no way to put it *inside* the clip. The
    /// other direction is [`super::splittable`]'s own test.
    #[gpui::test]
    fn split_cannot_be_chosen_with_the_playhead_on_the_clips_edge(cx: &mut TestAppContext) {
        let (app, cx, track, clip) = with_a_clip(cx);
        let at = lane_point(&app, cx, track, HALF_CLIP);

        right_press(cx, at);

        assert!(
            !offers(&app, cx, &MenuCommand::SplitClipAtPlayhead(clip)),
            "the playhead is at zero, which is the clip's own start"
        );
    }

    /// Choosing a row does what the row says, through the click rather than around it.
    #[gpui::test]
    fn choosing_delete_removes_the_clip(cx: &mut TestAppContext) {
        let (app, cx, track, clip) = with_a_clip(cx);
        let at = lane_point(&app, cx, track, HALF_CLIP);
        right_press(cx, at);
        paint(&app, cx);

        choose(&app, cx, &MenuCommand::DeleteClip(clip));

        app.read_with(cx, |this, _| {
            assert!(this.session.midi_clip(clip).is_none(), "the clip is gone");
            assert!(this.menu.is_none(), "and the menu closed behind it");
        });
    }

    /// A toggle row reports the state it would change, so the tick beside it is not a guess.
    #[gpui::test]
    fn choosing_mute_mutes_the_clip_and_the_row_says_so_next_time(cx: &mut TestAppContext) {
        let (app, cx, track, clip) = with_a_clip(cx);
        let at = lane_point(&app, cx, track, HALF_CLIP);
        right_press(cx, at);
        paint(&app, cx);

        choose(&app, cx, &MenuCommand::ToggleClipMute(clip));
        paint(&app, cx);

        right_press(cx, at);
        let ticked = app.read_with(cx, |this, _| {
            this.menu
                .as_ref()
                .expect("a menu is open")
                .entries
                .iter()
                .any(|entry| match entry {
                    MenuEntry::Item(item) => {
                        item.command == MenuCommand::ToggleClipMute(clip) && item.checked
                    }
                    MenuEntry::Separator => false,
                })
        });
        assert!(ticked, "the row it was chosen from now reads as on");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clip_is_splittable_only_strictly_inside_itself() {
        let (start, end) = (Ticks(960), Ticks(3840));
        assert!(splittable(Ticks(1920), start, end), "in the middle");
        // Both edges. A cut here makes an empty clip and leaves the other one whole, which is a
        // menu row that appears to do nothing.
        assert!(!splittable(start, start, end));
        assert!(!splittable(end, start, end));
        assert!(!splittable(Ticks(0), start, end), "before it");
        assert!(!splittable(Ticks(9_000), start, end), "after it");
    }

    #[test]
    fn a_clip_with_no_length_can_never_be_split() {
        let at = Ticks(960);
        assert!(!splittable(at, at, at));
    }
}
