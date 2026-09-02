//! From a track's frames to what one inference is shown.
//!
//! Two jobs, both pure so the tests can measure them. **Chunking**: a whole song is never sung
//! in one inference — the model's attention grows quadratically with the frame count, and a
//! three-minute piece asked for at once has taken a development machine down with it — so
//! [`chunk_ranges`] cuts the timeline into stretches of at most [`MAX_CHUNK_FRAMES`] frames,
//! cutting only where nothing is sung (or, for one unbroken phrase longer than the ceiling, at
//! its quietest frame) — and at every rest longer than [`MAX_REST_FRAMES`] whether or not the
//! ceiling asks, because a voice is trained on phrases and a rest of two seconds inside one
//! inference is something it has never seen; measured, it cost the phrase after the rest half
//! its words. **Arrangement**: [`arrange`] turns one chunk of frames into the score
//! the model reads — phonemes run-length-encoded into tokens and durations, the curves copied
//! through, and the voiced flag decided from the phoneme class, never from `f0 > 0`, which
//! would hum through every /k/ and /s/.

use std::ops::Range;

use auris_vocal::{SILENCE, SingerFrames, is_voiceless};

/// The model's own silence symbol — what the frames' [`SILENCE`] token maps to.
pub(crate) const MODEL_SILENCE: &str = "<sil>";

/// The model's stand-in for a phoneme its table never learned.
pub(crate) const MODEL_UNKNOWN: &str = "<unk>";

/// The most frames one inference is asked to sing — twenty seconds at the usual 10 ms hop.
///
/// The ceiling is memory, not patience: the model's attention buffers grow with the *square*
/// of the frame count, and 18 000 frames in one call exhausted a 32 GB machine, while 6 000
/// merely crawled. Two thousand keeps every inference comfortably small, and the chunks are
/// cut in silence, so the seams cost nothing audible.
pub const MAX_CHUNK_FRAMES: usize = 2_000;

/// The longest rest two phrases may share an inference across — half a second at the usual
/// 10 ms hop.
///
/// A voice is trained on phrases cut at their pauses: nothing it saw held a rest longer than
/// a breath inside one utterance, and a chunk that spans two seconds of silence between two
/// lines asks the model to sing its way through something it has no idea of. Measured on a
/// four-line verse, singing the whole in one inference against each line in its own put the
/// phoneme error rate of the lines after the rests at 0.47 and 0.67 against 0.10 and 0.23. So
/// a rest longer than this is a seam, cheap because it is cut in silence anyway; the pauses
/// within a line, a breath long, still share.
pub const MAX_REST_FRAMES: usize = 50;

/// Silent frames kept around each chunk, where the timeline has them to give.
///
/// A quarter second of lead-in: the model was trained on phrases that start from silence, and
/// a chunk beginning at the very first sung frame would ask it to begin mid-breath.
pub(crate) const CHUNK_PAD_FRAMES: usize = 25;

/// What full frame energy means to the model, in its linear-RMS terms.
///
/// Frame energy is a musical dynamic from 0 to 1 — velocity shaped by the envelope and the
/// expression pedal. The model reads linear RMS on its training scale, roughly 0 to 0.5 for
/// peak-normalised audio, with sung material living in the lower half of that; full velocity
/// lands at 0.25, a healthy forte. Calibrated by ear and by the rendered level being
/// commensurate with the built-in instruments; remeasure before moving it.
pub const ENERGY_FULL_SCALE: f32 = 0.25;

/// The stretches of the timeline worth singing, each at most `max` frames before padding.
///
/// Cuts fall in silence wherever silence exists — at every rest longer than
/// [`MAX_REST_FRAMES`], and between closer phrases only where `max` asks; an unbroken sung
/// span longer than `max` is split at the quietest frame in the back half of each window,
/// which is where a breath or a consonant is most likely sitting. Every range is then widened
/// by up to
/// [`CHUNK_PAD_FRAMES`] of *silent* neighbours on each side — never into another chunk, and
/// never over a sung frame. Ranges come back ascending and disjoint; frames outside every
/// range are silence and are not worth an inference.
pub(crate) fn chunk_ranges(frames: &SingerFrames, max: usize) -> Vec<Range<usize>> {
    let max = max.max(2);
    let sung = |at: usize| frames.phonemes.get(at).is_some_and(|id| *id != 0);

    // The maximal sung runs.
    let mut spans: Vec<Range<usize>> = Vec::new();
    let mut open: Option<usize> = None;
    for at in 0..frames.len() {
        match (sung(at), open) {
            (true, None) => open = Some(at),
            (false, Some(start)) => {
                spans.push(start..at);
                open = None;
            }
            _ => {}
        }
    }
    if let Some(start) = open {
        spans.push(start..frames.len());
    }

    // A span longer than the ceiling is cut at its quietest reachable frame.
    let mut pieces: Vec<Range<usize>> = Vec::new();
    for span in spans {
        let mut start = span.start;
        while span.end - start > max {
            let window = (start + max / 2)..(start + max);
            let cut = window
                .clone()
                .min_by(|a, b| frames.energy[*a].total_cmp(&frames.energy[*b]))
                .unwrap_or(window.end)
                .max(start + 1);
            pieces.push(start..cut);
            start = cut;
        }
        pieces.push(start..span.end);
    }

    // Neighbouring pieces share a chunk while they fit under the ceiling together and the
    // rest between them is no longer than a breath — the silence between two close phrases is
    // cheaper sung than seamed, and the silence between two lines is neither: it is a place
    // the voice has never been, and the seam costs nothing there.
    let mut groups: Vec<Range<usize>> = Vec::new();
    for piece in pieces {
        match groups.last_mut() {
            Some(last)
                if piece.end - last.start <= max && piece.start - last.end <= MAX_REST_FRAMES =>
            {
                last.end = piece.end
            }
            _ => groups.push(piece),
        }
    }

    // Silent padding, clamped to what actually separates the chunks.
    let mut out: Vec<Range<usize>> = Vec::with_capacity(groups.len());
    let mut floor = 0usize;
    for (at, group) in groups.iter().enumerate() {
        let mut start = group.start;
        while start > floor && group.start - start < CHUNK_PAD_FRAMES && !sung(start - 1) {
            start -= 1;
        }
        let ceiling = groups
            .get(at + 1)
            .map(|next| next.start)
            .unwrap_or(frames.len());
        let mut end = group.end;
        while end < ceiling && end - group.end < CHUNK_PAD_FRAMES && !sung(end) {
            end += 1;
        }
        floor = end;
        out.push(start..end);
    }
    out
}

/// One chunk of frames arranged as the model's inputs.
pub(crate) struct Score {
    /// Phoneme ids in the model's own table, one per run of equal frames.
    pub(crate) tokens: Vec<i64>,
    /// Frames per token; sums to the chunk's length.
    pub(crate) durations: Vec<i64>,
    /// Pitch per frame, Hz, 0 where nothing is sung.
    pub(crate) f0: Vec<f32>,
    /// Energy per frame, on the model's linear-RMS scale.
    pub(crate) energy: Vec<f32>,
    /// 1.0 on frames whose phoneme is voiced *and* whose f0 is nonzero.
    pub(crate) voiced: Vec<f32>,
}

/// Arranges `range` of the frames against the model's phoneme table.
///
/// [`SILENCE`] maps to [`MODEL_SILENCE`]; a token the table never learned maps to
/// [`MODEL_UNKNOWN`] rather than refusing — a strange symbol costs one strange syllable, and
/// the phoneme editor is the cure. The caller has validated that the table holds both
/// specials.
pub(crate) fn arrange(frames: &SingerFrames, range: Range<usize>, symbols: &[String]) -> Score {
    let position = |token: &str| symbols.iter().position(|symbol| symbol == token);
    let sil = position(MODEL_SILENCE).expect("checked when the model was loaded") as i64;
    let unk = position(MODEL_UNKNOWN).expect("checked when the model was loaded") as i64;

    // Per inventory entry: its model id, and whether it is voiceless — asked once, not per frame.
    let ids: Vec<i64> = frames
        .inventory
        .iter()
        .map(|token| match token.as_str() {
            SILENCE => sil,
            other => position(other).map(|at| at as i64).unwrap_or(unk),
        })
        .collect();
    let voiceless: Vec<bool> = frames
        .inventory
        .iter()
        .map(|token| is_voiceless(token))
        .collect();

    let mut score = Score {
        tokens: Vec::new(),
        durations: Vec::new(),
        f0: Vec::with_capacity(range.len()),
        energy: Vec::with_capacity(range.len()),
        voiced: Vec::with_capacity(range.len()),
    };
    for at in range {
        // A file edited by hand can hold an index past its own inventory; sing it as unknown
        // rather than panicking over it.
        let entry = frames.phonemes[at] as usize;
        let id = ids.get(entry).copied().unwrap_or(unk);
        match score.tokens.last() {
            Some(last) if *last == id => *score.durations.last_mut().expect("paired") += 1,
            _ => {
                score.tokens.push(id);
                score.durations.push(1);
            }
        }
        let f0 = frames.f0_hz[at];
        score.f0.push(f0);
        score.energy.push(frames.energy[at] * ENERGY_FULL_SCALE);
        let sounds = f0 > 0.0 && !voiceless.get(entry).copied().unwrap_or(false);
        score.voiced.push(if sounds { 1.0 } else { 0.0 });
    }
    score
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames with silence everywhere except the given spans, each `(start, end, token, f0)`.
    fn frames(len: usize, spans: &[(usize, usize, &str, f32)]) -> SingerFrames {
        let mut inventory = vec![SILENCE.to_string()];
        let mut phonemes = vec![0u32; len];
        let mut f0_hz = vec![0.0f32; len];
        let mut energy = vec![0.0f32; len];
        for (start, end, token, f0) in spans {
            let id = match inventory.iter().position(|entry| entry == token) {
                Some(id) => id,
                None => {
                    inventory.push(token.to_string());
                    inventory.len() - 1
                }
            };
            for at in *start..*end {
                phonemes[at] = id as u32;
                f0_hz[at] = *f0;
                energy[at] = 0.8;
            }
        }
        SingerFrames {
            hop_seconds: 0.010,
            inventory,
            phonemes,
            f0_hz,
            energy,
        }
    }

    const TABLE: [&str; 7] = ["<pad>", "<unk>", "<sil>", "<pau>", "a", "k", "ɴ"];

    fn table() -> Vec<String> {
        TABLE.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn silence_asks_for_no_inference_at_all() {
        assert!(chunk_ranges(&frames(500, &[]), 100).is_empty());
    }

    #[test]
    fn one_phrase_is_one_chunk_wearing_its_silent_padding() {
        let ranges = chunk_ranges(&frames(500, &[(100, 200, "a", 440.0)]), 1000);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0], 100 - CHUNK_PAD_FRAMES..200 + CHUNK_PAD_FRAMES);
    }

    #[test]
    fn padding_stops_at_the_edge_of_the_timeline() {
        let ranges = chunk_ranges(&frames(110, &[(5, 108, "a", 440.0)]), 1000);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0], 0..110);
    }

    #[test]
    fn far_apart_phrases_are_separate_chunks_and_close_ones_share() {
        // Two phrases 600 silent frames apart cannot share a 500-frame chunk...
        let far = frames(1000, &[(0, 100, "a", 440.0), (700, 800, "a", 440.0)]);
        assert_eq!(chunk_ranges(&far, 500).len(), 2);
        // ...but two phrases a breath apart can.
        let near = frames(1000, &[(0, 100, "a", 440.0), (150, 250, "a", 440.0)]);
        assert_eq!(chunk_ranges(&near, 500).len(), 1);
    }

    #[test]
    fn a_rest_longer_than_a_breath_is_a_seam_even_under_the_ceiling() {
        // Two lines two seconds apart fit one chunk with room to spare, and are still sung
        // apart: the voice has never sung across such a rest. Exactly a breath apart, they
        // share.
        let lines = frames(1000, &[(0, 200, "a", 440.0), (400, 600, "a", 440.0)]);
        let ranges = chunk_ranges(&lines, MAX_CHUNK_FRAMES);
        assert_eq!(ranges.len(), 2, "{ranges:?}");
        assert!(
            ranges[0].end <= 200 + CHUNK_PAD_FRAMES && ranges[1].start >= 400 - CHUNK_PAD_FRAMES
        );
        let breath = frames(
            1000,
            &[
                (0, 200, "a", 440.0),
                (200 + MAX_REST_FRAMES, 500, "a", 440.0),
            ],
        );
        assert_eq!(chunk_ranges(&breath, MAX_CHUNK_FRAMES).len(), 1);
    }

    #[test]
    fn chunks_are_ascending_disjoint_and_bounded() {
        let long = frames(9000, &[(10, 8990, "a", 440.0)]);
        let ranges = chunk_ranges(&long, MAX_CHUNK_FRAMES);
        assert!(ranges.len() >= 5, "one long phrase must still be cut up");
        let mut floor = 0;
        for range in &ranges {
            assert!(range.start >= floor, "{range:?} overlaps its neighbour");
            assert!(
                range.end - range.start <= MAX_CHUNK_FRAMES + 2 * CHUNK_PAD_FRAMES,
                "{range:?} is over the ceiling"
            );
            floor = range.end;
        }
        // Nothing sung falls between chunks: the pieces of the one span reassemble exactly.
        let sung: usize = ranges
            .iter()
            .map(|r| r.end.min(8990) - r.start.max(10))
            .sum();
        assert_eq!(sung, 8980);
    }

    #[test]
    fn an_overlong_phrase_is_cut_at_its_quietest_frame() {
        let mut long = frames(1000, &[(0, 1000, "a", 440.0)]);
        // One frame in the back half of the first window dips: the cut must land on it.
        long.energy[70] = 0.05;
        let ranges = chunk_ranges(&long, 100);
        assert_eq!(ranges[0].end, 70);
        assert_eq!(ranges[1].start, 70);
    }

    #[test]
    fn frames_are_arranged_into_the_models_own_words() {
        // sil sil k k a a a — the RLE, the mapping and the curves in one small score.
        let sung = frames(7, &[(2, 4, "k", 440.0), (4, 7, "a", 440.0)]);
        let score = arrange(&sung, 0..7, &table());
        assert_eq!(score.tokens, [2, 5, 4], "<sil> k a");
        assert_eq!(score.durations, [2, 2, 3]);
        assert_eq!(score.durations.iter().sum::<i64>(), 7);
        assert_eq!(score.f0[0], 0.0);
        assert_eq!(score.f0[3], 440.0);
        // k carries the vowel's pitch but sings unvoiced; the vowel is voiced; silence is not.
        assert_eq!(&score.voiced[..], [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
        // Energy arrives on the model's scale.
        assert!((score.energy[5] - 0.8 * ENERGY_FULL_SCALE).abs() < 1e-6);
        assert_eq!(score.energy[0], 0.0);
    }

    #[test]
    fn a_token_the_table_never_learned_sings_as_unknown() {
        let sung = frames(4, &[(0, 4, "ʈʂ", 220.0)]);
        let score = arrange(&sung, 0..4, &table());
        assert_eq!(score.tokens, [1], "<unk>");
        assert_eq!(score.durations, [4]);
        // Unknown errs voiced, keeping the contour.
        assert_eq!(score.voiced, [1.0; 4]);
    }

    #[test]
    fn a_chunk_range_reads_only_its_own_frames() {
        let sung = frames(20, &[(5, 10, "a", 330.0), (12, 18, "ɴ", 330.0)]);
        let score = arrange(&sung, 12..18, &table());
        assert_eq!(score.tokens, [6], "just the ɴ");
        assert_eq!(score.f0.len(), 6);
    }
}
