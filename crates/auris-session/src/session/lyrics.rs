//! Vocal writing, lyric validation and measurement for song sections.
//!
//! Japanese lyrics are read into moras and pitch-accent contours. The song composer fits
//! them to each section's bars and harmony, producing editable singer clips.

use auris_compose::vocal::{VocalRhythm, vocal_rhythm, vocal_rhythm_in_bars};
use auris_core::theory::contour::Contour;
use auris_core::time::{TICKS_PER_QUARTER, Ticks, TimeSignature};
use auris_core::{ClipId, ClipPreset, Note};
use auris_vocal::{SungMora, kana_accent_phrase};

use crate::error::SessionError;

use super::Session;

/// One musical phrase of a lyric: its moras, and what each asks of the melody.
pub(super) struct LyricPhrase {
    pub(super) moras: Vec<SungMora>,
    contours: Vec<Contour>,
    pub(super) group_starts: Vec<usize>,
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

/// Shapes a generated support part around an existing voice or explicitly generated lead.
pub(super) fn arrange_generated_backing(
    project: &auris_core::Project,
    start: Ticks,
    length: Ticks,
    recipe: &auris_core::ClipRecipe,
    excluded: Option<ClipId>,
    notes: &mut Vec<Note>,
) {
    // Keep the automatic writer's fast path: these parts need no foreground scan.
    if !matches!(recipe.preset, ClipPreset::Chords | ClipPreset::Arp)
        || recipe.rhythm.is_some()
        || (recipe.preset == ClipPreset::Chords
            && auris_compose::arrangement::chord_subdivision_fill(recipe.density) > 0.0)
    {
        return;
    }
    let end = start + length;
    let note_bounds = |clip: &auris_core::MidiClip, note: &Note| {
        let onset = (clip.start + note.start).max(clip.start).max(start);
        let release = (clip.start + note.end()).min(clip.end()).min(end);
        (note.length > Ticks::ZERO && release > onset).then_some((onset, release))
    };
    let is_foreground = |track: &auris_core::Track, clip: &auris_core::MidiClip| {
        !track.mixer.mute
            && !clip.muted
            && Some(clip.id) != excluded
            && clip.start < end
            && clip.start + clip.length > start
            && (clip
                .recipe
                .as_ref()
                .is_some_and(|r| r.preset == ClipPreset::Lead)
                || (track.kind.is_singer() && clip.recipe.is_none()))
            && clip
                .notes
                .iter()
                .any(|note| note_bounds(clip, note).is_some())
    };
    // A singer owns the foreground when words are present. Choosing one track avoids
    // treating an accompanying instrument's doubled lead as twice the vocal density.
    let foreground_track = project
        .tracks
        .iter()
        .filter(|track| {
            track
                .kind
                .note_clips()
                .is_some_and(|clips| clips.iter().any(|clip| is_foreground(track, clip)))
        })
        .min_by_key(|track| (!track.kind.is_singer(), &track.name, track.id));
    let Some(track) = foreground_track else {
        return;
    };
    let foreground: Vec<_> = track
        .kind
        .note_clips()
        .into_iter()
        .flatten()
        .filter(|clip| is_foreground(track, clip))
        .flat_map(|clip| {
            clip.notes.iter().filter_map(|note| {
                note_bounds(clip, note).map(|(onset, release)| Note {
                    start: onset - start,
                    length: release - onset,
                    ..note.clone()
                })
            })
        })
        .collect();
    arrange_backing_around_notes(project, start, length, recipe, &foreground, notes);
}

/// Applies the same arrangement pass to explicitly selected, clip-relative foreground notes.
pub(super) fn arrange_backing_around_notes(
    project: &auris_core::Project,
    start: Ticks,
    length: Ticks,
    recipe: &auris_core::ClipRecipe,
    foreground: &[Note],
    notes: &mut Vec<Note>,
) -> &'static str {
    if recipe.rhythm.is_some() {
        return "explicit_rhythm";
    }
    if recipe.preset == ClipPreset::Chords
        && auris_compose::arrangement::chord_subdivision_fill(recipe.density) > 0.0
    {
        return "driving_chord_density";
    }
    let role = match recipe.preset {
        ClipPreset::Chords => auris_compose::Role::Chords,
        ClipPreset::Arp => auris_compose::Role::Arp,
        _ => return "role_keeps_its_pattern",
    };
    if recipe.style == Some(auris_compose::PerformanceStyle::Ambient) {
        return "sustained_ambient_texture";
    }
    if foreground.is_empty() {
        return "no_foreground_notes_in_range";
    }
    let meter = project.signatures.signature_at(start);
    let bars = (length.raw() / meter.ticks_per_bar().raw().max(1)).max(0) as usize;
    let phrase_ends: Vec<_> = auris_compose::phrasing::plan_phrases(bars, recipe.style)
        .iter()
        .map(|phrase| meter.ticks_per_bar() * phrase.end_bar() as i64)
        .collect();
    auris_compose::arrangement::accompany_foreground(
        notes,
        foreground,
        meter,
        role,
        recipe.style,
        &phrase_ends,
        |at, pitch, desired| {
            let onset = start + at;
            let valid = |tick| {
                project
                    .harmony
                    .chord_at(tick)
                    .is_some_and(|chord| chord.contains_midi(i32::from(pitch)))
            };
            if !valid(onset) {
                return Ticks::ZERO;
            }
            project
                .harmony
                .chords
                .points()
                .iter()
                .map(|point| point.tick)
                .chain(project.harmony.keys.points().iter().map(|point| point.tick))
                .filter(|tick| *tick > onset && *tick < onset + desired && !valid(*tick))
                .min()
                .map_or(desired, |boundary| boundary - onset)
        },
    );
    "evaluated"
}

/// Mora counts and an unconstrained rhythm estimate for a lyric, line by line.
///
/// `bars` estimates the space a lyric needs without a fixed section length. `fits_in_bars`
/// checks the fixed-length rhythm allocator that song composition uses. Seed and style
/// change the placement of notes, but never this capacity or the estimated bar count.
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
    /// Estimated bars without a fixed section length, with phrase-final closures kept short.
    pub bars: usize,
}

impl LyricsMeasure {
    /// Whether all readable words fit a fixed section with sixteenth notes and breaths.
    /// Returns no estimate if any line cannot be read.
    pub fn fits_in_bars(&self, meter: TimeSignature, bars: usize) -> Option<bool> {
        (!self.lines.contains(&None))
            .then(|| vocal_rhythm_in_bars(&self.phrases, meter, bars).is_some())
    }

    /// Counts complete notes in a prefix of the unconstrained rhythm estimate.
    /// This estimates the unconstrained layout, not permission to drop a lyric's tail;
    /// use `fits_in_bars` to decide whether a fixed section can sing all of its words.
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
pub(super) fn read_lyrics(
    lyrics: &str,
    dictionary: Option<&auris_vocal::JapaneseDictionary>,
) -> Result<Vec<LyricPhrase>, SessionError> {
    let mut phrases = Vec::new();
    for segment in lyrics.split(['\n', '\r', '、', '。', '！', '？', '!', '?']) {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        let mut phrase = LyricPhrase {
            moras: Vec::new(),
            contours: Vec::new(),
            group_starts: Vec::new(),
        };
        // Read the complete phrase before attaching boundaries: a long-vowel mark needs
        // its preceding mora even when whitespace separates them, and the dictionary
        // needs the original context to determine the spoken accent.
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
        for accent in read {
            if !accent.moras.is_empty() {
                phrase.group_starts.push(phrase.moras.len());
                phrase.contours.extend(accent.contour());
                phrase.moras.extend(accent.moras);
            }
        }
        if dictionary.is_none() {
            let mut chars = 0;
            let boundaries: Vec<_> = segment
                .split_whitespace()
                .map(|word| {
                    let start = chars;
                    chars += word.chars().count();
                    start
                })
                .collect();
            chars = 0;
            phrase.group_starts.clear();
            for (index, mora) in phrase.moras.iter().enumerate() {
                if boundaries.contains(&chars) {
                    phrase.group_starts.push(index);
                }
                chars += mora.text.chars().count();
            }
        }
        if !phrase.moras.is_empty() {
            phrases.push(phrase);
        }
    }
    Ok(phrases)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::session;
    use auris_core::theory::numeral::Numeral;

    #[test]
    fn explicit_words_keep_their_groups_without_adding_phrases_or_moras() {
        let phrases = read_lyrics("さくら さいた\nはるが きた", None).unwrap();
        assert_eq!(phrases.len(), 2);
        assert_eq!(phrases[0].group_starts, [0, 3]);
        assert_eq!(phrases[1].group_starts, [0, 3]);
        assert_eq!(phrases[0].moras.len(), 6);
        assert_eq!(phrases[1].moras.len(), 5);
        assert!(
            phrases
                .iter()
                .flat_map(|phrase| &phrase.contours)
                .all(|c| *c == Contour::Free)
        );
        let continuous = read_lyrics("さくらさいた", None).unwrap();
        assert_eq!(continuous[0].group_starts, [0]);

        // Whitespace must not destroy the vowel context or split a two-kana mora.
        let stretched = read_lyrics("き ょ ー こ ー", None).unwrap();
        let contiguous = read_lyrics("きょーこー", None).unwrap();
        assert_eq!(stretched[0].moras, contiguous[0].moras);
        assert_eq!(stretched[0].moras.len(), 4);
        assert_eq!(stretched[0].moras[1].phonemes, ["o"]);
        assert_eq!(stretched[0].moras[3].phonemes, ["o"]);
    }

    #[test]
    fn foreground_selection_skips_silent_clips_and_survives_track_reordering() {
        let mut session = session();
        session
            .project
            .harmony
            .chords
            .set_point(Ticks::ZERO, Numeral::parse("I"));
        let bar = TimeSignature::default().ticks_per_bar();
        let vocal = session
            .project
            .add_singer_track("Vocal", auris_synth::Vocal::ID);
        let voice = session
            .project
            .add_midi_clip(vocal, "voice", Ticks::ZERO, bar)
            .unwrap();
        let instrument = session.project.add_instrument_track("Lead", "test-piano");
        let lead = session
            .project
            .add_midi_clip(instrument, "lead", Ticks::ZERO, bar)
            .unwrap();
        let clip = session.project.midi_clip_mut(lead).unwrap();
        clip.recipe = Some(auris_core::ClipRecipe::new(ClipPreset::Lead, 1));
        clip.notes = (0..6)
            .map(|at| Note::new(72, Ticks(at * 480), Ticks(480)))
            .collect();
        let original: Vec<_> = (0..6)
            .flat_map(|at| [60, 64, 67].map(|pitch| Note::new(pitch, Ticks(at * 480), Ticks(480))))
            .collect();
        let recipe = auris_core::ClipRecipe::new(ClipPreset::Chords, 1);
        for silent in [
            Vec::new(),
            vec![Note::new(72, bar, bar)],
            vec![Note::new(72, Ticks::ZERO, Ticks::ZERO)],
        ] {
            session.project.midi_clip_mut(voice).unwrap().notes = silent;
            let mut notes = original.clone();
            arrange_generated_backing(
                &session.project,
                Ticks::ZERO,
                bar,
                &recipe,
                None,
                &mut notes,
            );
            assert!(notes.iter().filter(|note| note.start < Ticks(2880)).count() < original.len());
            assert!(notes.iter().any(|note| note.start == Ticks(2880)));
        }
        let other_track = session.project.add_instrument_track("Zulu", "test-piano");
        let other = session
            .project
            .add_midi_clip(other_track, "lead", Ticks::ZERO, bar)
            .unwrap();
        let clip = session.project.midi_clip_mut(other).unwrap();
        clip.recipe = Some(auris_core::ClipRecipe::new(ClipPreset::Lead, 1));
        clip.notes = vec![Note::new(72, Ticks::ZERO, bar)];
        let mut expected = original.clone();
        arrange_generated_backing(
            &session.project,
            Ticks::ZERO,
            bar,
            &recipe,
            None,
            &mut expected,
        );
        session.project.tracks.reverse();
        let mut reordered = original;
        arrange_generated_backing(
            &session.project,
            Ticks::ZERO,
            bar,
            &recipe,
            None,
            &mut reordered,
        );
        assert_eq!(reordered, expected);
    }

    #[test]
    fn a_vocal_answer_stops_before_a_conflicting_chord_inside_the_bar() {
        let mut session = session();
        let bar = TimeSignature::default().ticks_per_bar();
        let vocal = session
            .project
            .add_singer_track("Vocal", auris_synth::Vocal::ID);
        let clip = session
            .project
            .add_midi_clip(vocal, "voice", Ticks::ZERO, bar)
            .unwrap();
        session.project.midi_clip_mut(clip).unwrap().notes = (0..6)
            .map(|at| Note::new(72, Ticks(at * 480), Ticks(480)))
            .collect();
        let original: Vec<_> = (0..6)
            .flat_map(|at| [60, 64, 67].map(|pitch| Note::new(pitch, Ticks(at * 480), Ticks(480))))
            .collect();
        let recipe = auris_core::ClipRecipe::new(ClipPreset::Chords, 1);
        session.project.harmony.chords.set_point(
            Ticks::ZERO,
            auris_compose::theory::numeral::Numeral::parse("I"),
        );
        session.project.harmony.chords.set_point(
            Ticks(3120),
            auris_compose::theory::numeral::Numeral::parse("ii"),
        );
        let mut notes = original.clone();
        arrange_generated_backing(
            &session.project,
            Ticks::ZERO,
            bar,
            &recipe,
            None,
            &mut notes,
        );
        let answers: Vec<_> = notes
            .iter()
            .filter(|note| note.start == Ticks(2880))
            .collect();
        assert!(!answers.is_empty());
        assert!(answers.iter().all(|note| note.end() <= Ticks(3120)));

        session.project.harmony.chords.set_point(
            Ticks(2880),
            auris_compose::theory::numeral::Numeral::parse("ii"),
        );
        let mut notes = original.clone();
        arrange_generated_backing(
            &session.project,
            Ticks::ZERO,
            bar,
            &recipe,
            None,
            &mut notes,
        );
        assert!(
            notes.iter().all(|note| note.start < Ticks(2880)),
            "an answer began on an incompatible chord"
        );
        session.project.harmony.chords.set_point(Ticks(2880), None);
        let mut notes = original;
        arrange_generated_backing(
            &session.project,
            Ticks::ZERO,
            bar,
            &recipe,
            None,
            &mut notes,
        );
        assert!(
            notes.iter().all(|note| note.start < Ticks(2880)),
            "an answer filled erased harmony"
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
}
