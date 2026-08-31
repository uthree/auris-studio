//! Composing from lyrics: the words first, then the tune, then the band.
//!
//! The Orpheus pipeline seen from this layer. [`Session::compose_from_lyrics`] reads a lyric,
//! asks `auris-vocal` for its moras and — where the Japanese dictionary is loaded — the pitch
//! accent of each phrase, asks `auris-compose`'s vocal writer for a melody that honours that
//! accent over the document's harmony, and lands the result as an ordinary singer clip: every
//! note carrying its mora and phonemes, ready for a voice model, editable like anything typed
//! by hand. Where the document has no chords yet, a stock progression is stamped first so the
//! search has ground to stand on — visibly, in the harmony lane, where it can be disagreed
//! with and the parts regenerated around the correction, exactly the [`Session::accompany`]
//! bargain.
//!
//! Without a dictionary the moras still split and the melody still writes, but every contour
//! is free: the Orpheus constraint — sing the words the way they are spoken — only has teeth
//! where something actually analysed the accent. The report says which of the two happened.

use auris_compose::vocal::{VocalRange, ornament_vocal, vocal_rhythm, write_vocal};
use auris_core::theory::contour::Contour;
use auris_core::time::{Ticks, TimeSignature};
use auris_core::{ClipId, ClipPreset, ClipRecipe, Note, PresetRef, TrackId};
use auris_vocal::{SungMora, kana_accent_phrase};

use crate::error::SessionError;
use crate::history::Edit;

use super::Session;

/// The progression stamped under a lyric when the document has none: 王道進行, the royal
/// road — IV V iii vi, the most-worn ground in J-pop, which is exactly what a default
/// should be.
pub const DEFAULT_LYRIC_PROGRESSION: &str = "royal-road";

/// One musical phrase of a lyric: its moras, and what each asks of the melody.
struct LyricPhrase {
    moras: Vec<SungMora>,
    contours: Vec<Contour>,
}

/// What composing from lyrics produced.
#[derive(Clone, Debug, PartialEq)]
pub struct LyricSongReport {
    /// The singer track the melody landed on.
    pub track: TrackId,
    /// The clip holding the sung notes.
    pub clip: ClipId,
    /// How many notes — one per mora — were written.
    pub notes: usize,
    /// How many phrases the lyric was cut into.
    pub phrases: usize,
    /// `true` when the pitch accent actually constrained the melody — the dictionary was
    /// loaded and analysed at least one phrase. `false` means the tune is free-composed
    /// over the words, which is worth telling the person who expected Orpheus.
    pub accented: bool,
    /// How many bars the song covers.
    pub bars: usize,
    /// How many chords were stamped — zero when the document already had its own.
    pub chords: usize,
    /// The backing tracks written, in order.
    pub parts: Vec<TrackId>,
    /// `true` when the parts play the built-in oscillators because no General MIDI font is
    /// installed.
    pub substituted: bool,
}

impl Session {
    /// Writes a song from a lyric: a melody that follows the words, sung notes that carry
    /// them, and a band behind it. One undo step for the lot.
    ///
    /// Phrases are cut at line breaks and sentence punctuation — those are *musical*
    /// boundaries, where a singer breathes. With the Japanese dictionary loaded every phrase
    /// is read for its pitch accent and the melody is searched under Orpheus's constraint;
    /// without one, kana lyrics still sing (with a free contour) and kanji still refuses
    /// with the error that names the dictionary setting. `seed` names the take, the way it
    /// does everywhere else; the same lyric, harmony and seed write the same song.
    pub fn compose_from_lyrics(
        &mut self,
        lyrics: &str,
        parts: &[ClipPreset],
        seed: u64,
    ) -> Result<LyricSongReport, SessionError> {
        // Everything that can refuse does so here, before anything is recorded.
        let phrases = read_lyrics(lyrics, self.japanese.as_ref())?;
        if phrases.is_empty() {
            return Err(SessionError::NoLyrics);
        }
        let accented = phrases
            .iter()
            .any(|phrase| phrase.contours.iter().any(|c| *c != Contour::Free));
        let counts: Vec<usize> = phrases.iter().map(|phrase| phrase.moras.len()).collect();
        let meter = self.signature_at(Ticks::ZERO);
        let rhythm = vocal_rhythm(&counts, meter);
        let bar = meter.ticks_per_bar().raw().max(1);
        let bars = (rhythm.length.raw() / bar) as usize;
        let contours: Vec<Vec<Contour>> = phrases
            .iter()
            .map(|phrase| phrase.contours.clone())
            .collect();

        self.begin_transaction(Edit::ComposeLyrics);

        // Ground to stand on: chords the search can read. Only where the document has none —
        // a harmony somebody wrote is theirs, however little of the span it covers.
        let mut chords = 0;
        if self.project.harmony.chords.is_empty() {
            match self.stamp_named_progression(DEFAULT_LYRIC_PROGRESSION, Ticks::ZERO, bars) {
                Ok(stamped) => chords = stamped,
                Err(error) => {
                    self.end_transaction();
                    return Err(error);
                }
            }
        }

        let mut notes = write_vocal(
            &self.project.harmony,
            Ticks::ZERO,
            &rhythm,
            &contours,
            VocalRange::default(),
            seed,
        );
        // The ornaments a singer would add, by rule: scoop into each phrase, sway on the
        // held notes, let go at the end. Ordinary note data, adjustable one by one.
        ornament_vocal(&mut notes, &rhythm, &self.project.tempo_map, Ticks::ZERO);

        // Each note finds its mora by onset rather than by position in a flat list, so a
        // phrase the writer could not fill (a degenerate range) cannot shift every word
        // after it onto the wrong note.
        let mut moras: std::collections::HashMap<Ticks, &SungMora> = Default::default();
        for (slots, phrase) in rhythm.phrases.iter().zip(&phrases) {
            for ((onset, _), mora) in slots.iter().zip(&phrase.moras) {
                moras.insert(*onset, mora);
            }
        }

        let track = self.add_singer_track("Vocal");
        let clip = match self.add_midi_clip(track, "Vocal", Ticks::ZERO, rhythm.length) {
            Ok(clip) => clip,
            Err(error) => {
                self.end_transaction();
                return Err(error);
            }
        };
        let mut written = 0;
        for note in notes {
            let Some(mora) = moras.get(&note.start) else {
                continue;
            };
            let sung = Note {
                lyric: mora.text.clone(),
                phonemes: mora.phonemes.clone(),
                ..note
            };
            if self.add_note(clip, sung).is_ok() {
                written += 1;
            }
        }

        // The band, the accompany way: stock parts on their General MIDI sounds, each a
        // recipe that can be argued with and regenerated, seeded off the melody's own seed
        // so one number names the whole song.
        let font = (!parts.is_empty())
            .then(|| self.adopt_general_midi_here())
            .flatten();
        let mut report = LyricSongReport {
            track,
            clip,
            notes: written,
            phrases: counts.len(),
            accented,
            bars,
            chords,
            parts: Vec::with_capacity(parts.len()),
            substituted: font.is_none() && !parts.is_empty(),
        };
        for (index, preset) in parts.iter().enumerate() {
            let Ok(band) = self.add_default_instrument_track(part_name(*preset)) else {
                continue;
            };
            if let Some(font) = font {
                let sound = auris_compose::analysis::sound_for(*preset);
                let _ = self.set_track_preset(
                    band,
                    PresetRef {
                        font,
                        bank: i32::from(sound.bank),
                        patch: i32::from(sound.patch),
                    },
                );
            }
            let recipe = ClipRecipe::new(*preset, seed.wrapping_add(1 + index as u64));
            match self.generate_clip(band, Ticks::ZERO, rhythm.length, recipe) {
                Ok(_) => report.parts.push(band),
                Err(error) => log::warn!("no {} was written: {error}", preset.name()),
            }
        }

        self.end_transaction();
        Ok(report)
    }
}

impl Session {
    /// Writes the singer track a composed piece's lyrics ask for, into a project still
    /// being built.
    ///
    /// Called from [`Session::compose`] before the document is swapped in, so the vocal is
    /// part of the same single edit. Every *playing* of a section sings that section's own
    /// lyrics — the same words both times round is what makes the second chorus the same
    /// chorus — over the harmony already stamped under that span, dressed by the ornament
    /// rules, one clip per playing. Words that outrun their section are dropped with a
    /// warning rather than spilling into the next one, and a lyric that cannot be read at
    /// all (kanji with no dictionary anywhere) costs its sections, never the piece: their
    /// names come back for the report. Answers `(sung notes, clips, unsung sections)`.
    pub(super) fn write_spec_vocal(
        &self,
        project: &mut auris_core::Project,
        composition: &auris_compose::Composition,
    ) -> (usize, usize, Vec<String>) {
        // The composition carries its specification as the text it will be saved as; the
        // lyrics ride it there, and reading them back costs one parse of a document this
        // session already validated.
        let Ok(spec) = auris_compose::SongSpec::parse(&composition.spec) else {
            return (0, 0, Vec::new());
        };
        if spec
            .sections
            .values()
            .all(|section| section.lyrics.trim().is_empty())
        {
            return (0, 0, Vec::new());
        }

        // Read every lyrical span first, so a piece whose every lyric refuses gains no
        // empty vocal track for its trouble.
        let mut unsung: Vec<String> = Vec::new();
        let mut prepared = Vec::new();
        for span in project.sections.spans_in(Ticks::ZERO, composition.length) {
            let Some(section) = spec.sections.get(&span.label) else {
                continue;
            };
            if section.lyrics.trim().is_empty() {
                continue;
            }
            match read_lyrics(&section.lyrics, self.japanese.as_ref()) {
                Ok(phrases) if !phrases.is_empty() => prepared.push((span, phrases)),
                Ok(_) => {}
                Err(_) => {
                    if !unsung.contains(&span.label) {
                        unsung.push(span.label.clone());
                    }
                }
            }
        }
        if prepared.is_empty() {
            return (0, 0, unsung);
        }

        let track = project.add_singer_track("Vocal", auris_synth::Vocal::ID);
        let meter = composition.meter;
        let (mut sung, mut clips) = (0usize, 0usize);
        for (span, phrases) in prepared {
            let counts: Vec<usize> = phrases.iter().map(|phrase| phrase.moras.len()).collect();
            let contours: Vec<Vec<Contour>> = phrases
                .iter()
                .map(|phrase| phrase.contours.clone())
                .collect();
            let rhythm = vocal_rhythm(&counts, meter);
            let mut notes = write_vocal(
                &project.harmony,
                span.start,
                &rhythm,
                &contours,
                VocalRange::default(),
                spec.seed,
            );
            ornament_vocal(&mut notes, &rhythm, &project.tempo_map, span.start);

            let mut moras: std::collections::HashMap<Ticks, &SungMora> = Default::default();
            for (slots, phrase) in rhythm.phrases.iter().zip(&phrases) {
                for ((onset, _), mora) in slots.iter().zip(&phrase.moras) {
                    moras.insert(*onset, mora);
                }
            }

            let length = span.end - span.start;
            let Some(clip) = project.add_midi_clip(track, &span.label, span.start, length) else {
                continue;
            };
            let mut cut = 0usize;
            if let Some(target) = project.midi_clip_mut(clip) {
                // The length is the section's, exactly as a band clip's is.
                target.length_is_explicit = true;
                for note in notes {
                    if note.end() > length {
                        cut += 1;
                        continue;
                    }
                    let Some(mora) = moras.get(&note.start) else {
                        continue;
                    };
                    target.notes.push(Note {
                        lyric: mora.text.clone(),
                        phonemes: mora.phonemes.clone(),
                        ..note
                    });
                    sung += 1;
                }
            }
            clips += 1;
            if cut > 0 {
                log::warn!(
                    "section `{}`: {cut} syllables outran its bars and were dropped",
                    span.label
                );
            }
        }
        (sung, clips, unsung)
    }
}

/// What a lyric would cost in notes and bars, line by line — the writer's tape measure.
///
/// Computed by the same splitting, the same mora reading and the same [`vocal_rhythm`] that
/// composing uses, so the numbers a lyrics editor shows are the numbers Write will act on:
/// a display that ran its own arithmetic would drift the day either side changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LyricsMeasure {
    /// Notes each line of the lyric would sing — one per mora — or `None` for a line that
    /// cannot be read at all (kanji with no dictionary anywhere).
    pub lines: Vec<Option<usize>>,
    /// Notes the whole lyric would sing, counting only the readable lines.
    pub notes: usize,
    /// Bars the sung rhythm would cover: each phrase on a fresh bar, the last syllable held.
    pub bars: usize,
}

impl Session {
    /// Measures a lyric without writing anything: notes per line, notes in all, bars needed.
    ///
    /// For the editor's margin, so words can be fitted to a section while they are typed
    /// rather than discovered to outrun it at Write. Unreadable lines measure as `None` and
    /// simply do not count — the same lines Write would refuse or skip.
    pub fn measure_lyrics(&self, lyrics: &str, meter: TimeSignature) -> LyricsMeasure {
        let mut lines = Vec::new();
        let mut counts: Vec<usize> = Vec::new();
        let mut notes = 0usize;
        for line in lyrics.split('\n') {
            let mut sung = Some(0usize);
            for segment in line.split(['\r', '、', '。', '！', '？', '!', '?']) {
                let segment = segment.trim();
                if segment.is_empty() {
                    continue;
                }
                let read: Option<usize> = match self.japanese.as_ref() {
                    Some(dictionary) => dictionary
                        .accent_phrases(segment)
                        .ok()
                        .map(|read| read.iter().map(|accent| accent.moras.len()).sum()),
                    None => kana_accent_phrase(segment).map(|phrase| phrase.moras.len()),
                };
                match read {
                    Some(count) if count > 0 => {
                        counts.push(count);
                        if let Some(total) = sung.as_mut() {
                            *total += count;
                        }
                        notes += count;
                    }
                    Some(_) => {}
                    None => sung = None,
                }
            }
            lines.push(sung);
        }
        let bars = match counts.is_empty() {
            true => 0,
            false => {
                let bar = meter.ticks_per_bar().raw().max(1);
                (vocal_rhythm(&counts, meter).length.raw() / bar) as usize
            }
        };
        LyricsMeasure { lines, notes, bars }
    }
}

/// Cuts a lyric into musical phrases and reads each one's moras and contours.
///
/// The dictionary is preferred over the kana table when it is loaded — the opposite of
/// [`lyric_phonemes`](auris_vocal::lyric_phonemes)'s order, on purpose: the phonemes come
/// out identical either way (that equality is a tested contract of `auris-vocal`), and only
/// the dictionary knows the accent, which here is the whole point.
fn read_lyrics(
    lyrics: &str,
    dictionary: Option<&auris_vocal::JapaneseDictionary>,
) -> Result<Vec<LyricPhrase>, SessionError> {
    let mut phrases = Vec::new();
    for segment in lyrics.split(['\n', '\r', '、', '。', '！', '？', '!', '?']) {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        let read = match dictionary {
            Some(dictionary) => dictionary.accent_phrases(segment)?,
            None => match kana_accent_phrase(segment) {
                Some(phrase) => vec![phrase],
                None => {
                    return Err(auris_vocal::VocalError::NeedsDictionary {
                        text: segment.to_string(),
                    }
                    .into());
                }
            },
        };
        let mut phrase = LyricPhrase {
            moras: Vec::new(),
            contours: Vec::new(),
        };
        for accent in read {
            phrase.contours.extend(accent.contour());
            phrase.moras.extend(accent.moras);
        }
        if !phrase.moras.is_empty() {
            phrases.push(phrase);
        }
    }
    Ok(phrases)
}

/// What a written part's track is called — the accompany convention.
fn part_name(preset: ClipPreset) -> String {
    let name = preset.name();
    let mut first = name.chars();
    match first.next() {
        Some(letter) => letter.to_uppercase().collect::<String>() + first.as_str(),
        None => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::session;

    #[test]
    fn a_kana_lyric_becomes_a_sung_clip_over_stamped_chords() {
        let mut session = session();
        let report = session
            .compose_from_lyrics("さくら さいた\nはるが きた", &[], 7)
            .unwrap();

        assert_eq!(report.phrases, 2);
        assert_eq!(report.notes, 11, "six moras and five, one note each");
        assert!(!report.accented, "no dictionary, no accent");
        assert!(report.chords > 0, "the royal road was laid down");
        assert!(report.parts.is_empty());

        let clip = session.midi_clip(report.clip).unwrap();
        assert_eq!(clip.notes.len(), 11);
        assert_eq!(clip.notes[0].lyric, "さ");
        assert_eq!(clip.notes[0].phonemes, ["s", "a"]);
        assert!(
            clip.notes.iter().all(|note| !note.phonemes.is_empty()),
            "every note carries its word"
        );
        // The harmony is in the document, visible and arguable.
        assert!(session.project().harmony.numeral_at(Ticks::ZERO).is_some());
    }

    #[test]
    fn the_band_comes_along_and_one_undo_takes_the_song_back() {
        let mut session = session();
        let tracks = session.project().tracks.len();
        let report = session
            .compose_from_lyrics("こんにちは", &[ClipPreset::Bass, ClipPreset::Drums], 1)
            .unwrap();
        assert_eq!(report.parts.len(), 2);
        assert_eq!(session.project().tracks.len(), tracks + 3);

        assert_eq!(session.undo(), Some(Edit::ComposeLyrics));
        assert_eq!(session.project().tracks.len(), tracks);
        assert!(
            session.project().harmony.numeral_at(Ticks::ZERO).is_none(),
            "the stamped chords came back off with the song"
        );
    }

    #[test]
    fn the_seed_names_the_song_and_refusals_cost_nothing() {
        let sung = |seed: u64| {
            let mut session = session();
            let report = session
                .compose_from_lyrics("ゆきが ふる", &[], seed)
                .unwrap();
            session.midi_clip(report.clip).unwrap().notes.clone()
        };
        assert_eq!(sung(3), sung(3), "one seed, one song");

        let mut session = session();
        assert!(matches!(
            session.compose_from_lyrics("  \n 、。", &[], 0),
            Err(SessionError::NoLyrics)
        ));
        assert!(
            matches!(
                session.compose_from_lyrics("漢字の歌詞", &[], 0),
                Err(SessionError::Vocal(_))
            ),
            "kanji without a dictionary names the cure"
        );
        assert!(!session.can_undo(), "a refusal costs no step");
        assert_eq!(session.project().tracks.len(), 0);
    }

    #[test]
    fn a_composed_piece_sings_the_sections_that_carry_words() {
        let spec = auris_compose::SongSpec::parse(
            "form = \"verse chorus verse\"\nending = \"none\"\n[section.verse]\nbars = 4\nlyrics = \"さくら さいた\"\n[section.chorus]\nbars = 4\n",
        )
        .unwrap();
        let piece = auris_compose::compose(&spec);
        let mut session =
            crate::Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
        let report = session.compose(&piece).unwrap();

        assert_eq!(report.sung, 12, "six moras, sung on both playings");
        assert!(report.unsung.is_empty());
        let singer = session
            .project()
            .tracks
            .iter()
            .find(|track| track.kind.is_singer())
            .expect("a vocal track was written");
        let clips = &singer.kind.as_singer().unwrap().clips;
        assert_eq!(
            clips.len(),
            2,
            "one clip per playing of the lyrical section"
        );
        assert_eq!(clips[0].notes.len(), 6);
        assert_eq!(clips[0].notes[0].lyric, "さ");
        assert!(clips[0].notes.iter().all(|note| !note.phonemes.is_empty()));
        assert!(clips[0].notes[0].scoop.is_some(), "the phrase scoops in");
        assert!(
            clips[0].notes.last().unwrap().vibrato.is_some(),
            "the held final sways"
        );
        // Two playings of one section are one idea, sung the same both times.
        assert_eq!(clips[0].notes, clips[1].notes);

        // An instrumental spec writes no vocal track at all.
        let plain =
            auris_compose::compose(&auris_compose::SongSpec::parse("form = \"verse\"").unwrap());
        let mut second =
            crate::Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
        let report = second.compose(&plain).unwrap();
        assert_eq!(report.sung, 0);
        assert!(
            second
                .project()
                .tracks
                .iter()
                .all(|track| !track.kind.is_singer()),
            "no empty vocal track for an instrumental piece"
        );
    }

    #[test]
    fn the_measure_counts_what_write_would_sing() {
        let session = session();
        let meter = auris_core::time::TimeSignature::new(4, 4);
        let measure = session.measure_lyrics("さくら さいた\nはるが きた", meter);

        assert_eq!(measure.lines, vec![Some(6), Some(5)]);
        assert_eq!(measure.notes, 11, "one note per mora, same as composing");
        // The same rhythm Write uses: phrase one holds its last syllable into bar two,
        // phrase two starts on the next free bar line and does the same.
        let rhythm = vocal_rhythm(&[6, 5], meter);
        let bar = meter.ticks_per_bar().raw();
        assert_eq!(measure.bars as i64, rhythm.length.raw() / bar);
        assert_eq!(measure.bars, 3);

        // A line nobody can read measures as None and costs nothing; the readable line
        // still counts. Punctuation splits phrases without adding notes.
        let mixed = session.measure_lyrics("漢字の歌詞\nゆき、ふる", meter);
        assert_eq!(mixed.lines, vec![None, Some(4)]);
        assert_eq!(mixed.notes, 4);

        // Nothing to sing, nothing to measure: no phantom bar for an empty box.
        let empty = session.measure_lyrics("", meter);
        assert_eq!(empty.bars, 0);
        assert_eq!(empty.notes, 0);
    }

    #[test]
    fn a_harmony_already_written_is_left_alone() {
        let mut session = session();
        session
            .stamp_named_progression("canon", Ticks::ZERO, 4)
            .unwrap();
        session.forget_history();
        let before = session.project().harmony.chords.points().to_vec();

        let report = session.compose_from_lyrics("そらを とぶ", &[], 2).unwrap();
        assert_eq!(report.chords, 0, "nothing was stamped");
        assert_eq!(session.project().harmony.chords.points(), &before[..]);
    }
}
