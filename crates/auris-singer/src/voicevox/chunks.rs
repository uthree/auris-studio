//! Score-aware windows with real neighbouring context for bounded Engine requests.

use std::ops::Range;

use auris_vocal::{SingerFrames, SingerScore};

use super::{BOUNDARY_SECONDS, starts_with_consonant};

/// A bounded inference includes both neighbouring music and the adapter's boundary rests.
const REQUEST_SECONDS: f64 = 20.0;
/// A short overlap masks decoder phase differences when a phrase has no usable pause.
const CROSSFADE_SECONDS: f64 = 0.04;

pub(super) struct ChunkPlan {
    pub(super) cores: Vec<Range<usize>>,
    pub(super) context: usize,
    pub(super) crossfade: usize,
}

impl ChunkPlan {
    pub(super) fn new(score: &SingerScore, count: usize, frame_rate: f64) -> Self {
        let context = (frame_rate * BOUNDARY_SECONDS).ceil() as usize;
        let crossfade = (frame_rate * CROSSFADE_SECONDS).ceil() as usize;
        let limit = (frame_rate * REQUEST_SECONDS).floor() as usize;
        let mut cores = Vec::new();
        if count + 2 * context <= limit {
            cores.push(0..count);
            return Self {
                cores,
                context,
                crossfade,
            };
        }
        let maximum = limit.saturating_sub(4 * context + crossfade).max(2);
        let mut boundaries = Vec::new();
        let mut rests = Vec::new();
        let mut at = 0;
        for note in &score.notes {
            let end = at + note.frame_length as usize;
            if note.key.is_none() {
                rests.push(at + (end - at) / 2);
            }
            boundaries.push(end);
            at = end;
        }
        let mut start = 0;
        while count - start > maximum {
            let ceiling = start + maximum;
            let floor = start + maximum / 2;
            let latest = |candidates: &[usize]| {
                candidates[..candidates.partition_point(|at| *at <= ceiling)]
                    .last()
                    .copied()
                    .filter(|at| *at >= floor)
            };
            let end = latest(&rests)
                .or_else(|| latest(&boundaries))
                .unwrap_or(ceiling);
            cores.push(start..end);
            start = end;
        }
        cores.push(start..count);
        Self {
            cores,
            context,
            crossfade,
        }
    }

    pub(super) fn window(&self, core: &Range<usize>, count: usize) -> Range<usize> {
        core.start.saturating_sub(self.context)
            ..(core.end + self.context + self.crossfade).min(count)
    }
}

/// Frame ranges sung independently because only silence lies within `margin` of each.
///
/// `normalized` is the padded score, whose host clock starts `offset` frames in. A rest of at
/// least `2 * margin` frames separates two phrases; each range keeps `margin` frames of that
/// rest on either side, clamped to the timeline. What a range holds therefore depends on its
/// own notes alone, not on where its neighbours are, which is what lets a rendered phrase be
/// reused after an edit elsewhere. The ranges are ordered and never overlap.
pub(super) fn phrase_spans(
    normalized: &SingerScore,
    offset: usize,
    count: usize,
    margin: usize,
) -> Vec<Range<usize>> {
    let mut sung: Vec<Range<usize>> = Vec::new();
    let mut at = 0usize;
    for note in &normalized.notes {
        let end = at + note.frame_length as usize;
        let silent = note.key.is_none() && note.lyric.is_empty();
        let range = at.saturating_sub(offset).min(count)..end.saturating_sub(offset).min(count);
        at = end;
        if silent || range.is_empty() {
            continue;
        }
        match sung.last_mut() {
            Some(last) if range.start < last.end + 2 * margin => last.end = range.end,
            _ => sung.push(range),
        }
    }
    sung.into_iter()
        .map(|range| range.start.saturating_sub(margin)..(range.end + margin).min(count))
        .collect()
}

/// Slice an already normalized score. A sustained syllable resumes on its final sound,
/// rather than singing its initial consonant again in the middle of a held note.
pub(super) fn slice_score(score: &SingerScore, range: Range<usize>) -> SingerScore {
    let mut notes = Vec::new();
    let mut at = 0;
    for note in &score.notes {
        let end = at + note.frame_length as usize;
        if at >= range.end {
            break;
        }
        if end > range.start {
            let mut part = note.clone();
            part.frame_length = (end.min(range.end) - at.max(range.start)) as u32;
            if at < range.start
                && part.key.is_some()
                && let Some(sound) = auris_vocal::kana_phonemes(&part.lyric)
                    .and_then(|sounds| sounds.last().cloned())
                    .and_then(|sound| auris_vocal::kana::phonemes_to_kana(&[sound]))
            {
                part.lyric = sound;
            }
            notes.push(part);
        }
        at = end;
    }
    // A clipped one-frame fragment is only look-behind context; the next consonant
    // needs a rest here instead of an impossible one-frame vowel.
    if notes.len() > 1 && notes[0].frame_length == 1 && starts_with_consonant(&notes[1]) {
        notes[0].key = None;
        notes[0].lyric.clear();
    }
    SingerScore { notes }
}

pub(super) fn slice_frames(frames: &SingerFrames, range: Range<usize>) -> SingerFrames {
    SingerFrames {
        hop_seconds: frames.hop_seconds,
        inventory: frames.inventory.clone(),
        phonemes: frames.phonemes[range.clone()].to_vec(),
        f0_hz: frames.f0_hz[range.clone()].to_vec(),
        energy: frames.energy[range].to_vec(),
    }
}

/// Each preceding core leaves a short tail for the next core to fade into.
pub(super) fn stitch_audio(output: &mut [f32], incoming: &[f32], overlap: usize) {
    let overlap = overlap.min(output.len());
    for (index, (dest, sample)) in output.iter_mut().zip(incoming).enumerate() {
        if index < overlap {
            let mix = (index + 1) as f32 / (overlap + 1) as f32;
            *dest += (sample - *dest) * mix;
        } else {
            *dest = *sample;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_vocal::SingerNote;

    #[test]
    fn requests_include_context_and_padding_within_the_duration_ceiling() {
        for frame_rate in [10.0, 93.75, 100.0, 1000.0] {
            let score = SingerScore {
                notes: vec![SingerNote {
                    key: Some(60),
                    frame_length: 100_000,
                    lyric: "カ".into(),
                }],
            };
            let plan = ChunkPlan::new(&score, 100_000, frame_rate);
            let mut at = 0;
            for core in &plan.cores {
                assert_eq!(core.start, at);
                assert!(
                    plan.window(core, 100_000).len() + 2 * plan.context
                        <= (frame_rate * REQUEST_SECONDS).floor() as usize
                );
                at = core.end;
            }
            assert_eq!(at, 100_000);
        }
    }

    #[test]
    fn a_long_absolute_lead_in_stays_in_bounded_local_windows() {
        let frame_rate = 93.75;
        let leading = (200.0 * frame_rate) as usize;
        let sung = 1_000;
        let score = SingerScore {
            notes: vec![
                SingerNote {
                    key: None,
                    frame_length: leading as u32,
                    lyric: String::new(),
                },
                SingerNote {
                    key: Some(60),
                    frame_length: sung as u32,
                    lyric: "ア".into(),
                },
            ],
        };
        let count = leading + sung;
        let plan = ChunkPlan::new(&score, count, frame_rate);
        let limit = (frame_rate * REQUEST_SECONDS).floor() as usize;
        let context = plan.context;
        let mut cursor = 0;
        let mut sung_core_frames = 0;
        for core in &plan.cores {
            assert_eq!(
                core.start, cursor,
                "cores must preserve absolute frame order"
            );
            assert!(core.end > core.start);
            let window = plan.window(core, count);
            assert!(
                window.len() + 2 * context <= limit,
                "each local Engine request must stay within the seconds budget"
            );
            let local = slice_score(&score, window.clone());
            let mut local_at = window.start;
            for note in local.notes {
                let end = local_at + note.frame_length as usize;
                if note.key.is_some() {
                    assert!(local_at >= leading && end <= count);
                    sung_core_frames += end.min(core.end).saturating_sub(local_at.max(core.start));
                }
                local_at = end;
            }
            assert_eq!(local_at, window.end);
            cursor = core.end;
        }
        assert_eq!(cursor, count);
        assert_eq!(
            sung_core_frames, sung,
            "all sung frames retain their absolute placement"
        );
    }

    #[test]
    fn cuts_prefer_a_rest_then_note_boundaries_over_a_sustained_sound() {
        let score = SingerScore {
            notes: vec![
                SingerNote {
                    key: Some(60),
                    frame_length: 900,
                    lyric: "カ".into(),
                },
                SingerNote {
                    key: None,
                    frame_length: 100,
                    lyric: String::new(),
                },
                SingerNote {
                    key: Some(60),
                    frame_length: 500,
                    lyric: "ア".into(),
                },
                SingerNote {
                    key: Some(60),
                    frame_length: 3000,
                    lyric: "ア".into(),
                },
            ],
        };
        assert_eq!(ChunkPlan::new(&score, 4500, 93.75).cores[0].end, 950);
        let mut connected = score.clone();
        connected.notes[1].key = Some(60);
        connected.notes[1].lyric = "ア".into();
        assert_eq!(ChunkPlan::new(&connected, 4500, 93.75).cores[0].end, 1000);
    }

    #[test]
    fn clipping_a_sustained_note_does_not_repeat_its_consonant() {
        let score = SingerScore {
            notes: vec![SingerNote {
                key: Some(60),
                frame_length: 5000,
                lyric: "カ".into(),
            }],
        };
        assert_eq!(
            slice_score(&score, 1000..2000).notes,
            vec![SingerNote {
                key: Some(60),
                frame_length: 1000,
                lyric: "あ".into()
            }]
        );
    }

    #[test]
    fn normalized_words_and_prolonged_vowels_keep_their_full_frame_clock_when_sliced() {
        let score = SingerScore {
            notes: vec![
                SingerNote {
                    key: Some(60),
                    frame_length: 100,
                    lyric: "コーヒー".into(),
                },
                SingerNote {
                    key: Some(60),
                    frame_length: 100,
                    lyric: "ー".into(),
                },
            ],
        };
        let normalized = super::super::padded_score(&score, 94).unwrap();
        let sliced = slice_score(&normalized, 94 + 40..94 + 160);
        assert_eq!(
            sliced
                .notes
                .iter()
                .map(|note| note.frame_length)
                .sum::<u32>(),
            120
        );
        assert!(sliced.notes.iter().all(|note| !note.lyric.contains('ー')));
        assert_eq!(sliced.notes.last().unwrap().lyric, "イ");
        assert_eq!(score.notes[0].lyric, "コーヒー");
    }

    #[test]
    fn a_one_frame_look_behind_fragment_cannot_make_the_next_consonant_unsingable() {
        let score = SingerScore {
            notes: vec![
                SingerNote {
                    key: Some(60),
                    frame_length: 50,
                    lyric: "ア".into(),
                },
                SingerNote {
                    key: Some(60),
                    frame_length: 50,
                    lyric: "カ".into(),
                },
            ],
        };
        let sliced = slice_score(&score, 49..100);
        assert_eq!(
            sliced
                .notes
                .iter()
                .map(|note| note.frame_length)
                .sum::<u32>(),
            51
        );
        super::super::padded_score(&sliced, 94).unwrap();
    }

    #[test]
    fn phrases_split_only_at_rests_twice_the_margin_and_keep_the_margin() {
        let note = |key: Option<u8>, frame_length: u32| SingerNote {
            key,
            frame_length,
            lyric: if key.is_some() { "ア" } else { "" }.into(),
        };
        // Offset 10: the padded score's leading rest holds ten frames before the host clock.
        let score = SingerScore {
            notes: vec![
                note(None, 15),
                note(Some(60), 20),
                note(None, 19),
                note(Some(60), 20),
                note(None, 20),
                note(Some(60), 20),
                note(None, 10),
            ],
        };
        let count = 15 + 20 + 19 + 20 + 20 + 20 + 10 - 2 * 10;
        // Host frames: sung 5..25, 44..64 and 84..104; rests of 19 and 20 between them.
        assert_eq!(
            phrase_spans(&score, 10, count, 10),
            vec![0..74, 74..104],
            "a 19-frame rest joins two phrases; a 20-frame rest separates them"
        );
        assert!(
            phrase_spans(
                &SingerScore {
                    notes: vec![note(None, 30)]
                },
                10,
                10,
                5
            )
            .is_empty()
        );
    }

    #[test]
    fn overlap_blends_without_inserting_or_dropping_samples() {
        let mut samples = vec![0.0; 6];
        stitch_audio(&mut samples, &[1.0; 6], 3);
        assert_eq!(samples, [0.25, 0.5, 0.75, 1.0, 1.0, 1.0]);
    }
}
