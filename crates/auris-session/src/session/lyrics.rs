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

use auris_compose::vocal::{
    VocalRange, VocalRhythm, ornament_vocal, vocal_rhythm, vocal_rhythm_in_bars, write_vocal,
};
use auris_core::theory::contour::Contour;
use auris_core::time::{TICKS_PER_QUARTER, Ticks, TimeSignature};
use auris_core::{ClipId, ClipPreset, Note, PresetRef, TrackId};
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

fn is_closure(mora: &SungMora) -> bool {
    mora.phonemes == ["ʔ"]
}

fn closure_positions(phrases: &[LyricPhrase]) -> Vec<Vec<bool>> {
    phrases
        .iter()
        .map(|phrase| phrase.moras.iter().map(is_closure).collect())
        .collect()
}

/// Keeps phrase onsets while making both note lengths and the span follow closure durations.
fn rhythm_with_closures(
    counts: &[usize],
    closures: &[Vec<bool>],
    meter: TimeSignature,
) -> VocalRhythm {
    let mut rhythm = vocal_rhythm(counts, meter);
    for (slots, closures) in rhythm.phrases.iter_mut().zip(closures) {
        for ((_, length), closure) in slots.iter_mut().zip(closures) {
            if *closure {
                *length = (*length).min(Ticks(TICKS_PER_QUARTER / 2));
            }
        }
    }
    let end = rhythm
        .phrases
        .iter()
        .flatten()
        .map(|(onset, length)| *onset + *length)
        .max()
        .unwrap_or(Ticks::ZERO)
        .raw();
    let bar = meter.ticks_per_bar().raw().max(1);
    rhythm.length = Ticks(((end + bar - 1) / bar).max(1) * bar);
    rhythm
}

fn lyric_rhythm(phrases: &[LyricPhrase], meter: TimeSignature) -> VocalRhythm {
    let counts: Vec<_> = phrases.iter().map(|phrase| phrase.moras.len()).collect();
    rhythm_with_closures(&counts, &closure_positions(phrases), meter)
}

fn fitted_lyric_rhythm(
    phrases: &[LyricPhrase],
    meter: TimeSignature,
    bars: usize,
) -> Option<VocalRhythm> {
    let counts: Vec<_> = phrases.iter().map(|phrase| phrase.moras.len()).collect();
    let mut rhythm = vocal_rhythm_in_bars(&counts, meter, bars)?;
    for (slots, phrase) in rhythm.phrases.iter_mut().zip(phrases) {
        for ((_, length), mora) in slots.iter_mut().zip(&phrase.moras) {
            if is_closure(mora) {
                *length = (*length).min(Ticks(TICKS_PER_QUARTER / 2));
            }
        }
    }
    Some(rhythm)
}

/// A shared melody must fit the shortest section that sings it.
fn vocal_bars(spec: &auris_compose::SongSpec, source: &str) -> usize {
    spec.form
        .iter()
        .filter_map(|name| spec.sections.get(name))
        .filter(|section| {
            section.name == source
                || (section.melody_from.as_deref() == Some(source)
                    && !section.lyrics.trim().is_empty())
        })
        .map(|section| section.bars)
        .min()
        .unwrap_or(0)
}

/// Search only the voiced melody, then restore the unvoiced, lyric-bearing closure slots.
fn write_lyric_vocal(
    project: &auris_core::Project,
    start: Ticks,
    rhythm: &VocalRhythm,
    phrases: &[LyricPhrase],
    seed: u64,
) -> Vec<Note> {
    let mut sung = rhythm.clone();
    let mut contours = Vec::new();
    let mut closures = Vec::new();
    for (slots, phrase) in sung.phrases.iter_mut().zip(phrases) {
        let mut voiced_contours = Vec::new();
        let mut index = 0;
        let mut skipped = false;
        slots.retain(|&(onset, length)| {
            let mora = &phrase.moras[index];
            let contour = phrase.contours[index];
            index += 1;
            if is_closure(mora) {
                closures.push((onset, length));
                skipped = true;
                false
            } else {
                // The contour describes adjacent spoken moras. Across a closure there is
                // no single audible step to constrain, so let harmony choose that interval.
                voiced_contours.push(if skipped { Contour::Free } else { contour });
                skipped = false;
                true
            }
        });
        contours.push(voiced_contours);
    }
    let mut notes = write_vocal(
        &project.harmony,
        start,
        &sung,
        &contours,
        VocalRange::default(),
        seed,
    );
    ornament_vocal(&mut notes, &sung, &project.tempo_map, start);
    let closure_notes: Vec<_> = closures
        .into_iter()
        .map(|(onset, length)| {
            let pitch = notes
                .iter()
                .find(|note| note.start > onset)
                .or_else(|| notes.last())
                .map_or(60, |note| note.pitch);
            Note::new(pitch, onset, length)
        })
        .collect();
    notes.extend(closure_notes);
    notes.sort_by_key(|note| note.start);
    notes
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
    /// Checks vocal density and shared phrases before composing can replace the document.
    /// Different verses must fit the same note slots, including phrase boundaries.
    pub fn validate_song_lyrics(&self, spec: &auris_compose::SongSpec) -> Result<(), SessionError> {
        for name in &spec.form {
            let Some(section) = spec.sections.get(name) else {
                continue;
            };
            let Some(source) = &section.melody_from else {
                if !section.lyrics.trim().is_empty()
                    && let Ok(phrases) = read_lyrics(&section.lyrics, self.japanese.as_ref())
                    && fitted_lyric_rhythm(&phrases, spec.meter, vocal_bars(spec, name)).is_none()
                {
                    return Err(SessionError::SongLyrics(format!(
                        "{name}: too many syllables for the fixed section length; shorten the lyrics or reduce phrase breaks"
                    )));
                }
                continue;
            };
            if section.lyrics.trim().is_empty() {
                continue;
            }
            let original = spec
                .sections
                .get(source)
                .filter(|s| s.melody_from.is_none() && source != name && spec.form.contains(source))
                .ok_or_else(|| {
                    SessionError::SongLyrics(format!(
                        "{name}: original section `{source}` must be in the form"
                    ))
                })?;
            let original_phrases = read_lyrics(&original.lyrics, self.japanese.as_ref())?;
            let repeated_phrases = read_lyrics(&section.lyrics, self.japanese.as_ref())?;
            let counts = |phrases: &[LyricPhrase]| -> Vec<usize> {
                phrases.iter().map(|phrase| phrase.moras.len()).collect()
            };
            let expected = counts(&original_phrases);
            let actual = counts(&repeated_phrases);
            if expected.is_empty() || expected != actual {
                return Err(SessionError::SongLyrics(format!(
                    "{name} → {source}: notes per phrase must match; expected {expected:?}, got {actual:?}"
                )));
            }
            if closure_positions(&original_phrases) != closure_positions(&repeated_phrases) {
                return Err(SessionError::SongLyrics(format!(
                    "{name} → {source}: closure positions per phrase must match"
                )));
            }
        }
        Ok(())
    }

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
        let rhythm = lyric_rhythm(&phrases, meter);
        let bar = meter.ticks_per_bar().raw().max(1);
        let bars = (rhythm.length.raw() / bar) as usize;

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

        let notes = write_lyric_vocal(&self.project, Ticks::ZERO, &rhythm, &phrases, seed);

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
            let added = if preset.is_drums() {
                self.add_default_drum_track(part_name(*preset))
            } else {
                self.add_default_instrument_track(part_name(*preset))
            };
            let Ok(band) = added else {
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
            let recipe =
                super::accompany::backing_part_recipe(*preset, seed.wrapping_add(1 + index as u64));
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
    /// part of the same single edit. Each original melody is written once over its first
    /// playing's harmony. A section with `melody_from` reuses those notes with its own words,
    /// after validation has proved that every mora fits. There is one clip per playing.
    /// Words are fitted to the section's fixed bars. A lyric that cannot be read at
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
        // Compose originals once. Repeated sections reuse the actual notes, so a
        // new lyric's accent cannot change the melody or consume different slots.
        let mut melodies = std::collections::HashMap::new();
        for (span, phrases) in &prepared {
            if spec.sections[&span.label].melody_from.is_some()
                || melodies.contains_key(&span.label)
            {
                continue;
            }
            let Some(rhythm) = fitted_lyric_rhythm(phrases, meter, vocal_bars(&spec, &span.label))
            else {
                continue;
            };
            let notes = write_lyric_vocal(project, span.start, &rhythm, phrases, spec.seed);
            melodies.insert(span.label.clone(), (rhythm, notes));
        }
        let (mut sung, mut clips) = (0usize, 0usize);
        for (span, phrases) in prepared {
            let source = spec.sections[&span.label]
                .melody_from
                .as_ref()
                .unwrap_or(&span.label);
            let Some((rhythm, notes)) = melodies.get(source) else {
                continue;
            };

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
            if let Some(target) = project.midi_clip_mut(clip) {
                // The length is the section's, exactly as a band clip's is.
                target.length_is_explicit = true;
                for note in notes {
                    let Some(mora) = moras.get(&note.start) else {
                        continue;
                    };
                    target.notes.push(Note {
                        lyric: mora.text.clone(),
                        phonemes: mora.phonemes.clone(),
                        ..note.clone()
                    });
                    sung += 1;
                }
            }
            clips += 1;
        }
        (sung, clips, unsung)
    }
}

/// Mora counts and an unconstrained rhythm estimate for a lyric, line by line.
///
/// `bars` describes the standalone lyric command. For preset sections, `fits_in_bars`
/// checks the fixed-length rhythm allocator that song composition uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LyricsMeasure {
    /// Mora counts of readable musical phrases, including punctuation boundaries.
    pub phrases: Vec<usize>,
    /// Closure positions needed to recompute the same note lengths in another meter.
    closures: Vec<Vec<bool>>,
    /// Notes each line of the lyric would sing — one per mora — or `None` for a line that
    /// cannot be read at all (kanji with no dictionary anywhere).
    pub lines: Vec<Option<usize>>,
    /// Notes the whole lyric would sing, counting only the readable lines.
    pub notes: usize,
    /// Bars the standalone lyric rhythm would cover, with phrase-final closures kept short.
    pub bars: usize,
}

impl LyricsMeasure {
    /// Whether all readable words fit a fixed section with sixteenth notes and breaths.
    /// Returns no estimate if any line cannot be read.
    pub fn fits_in_bars(&self, meter: TimeSignature, bars: usize) -> Option<bool> {
        (!self.lines.contains(&None))
            .then(|| vocal_rhythm_in_bars(&self.phrases, meter, bars).is_some())
    }

    /// Counts complete notes that fit the section, using the composer's phrase rhythm.
    /// An unreadable lyric has no reliable capacity estimate and returns no count.
    pub fn notes_within_bars(&self, meter: TimeSignature, bars: usize) -> Option<usize> {
        if self.lines.contains(&None) {
            return None;
        }
        let end = meter.ticks_per_bar() * bars as i64;
        Some(
            rhythm_with_closures(&self.phrases, &self.closures, meter)
                .phrases
                .iter()
                .flatten()
                .filter(|(onset, duration)| *onset + *duration <= end)
                .count(),
        )
    }
}

impl Session {
    /// Measures a lyric without writing anything: notes per line, notes in all, bars needed.
    ///
    /// For the editor's margin, so words can be fitted to a section while they are typed
    /// rather than discovered to outrun it at Write. Unreadable lines measure as `None` and
    /// simply do not count — the same lines Write would refuse or skip.
    pub fn measure_lyrics(&self, lyrics: &str, meter: TimeSignature) -> LyricsMeasure {
        let mut lines = Vec::new();
        let mut phrases = Vec::new();
        let mut notes = 0usize;
        for line in lyrics.split('\n') {
            match read_lyrics(line, self.japanese.as_ref()) {
                Ok(read) => {
                    let count = read.iter().map(|phrase| phrase.moras.len()).sum::<usize>();
                    notes += count;
                    lines.push(Some(count));
                    phrases.extend(read);
                }
                Err(_) => lines.push(None),
            }
        }
        let bars = match phrases.is_empty() {
            true => 0,
            false => {
                let bar = meter.ticks_per_bar().raw().max(1);
                (lyric_rhythm(&phrases, meter).length.raw() / bar) as usize
            }
        };
        LyricsMeasure {
            lines,
            notes,
            bars,
            closures: closure_positions(&phrases),
            phrases: phrases.iter().map(|phrase| phrase.moras.len()).collect(),
        }
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

    fn two_verses() -> auris_compose::SongSpec {
        auris_compose::SongSpec::parse(
            r#"
            form = "verse verse2 verse3"
            [section.verse]
            lyrics = "さくら\nさいた"
            [section.verse2]
            melody_from = "verse"
            lyrics = "ひかり\nとどく"
            [section.verse3]
            melody_from = "verse"
            lyrics = "あした\nはれる"
        "#,
        )
        .unwrap()
    }

    #[test]
    fn closures_keep_words_and_timing_without_sung_ornaments() {
        for words in ["ずっと", "あっ", "ッあ", "あっっと", "っっ"] {
            let mut session = session();
            let report = session.compose_from_lyrics(words, &[], 7).unwrap();
            let clip = session.midi_clip(report.clip).unwrap();
            let moras = auris_vocal::split_kana_lyric(words).unwrap();
            assert_eq!(clip.notes.len(), moras.len());
            for (index, (note, (text, phonemes))) in clip.notes.iter().zip(moras).enumerate() {
                assert_eq!(note.lyric, text);
                assert_eq!(note.phonemes, phonemes);
                assert_eq!(note.start, Ticks(TICKS_PER_QUARTER / 2 * index as i64));
                if phonemes == ["ʔ"] {
                    assert_eq!(note.length, Ticks(TICKS_PER_QUARTER / 2));
                    assert!(note.scoop.is_none() && note.fall.is_none() && note.vibrato.is_none());
                }
            }
            assert!(
                clip.notes
                    .windows(2)
                    .all(|pair| pair[0].end() <= pair[1].start)
            );
        }
    }

    #[test]
    fn phrase_final_sokuon_measurement_matches_bounded_composition() {
        let mut session = session();
        let meter = TimeSignature::new(4, 4);
        let lyrics = "あいうえおっ";
        let spec = auris_compose::SongSpec::parse(
            r#"
            form = "verse"
            [section.verse]
            bars = 1
            lyrics = "あいうえおっ"
        "#,
        )
        .unwrap();
        let report = session.compose(&auris_compose::compose(&spec)).unwrap();
        assert_eq!(report.sung, 6, "all six written slots fit in one bar");
        let clip = &session
            .project()
            .tracks
            .iter()
            .find_map(|track| track.kind.as_singer())
            .unwrap()
            .clips[0];
        assert!(clip.notes.last().unwrap().length <= Ticks(TICKS_PER_QUARTER / 2));
        assert!(clip.notes.iter().all(|note| note.end() <= clip.length));

        let measured = session.measure_lyrics(lyrics, meter);
        assert_eq!(measured.notes, report.sung);
        assert_eq!(
            (measured.bars, measured.notes_within_bars(meter, 1)),
            (1, Some(6)),
            "the displayed fit must agree with the actual closure duration"
        );
        let from_lyrics = session.compose_from_lyrics(lyrics, &[], 7).unwrap();
        assert_eq!(from_lyrics.bars, 1);
        assert_eq!(from_lyrics.notes, 6);
    }

    #[test]
    fn phrase_final_sokuon_shared_melodies_fit_one_bar() {
        let mut session = session();
        let spec = auris_compose::SongSpec::parse(
            r#"
            form = "verse verse2"
            [section.verse]
            bars = 1
            lyrics = "あいうえおっ"
            [section.verse2]
            bars = 1
            melody_from = "verse"
            lyrics = "かきくけこっ"
        "#,
        )
        .unwrap();
        session.validate_song_lyrics(&spec).unwrap();
        let report = session.compose(&auris_compose::compose(&spec)).unwrap();
        assert_eq!(report.sung, 12);
        let singer = session
            .project()
            .tracks
            .iter()
            .find_map(|track| track.kind.as_singer())
            .unwrap();
        assert_eq!(singer.clips.len(), 2);
        for clip in &singer.clips {
            assert_eq!(clip.length, Ticks::QUARTER * 4);
            assert_eq!(clip.notes.len(), 6);
            assert!(clip.notes.iter().all(|note| note.end() <= clip.length));
        }
    }

    #[test]
    fn shared_melodies_require_matching_closures() {
        let mut session = session();
        let mut spec = two_verses();
        spec.sections.get_mut("verse").unwrap().lyrics = "ずっと\nあっっ".into();
        spec.sections.get_mut("verse2").unwrap().lyrics = "きっと\nうっっ".into();
        spec.sections.get_mut("verse3").unwrap().lyrics.clear();
        session.validate_song_lyrics(&spec).unwrap();
        session.compose(&auris_compose::compose(&spec)).unwrap();
        let closures: Vec<_> = session
            .project()
            .tracks
            .iter()
            .filter_map(|track| track.kind.as_singer())
            .flat_map(|track| &track.clips)
            .flat_map(|clip| &clip.notes)
            .filter(|note| note.phonemes == ["ʔ"])
            .collect();
        assert_eq!(closures.len(), 6);
        assert!(
            closures
                .iter()
                .all(|note| note.length == Ticks(TICKS_PER_QUARTER / 2)
                    && note.vibrato.is_none()
                    && note.fall.is_none()
                    && note.scoop.is_none())
        );
        spec.sections.get_mut("verse2").unwrap().lyrics = "きみと\nうっっ".into();
        let before = session.project().clone();
        assert!(session.compose(&auris_compose::compose(&spec)).is_err());
        assert_eq!(session.project(), &before);
    }

    #[test]
    fn a_closure_cannot_steer_the_pitch_search() {
        let session = session();
        let mut phrases = read_lyrics("ずっと", None).unwrap();
        let rhythm = vocal_rhythm(&[3], TimeSignature::default());
        let original = write_lyric_vocal(session.project(), Ticks::ZERO, &rhythm, &phrases, 7);
        phrases[0].contours[1] = Contour::Fall;
        let falling = write_lyric_vocal(session.project(), Ticks::ZERO, &rhythm, &phrases, 7);
        phrases[0].contours[1] = Contour::Rise;
        let rising = write_lyric_vocal(session.project(), Ticks::ZERO, &rhythm, &phrases, 7);
        assert_eq!(original, falling);
        assert_eq!(original, rising);
        assert_eq!(original[1].pitch, original[2].pitch);
    }

    #[test]
    fn later_verses_change_only_words_and_keep_every_note_slot() {
        let mut session = session();
        let spec = two_verses();
        let report = session.compose(&auris_compose::compose(&spec)).unwrap();
        assert_eq!(report.sung, 18);
        let vocal = session
            .project()
            .tracks
            .iter()
            .find(|t| t.kind.is_singer())
            .unwrap();
        let clips = &vocal.kind.as_singer().unwrap().clips;
        assert_eq!(clips.len(), 3);
        for repeated in &clips[1..] {
            assert_eq!(repeated.notes.len(), clips[0].notes.len());
            for (original, note) in clips[0].notes.iter().zip(&repeated.notes) {
                assert_ne!(original.lyric, note.lyric);
                assert!(!note.phonemes.is_empty());
                let mut musical = note.clone();
                musical.lyric = original.lyric.clone();
                musical.phonemes = original.phonemes.clone();
                assert_eq!(&musical, original);
            }
        }
    }

    #[test]
    fn mismatched_phrases_refuse_before_editing() {
        let mut session = session();
        let before = session.project().clone();
        for lyrics in [
            "あい\nうえおか",
            "あいうえ\nおか",
            "あいうえおか",
            "あい、うえおか",
        ] {
            let mut spec = two_verses();
            spec.sections.get_mut("verse2").unwrap().lyrics = lyrics.into();
            assert!(matches!(
                session.compose(&auris_compose::compose(&spec)),
                Err(SessionError::SongLyrics(_))
            ));
            assert_eq!(session.project(), &before);
        }
        let mut spec = two_verses();
        spec.sections.get_mut("verse2").unwrap().bars = 1;
        assert!(session.validate_song_lyrics(&spec).is_ok());
        session.compose(&auris_compose::compose(&spec)).unwrap();
        let singer = session
            .project()
            .tracks
            .iter()
            .find_map(|track| track.kind.as_singer())
            .unwrap();
        for clip in &singer.clips {
            assert_eq!(clip.notes.len(), 6);
            assert!(
                clip.notes
                    .iter()
                    .all(|note| note.end() <= Ticks::QUARTER * 4)
            );
        }
        assert_eq!(
            singer.clips[0]
                .notes
                .iter()
                .map(|note| (note.start, note.length, note.pitch))
                .collect::<Vec<_>>(),
            singer.clips[1]
                .notes
                .iter()
                .map(|note| (note.start, note.length, note.pitch))
                .collect::<Vec<_>>()
        );
        spec.sections.get_mut("verse2").unwrap().lyrics.clear();
        assert!(session.validate_song_lyrics(&spec).is_ok());
        spec.sections.get_mut("verse").unwrap().lyrics.clear();
        assert!(session.validate_song_lyrics(&spec).is_err());
    }

    #[test]
    fn preset_sections_fit_short_and_long_lyrics_without_moving_the_band() {
        for lyrics in [
            "さくら",
            "さくらさいた\nはるがきた",
            &"あいうえお".repeat(18),
        ] {
            let mut session = session();
            let mut spec = auris_compose::preset("pop-band").unwrap().spec();
            let before = spec.total_bars();
            spec.sections.get_mut("verse").unwrap().lyrics = lyrics.into();
            let composition = auris_compose::compose(&spec);
            let report = session.compose(&composition).unwrap();
            let expected = session.measure_lyrics(lyrics, spec.meter).notes;
            let singer = session
                .project()
                .tracks
                .iter()
                .find_map(|track| track.kind.as_singer())
                .unwrap();
            assert!(!singer.clips.is_empty());
            assert_eq!(report.sung, expected * singer.clips.len());
            for clip in &singer.clips {
                assert_eq!(
                    clip.length,
                    spec.meter.ticks_per_bar() * spec.sections["verse"].bars as i64
                );
                assert_eq!(clip.notes.len(), expected);
                assert!(
                    clip.notes
                        .iter()
                        .all(|note| !note.lyric.is_empty() && note.end() <= clip.length)
                );
                assert!(
                    clip.notes
                        .windows(2)
                        .all(|pair| pair[0].end() <= pair[1].start)
                );
                assert!(clip.notes.last().unwrap().end().raw() > clip.length.raw() / 2);
            }
            let saved =
                auris_compose::SongSpec::parse(session.project().song_spec.as_ref().unwrap())
                    .unwrap();
            assert_eq!(saved.total_bars(), before);
            for (name, section) in &spec.sections {
                assert_eq!(saved.sections[name].bars, section.bars);
            }
        }
    }

    #[test]
    fn impossible_density_is_reported_without_dropping_words_or_replacing_the_document() {
        let mut session = session();
        let before = session.project().clone();
        let mut spec =
            auris_compose::SongSpec::parse("form = 'verse'\n[section.verse]\nbars = 1").unwrap();
        spec.sections.get_mut("verse").unwrap().lyrics = "あ".repeat(100);
        let measure = session.measure_lyrics(&spec.sections["verse"].lyrics, spec.meter);
        assert_eq!(measure.fits_in_bars(spec.meter, 1), Some(false));
        assert!(matches!(
            session.compose(&auris_compose::compose(&spec)),
            Err(SessionError::SongLyrics(_))
        ));
        assert_eq!(session.project(), &before);
    }

    #[test]
    fn a_missing_selected_singer_does_not_replace_the_song() {
        let mut session = session();
        let before = session.project().clone();
        let mut spec = two_verses();
        spec.singer = Some("missing-composition-test-voice.onnx".into());
        assert!(session.compose(&auris_compose::compose(&spec)).is_err());
        assert_eq!(session.project(), &before);
    }

    #[test]
    fn the_selected_voice_and_song_arrive_as_one_undoable_document() {
        let scratch = crate::session::fixtures::Scratch::new("composed-singer");
        let path = scratch.join("singer.voicevox.json");
        std::fs::write(
            &path,
            r#"{
            "format_version": 1, "name": "Song singer", "url": "http://127.0.0.1:1",
            "styles": [{"name": "First", "query_style_id": 6000, "decode_style_id": 3001}]
        }"#,
        )
        .unwrap();
        let mut session = session();
        let before = session.project().clone();
        let mut spec = two_verses();
        spec.singer = Some(path.to_string_lossy().into_owned());
        session.compose(&auris_compose::compose(&spec)).unwrap();
        let singer = session
            .project()
            .tracks
            .iter()
            .find_map(|t| t.kind.as_singer())
            .unwrap();
        let voice = singer.voice.as_ref().unwrap();
        assert_eq!(voice.name, "Song singer");
        assert_eq!(voice.path, auris_core::AssetPath::external(&path));
        let saved =
            auris_compose::SongSpec::parse(session.project().song_spec.as_ref().unwrap()).unwrap();
        assert_eq!(saved.singer, spec.singer);
        assert!(session.undo().is_some());
        assert_eq!(session.project(), &before);
    }

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
        let partly_unreadable = session.measure_lyrics("さくら、歌詞\nはるが きた", meter);
        assert_eq!(partly_unreadable.lines, vec![None, Some(5)]);
        assert_eq!(partly_unreadable.notes, 5);

        // Nothing to sing, nothing to measure: no phantom bar for an empty box.
        let empty = session.measure_lyrics("", meter);
        assert_eq!(empty.bars, 0);
        assert_eq!(empty.notes, 0);
    }

    #[test]
    fn lyric_capacity_counts_complete_notes_and_refuses_partial_readings() {
        let session = session();
        for meter in [
            TimeSignature::new(4, 4),
            TimeSignature::new(3, 4),
            TimeSignature::new(7, 8),
        ] {
            let measured = session.measure_lyrics("こーひー\nさくらさいた", meter);
            assert_eq!(measured.notes, 10);
            assert_eq!(measured.notes_within_bars(meter, 0), Some(0));
            assert_eq!(
                measured.notes_within_bars(meter, measured.bars),
                Some(measured.notes)
            );
            assert_eq!(
                measured.notes_within_bars(meter, measured.bars + 2),
                Some(measured.notes)
            );
        }
        let meter = TimeSignature::new(4, 4);
        assert_eq!(
            session
                .measure_lyrics("", meter)
                .notes_within_bars(meter, 8),
            Some(0)
        );
        assert_eq!(
            session
                .measure_lyrics("さくら\n歌詞", meter)
                .notes_within_bars(meter, 8),
            None
        );
        let measure = session.measure_lyrics("さくら さいた\nはるが きた", meter);
        assert_eq!(measure.notes_within_bars(meter, 1), Some(5));
        assert_eq!(measure.notes_within_bars(meter, 2), Some(6));
        assert_eq!(measure.notes_within_bars(meter, 3), Some(11));
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
