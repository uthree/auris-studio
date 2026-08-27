//! What a singer track can be asked to do.
//!
//! Adding one, putting words on its notes, correcting the phonemes a word became, and writing
//! the frames its voice model is fed. The order of operations inside each command follows the
//! rule the other command files follow — everything that can refuse does so *before* anything
//! is recorded, so a failed command costs no undo step — and the one piece of machinery of its
//! own here is the dictionary: loaded once when the settings name a folder, owned by the
//! session, and consulted only for text the built-in kana table cannot read.

use std::path::Path;

use auris_core::{ClipId, TrackId};
use auris_vocal::{
    JapaneseDictionary, SingerFrames, kana_phonemes, lyric_phonemes, phoneme_moras, render_frames,
    split_kana_moras,
};

use crate::error::SessionError;
use crate::history::Edit;

use super::Session;

/// The lyric written on every note of a phrase after its first, OpenUTAU's way.
///
/// A kanji phrase distributed over notes has no per-note spelling to show — the word is one
/// thing and the notes are many — so the first note carries the word and the rest carry this,
/// which anybody who has lyriced a run of notes elsewhere already reads as "still that word".
pub const LYRIC_CONTINUATION: &str = "+";

impl Session {
    /// Appends a singer track, previewing through the built-in vocal instrument.
    pub fn add_singer_track(&mut self, name: impl Into<String>) -> TrackId {
        self.record(Edit::AddSingerTrack);
        let id = self.project.add_singer_track(name, auris_synth::Vocal::ID);
        self.invalidate_graph();
        id
    }

    /// Points the Japanese text frontend at a compiled dictionary folder, or unloads it.
    ///
    /// Not an edit: which machine has a dictionary is a fact about the machine, exactly like
    /// which folders hold its plugins, and an Undo that unloaded one would be a surprise. The
    /// folder is opened here rather than at first use so a wrong path fails at the settings
    /// screen that names it, not under a lyric someone typed an hour later.
    pub fn set_japanese_dictionary(&mut self, folder: Option<&Path>) -> Result<(), SessionError> {
        self.japanese = match folder {
            Some(folder) => Some(JapaneseDictionary::load(folder)?),
            None => None,
        };
        Ok(())
    }

    /// The dictionary folder currently loaded, if one is.
    pub fn japanese_dictionary(&self) -> Option<&Path> {
        self.japanese.as_ref().map(JapaneseDictionary::path)
    }

    /// Writes one note's lyric, and the phonemes it will be sung as.
    ///
    /// Both fields in one command, because that is what typing a word *means*: the phonemes are
    /// derived on the spot — kana through the built-in table, anything else through the
    /// dictionary — and a text that cannot be read fails the whole command, leaving the note
    /// exactly as it was. An empty lyric clears both, which is how a word is taken off a note.
    pub fn set_note_lyric(
        &mut self,
        clip: ClipId,
        index: usize,
        lyric: &str,
    ) -> Result<(), SessionError> {
        self.require_note(clip, index)?;
        let phonemes = lyric_phonemes(lyric, self.japanese.as_ref())?;
        self.record(Edit::SetLyric);
        if let Some(note) = self
            .project
            .midi_clip_mut(clip)
            .and_then(|target| target.notes.get_mut(index))
        {
            note.lyric = lyric.trim().to_string();
            note.phonemes = phonemes;
        }
        Ok(())
    }

    /// Overwrites one note's phonemes, leaving its lyric as written.
    ///
    /// The escape hatch the stored-phonemes design exists for: a reading the table or the
    /// dictionary got wrong, or a language neither knows, corrected symbol by symbol. Empty
    /// tokens are dropped rather than stored — a phoneme with no symbol is nothing to sing.
    pub fn set_note_phonemes(
        &mut self,
        clip: ClipId,
        index: usize,
        phonemes: Vec<String>,
    ) -> Result<(), SessionError> {
        self.require_note(clip, index)?;
        self.record(Edit::SetPhonemes);
        if let Some(note) = self
            .project
            .midi_clip_mut(clip)
            .and_then(|target| target.notes.get_mut(index))
        {
            note.phonemes = phonemes
                .into_iter()
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect();
        }
        Ok(())
    }

    /// Lays a phrase across a run of notes, one mora to each, and says how many it filled.
    ///
    /// The notes are taken in timeline order whatever order the indices arrive in, because the
    /// phrase has one. Kana is split by the table, so each note gets its own mora as its lyric —
    /// こんにちは across five notes reads こ ん に ち は. Text the table cannot read goes
    /// through the dictionary as a whole (a word's reading depends on all of it), the phonemes
    /// are split at each syllabic, and the first note carries the word with
    /// [`LYRIC_CONTINUATION`] on the rest. Notes past the end of the phrase are left alone —
    /// filling a verse one line at a time is the ordinary use, and a line that cleared the rest
    /// of the verse would make it the only one.
    pub fn write_lyrics(
        &mut self,
        clip: ClipId,
        indices: &[usize],
        text: &str,
    ) -> Result<usize, SessionError> {
        let Some((_, target)) = self.project.midi_clip(clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        // Timeline order, dropping indices that name no note — the same forgiveness every
        // other many-note command extends to a stale selection.
        let mut order: Vec<usize> = indices
            .iter()
            .copied()
            .filter(|index| *index < target.notes.len())
            .collect();
        order.sort_by_key(|index| (target.notes[*index].start, *index));
        order.dedup();

        // Each note's new words, worked out in full before anything is recorded.
        let portions: Vec<(String, Vec<String>)> = match split_kana_moras(text.trim()) {
            Some(moras) => moras
                .into_iter()
                .map(|mora| {
                    let phonemes = kana_phonemes(&mora).unwrap_or_default();
                    (mora, phonemes)
                })
                .collect(),
            None => {
                let phonemes = lyric_phonemes(text, self.japanese.as_ref())?;
                phoneme_moras(&phonemes)
                    .into_iter()
                    .enumerate()
                    .map(|(at, mora)| match at {
                        0 => (text.trim().to_string(), mora),
                        _ => (LYRIC_CONTINUATION.to_string(), mora),
                    })
                    .collect()
            }
        };

        let filled = order.len().min(portions.len());
        if filled == 0 {
            return Ok(0);
        }
        self.record(Edit::WriteLyrics);
        if let Some(target) = self.project.midi_clip_mut(clip) {
            for (index, (lyric, phonemes)) in order.into_iter().zip(portions) {
                if let Some(note) = target.notes.get_mut(index) {
                    note.lyric = lyric;
                    note.phonemes = phonemes;
                }
            }
        }
        Ok(filled)
    }

    /// Sets the seconds-per-frame a singer track's features are sampled at.
    ///
    /// Clamped into 1–100 ms rather than refused: every value in that range is a hop some model
    /// somewhere uses, and outside it is either a frame per sample or a frame per phrase,
    /// neither of which anybody means.
    pub fn set_frame_hop(&mut self, track: TrackId, seconds: f64) -> Result<(), SessionError> {
        let singer = self.require_singer(track)?;
        let seconds = match seconds.is_finite() {
            true => seconds.clamp(0.001, 0.1),
            false => auris_core::default_frame_hop(),
        };
        if singer.frame_hop == seconds {
            return Ok(());
        }
        self.record(Edit::SetFrameHop);
        if let Some(singer) = self
            .project
            .track_mut(track)
            .and_then(|track| track.kind.as_singer_mut())
        {
            singer.frame_hop = seconds;
        }
        Ok(())
    }

    /// The frames a singer track's voice model would be fed right now.
    ///
    /// A question, not a command: nothing is recorded and nothing changes. The frontend's
    /// export goes through this, and so can anything that wants to show the sequences.
    pub fn singer_frames(&self, track: TrackId) -> Result<SingerFrames, SessionError> {
        let singer = self.require_singer(track)?;
        Ok(render_frames(singer, &self.project.tempo_map))
    }

    /// Writes a singer track's frames to `path` as JSON, and says how many frames there were.
    ///
    /// Compact JSON rather than the pretty form the project file uses: three arrays with an
    /// entry every ten milliseconds is a file a machine reads, and one number per line would
    /// quintuple it for nobody.
    pub fn export_singer_frames(
        &mut self,
        track: TrackId,
        path: &Path,
    ) -> Result<usize, SessionError> {
        let frames = self.singer_frames(track)?;
        let text = serde_json::to_string(&frames)
            .map_err(|error| SessionError::Io(auris_io::IoError::Json(error)))?;
        std::fs::write(path, text)
            .map_err(|error| SessionError::Io(auris_io::IoError::from_fs(path, error)))?;
        Ok(frames.len())
    }

    /// The note, or the error naming what was missing — asked before an edit is recorded.
    fn require_note(&self, clip: ClipId, index: usize) -> Result<(), SessionError> {
        let Some((_, target)) = self.project.midi_clip(clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        match index < target.notes.len() {
            true => Ok(()),
            false => Err(SessionError::UnknownNote {
                clip: clip.0,
                index,
            }),
        }
    }

    /// The singer track, or the error saying what the track actually is.
    fn require_singer(&self, track: TrackId) -> Result<&auris_core::SingerTrack, SessionError> {
        let found = self
            .project
            .track(track)
            .ok_or(SessionError::UnknownTrack(track.0))?;
        found
            .kind
            .as_singer()
            .ok_or_else(|| SessionError::WrongTrackKind {
                id: track.0,
                actual: found.kind.label(),
                expected: "a singer track",
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::session;
    use auris_core::time::Ticks;
    use auris_core::{Note, default_frame_hop};

    /// A session holding one singer track with one four-beat clip of `count` quarter notes.
    fn sung(count: usize) -> (crate::Session, TrackId, ClipId) {
        let mut session = session();
        let track = session.add_singer_track("Melody");
        let clip = session
            .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks::from_beats(4.0))
            .expect("a singer track takes a note clip");
        for at in 0..count {
            session
                .add_note(
                    clip,
                    Note::new(60 + at as u8, Ticks::from_beats(at as f64), Ticks::QUARTER),
                )
                .unwrap();
        }
        (session, track, clip)
    }

    #[test]
    fn a_kana_lyric_lands_with_its_phonemes_and_undoes_as_one_step() {
        let (mut session, _, clip) = sung(1);
        session.set_note_lyric(clip, 0, "きょ").unwrap();
        let note = &session.midi_clip(clip).unwrap().notes[0];
        assert_eq!(note.lyric, "きょ");
        assert_eq!(note.phonemes, ["kʲ", "o"]);

        session.undo();
        let note = &session.midi_clip(clip).unwrap().notes[0];
        assert!(note.lyric.is_empty() && note.phonemes.is_empty());
    }

    #[test]
    fn an_empty_lyric_takes_the_word_off_the_note() {
        let (mut session, _, clip) = sung(1);
        session.set_note_lyric(clip, 0, "さ").unwrap();
        session.set_note_lyric(clip, 0, "  ").unwrap();
        let note = &session.midi_clip(clip).unwrap().notes[0];
        assert!(note.lyric.is_empty() && note.phonemes.is_empty());
    }

    #[test]
    fn kanji_without_a_dictionary_fails_whole_and_names_the_cure() {
        let (mut session, _, clip) = sung(1);
        let error = session.set_note_lyric(clip, 0, "歌").unwrap_err();
        assert!(matches!(
            error,
            SessionError::Vocal(auris_vocal::VocalError::NeedsDictionary { .. })
        ));
        // Nothing was written and nothing was recorded: the last undoable step is still the
        // note that was added, not a lyric that never landed.
        let note = &session.midi_clip(clip).unwrap().notes[0];
        assert!(note.lyric.is_empty());
        assert_eq!(session.undo(), Some(crate::history::Edit::AddNote));
    }

    #[test]
    fn phonemes_can_be_corrected_without_touching_the_word() {
        let (mut session, _, clip) = sung(1);
        session.set_note_lyric(clip, 0, "は").unwrap();
        session
            .set_note_phonemes(clip, 0, vec!["ɸ".into(), " ".into(), "a".into()])
            .unwrap();
        let note = &session.midi_clip(clip).unwrap().notes[0];
        assert_eq!(note.lyric, "は", "the word stays as written");
        assert_eq!(note.phonemes, ["ɸ", "a"], "blank tokens are dropped");
    }

    #[test]
    fn a_kana_phrase_falls_one_mora_to_a_note_in_timeline_order() {
        let (mut session, _, clip) = sung(5);
        // Indices deliberately shuffled: the phrase follows the music, not the selection.
        let filled = session
            .write_lyrics(clip, &[3, 0, 4, 1, 2], "こんにちは")
            .unwrap();
        assert_eq!(filled, 5);
        let notes = &session.midi_clip(clip).unwrap().notes;
        let lyrics: Vec<&str> = notes.iter().map(|note| note.lyric.as_str()).collect();
        assert_eq!(lyrics, ["こ", "ん", "に", "ち", "は"]);
        assert_eq!(notes[1].phonemes, ["ɴ"]);

        // One step: a pasted phrase undoes as a phrase.
        session.undo();
        assert!(
            session
                .midi_clip(clip)
                .unwrap()
                .notes
                .iter()
                .all(|note| note.lyric.is_empty())
        );
    }

    #[test]
    fn a_short_phrase_leaves_the_notes_past_it_alone() {
        let (mut session, _, clip) = sung(3);
        session.set_note_lyric(clip, 2, "ら").unwrap();
        let filled = session.write_lyrics(clip, &[0, 1, 2], "さく").unwrap();
        assert_eq!(filled, 2);
        let notes = &session.midi_clip(clip).unwrap().notes;
        assert_eq!(notes[2].lyric, "ら", "the third note keeps its word");
    }

    #[test]
    fn the_frame_hop_is_a_singer_setting_and_a_bus_is_told_so() {
        let (mut session, track, _) = sung(1);
        session.set_frame_hop(track, 0.005).unwrap();
        let singer = |session: &crate::Session, track| {
            session
                .project()
                .track(track)
                .unwrap()
                .kind
                .as_singer()
                .unwrap()
                .frame_hop
        };
        assert_eq!(singer(&session, track), 0.005);
        // Nonsense clamps or falls back rather than refusing.
        session.set_frame_hop(track, f64::NAN).unwrap();
        assert_eq!(singer(&session, track), default_frame_hop());
        session.set_frame_hop(track, 99.0).unwrap();
        assert_eq!(singer(&session, track), 0.1);

        let bus = session.add_bus_track("Mix");
        assert!(matches!(
            session.set_frame_hop(bus, 0.01),
            Err(SessionError::WrongTrackKind { .. })
        ));
    }

    #[test]
    fn exported_frames_land_on_disk_and_read_back() {
        let (mut session, track, clip) = sung(2);
        session.write_lyrics(clip, &[0, 1], "さく").unwrap();

        let dir = std::env::temp_dir().join("auris-singer-frames-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("melody.frames.json");
        let count = session.export_singer_frames(track, &path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let frames: SingerFrames = serde_json::from_str(&text).unwrap();
        assert_eq!(frames.len(), count);
        assert!(count > 0);
        assert_eq!(frames.inventory[0], auris_vocal::SILENCE);
        assert!(frames.inventory.iter().any(|p| p == "s"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_singer_track_previews_through_the_vocal_instrument() {
        let (session, track, _) = sung(1);
        let singer = session
            .project()
            .track(track)
            .unwrap()
            .kind
            .as_singer()
            .unwrap();
        assert_eq!(singer.instrument_id, auris_synth::Vocal::ID);
        assert_eq!(singer.frame_hop, default_frame_hop());
    }
}
