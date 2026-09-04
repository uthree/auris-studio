//! What a singer track can be asked to do.
//!
//! Adding one, putting words on its notes, correcting the phonemes a word became, and writing
//! the frames its voice model is fed. The order of operations inside each command follows the
//! rule the other command files follow — everything that can refuse does so *before* anything
//! is recorded, so a failed command costs no undo step — and the one piece of machinery of its
//! own here is the dictionary: loaded once when the settings name a folder, owned by the
//! session, and consulted only for text the built-in kana table cannot read.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use auris_core::project::BEND_LIMIT;
use auris_core::{AssetPath, ClipId, Fall, Scoop, SingerTake, SingerVoice, TrackId, Vibrato};
use auris_singer::VoiceModel;
use auris_vocal::{
    JapaneseDictionary, SingerFrames, SingerScore, lyric_phonemes, phoneme_moras, render_frames,
    render_score, split_kana_lyric,
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

/// How long an auditioned note is held, in seconds.
///
/// Long enough to hear the syllable open and settle, short enough that a drag across the
/// keyboard answers as it moves. Fixed rather than the note's own length so a preview is one
/// render whatever is grabbed, and so a cache keyed on pitch and phonemes actually hits.
pub const PREVIEW_NOTE_SECONDS: f64 = 0.5;

/// Silence appended after the held note, where the voice lets go of the syllable.
const PREVIEW_TAIL_SECONDS: f64 = 0.3;

/// The narrowest a phoneme may be pinned, in seconds.
///
/// One frame at the default hop: any narrower and the pin rounds to no frames at all, which
/// reads as the drag refusing to work.
pub const MIN_PHONEME_SECONDS: f64 = 0.01;

impl Session {
    /// Appends a singer track, previewing through the built-in vocal instrument.
    pub fn add_singer_track(&mut self, name: impl Into<String>) -> TrackId {
        self.record(Edit::AddSingerTrack);
        let id = self.project.add_singer_track(name, auris_synth::Vocal::ID);
        self.invalidate_graph();
        id
    }

    /// Points the Japanese text frontend at a compiled dictionary folder, or lets it go.
    ///
    /// Not an edit: which machine has a dictionary is a fact about the machine, exactly like
    /// which folders hold its plugins, and an Undo that unloaded one would be a surprise. The
    /// folder is opened here rather than at first use so a wrong path fails at the settings
    /// screen that names it, not under a lyric someone typed an hour later.
    ///
    /// `None` does not mean *no dictionary* on a build that ships one: clearing the setting
    /// returns to the shipped dictionary, because the setting is an *override* — the way to
    /// swap in a folder of one's own — and taking the override off should restore the
    /// default, not silence the kanji.
    pub fn set_japanese_dictionary(&mut self, folder: Option<&Path>) -> Result<(), SessionError> {
        self.japanese = match folder {
            Some(folder) => Some(JapaneseDictionary::load(folder)?),
            None => None,
        };
        if folder.is_none() {
            self.install_shipped_dictionary();
        }
        Ok(())
    }

    /// Loads the Japanese dictionary the build ships with, where one is installed.
    ///
    /// A failure is logged rather than fatal: a session without the shipped dictionary is
    /// still a session — kana lyrics sing through the built-in table — and a *named* folder
    /// that fails gets its visible error at the settings screen instead, through
    /// [`Self::set_japanese_dictionary`].
    pub(super) fn install_shipped_dictionary(&mut self) {
        if !self.shipped_dictionary || self.japanese.is_some() {
            return;
        }
        if let Some(folder) = crate::library::installed_dictionary() {
            match JapaneseDictionary::load(&folder) {
                Ok(dictionary) => self.japanese = Some(dictionary),
                Err(error) => log::warn!("the shipped dictionary did not load: {error}"),
            }
        }
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
            // A pin belongs to the phoneme it was placed on; the new word has new ones.
            note.phoneme_seconds.clear();
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
            // A pin belongs to the phoneme it was placed on, and those were just rewritten.
            note.phoneme_seconds.clear();
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

        // Each note's new words, worked out in full before anything is recorded. The moras come
        // with their phonemes attached — asking `kana_phonemes` of each mora on its own would
        // lose ー its vowel, which only the mora before it knows.
        let portions: Vec<(String, Vec<String>)> = match split_kana_lyric(text.trim()) {
            Some(moras) => moras,
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
                    // A pin belongs to the phoneme it was placed on; the phrase brought new
                    // ones.
                    note.phoneme_seconds.clear();
                }
            }
        }
        Ok(filled)
    }

    /// Pins one phoneme of one note to `seconds` of sung time, or hands it back to the rule.
    ///
    /// The hand adjustment behind dragging a boundary in the roll: the pin rides the note in
    /// [`Note::phoneme_seconds`](auris_core::Note::phoneme_seconds), and
    /// [`auris_vocal::phoneme_layout`] bends the timing rule around it. `None` unpins. An
    /// index past the note's phonemes is forgiven rather than refused — the allowance every
    /// many-note command extends to a stale selection — and so is a note with no phonemes,
    /// whose placeholder vowel is nothing a person ever placed. Repeated pins on one
    /// boundary fold into one undo step, because a drag arrives as repeats.
    pub fn set_phoneme_duration(
        &mut self,
        clip: ClipId,
        note: usize,
        phoneme: usize,
        seconds: Option<f64>,
    ) -> Result<(), SessionError> {
        self.require_note(clip, note)?;
        let count = self
            .project
            .midi_clip(clip)
            .and_then(|(_, target)| target.notes.get(note))
            .map(|target| target.phonemes.len())
            .unwrap_or(0);
        if phoneme >= count {
            return Ok(());
        }
        if let Some(seconds) = seconds
            && !seconds.is_finite()
        {
            return Err(SessionError::NotFinite(seconds));
        }
        let frame_hop = self.frame_hop_for_clip(clip);
        // Clamped rather than refused, unlike most bounded numbers at this door, because the
        // number arrives from a drag: the hand at the edge of the range wants the edge.
        let seconds = seconds.map(|seconds| seconds.clamp(frame_hop, 10.0));
        self.record_repeating(Edit::SetPhonemeDuration(clip, note, phoneme));
        if let Some(target) = self
            .project
            .midi_clip_mut(clip)
            .and_then(|target| target.notes.get_mut(note))
        {
            if target.phoneme_seconds.len() < count {
                target.phoneme_seconds.resize(count, 0.0);
            }
            target.phoneme_seconds[phoneme] = seconds.unwrap_or(0.0);
            // Trailing zeros say nothing; dropping them keeps an unpinned note's file entry
            // exactly what it was before this field existed.
            while target.phoneme_seconds.last() == Some(&0.0) {
                target.phoneme_seconds.pop();
            }
        }
        Ok(())
    }

    /// Takes every pin off one note, handing the whole syllable back to the timing rule.
    pub fn clear_phoneme_timing(&mut self, clip: ClipId, note: usize) -> Result<(), SessionError> {
        self.require_note(clip, note)?;
        let pinned = self
            .project
            .midi_clip(clip)
            .and_then(|(_, target)| target.notes.get(note))
            .is_some_and(|target| !target.phoneme_seconds.is_empty());
        if !pinned {
            return Ok(());
        }
        self.record(Edit::ResetPhonemeTiming);
        if let Some(target) = self
            .project
            .midi_clip_mut(clip)
            .and_then(|target| target.notes.get_mut(note))
        {
            target.phoneme_seconds.clear();
        }
        Ok(())
    }

    /// Puts a scoop on one note — a rise into it from below — adjusts it, or takes it off.
    ///
    /// The ornament rides the note in [`Note::scoop`](auris_core::Note::scoop), and
    /// [`auris_vocal::ornament_offset`] is the one implementation of its shape. Numbers are
    /// clamped rather than refused, like a phoneme pin's, because they arrive from a drag:
    /// the hand at the edge of the range wants the edge. Repeats on one note fold into one
    /// undo step, because a drag arrives as repeats.
    pub fn set_note_scoop(
        &mut self,
        clip: ClipId,
        note: usize,
        scoop: Option<Scoop>,
    ) -> Result<(), SessionError> {
        self.require_note(clip, note)?;
        let frame_hop = self.frame_hop_for_clip(clip);
        let scoop = match scoop {
            Some(scoop) => Some(Scoop {
                depth: ornament_depth(scoop.depth)?,
                seconds: ornament_seconds(scoop.seconds, frame_hop)?,
            }),
            None => None,
        };
        self.record_repeating(Edit::SetScoop(clip, note));
        if let Some(target) = self
            .project
            .midi_clip_mut(clip)
            .and_then(|target| target.notes.get_mut(note))
        {
            target.scoop = scoop;
        }
        Ok(())
    }

    /// Puts a fall on one note — a drop away at its end — adjusts it, or takes it off.
    ///
    /// Everything said of [`Session::set_note_scoop`] holds here too.
    pub fn set_note_fall(
        &mut self,
        clip: ClipId,
        note: usize,
        fall: Option<Fall>,
    ) -> Result<(), SessionError> {
        self.require_note(clip, note)?;
        let frame_hop = self.frame_hop_for_clip(clip);
        let fall = match fall {
            Some(fall) => Some(Fall {
                depth: ornament_depth(fall.depth)?,
                seconds: ornament_seconds(fall.seconds, frame_hop)?,
            }),
            None => None,
        };
        self.record_repeating(Edit::SetFall(clip, note));
        if let Some(target) = self
            .project
            .midi_clip_mut(clip)
            .and_then(|target| target.notes.get_mut(note))
        {
            target.fall = fall;
        }
        Ok(())
    }

    fn frame_hop_for_clip(&self, clip: ClipId) -> f64 {
        self.project
            .tracks
            .iter()
            .filter_map(|track| track.kind.as_singer())
            .find(|singer| singer.clips.iter().any(|candidate| candidate.id == clip))
            .map_or(MIN_PHONEME_SECONDS, |singer| {
                singer.frame_hop.max(MIN_PHONEME_SECONDS)
            })
    }

    /// Puts a vibrato on one note — a sway around its pitch — adjusts it, or takes it off.
    ///
    /// Everything said of [`Session::set_note_scoop`] holds here too. The rate is clamped
    /// into 0.1–12 Hz: below is a slow bend nobody hears as vibrato, above is a trill no
    /// singer produces.
    pub fn set_note_vibrato(
        &mut self,
        clip: ClipId,
        note: usize,
        vibrato: Option<Vibrato>,
    ) -> Result<(), SessionError> {
        self.require_note(clip, note)?;
        let vibrato = match vibrato {
            Some(vibrato) => {
                for value in [vibrato.delay, vibrato.fade_in] {
                    if !value.is_finite() {
                        return Err(SessionError::NotFinite(value));
                    }
                }
                if !vibrato.rate.is_finite() {
                    return Err(SessionError::NotFinite(f64::from(vibrato.rate)));
                }
                Some(Vibrato {
                    depth: ornament_depth(vibrato.depth)?,
                    rate: vibrato.rate.clamp(0.1, 12.0),
                    delay: vibrato.delay.clamp(0.0, 10.0),
                    fade_in: vibrato.fade_in.clamp(0.0, 10.0),
                })
            }
            None => None,
        };
        self.record_repeating(Edit::SetVibrato(clip, note));
        if let Some(target) = self
            .project
            .midi_clip_mut(clip)
            .and_then(|target| target.notes.get_mut(note))
        {
            target.vibrato = vibrato;
        }
        Ok(())
    }

    /// Takes every pitch ornament off one note, in one step.
    pub fn clear_note_ornaments(&mut self, clip: ClipId, note: usize) -> Result<(), SessionError> {
        self.require_note(clip, note)?;
        let worn = self
            .project
            .midi_clip(clip)
            .and_then(|(_, target)| target.notes.get(note))
            .is_some_and(|target| {
                target.scoop.is_some() || target.fall.is_some() || target.vibrato.is_some()
            });
        if !worn {
            return Ok(());
        }
        self.record(Edit::ResetOrnaments);
        if let Some(target) = self
            .project
            .midi_clip_mut(clip)
            .and_then(|target| target.notes.get_mut(note))
        {
            target.scoop = None;
            target.fall = None;
            target.vibrato = None;
        }
        Ok(())
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

    /// Reads frames off disk — [`Session::export_singer_frames`]'s file, or anything else
    /// written in its shape.
    ///
    /// An associated function rather than a method because no document is involved: the
    /// frames are one track's features already sampled, and where they came from is not this
    /// reader's business. That is the point of the door — [`Session::sing_frames`] is how a
    /// voice is handed curves that a corpus was *recorded* with, not curves the timing rules
    /// laid out, so what comes back can be held against the recording.
    pub fn read_singer_frames(path: &Path) -> Result<SingerFrames, SessionError> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| SessionError::Io(auris_io::IoError::from_fs(path, error)))?;
        let frames = serde_json::from_str(&text)
            .map_err(|error| SessionError::Io(auris_io::IoError::Json(error)))?;
        auris_singer::validate_frames(&frames)?;
        Ok(frames)
    }

    /// Sings frames through a voice model into the WAV file at `path`, and nothing else.
    ///
    /// The same inference a take goes through — the model from the same cache, loaded the way
    /// the acceleration setting says, cut into the same chunks and stitched the same way,
    /// written by the same recorder — with the notes taken off the front and the document off
    /// the back. That is what makes it a measuring instrument: hand it the curves a corpus
    /// was recorded with and compare what comes back with the recording, and every
    /// difference belongs to the export or to this host, because the score-to-frames step
    /// never ran. Nothing is recorded in the document, and there need not be one.
    ///
    /// The answer carries what a measurement wants beside the audio: how the timeline was
    /// chunked, how long the model took to open and to sing, and whether it sang on the GPU.
    /// The load time is zero when the voice was already open, which the second call in a
    /// batch always finds it.
    pub fn sing_frames(
        &mut self,
        voice: &Path,
        frames: &SingerFrames,
        speaker: Option<&str>,
        seed: u64,
        path: &Path,
    ) -> Result<SungFrames, SessionError> {
        let opening = Instant::now();
        let cached = self.voices.contains_key(voice);
        let model = self.voice_model_at(voice)?;
        let load_seconds = match cached {
            true => 0.0,
            false => opening.elapsed().as_secs_f64(),
        };
        let (samples, sung) = {
            let mut model = model.lock().expect("no thread panics holding a voice");
            let speaker = speaker_id(model.info(), speaker)?;
            let mut chunks = 0;
            let singing = Instant::now();
            let samples = model.sing_with(frames, speaker, seed, |_, total| {
                chunks = total;
                true
            })?;
            let render_seconds = singing.elapsed().as_secs_f64();
            let sample_rate = model.info().sample_rate;
            let sung = SungFrames {
                voice: voice_name(&model, voice),
                speaker: model.info().speakers()[speaker as usize].clone(),
                seconds: samples.len() as f64 / f64::from(sample_rate),
                sample_rate,
                frames: frames.len(),
                chunks,
                load_seconds,
                render_seconds,
                on_gpu: model.on_gpu(),
            };
            (samples, sung)
        };
        let mut recorder = auris_io::WavRecorder::create(path, f64::from(sung.sample_rate), 1)?;
        recorder.write(&samples)?;
        recorder.finish()?;
        Ok(sung)
    }

    /// Points a singer track at a voice model, or takes its voice away.
    ///
    /// The file is opened *before* anything is recorded, so a path that is not a voice fails at
    /// the picker that chose it and costs no undo step. Choosing a voice also sets the track's
    /// frame hop to the model's own — the two disagreeing is an error every render would repeat
    /// — and writes the model's display name into the document, so a track header can say
    /// 波音リツ without opening two hundred megabytes first. Taking the voice away leaves any
    /// rendered take in place: the take is kept audio, not a setting.
    pub fn set_singer_voice(
        &mut self,
        track: TrackId,
        path: Option<&Path>,
    ) -> Result<(), SessionError> {
        self.require_singer(track)?;
        let chosen = match path {
            Some(file) => {
                let model = self.voice_model_at(file)?;
                let (name, hop, consonants, levels) = {
                    let model = model.lock().expect("no thread panics holding a voice");
                    let name = voice_name(&model, file);
                    // The consonant widths ride into the document beside the name, for the
                    // name's reason: the phoneme timing must lay out the same before the
                    // model file is opened again — or on a machine that does not have it.
                    (
                        name,
                        model.info().hop_seconds(),
                        // The first speaker's, until one is chosen.
                        model.info().consonant_widths(0),
                        model.info().consonant_levels(0),
                    )
                };
                let relative = self.project_folder().and_then(|folder| {
                    file.strip_prefix(folder)
                        .ok()
                        .map(Path::to_path_buf)
                        .or_else(|| {
                            if cfg!(target_os = "windows") {
                                let file = std::fs::canonicalize(file).ok()?;
                                let folder = std::fs::canonicalize(folder).ok()?;
                                file.strip_prefix(folder).ok().map(Path::to_path_buf)
                            } else {
                                None
                            }
                        })
                });
                let reference = match relative {
                    Some(relative) => AssetPath::inside(relative),
                    None => AssetPath::external(file),
                };
                Some((reference, name, hop, consonants, levels))
            }
            None => None,
        };
        self.record(Edit::SetSingerVoice);
        if let Some(singer) = self
            .project
            .track_mut(track)
            .and_then(|track| track.kind.as_singer_mut())
        {
            match chosen {
                Some((path, name, hop, consonants, levels)) => {
                    singer.voice = Some(SingerVoice {
                        path,
                        name,
                        consonants,
                        levels,
                        speaker: None,
                    });
                    singer.frame_hop = hop;
                }
                None => singer.voice = None,
            }
        }
        Ok(())
    }

    /// The voice a singer track is sung by, when one has been chosen.
    pub fn singer_voice(&self, track: TrackId) -> Result<Option<&SingerVoice>, SessionError> {
        Ok(self.require_singer(track)?.voice.as_ref())
    }

    /// Tells a singer track which of its voice's speakers sings: a name from
    /// [`Session::singer_speakers`], or `None` for the model's first.
    ///
    /// The name is checked against the model before anything is recorded, so a speaker the
    /// voice does not have is refused here — naming what it does have — and costs no undo step.
    pub fn set_singer_speaker(
        &mut self,
        track: TrackId,
        speaker: Option<&str>,
    ) -> Result<(), SessionError> {
        let model = self.singer_voice_model(track)?;
        let (chosen, consonants, levels) = {
            let model = model.lock().expect("no thread panics holding a voice");
            let offered = model.info().speakers();
            let id = match speaker {
                Some(name) => offered
                    .iter()
                    .position(|known| known == name)
                    .ok_or_else(|| SessionError::NoSuchSpeaker {
                        name: name.to_string(),
                        offered: offered.clone(),
                    })? as u32,
                None => 0,
            };
            // The tables are the speaker's, and ride into the document with the choice for
            // the reason they ride in with the voice: the layout must not change with
            // whether the model file is present.
            (
                speaker.map(str::to_string),
                model.info().consonant_widths(id),
                model.info().consonant_levels(id),
            )
        };
        self.record(Edit::SetSingerSpeaker);
        if let Some(voice) = self
            .project
            .track_mut(track)
            .and_then(|track| track.kind.as_singer_mut())
            .and_then(|singer| singer.voice.as_mut())
        {
            voice.speaker = chosen;
            voice.consonants = consonants;
            voice.levels = levels;
        }
        Ok(())
    }

    /// The speakers a singer track's voice can sing as, in the model's id order.
    ///
    /// Opens the model, so it costs what choosing the voice cost; a track with no voice
    /// refuses the way singing does.
    pub fn singer_speakers(&mut self, track: TrackId) -> Result<Vec<String>, SessionError> {
        let model = self.singer_voice_model(track)?;
        let model = model.lock().expect("no thread panics holding a voice");
        Ok(model.info().speakers())
    }

    /// The id the track's chosen speaker has in its voice model, for a caller's own render.
    ///
    /// The audition half of the speaker rule, the way [`Session::singer_voice_model`] is of
    /// the voice: resolved against the model so a stale name fails here and not mid-render.
    pub fn singer_speaker(&mut self, track: TrackId) -> Result<u32, SessionError> {
        let chosen = self
            .require_singer(track)?
            .voice
            .as_ref()
            .and_then(|voice| voice.speaker.clone());
        let model = self.singer_voice_model(track)?;
        let model = model.lock().expect("no thread panics holding a voice");
        speaker_id(model.info(), chosen.as_deref())
    }

    /// Renders a singer track through its voice model and keeps the result as its take.
    ///
    /// The whole pipeline in one synchronous call — frames, inference, the file, the document —
    /// which is what a CLI or a test wants; a window that must keep painting takes
    /// [`Session::sing_plan`], renders on its own thread, and lands the result through
    /// [`Session::land_singer_take`]. `seed` pins the render's random choices; `None` keeps the
    /// current take's seed (or 0 for a first take), so singing again after an edit is the same
    /// performance of the new text. Answers with the seconds of audio the take now holds.
    pub fn sing(&mut self, track: TrackId, seed: Option<u64>) -> Result<f64, SessionError> {
        let plan = self.sing_plan(track, seed)?;
        let model = self.voice_model_at(&plan.voice)?;
        let samples = {
            let mut model = model.lock().expect("no thread panics holding a voice");
            model.sing_score(&plan.frames, &plan.score, plan.speaker, plan.seed)?
        };
        self.land_singer_take(&plan, &samples)
    }

    /// Everything a background render needs, gathered and checked before any work is spent.
    ///
    /// Refusals come in the order a person can act on them: no voice chosen, nothing to sing,
    /// nowhere to keep the result — and only then is the model file itself opened. The plan owns
    /// its frames, so the thread that renders it borrows nothing from the session.
    pub fn sing_plan(
        &mut self,
        track: TrackId,
        seed: Option<u64>,
    ) -> Result<SingPlan, SessionError> {
        let singer = self.require_singer(track)?;
        let voice = singer.voice.clone().ok_or(SessionError::NoVoice(track.0))?;
        let seed = seed
            .or(singer.take.as_ref().map(|take| take.seed))
            .unwrap_or(0);
        let frames = render_frames(singer, &self.project.tempo_map);
        let score = render_score(singer, &self.project.tempo_map);
        if frames.is_empty() {
            return Err(SessionError::NothingToSing(track.0));
        }
        let folder = self
            .project_folder()
            .ok_or(SessionError::SingingNeedsFolder)?;
        let resolved = voice
            .path
            .resolve(Some(folder))
            .ok_or(SessionError::NoVoice(track.0))?;
        let fingerprint = take_fingerprint(&frames, &voice.path, voice.speaker.as_deref(), seed);
        let model = self.voice_model_at(&resolved)?;
        let (speaker, sample_rate) = {
            let model = model.lock().expect("no thread panics holding a voice");
            (
                speaker_id(model.info(), voice.speaker.as_deref())?,
                model.info().sample_rate,
            )
        };
        Ok(SingPlan {
            track,
            frames,
            score,
            voice: resolved,
            speaker,
            seed,
            fingerprint,
            sample_rate,
        })
    }

    /// The loaded voice behind a singer track, for a caller's own render thread.
    ///
    /// The audition half of [`Session::sing_plan`]: no folder is needed because nothing
    /// lands, and no notes are needed because the note being auditioned is the caller's to
    /// describe. A track with no voice refuses the same way singing does.
    pub fn singer_voice_model(
        &mut self,
        track: TrackId,
    ) -> Result<Arc<Mutex<VoiceModel>>, SessionError> {
        let singer = self.require_singer(track)?;
        let voice = singer.voice.clone().ok_or(SessionError::NoVoice(track.0))?;
        let resolved = voice
            .path
            .resolve(self.project_folder())
            .ok_or(SessionError::NoVoice(track.0))?;
        self.voice_model_at(&resolved)
    }

    /// The frames one auditioned note would sing, in a singer track's voice.
    ///
    /// A tiny score built around the note alone — its pitch, its phonemes, a fixed held
    /// length with a silent tail for the release — rendered through the same
    /// [`render_frames`] as the song, so what a dragged note sounds like and what the take
    /// will sing stay one story. The velocity is the default on purpose: a preview keyed by
    /// every velocity would defeat the cache every caller wants to keep.
    pub fn preview_note_frames(
        &self,
        track: TrackId,
        pitch: u8,
        phonemes: &[String],
    ) -> Result<SingerFrames, SessionError> {
        let singer = self.require_singer(track)?;
        let tempo = &self.project.tempo_map;
        let held = tempo.seconds_to_ticks(auris_core::Seconds(PREVIEW_NOTE_SECONDS));
        let mut clip =
            auris_core::MidiClip::new(ClipId(0), "Preview", auris_core::Ticks::ZERO, held);
        let mut note = auris_core::Note::new(pitch, auris_core::Ticks::ZERO, held);
        note.phonemes = phonemes.to_vec();
        clip.notes.push(note);
        let one_note = auris_core::SingerTrack {
            instrument_id: singer.instrument_id.clone(),
            instrument_state: auris_core::PluginState::default(),
            clips: vec![clip],
            frame_hop: singer.frame_hop,
            // The real track's voice, so the preview's consonants take the widths the take
            // will — a dragged note and the song must stay one story.
            voice: singer.voice.clone(),
            take: None,
        };
        let mut frames = render_frames(&one_note, tempo);
        // The tail is appended rather than scored: silence after the note is where the
        // model lets go of the syllable, and the first inventory entry is always SILENCE.
        let tail = (PREVIEW_TAIL_SECONDS / frames.hop_seconds).ceil() as usize;
        for _ in 0..tail {
            frames.phonemes.push(0);
            frames.f0_hz.push(0.0);
            frames.energy.push(0.0);
        }
        Ok(frames)
    }

    /// Builds the lyric-bearing score paired with [`Self::preview_note_frames`].
    ///
    /// The preview has no written lyric, so score-based engines receive the neutral Japanese
    /// vowel `ア`; the full take always receives the note's actual lyric.
    pub fn preview_note_score(&self, frames: &SingerFrames, pitch: u8) -> SingerScore {
        let tail = (PREVIEW_TAIL_SECONDS / frames.hop_seconds).ceil() as u32;
        let held = (frames.len() as u32).saturating_sub(tail);
        let mut notes = vec![auris_vocal::SingerNote {
            key: None,
            frame_length: 1,
            lyric: String::new(),
        }];
        if held > 1 {
            notes.push(auris_vocal::SingerNote {
                key: Some(pitch),
                frame_length: held - 1,
                lyric: "ア".into(),
            });
        }
        if tail > 0 {
            notes.push(auris_vocal::SingerNote {
                key: None,
                frame_length: tail,
                lyric: String::new(),
            });
        }
        SingerScore { notes }
    }

    /// Turns a preview render into the buffer the engine plays.
    ///
    /// The model sings at its own rate and the engine renders at the device's; the resample
    /// happens here, off the audio thread. The buffer comes back to the caller so the same
    /// note can be played again without another render.
    pub fn singer_preview_buffer(
        &self,
        samples: &[f32],
        source_rate: f64,
    ) -> Arc<auris_core::AudioBuffer> {
        let mut buffer = auris_core::AudioBuffer::new(1, samples.len(), source_rate);
        buffer.channel_mut(0)[..samples.len()].copy_from_slice(samples);
        let rate = self.engine.sample_rate();
        let buffer = match auris_io::resample_buffer(&buffer, rate) {
            Ok(resampled) => resampled,
            // A rate the resampler cannot bridge: the note previews at the wrong speed
            // rather than not at all.
            Err(_) => buffer,
        };
        Arc::new(buffer)
    }

    /// Plays a prepared preview buffer once on a track, immediately.
    ///
    /// The buffer crosses the command channel whole, the discipline every heap crossing to
    /// the audio thread follows; whatever the track was previewing before travels back to be
    /// dropped here on the next garbage sweep.
    pub fn play_singer_preview(&mut self, track: TrackId, buffer: &Arc<auris_core::AudioBuffer>) {
        if let Ok(index) = self.require_track(track) {
            self.send(auris_engine::EngineCommand::PlayOneShot {
                track: index,
                buffer: Arc::clone(buffer),
            });
        }
    }

    /// Silences a track's preview, the way a note-off ends an auditioned note.
    pub fn stop_singer_preview(&mut self, track: TrackId) {
        if let Ok(index) = self.require_track(track) {
            self.send(auris_engine::EngineCommand::StopOneShot { track: index });
        }
    }

    /// The loaded model behind a plan's voice, for rendering on a caller's thread.
    pub fn voice_model_at(&mut self, file: &Path) -> Result<Arc<Mutex<VoiceModel>>, SessionError> {
        let stamp = voice_stamp(file);
        if let Some((cached_stamp, loaded)) = self.voices.get(file)
            && stamp.as_ref() == Some(cached_stamp)
        {
            return Ok(Arc::clone(loaded));
        }
        let model = Arc::new(Mutex::new(VoiceModel::load(file, self.acceleration)?));
        if let Some(stamp) = stamp {
            self.voices
                .insert(file.to_path_buf(), (stamp, Arc::clone(&model)));
        }
        Ok(model)
    }

    /// Chooses where singer voices run their inference, from now on.
    ///
    /// Not an edit, for [`Self::set_japanese_dictionary`]'s reason: which processor a machine
    /// sings on is a fact about the machine, and an Undo that moved a render to the CPU would
    /// be a surprise. Changing it drops every cached model, so the very next render loads the
    /// voice the new way; a render already in flight keeps the model it holds and finishes as
    /// it started.
    pub fn set_singer_acceleration(&mut self, acceleration: auris_singer::Acceleration) {
        if self.acceleration == acceleration {
            return;
        }
        self.acceleration = acceleration;
        self.voices.clear();
    }

    /// Where singer voices run their inference.
    pub fn singer_acceleration(&self) -> auris_singer::Acceleration {
        self.acceleration
    }

    /// Writes a rendered take to disk and into the document, replacing any previous take.
    ///
    /// Everything that can fail — the file, the read-back through the importer — happens before
    /// anything is recorded, so a full disk costs no undo step. The waveform lands in `Audio/`
    /// under the track's name exactly as a recorded take would, is registered as an ordinary
    /// audio source (so reopening the project reloads it with everything else), and the previous
    /// take's source entry goes with the take that owned it; its file stays, as every replaced
    /// take's file does. One edit, one undo step. Answers with the seconds the take holds.
    pub fn land_singer_take(
        &mut self,
        plan: &SingPlan,
        samples: &[f32],
    ) -> Result<f64, SessionError> {
        self.require_singer(plan.track)?;
        let folder = self
            .project_folder()
            .ok_or(SessionError::SingingNeedsFolder)?
            .to_path_buf();
        let name = self
            .project
            .track(plan.track)
            .map(|track| track.name.clone())
            .unwrap_or_else(|| "Singer".to_string());

        let audio_dir = folder.join(auris_io::AUDIO_DIR);
        std::fs::create_dir_all(&audio_dir)
            .map_err(|error| auris_io::IoError::from_fs(&audio_dir, error))?;
        let file_name = super::record::take_file_name(&folder, &name);
        let inside = PathBuf::from(auris_io::AUDIO_DIR).join(&file_name);
        let path = folder.join(&inside);
        let mut recorder = auris_io::WavRecorder::create(&path, f64::from(plan.sample_rate), 1)?;
        if let Err(error) = recorder.write(samples).and_then(|_| recorder.finish()) {
            // This file is not in the document yet. A failed write must not reserve a take name
            // forever with an orphaned, partial WAV.
            let _ = std::fs::remove_file(&path);
            return Err(error.into());
        }

        // Read back through the importer rather than kept from memory, exactly as a recorded
        // take is: the engine renders every source at the project's rate, and the model sings
        // at its own.
        let buffer = auris_io::import_audio_file(&path, self.project.sample_rate)?;
        let seconds = buffer.frame_count() as f64 / buffer.sample_rate();

        self.record(Edit::Sing);
        let previous = self
            .project
            .track_mut(plan.track)
            .and_then(|track| track.kind.as_singer_mut())
            .and_then(|singer| singer.take.take());
        if let Some(previous) = previous {
            self.project.audio_sources.remove(&previous.source);
        }
        let stem = Path::new(&file_name)
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or(file_name.clone());
        let source = self.project.add_audio_source(
            stem,
            AssetPath::inside(&inside),
            buffer.frame_count() as u64,
            buffer.sample_rate(),
            buffer.channel_count(),
        );
        self.record_source_size(source, &path);
        if let Some(singer) = self
            .project
            .track_mut(plan.track)
            .and_then(|track| track.kind.as_singer_mut())
        {
            singer.take = Some(SingerTake {
                source,
                fingerprint: plan.fingerprint,
                seed: plan.seed,
            });
        }
        self.install_source(source, std::sync::Arc::new(buffer));
        self.prune_sources();
        self.invalidate_graph();
        Ok(seconds)
    }

    /// Whether a singer track's take still matches its notes.
    ///
    /// Playback uses the take regardless — a voice someone chose should not fall back to the
    /// formant preview the moment a word is edited — so this is how a frontend knows to say
    /// "behind the score" and offer to sing again. Cheap enough to ask on an edit, not meant
    /// for every paint: it renders the track's frames to compare fingerprints.
    pub fn singer_take_state(&self, track: TrackId) -> Result<SingerTakeState, SessionError> {
        let singer = self.require_singer(track)?;
        let Some(take) = &singer.take else {
            return Ok(SingerTakeState::Absent);
        };
        let Some(voice) = &singer.voice else {
            // The voice was taken away after the take was rendered; whatever the notes say now,
            // nothing could render them again unchanged.
            return Ok(SingerTakeState::Behind);
        };
        let frames = render_frames(singer, &self.project.tempo_map);
        match take_fingerprint(&frames, &voice.path, voice.speaker.as_deref(), take.seed)
            == take.fingerprint
        {
            true => Ok(SingerTakeState::Current),
            false => Ok(SingerTakeState::Behind),
        }
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

/// Everything a render needs, checked and gathered by [`Session::sing_plan`].
///
/// Owns its frames so a worker thread can hold it without borrowing the session: the window
/// keeps answering commands while the voice sings.
#[derive(Clone, Debug)]
pub struct SingPlan {
    /// The track the take will land on.
    pub track: TrackId,
    /// The frames the model will be fed, at the track's hop.
    pub frames: SingerFrames,
    /// The same timeline as lyric-bearing notes and rests for score-based backends.
    pub score: SingerScore,
    /// The resolved path of the model file.
    pub voice: PathBuf,
    /// The id of the speaker who sings, in the model's own numbering.
    pub speaker: u32,
    /// The seed pinning the render's random choices.
    pub seed: u64,
    /// What [`take_fingerprint`] said of exactly these inputs, stored into the landed take.
    pub fingerprint: u64,
    /// The rate the model sings at — the rate the take's file is written at.
    pub sample_rate: u32,
}

/// What [`Session::sing_frames`] wrote, and what it cost.
///
/// Serialisable so a frontend can hand the whole account to whatever asked for the file — a
/// measurement wants the timings beside the audio, and a number retyped from a status line is
/// a number retyped wrong.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct SungFrames {
    /// The voice's display name, from its card, or the file's stem where the card has none.
    pub voice: String,
    /// The speaker who sang, by the name the model gives the id.
    pub speaker: String,
    /// Seconds of audio the file holds.
    pub seconds: f64,
    /// The rate the file was written at — the model's own.
    pub sample_rate: u32,
    /// How many frames were sung.
    pub frames: usize,
    /// How many inferences the timeline was cut into.
    pub chunks: usize,
    /// Seconds spent opening the model, or zero when it was already open.
    pub load_seconds: f64,
    /// Seconds the model spent singing, the chunks together.
    pub render_seconds: f64,
    /// Whether the GPU provider was in the session when the last chunk was sung.
    pub on_gpu: bool,
}

/// The name a voice goes by in a document: its card's, or its file's where the card has none.
fn voice_name(model: &VoiceModel, file: &Path) -> String {
    let card = model.info().display_name();
    match card.is_empty() {
        true => file
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| "Voice".to_string()),
        false => card.to_string(),
    }
}

/// The id a speaker's name has in a model, `None` being the first speaker.
///
/// One rule for every path that sings — the take, the audition, a frames file — so a name
/// the model does not know is refused the same way at each, naming what it does know.
fn speaker_id(info: &auris_singer::VoiceInfo, name: Option<&str>) -> Result<u32, SessionError> {
    match name {
        None => Ok(0),
        Some(name) => info
            .speaker_id(name)
            .ok_or_else(|| SessionError::NoSuchSpeaker {
                name: name.to_string(),
                offered: info.speakers(),
            }),
    }
}

/// How a singer track's take stands against its notes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SingerTakeState {
    /// No take has been rendered.
    Absent,
    /// The take was rendered from exactly what the track says now.
    Current,
    /// The notes, the voice or the seed have moved on since the take was rendered.
    Behind,
}

/// A finite ornament depth, clamped into the bend's own range.
fn ornament_depth(depth: f32) -> Result<f32, SessionError> {
    match depth.is_finite() {
        true => Ok(depth.clamp(0.0, BEND_LIMIT)),
        false => Err(SessionError::NotFinite(f64::from(depth))),
    }
}

/// A finite scoop or fall span, clamped between one frame and two seconds.
///
/// The floor is [`MIN_PHONEME_SECONDS`] for the pin's reason — narrower rounds to no frames
/// and reads as the drag refusing to work — and past two seconds the gesture is a slide the
/// bend curve draws better.
fn ornament_seconds(seconds: f64, frame_hop: f64) -> Result<f64, SessionError> {
    match seconds.is_finite() {
        true => Ok(seconds.clamp(frame_hop, 2.0)),
        false => Err(SessionError::NotFinite(seconds)),
    }
}

/// One number naming everything a render reads: the frames, the voice, the seed.
///
/// FNV-1a, written out the way [`auris_core::rng`] writes it and for the same reason: the value
/// is stored in project files, so it has to mean the same thing next year, and std's hashers
/// promise no such thing. Each variable-length part is preceded by its length so two adjacent
/// parts cannot trade bytes and hash the same.
pub fn take_fingerprint(
    frames: &SingerFrames,
    voice: &AssetPath,
    speaker: Option<&str>,
    seed: u64,
) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    let mut eat = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(PRIME);
        }
    };
    eat(&seed.to_le_bytes());
    let speaker = speaker.unwrap_or("");
    eat(&(speaker.len() as u64).to_le_bytes());
    eat(speaker.as_bytes());
    let stored = voice.as_stored().to_string_lossy();
    eat(&(stored.len() as u64).to_le_bytes());
    eat(stored.as_bytes());
    eat(&frames.hop_seconds.to_le_bytes());
    eat(&(frames.inventory.len() as u64).to_le_bytes());
    for token in &frames.inventory {
        eat(&(token.len() as u64).to_le_bytes());
        eat(token.as_bytes());
    }
    eat(&(frames.phonemes.len() as u64).to_le_bytes());
    for id in &frames.phonemes {
        eat(&id.to_le_bytes());
    }
    for f0 in &frames.f0_hz {
        eat(&f0.to_le_bytes());
    }
    for energy in &frames.energy {
        eat(&energy.to_le_bytes());
    }
    hash
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
    fn a_pinned_phoneme_moves_the_frames_and_a_new_word_takes_the_pin_off() {
        let (mut session, track, clip) = sung(1);
        session.set_note_lyric(clip, 0, "か").unwrap();

        // Pin the k to 150 ms: the boundary in the frames moves to match.
        session
            .set_phoneme_duration(clip, 0, 0, Some(0.150))
            .unwrap();
        let frames = session.singer_frames(track).unwrap();
        let k = frames
            .inventory
            .iter()
            .position(|entry| entry == "k")
            .unwrap() as u32;
        let held = frames.phonemes.iter().filter(|id| **id == k).count();
        assert_eq!(held, 15, "150 ms of k at a 10 ms hop");

        // A drag is repeats on one address: they fold into a single undo step, and undoing
        // it lands back at no pin at all.
        session
            .set_phoneme_duration(clip, 0, 0, Some(0.200))
            .unwrap();
        session.undo();
        let note = &session.midi_clip(clip).unwrap().notes[0];
        assert!(note.phoneme_seconds.is_empty(), "one gesture, one step");

        // Pin again, then retype the word: the pin belongs to phonemes that are gone.
        session
            .set_phoneme_duration(clip, 0, 0, Some(0.150))
            .unwrap();
        session.set_note_lyric(clip, 0, "さ").unwrap();
        let note = &session.midi_clip(clip).unwrap().notes[0];
        assert!(note.phoneme_seconds.is_empty(), "a new word unpins");

        // A stale address is forgiven, and unpinning by hand trims the field away whole.
        session.set_phoneme_duration(clip, 0, 9, Some(0.5)).unwrap();
        session
            .set_phoneme_duration(clip, 0, 1, Some(0.300))
            .unwrap();
        session.set_phoneme_duration(clip, 0, 1, None).unwrap();
        let note = &session.midi_clip(clip).unwrap().notes[0];
        assert!(
            note.phoneme_seconds.is_empty(),
            "trailing zeros are dropped"
        );

        // And a reset takes every pin off in one step.
        session
            .set_phoneme_duration(clip, 0, 0, Some(0.100))
            .unwrap();
        session.clear_phoneme_timing(clip, 0).unwrap();
        let note = &session.midi_clip(clip).unwrap().notes[0];
        assert!(note.phoneme_seconds.is_empty());
    }

    #[test]
    fn a_phoneme_pin_is_at_least_one_frame_at_the_tracks_hop() {
        let (mut session, track, clip) = sung(1);
        session.set_note_lyric(clip, 0, "か").unwrap();
        session.set_frame_hop(track, 0.05).unwrap();
        session
            .set_phoneme_duration(clip, 0, 0, Some(0.001))
            .unwrap();

        assert_eq!(
            session.midi_clip(clip).unwrap().notes[0].phoneme_seconds[0],
            0.05
        );
    }

    #[test]
    fn an_ornament_moves_the_frames_and_survives_a_new_word() {
        use auris_core::plugin::pitch_to_hz;
        let (mut session, track, clip) = sung(1);
        session.set_note_lyric(clip, 0, "あ").unwrap();

        // Wild numbers are clamped rather than refused — the hand at the edge wants the edge.
        session
            .set_note_scoop(
                clip,
                0,
                Some(Scoop {
                    depth: 99.0,
                    seconds: 99.0,
                }),
            )
            .unwrap();
        let note = &session.midi_clip(clip).unwrap().notes[0];
        let scoop = note.scoop.unwrap();
        assert_eq!(scoop.depth, BEND_LIMIT);
        assert_eq!(scoop.seconds, 2.0);

        // The frames start the full clamped depth under the note and settle onto it — the
        // span itself is capped at half the note by the shape.
        let frames = session.singer_frames(track).unwrap();
        let sung: Vec<f32> = frames
            .f0_hz
            .iter()
            .copied()
            .filter(|hz| *hz > 0.0)
            .collect();
        assert!((sung[0] - pitch_to_hz(60.0 - BEND_LIMIT)).abs() < 1.0);
        assert!((sung[sung.len() - 1] - pitch_to_hz(60.0)).abs() < 0.5);

        // A drag is repeats on one address: they fold into one step, and undo takes it off.
        session
            .set_note_scoop(
                clip,
                0,
                Some(Scoop {
                    depth: 1.0,
                    seconds: 0.1,
                }),
            )
            .unwrap();
        session.undo();
        let note = &session.midi_clip(clip).unwrap().notes[0];
        assert!(note.scoop.is_none(), "one gesture, one step");

        // Ornaments are pitch, not phonemes: a new word keeps them, unlike a timing pin.
        session
            .set_note_vibrato(
                clip,
                0,
                Some(Vibrato {
                    rate: 99.0,
                    ..Vibrato::default()
                }),
            )
            .unwrap();
        session.set_note_lyric(clip, 0, "さ").unwrap();
        let note = &session.midi_clip(clip).unwrap().notes[0];
        let vibrato = note.vibrato.unwrap();
        assert_eq!(vibrato.rate, 12.0, "the trill is clamped to a vibrato");

        // The sway reaches above the note, which no other ornament does.
        session
            .set_note_vibrato(
                clip,
                0,
                Some(Vibrato {
                    depth: 0.5,
                    rate: 6.0,
                    delay: 0.0,
                    fade_in: 0.0,
                }),
            )
            .unwrap();
        let frames = session.singer_frames(track).unwrap();
        let ceiling = frames.f0_hz.iter().copied().fold(0.0f32, f32::max);
        assert!(ceiling > pitch_to_hz(60.2), "the crest rises over the note");

        // One reset takes every ornament off, and a note past the clip refuses.
        session
            .set_note_fall(clip, 0, Some(Fall::default()))
            .unwrap();
        session.clear_note_ornaments(clip, 0).unwrap();
        let note = &session.midi_clip(clip).unwrap().notes[0];
        assert!(note.scoop.is_none() && note.fall.is_none() && note.vibrato.is_none());
        assert!(session.set_note_scoop(clip, 9, None).is_err());
    }

    #[test]
    fn a_preview_note_is_a_tiny_score_with_a_silent_tail() {
        let (session, track, _clip) = sung(1);
        let frames = session
            .preview_note_frames(track, 69, &["r".to_string(), "a".to_string()])
            .unwrap();
        // The note sings at its pitch — A4 at 440 Hz, consonant frames included, since a
        // consonant carries its vowel's pitch through the contour...
        let voiced: Vec<f32> = frames
            .f0_hz
            .iter()
            .copied()
            .filter(|hz| *hz > 0.0)
            .collect();
        assert!(!voiced.is_empty());
        assert!(
            voiced.iter().all(|hz| (*hz - 440.0).abs() < 0.5),
            "A4 previews at 440 Hz"
        );
        // ...for its fixed length, and the frames past it are silence for the release.
        let held = (PREVIEW_NOTE_SECONDS / frames.hop_seconds).round() as usize;
        assert!(frames.len() > held, "the tail extends past the note");
        assert_eq!(*frames.phonemes.last().unwrap(), 0, "the tail is silence");
        assert_eq!(*frames.f0_hz.last().unwrap(), 0.0);
        // The same syllable at another pitch is the same length — what a cache wants.
        let other = session
            .preview_note_frames(track, 57, &["a".to_string()])
            .unwrap();
        assert_eq!(other.len(), frames.len());
    }

    /// Puts a voice on the track without opening a model file — the doors can be tested
    /// without two hundred megabytes on the machine.
    fn put_voice(session: &mut crate::Session, track: TrackId) {
        if let Some(singer) = session
            .project
            .track_mut(track)
            .and_then(|track| track.kind.as_singer_mut())
        {
            singer.voice = Some(SingerVoice {
                path: AssetPath::external("/voices/test.onnx"),
                name: "Test Voice".into(),
                consonants: None,
                levels: None,
                speaker: None,
            });
        }
    }

    #[test]
    fn a_voice_that_cannot_be_opened_is_refused_before_anything_is_recorded() {
        let (mut session, track, _) = sung(1);
        let error = session
            .set_singer_voice(track, Some(std::path::Path::new("nowhere/voice.onnx")))
            .unwrap_err();
        assert!(matches!(error, SessionError::Sing(_)), "{error}");
        assert_eq!(
            session.undo(),
            Some(crate::history::Edit::AddNote),
            "the refusal must cost no undo step"
        );
    }

    #[test]
    fn a_speaker_cannot_be_chosen_before_a_voice() {
        let (mut session, track, _) = sung(0);
        assert!(matches!(
            session.set_singer_speaker(track, Some("alto")),
            Err(SessionError::NoVoice(_))
        ));
        assert!(matches!(
            session.singer_speakers(track),
            Err(SessionError::NoVoice(_))
        ));
        assert_ne!(
            session.undo(),
            Some(crate::history::Edit::SetSingerSpeaker),
            "a refusal costs no undo step"
        );
    }

    /// A real voice names its speakers, refuses a stranger by name — listing its own — and
    /// treats a chosen speaker as part of the take.
    #[test]
    fn a_real_voice_s_speakers_are_named_checked_and_part_of_the_take() {
        let Some(model) = std::env::var_os("AURIS_SINGER_TEST_MODEL") else {
            return;
        };
        let (mut session, track, clip) = sung(0);
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        session
            .set_singer_voice(track, Some(std::path::Path::new(&model)))
            .unwrap();
        let speakers = session.singer_speakers(track).unwrap();
        assert!(!speakers.is_empty(), "every model has a first speaker");
        let refused = session
            .set_singer_speaker(track, Some("nobody-the-model-knows"))
            .unwrap_err();
        match refused {
            SessionError::NoSuchSpeaker { name, offered } => {
                assert_eq!(name, "nobody-the-model-knows");
                assert_eq!(offered, speakers, "the refusal names what it does have");
            }
            other => panic!("{other}"),
        }
        assert_eq!(
            session.undo(),
            Some(crate::history::Edit::SetSingerVoice),
            "the refusal cost no undo step"
        );
        session.redo();

        session
            .set_singer_speaker(track, Some(&speakers[0]))
            .unwrap();
        assert_eq!(
            session
                .singer_voice(track)
                .unwrap()
                .unwrap()
                .speaker
                .as_deref(),
            Some(speakers[0].as_str())
        );
        assert_eq!(
            session.singer_speaker(track).unwrap(),
            0,
            "the first name is id 0"
        );
        assert_eq!(session.undo(), Some(crate::history::Edit::SetSingerSpeaker));
    }

    #[test]
    fn singing_refuses_in_the_order_a_person_can_act_on() {
        let (mut session, track, clip) = sung(0);
        assert!(matches!(
            session.sing(track, None),
            Err(SessionError::NoVoice(_))
        ));
        put_voice(&mut session, track);
        assert!(matches!(
            session.sing(track, None),
            Err(SessionError::NothingToSing(_))
        ));
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        assert!(matches!(
            session.sing(track, None),
            Err(SessionError::SingingNeedsFolder)
        ));
    }

    #[test]
    fn the_fingerprint_pins_frames_voice_and_seed() {
        let (mut session, track, clip) = sung(1);
        let voice = AssetPath::external("/voices/test.onnx");
        let frames = session.singer_frames(track).unwrap();
        assert_eq!(
            take_fingerprint(&frames, &voice, None, 3),
            take_fingerprint(&frames, &voice, None, 3),
            "the same inputs are the same number, today and next year"
        );
        assert_ne!(
            take_fingerprint(&frames, &voice, None, 3),
            take_fingerprint(&frames, &voice, None, 4),
            "another seed is another take"
        );
        assert_ne!(
            take_fingerprint(&frames, &voice, None, 3),
            take_fingerprint(&frames, &AssetPath::external("/voices/other.onnx"), None, 3),
            "another voice is another take"
        );
        assert_ne!(
            take_fingerprint(&frames, &voice, None, 3),
            take_fingerprint(&frames, &voice, Some("alto"), 3),
            "another speaker is another take"
        );
        session
            .add_note(clip, Note::new(64, Ticks::QUARTER, Ticks::QUARTER))
            .unwrap();
        let edited = session.singer_frames(track).unwrap();
        assert_ne!(
            take_fingerprint(&frames, &voice, None, 3),
            take_fingerprint(&edited, &voice, None, 3),
            "an edited score is another take"
        );
    }

    #[test]
    fn take_state_reads_absent_current_and_behind() {
        let (mut session, track, clip) = sung(1);
        assert_eq!(
            session.singer_take_state(track).unwrap(),
            SingerTakeState::Absent
        );

        put_voice(&mut session, track);
        let frames = session.singer_frames(track).unwrap();
        let voice = AssetPath::external("/voices/test.onnx");
        let fingerprint = take_fingerprint(&frames, &voice, None, 0);
        if let Some(singer) = session
            .project
            .track_mut(track)
            .and_then(|track| track.kind.as_singer_mut())
        {
            singer.take = Some(SingerTake {
                source: auris_core::SourceId(999),
                fingerprint,
                seed: 0,
            });
        }
        assert_eq!(
            session.singer_take_state(track).unwrap(),
            SingerTakeState::Current
        );

        session
            .add_note(clip, Note::new(72, Ticks::QUARTER, Ticks::QUARTER))
            .unwrap();
        assert_eq!(
            session.singer_take_state(track).unwrap(),
            SingerTakeState::Behind,
            "the edit moved the score past the take"
        );
    }

    /// A voice whose export measured its consonants times them by that measure, end to end:
    /// the model's metadata, through the document, into the frames it is fed.
    ///
    /// Runs only where the test model actually carries the table — an old export skips
    /// rather than fails, since predating a measurement is not a defect — so the day a
    /// measuring export lands on the machine, the whole chain starts being held to its
    /// numbers.
    #[test]
    fn a_measuring_voice_times_its_consonants_end_to_end() {
        let Some(model) = std::env::var_os("AURIS_SINGER_TEST_MODEL") else {
            eprintln!("AURIS_SINGER_TEST_MODEL not set; skipping the consonant-width test");
            return;
        };
        let (mut session, track, clip) = sung(1);
        session
            .set_singer_voice(track, Some(std::path::Path::new(&model)))
            .unwrap();
        let Some(widths) = session
            .singer_voice(track)
            .unwrap()
            .unwrap()
            .consonants
            .clone()
        else {
            eprintln!("the test model predates phoneme_durations; skipping");
            return;
        };
        assert!(
            !widths.seconds.is_empty(),
            "a model that carries the table measured at least one phoneme"
        );

        // つ — [ts ɯ] — the affricate whose measured width strays furthest from the old
        // fixed sixty milliseconds. A quarter note leaves it room not to be scaled.
        session
            .set_note_phonemes(clip, 0, vec!["ts".into(), "ɯ".into()])
            .unwrap();
        let frames = session.singer_frames(track).unwrap();
        let ts = frames
            .inventory
            .iter()
            .position(|phoneme| phoneme == "ts")
            .expect("the affricate reached the frames") as u32;
        let held = frames.phonemes.iter().filter(|id| **id == ts).count() as f64;
        let asked = widths.width("ts") / frames.hop_seconds;
        assert!(
            (held - asked).abs() <= 1.0,
            "ts held {held} frames where its own measure asks {asked}"
        );
    }

    /// The acceleration setting reaches the model a render is handed, and changing it does
    /// not wait for a relaunch: the cache is emptied, so the same file comes back loaded the
    /// new way.
    #[test]
    fn the_acceleration_setting_reaches_the_next_loaded_voice() {
        let Some(model) = std::env::var_os("AURIS_SINGER_TEST_MODEL") else {
            eprintln!("AURIS_SINGER_TEST_MODEL not set; skipping the acceleration test");
            return;
        };
        let path = std::path::PathBuf::from(&model);
        let (mut session, _, _) = sung(1);

        session.set_singer_acceleration(auris_singer::Acceleration::Cpu);
        let on_cpu = session.voice_model_at(&path).unwrap();
        assert_eq!(
            on_cpu.lock().unwrap().acceleration(),
            auris_singer::Acceleration::Cpu
        );
        // Asking again without changing anything answers from the cache, not a reload.
        assert!(Arc::ptr_eq(
            &on_cpu,
            &session.voice_model_at(&path).unwrap()
        ));

        session.set_singer_acceleration(auris_singer::Acceleration::Auto);
        let on_auto = session.voice_model_at(&path).unwrap();
        assert!(
            !Arc::ptr_eq(&on_cpu, &on_auto),
            "changing the setting must reload the voice, not answer from the cache"
        );
        assert_eq!(
            on_auto.lock().unwrap().acceleration(),
            auris_singer::Acceleration::Auto
        );
    }

    #[test]
    fn the_real_voice_sings_a_take_into_the_project() {
        let Some(model) = std::env::var_os("AURIS_SINGER_TEST_MODEL") else {
            eprintln!("AURIS_SINGER_TEST_MODEL not set; skipping the real-voice session test");
            return;
        };
        let scratch = crate::session::fixtures::Scratch::new("sing-take");
        let folder = scratch.join("Song");
        std::fs::create_dir_all(&folder).unwrap();

        let (mut session, track, clip) = sung(2);
        session.write_lyrics(clip, &[0, 1], "らら").unwrap();
        session.save(&folder.join("Song.auris")).unwrap();
        session
            .set_singer_voice(track, Some(std::path::Path::new(&model)))
            .unwrap();
        assert!(
            !session
                .singer_voice(track)
                .unwrap()
                .unwrap()
                .name
                .is_empty(),
            "the card's name rides into the document"
        );
        // The consonant table rides in beside the name exactly when the export measured one
        // — asserted against the model's own metadata, so this holds for old and new
        // exports alike.
        let measured = {
            let model = session
                .voice_model_at(std::path::Path::new(&model))
                .unwrap();
            let model = model.lock().unwrap();
            model.info().consonant_widths(0)
        };
        assert_eq!(
            session.singer_voice(track).unwrap().unwrap().consonants,
            measured,
            "the document carries what the model measured, no more and no less"
        );
        let levels = {
            let model = session
                .voice_model_at(std::path::Path::new(&model))
                .unwrap();
            let model = model.lock().unwrap();
            model.info().consonant_levels(0)
        };
        assert_eq!(
            session.singer_voice(track).unwrap().unwrap().levels,
            levels,
            "and the levels beside the widths"
        );

        let seconds = session.sing(track, None).unwrap();
        assert!(
            seconds > 0.5,
            "two quarter notes are audible, got {seconds}"
        );
        assert_eq!(
            session.singer_take_state(track).unwrap(),
            SingerTakeState::Current
        );
        let take = session
            .project()
            .track(track)
            .and_then(|track| track.kind.as_singer())
            .and_then(|singer| singer.take.clone())
            .expect("the take is in the document");
        let source = &session.project().audio_sources[&take.source];
        let file = source.path.resolve(session.project_folder()).unwrap();
        assert!(file.is_file(), "the waveform is in Audio/");

        // An edit leaves the take standing, but behind the score; singing again replaces it,
        // and the replaced take's source entry goes with it.
        session
            .add_note(clip, Note::new(72, Ticks::from_beats(2.0), Ticks::QUARTER))
            .unwrap();
        assert_eq!(
            session.singer_take_state(track).unwrap(),
            SingerTakeState::Behind
        );
        session.sing(track, None).unwrap();
        assert_eq!(
            session.singer_take_state(track).unwrap(),
            SingerTakeState::Current
        );
        assert!(
            !session.project().audio_sources.contains_key(&take.source),
            "the replaced take's source entry is gone"
        );
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
    fn a_prolonged_sound_is_sung_as_the_vowel_it_stretches() {
        // こーひー across four notes: the ー notes must carry the vowel of the mora before
        // them. They used to arrive with no phonemes at all — ー asked alone answers nothing —
        // and fall through to the open-vowel placeholder, singing こあひあ.
        let (mut session, _, clip) = sung(4);
        let filled = session
            .write_lyrics(clip, &[0, 1, 2, 3], "こーひー")
            .unwrap();
        assert_eq!(filled, 4);

        let notes = &session.midi_clip(clip).unwrap().notes;
        assert_eq!(notes[1].lyric, "ー");
        assert_eq!(notes[1].phonemes, ["o"], "こー stretches o");
        assert_eq!(notes[3].lyric, "ー");
        assert_eq!(notes[3].phonemes, ["i"], "ひー stretches i");
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
    fn frames_read_back_from_where_they_were_exported() {
        let (mut session, track, clip) = sung(2);
        session.write_lyrics(clip, &[0, 1], "さく").unwrap();
        let scratch = crate::session::fixtures::Scratch::new("frames-read");
        let path = scratch.join("melody.frames.json");
        session.export_singer_frames(track, &path).unwrap();
        let read = crate::Session::read_singer_frames(&path).unwrap();
        assert_eq!(read, session.singer_frames(track).unwrap());
    }

    #[test]
    fn a_file_that_is_not_frames_is_refused_as_a_file_error() {
        let scratch = crate::session::fixtures::Scratch::new("frames-refused");
        let path = scratch.join("notes.txt");
        std::fs::write(&path, "these are not frames").unwrap();
        match crate::Session::read_singer_frames(&path) {
            Err(SessionError::Io(_)) => {}
            other => panic!("expected a file error, got {other:?}"),
        }
        match crate::Session::read_singer_frames(&scratch.join("absent.json")) {
            Err(SessionError::Io(_)) => {}
            other => panic!("expected a file error, got {other:?}"),
        }
    }

    #[test]
    fn a_frames_file_with_mismatched_sequences_is_refused_without_panicking() {
        let scratch = crate::session::fixtures::Scratch::new("frames-mismatched");
        let path = scratch.join("broken.frames.json");
        std::fs::write(
            &path,
            r#"{"hop_seconds":0.01,"inventory":["<sil>","a"],"phonemes":[1,1,1],"f0_hz":[],"energy":[0.5]}"#,
        )
        .unwrap();

        match crate::Session::read_singer_frames(&path) {
            Err(SessionError::Sing(auris_singer::SingError::InvalidFrames {
                phonemes: 3,
                f0_hz: 0,
                energy: 1,
            })) => {}
            other => panic!("expected a frame-length error, got {other:?}"),
        }
    }

    #[test]
    fn singing_frames_without_a_voice_writes_nothing() {
        let (mut session, track, clip) = sung(1);
        session.write_lyrics(clip, &[0], "ら").unwrap();
        let frames = session.singer_frames(track).unwrap();
        let scratch = crate::session::fixtures::Scratch::new("frames-no-voice");
        let output = scratch.join("take.wav");
        let error = session
            .sing_frames(
                Path::new("nowhere/no-such-voice.onnx"),
                &frames,
                None,
                0,
                &output,
            )
            .expect_err("a voice that is not there cannot sing");
        assert!(matches!(error, SessionError::Sing(_)), "{error}");
        assert!(
            !output.exists(),
            "the file is only made once there is something to write"
        );
    }

    /// Frames in, a file out, through the same model and recorder a take uses — asserted on
    /// the numbers the measurement reads back. Skips without `AURIS_SINGER_TEST_MODEL`.
    #[test]
    fn the_real_voice_sings_frames_to_a_file() {
        let Some(model) = std::env::var_os("AURIS_SINGER_TEST_MODEL") else {
            eprintln!("AURIS_SINGER_TEST_MODEL not set; skipping the frames-in test");
            return;
        };
        let voice = std::path::PathBuf::from(model);
        let (mut session, track, clip) = sung(2);
        session.write_lyrics(clip, &[0, 1], "らら").unwrap();
        let scratch = crate::session::fixtures::Scratch::new("frames-sung");
        let frames_path = scratch.join("melody.frames.json");
        session.export_singer_frames(track, &frames_path).unwrap();
        let frames = crate::Session::read_singer_frames(&frames_path).unwrap();

        let output = scratch.join("take.wav");
        let sung = session
            .sing_frames(&voice, &frames, None, 7, &output)
            .unwrap();
        assert!(output.is_file(), "the waveform is where it was asked for");
        assert_eq!(sung.frames, frames.len());
        assert!(
            sung.chunks >= 1,
            "two sung notes are at least one inference"
        );
        assert!(sung.load_seconds > 0.0, "the first call opened the model");
        assert!(sung.render_seconds > 0.0);
        assert!(!sung.voice.is_empty(), "a voice always has a name to go by");
        let expected = frames.len() as f64 * frames.hop_seconds;
        assert!(
            (sung.seconds - expected).abs() < frames.hop_seconds,
            "{} s of audio for {expected} s of frames",
            sung.seconds
        );
        let imported = auris_io::import_audio_file(&output, f64::from(sung.sample_rate)).unwrap();
        assert_eq!(imported.sample_rate() as u32, sung.sample_rate);
        assert!(
            (imported.frame_count() as f64 / f64::from(sung.sample_rate) - sung.seconds).abs()
                < 1e-6
        );

        // The second call finds the voice open, and the same seed is the same take to the byte.
        let again = scratch.join("again.wav");
        let second = session
            .sing_frames(&voice, &frames, None, 7, &again)
            .unwrap();
        assert_eq!(second.load_seconds, 0.0, "the voice was already open");
        assert_eq!(
            std::fs::read(&output).unwrap(),
            std::fs::read(&again).unwrap(),
            "a seed names a take"
        );
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
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct VoiceStamp {
    modified: Option<std::time::SystemTime>,
    len: u64,
}

fn voice_stamp(file: &Path) -> Option<VoiceStamp> {
    let metadata = std::fs::metadata(file).ok()?;
    Some(VoiceStamp {
        modified: metadata.modified().ok(),
        len: metadata.len(),
    })
}
