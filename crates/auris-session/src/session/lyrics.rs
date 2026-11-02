//! Vocal writing, lyric validation and measurement for song sections.
//!
//! Japanese lyrics are read into moras and pitch-accent contours. The song composer fits
//! them to each section's bars and harmony, producing editable singer clips.

use auris_compose::vocal::{
    VocalPhraseProsody, VocalRange, VocalRhythm, ornament_vocal, vocal_rhythm,
    vocal_rhythm_expressive_in_bars, vocal_rhythm_in_bars, write_vocal,
};
use auris_core::theory::contour::Contour;
use auris_core::time::{TICKS_PER_QUARTER, Ticks, TimeSignature};
use auris_core::{ClipId, ClipPreset, Note, TrackId};
use auris_vocal::{SungMora, kana_accent_phrase};

use crate::error::SessionError;

use super::Session;

/// One musical phrase of a lyric: its moras, and what each asks of the melody.
pub(super) struct LyricPhrase {
    /// The phrase as written, kanji and all, which is what a caption shows.
    pub(super) text: String,
    pub(super) moras: Vec<SungMora>,
    contours: Vec<Contour>,
    group_starts: Vec<usize>,
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

pub(super) fn fitted_lyric_rhythm(
    phrases: &[LyricPhrase],
    meter: TimeSignature,
    bars: usize,
    style: Option<auris_compose::PerformanceStyle>,
    seed: u64,
) -> Option<VocalRhythm> {
    let prosody: Vec<_> = phrases
        .iter()
        .map(|phrase| VocalPhraseProsody {
            // A closure has no sung pitch accent and must not steer rhythmic emphasis.
            contours: phrase
                .moras
                .iter()
                .zip(&phrase.contours)
                .map(|(mora, contour)| {
                    if is_closure(mora) {
                        Contour::Free
                    } else {
                        *contour
                    }
                })
                .collect(),
            group_starts: phrase.group_starts.clone(),
        })
        .collect();
    let mut rhythm = vocal_rhythm_expressive_in_bars(&prosody, meter, bars, style, seed)?;
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
pub(super) fn vocal_bars(spec: &auris_compose::SongSpec, source: &str) -> usize {
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

/// Gives only the newly generated backing parts room around the actual sung notes.
fn arrange_vocal_backing(
    project: &mut auris_core::Project,
    vocal_track: TrackId,
    backing_tracks: &[TrackId],
) {
    let vocals = project
        .track(vocal_track)
        .and_then(|track| track.kind.note_clips())
        .cloned()
        .unwrap_or_default();
    for vocal in vocals {
        let clips: Vec<_> = project
            .tracks
            .iter()
            .filter(|track| track.id != vocal_track && backing_tracks.contains(&track.id))
            .filter_map(|track| track.kind.note_clips())
            .flatten()
            .filter(|clip| clip.start == vocal.start && clip.length == vocal.length)
            .filter_map(|clip| {
                clip.recipe
                    .clone()
                    .map(|recipe| (clip.id, recipe, clip.notes.clone()))
            })
            .collect();
        for (id, recipe, mut notes) in clips {
            arrange_generated_backing(
                project,
                vocal.start,
                vocal.length,
                &recipe,
                None,
                &mut notes,
            );
            if let Some(clip) = project.midi_clip_mut(id) {
                clip.notes = notes;
                if let Some(recipe) = &mut clip.recipe {
                    recipe.text_digest = auris_core::notes_digest(&clip.notes);
                }
            }
        }
    }
}

impl Session {
    /// Checks vocal density and shared phrases before composing can replace the document.
    /// Different verses must fit the same note slots, including phrase boundaries.
    pub fn validate_song_lyrics(&self, spec: &auris_compose::SongSpec) -> Result<(), SessionError> {
        let report = self
            .diagnose_song_lyrics(spec)
            .map_err(SessionError::SongLyrics)?;
        if report.status == "pass" {
            Ok(())
        } else {
            Err(SessionError::SongLyrics(
                report
                    .issues
                    .iter()
                    .map(|i| i.message.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ))
        }
    }

    /// Writes the singer track a composed piece's lyrics ask for, into a project still
    /// being built.
    ///
    /// Called from [`Session::compose`] before the document is swapped in, so the vocal is
    /// part of the same single edit. Each original melody is written once over its first
    /// playing's harmony. A section with `melody_from` reuses those notes with its own words,
    /// after validation has proved that every mora fits. There is one clip per playing.
    /// Words are fitted to the section's fixed bars after validation establishes their
    /// pronunciation and capacity. Every phrase is also written into the song-wide
    /// [`lyrics`](auris_core::Project::lyrics) as it was typed, over the notes that sing it, so
    /// a composed song is captioned without being asked. Answers `(sung notes, clips, unsung sections)`;
    /// defensive preparation failures are retained in the report.
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
        // The automatic held coda can share the name "ending" with a written section,
        // but only occurrences in the explicit form carry that section's lyrics.
        let form_end =
            (composition.meter.ticks_per_bar() * spec.total_bars() as i64).min(composition.length);
        for span in project.sections.spans_in(Ticks::ZERO, form_end) {
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
            let Some(rhythm) = fitted_lyric_rhythm(
                phrases,
                meter,
                vocal_bars(&spec, &span.label),
                spec.writing_style,
                spec.seed,
            ) else {
                continue;
            };
            let notes = write_lyric_vocal(project, span.start, &rhythm, phrases, spec.seed);
            melodies.insert(span.label.clone(), (rhythm, notes));
        }
        let (mut sung, mut clips) = (0usize, 0usize);
        let mut lines = Vec::new();
        for (span, phrases) in prepared {
            let source = spec.sections[&span.label]
                .melody_from
                .as_ref()
                .unwrap_or(&span.label);
            let Some((rhythm, notes)) = melodies.get(source) else {
                continue;
            };

            let mut moras: std::collections::HashMap<Ticks, &SungMora> = Default::default();
            let mut section_lines = Vec::new();
            for (slots, phrase) in rhythm.phrases.iter().zip(&phrases) {
                for ((onset, _), mora) in slots.iter().zip(&phrase.moras) {
                    moras.insert(*onset, mora);
                }
                // The song-wide lyrics get the phrase as written, over the notes that sing it.
                if let Some(&(first, _)) = slots.first() {
                    let end = slots
                        .iter()
                        .map(|(onset, length)| *onset + *length)
                        .max()
                        .unwrap_or(first);
                    section_lines.push((span.start + first, span.start + end, phrase.text.clone()));
                }
            }

            let length = span.end - span.start;
            let Some(clip) = project.add_midi_clip(track, &span.label, span.start, length) else {
                continue;
            };
            lines.extend(section_lines);
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
                let original = spec.sections[&span.label].lyrics.trim();
                let written: String = target
                    .notes
                    .iter()
                    .map(|note| note.lyric.as_str())
                    .collect();
                if written != original {
                    for (index, note) in target.notes.iter_mut().enumerate() {
                        note.lyric = if index == 0 {
                            original.to_owned()
                        } else {
                            super::singer::LYRIC_CONTINUATION.to_owned()
                        };
                    }
                }
                target.name = original.to_owned();
            }
            clips += 1;
        }
        for (start, end, text) in lines {
            project.lyrics.write_line(start, end, &text);
        }
        let backing_tracks: Vec<_> = project.tracks.iter().map(|track| track.id).collect();
        arrange_vocal_backing(project, track, &backing_tracks);
        (sung, clips, unsung)
    }
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
            text: segment.to_string(),
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
        assert!((0..16).any(|seed| {
            fitted_lyric_rhythm(&phrases[..1], TimeSignature::default(), 4, None, seed)
                != fitted_lyric_rhythm(&continuous, TimeSignature::default(), 4, None, seed)
        }));

        // Whitespace must not destroy the vowel context or split a two-kana mora.
        let stretched = read_lyrics("き ょ ー こ ー", None).unwrap();
        let contiguous = read_lyrics("きょーこー", None).unwrap();
        assert_eq!(stretched[0].moras, contiguous[0].moras);
        assert_eq!(stretched[0].moras.len(), 4);
        assert_eq!(stretched[0].moras[1].phonemes, ["o"]);
        assert_eq!(stretched[0].moras[3].phonemes, ["o"]);
    }

    #[test]
    fn generated_backing_answers_the_actual_vocal_breath_and_refreshes_its_digest() {
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
        let lead = session
            .project
            .add_midi_clip(vocal, "voice", Ticks::ZERO, bar)
            .unwrap();
        let foreground: Vec<_> = (0..6)
            .map(|at| Note::new(72, Ticks(at * 480), Ticks(480)))
            .collect();
        session.project.midi_clip_mut(lead).unwrap().notes = foreground.clone();
        let original: Vec<_> = (0..6)
            .flat_map(|at| [60, 64, 67].map(|pitch| Note::new(pitch, Ticks(at * 480), Ticks(480))))
            .collect();
        let mut tracks = Vec::new();
        let mut clips = Vec::new();
        for authored in [false, true] {
            let track = session.project.add_instrument_track("Comp", "test-piano");
            let id = session
                .project
                .add_midi_clip(track, "backing", Ticks::ZERO, bar)
                .unwrap();
            let clip = session.project.midi_clip_mut(id).unwrap();
            clip.notes = original.clone();
            let mut recipe = auris_core::ClipRecipe::new(ClipPreset::Chords, 1);
            recipe.rhythm = authored.then(|| "xxxxxxxx".into());
            recipe.text_digest = auris_core::notes_digest(&clip.notes);
            clip.recipe = Some(recipe);
            tracks.push(track);
            clips.push(id);
        }
        arrange_vocal_backing(&mut session.project, vocal, &tracks);
        assert_eq!(session.midi_clip(lead).unwrap().notes, foreground);
        let generated = &session.midi_clip(clips[0]).unwrap().notes;
        assert!(
            generated
                .iter()
                .filter(|note| note.start < Ticks(2880))
                .count()
                < original.len()
        );
        assert!(generated.iter().any(|note| note.start == Ticks(2880)));
        assert!(generated.iter().all(|note| note.end() <= bar));
        assert_eq!(session.midi_clip(clips[1]).unwrap().notes, original);
        assert!(!session.clip_hand_edited(clips[0]));
        assert!(!session.clip_hand_edited(clips[1]));

        session.project.track_mut(vocal).unwrap().mixer.mute = true;
        let recipe = auris_core::ClipRecipe::new(ClipPreset::Chords, 1);
        let mut unchanged = original.clone();
        arrange_generated_backing(
            &session.project,
            Ticks::ZERO,
            bar,
            &recipe,
            None,
            &mut unchanged,
        );
        assert_eq!(
            unchanged, original,
            "a muted singer still displaced the backing"
        );
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
    fn composed_backing_reports_the_final_notes_after_following_the_real_voice() {
        let spec = auris_compose::SongSpec::parse(
            r#"
            form = "verse"
            ending = "none"
            writing_style = "pop-band"
            [section.verse]
            bars = 1
            lyrics = "あさのひかりが"
            [[part]]
            name = "comp"
            role = "chords"
            density = 0.7
        "#,
        )
        .unwrap();
        let piece = auris_compose::compose(&spec);
        let mut session = session();
        let report = session.compose(&piece).unwrap();
        let installed: Vec<_> = session
            .project
            .tracks
            .iter()
            .filter_map(|track| track.kind.note_clips())
            .flatten()
            .collect();
        assert_eq!(
            report.notes,
            installed.iter().map(|clip| clip.notes.len()).sum::<usize>()
        );
        assert!(
            installed
                .iter()
                .all(|clip| !session.clip_hand_edited(clip.id))
        );
        let comp = session
            .project
            .tracks
            .iter()
            .find(|track| track.name == "comp")
            .unwrap()
            .kind
            .note_clips()
            .unwrap();
        let initial = piece
            .tracks
            .iter()
            .find(|track| track.name == "comp")
            .unwrap();
        assert_ne!(
            comp[0].notes, initial.clips[0].notes,
            "the backing never consulted the actual sung line"
        );
    }

    #[test]
    fn fixed_capacity_matches_every_styled_take_and_preserves_accent_constraints() {
        let session = session();
        let meter = TimeSignature::default();
        for count in 1..20 {
            let words = "あ".repeat(count);
            let phrases = read_lyrics(&words, None).unwrap();
            let fits = session
                .measure_lyrics(&words, meter)
                .fits_in_bars(meter, 1)
                .unwrap();
            for style in [
                None,
                Some(auris_compose::PerformanceStyle::CityPop),
                Some(auris_compose::PerformanceStyle::Rock),
            ] {
                for seed in 0..4 {
                    let fitted = fitted_lyric_rhythm(&phrases, meter, 1, style, seed);
                    assert_eq!(fitted.is_some(), fits);
                    if let Some(rhythm) = fitted {
                        let notes = write_lyric_vocal(
                            session.project(),
                            Ticks::ZERO,
                            &rhythm,
                            &phrases,
                            seed,
                        );
                        assert_eq!(notes.len(), count);
                        assert!(notes.iter().all(|note| note.end() <= rhythm.length));
                    }
                }
            }
        }
        let mut phrases = read_lyrics("あいうえお", None).unwrap();
        phrases[0].contours = vec![
            Contour::Free,
            Contour::Rise,
            Contour::NoFall,
            Contour::Fall,
            Contour::NoFall,
        ];
        let rhythm = fitted_lyric_rhythm(&phrases, meter, 2, None, 7).unwrap();
        let notes = write_lyric_vocal(session.project(), Ticks::ZERO, &rhythm, &phrases, 7);
        assert!(notes[1].pitch > notes[0].pitch);
        assert!(notes[2].pitch >= notes[1].pitch);
        assert!(notes[3].pitch < notes[2].pitch);
        assert!(notes[4].pitch >= notes[3].pitch);
    }

    #[test]
    fn closures_keep_words_and_timing_without_sung_ornaments() {
        for words in ["ずっと", "あっ", "ッあ", "あっっと", "っっ"] {
            let mut session = session();
            let mut spec = auris_compose::SongSpec::parse(
                "form = \"verse\"\nseed = 7\n[section.verse]\nbars = 2\n",
            )
            .unwrap();
            spec.sections.get_mut("verse").unwrap().lyrics = words.into();
            session.compose(&auris_compose::compose(&spec)).unwrap();
            let clip = &session
                .project()
                .tracks
                .iter()
                .find_map(|track| track.kind.as_singer())
                .unwrap()
                .clips[0];
            let moras = auris_vocal::split_kana_lyric(words).unwrap();
            assert_eq!(clip.name, words);
            assert_eq!(clip.notes.len(), moras.len());
            for (note, (_, phonemes)) in clip.notes.iter().zip(moras) {
                assert_eq!(note.phonemes, phonemes);
                assert!(note.start >= Ticks::ZERO && note.end() <= clip.length);
                if phonemes == ["ʔ"] {
                    assert!(note.length <= Ticks(TICKS_PER_QUARTER / 2));
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
                .all(|note| note.length <= Ticks(TICKS_PER_QUARTER / 2)
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
            assert_ne!(repeated.name, clips[0].name);
            assert_eq!(repeated.notes.len(), clips[0].notes.len());
            for (original, note) in clips[0].notes.iter().zip(&repeated.notes) {
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
            // This checks symbolic lyric placement, independent of render-based mix balancing.
            let report = session.compose_without_balance(&composition).unwrap();
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
    fn vocals_follow_the_explicit_form_even_when_a_section_is_named_ending() {
        for form in ["ending", "ending ending"] {
            let mut session = session();
            let spec = auris_compose::SongSpec::parse(&format!(
                "form = '{form}'\n[section.ending]\nbars = 4\nlyrics = 'さくらさいた'"
            ))
            .unwrap();
            let composition = auris_compose::compose(&spec);
            let bar = spec.meter.ticks_per_bar();
            assert_eq!(composition.length, bar * (spec.total_bars() as i64 + 1));
            let report = session.compose(&composition).unwrap();
            let singer = session
                .project()
                .tracks
                .iter()
                .find_map(|track| track.kind.as_singer())
                .unwrap();
            assert!(
                singer
                    .clips
                    .iter()
                    .all(|clip| clip.notes.iter().all(|note| note.end() <= clip.length)),
                "a four-bar melody must not spill out of the automatic one-bar coda"
            );
            assert_eq!(
                singer.clips.len(),
                spec.form.len(),
                "the automatic coda has no lyrics of its own"
            );
            assert_eq!(report.sung, 6 * spec.form.len());
            for (index, clip) in singer.clips.iter().enumerate() {
                assert_eq!(clip.start, bar * (index as i64 * 4));
                assert_eq!(clip.length, bar * 4);
                assert_eq!(clip.notes.len(), 6);
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
        assert_eq!(clips[0].name, "さくら さいた");
        assert_eq!(clips[0].notes[0].lyric, "さくら さいた");
        assert!(clips[0].notes.iter().all(|note| !note.phonemes.is_empty()));
        assert!(clips[0].notes[0].scoop.is_some(), "the phrase scoops in");
        assert!(
            clips[0].notes.last().unwrap().vibrato.is_some(),
            "the held final sways"
        );
        // Two playings of one section are one idea, sung the same both times.
        assert_eq!(clips[0].notes, clips[1].notes);

        // And the song-wide lyrics carry each playing's line, as written, over its notes.
        let lines = session
            .project()
            .lyrics
            .spans_in(Ticks::ZERO, session.project().end_tick());
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|line| line.text == "さくら さいた"));
        for (line, clip) in lines.iter().zip(clips) {
            let first = clip.start + clip.notes.iter().map(|n| n.start).min().unwrap();
            let last = clip.start + clip.notes.iter().map(|n| n.start + n.length).max().unwrap();
            assert_eq!((line.start, line.end), (first, last));
        }

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
}
