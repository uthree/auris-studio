//! Notes to frames: the three sequences a voice model is fed, on its own clock.
//!
//! [`render_frames`] reads a whole singer track — every unmuted clip, repeats included, bends
//! and expression and all — and samples it every [`frame_hop`](SingerTrack::frame_hop) seconds
//! into a phoneme id, a pitch in Hz and an energy from 0 to 1 per frame. The rules are few and
//! deliberately plain, and each is asserted on by number in the tests below:
//!
//! * **A singer sings one note at a time.** Where notes overlap, the later-starting note cuts
//!   the earlier one off at its own start — the legato a keyboardist means by overlapping two
//!   notes slightly.
//! * **Consonants are short; syllabics stretch.** Each consonant before the first syllabic
//!   phoneme takes [`CONSONANT_SECONDS`] at the note's start, each one after the last takes the
//!   same at its end, and everything between shares the remainder equally. Consonants scale
//!   down rather than swallow a short note, never past half of it.
//! * **Pitch is the note plus its bend, everywhere in the note.** No portamento is invented
//!   between notes — the bend curve is where a slide is written — and consonant frames carry
//!   the same pitch as the vowel they lead into, because a model treats f0 as a contour and
//!   decides voicing from the phoneme, not from a zero.
//! * **Energy is the velocity, shaped.** A linear rise over [`ATTACK_SECONDS`], a linear fall
//!   over the last [`RELEASE_SECONDS`], scaled by the expression pedal (controller 11) where
//!   one is written. Outside every note the energy is zero, the pitch is zero, and the phoneme
//!   is [`SILENCE`].
//! * **A note with no phonemes sings `a`.** Losing the melody line over a missing word would
//!   make every export of a half-written song useless; an open vowel keeps the contour intact
//!   and is obviously a placeholder to the ear.

use serde::{Deserialize, Serialize};

use auris_core::plugin::{CC_EXPRESSION, pitch_to_hz};
use auris_core::project::{ClipCurve, CurvePoint, curve_at};
use auris_core::time::{Seconds, TempoMap, Ticks};
use auris_core::{SingerTrack, default_frame_hop, loop_passes};

use crate::phoneme::{SILENCE, is_syllabic};

/// Seconds a consonant is given before its vowel.
///
/// Sixty milliseconds sits inside the range measured for Japanese obstruents in running speech
/// and is short enough that a sixteenth note at 120 BPM (125 ms) keeps most of itself for the
/// vowel. A voice model learns real durations; this only has to hand it a sane target.
pub const CONSONANT_SECONDS: f64 = 0.060;

/// Seconds the energy takes to rise from silence at a note's start.
pub const ATTACK_SECONDS: f64 = 0.015;

/// Seconds the energy takes to fall back to silence before a note's end.
pub const RELEASE_SECONDS: f64 = 0.040;

/// A singer track sampled onto the model's clock.
///
/// One entry per frame in each of the three sequences, which stay the same length by
/// construction. Phonemes are indices into `inventory` rather than strings so the file a model
/// reads carries each symbol once; `inventory[0]` is always [`SILENCE`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SingerFrames {
    /// Seconds per frame — the hop the track was sampled at.
    pub hop_seconds: f64,
    /// Every phoneme the frames use, [`SILENCE`] first, then in order of first appearance.
    pub inventory: Vec<String>,
    /// Per frame: an index into [`Self::inventory`].
    pub phonemes: Vec<u32>,
    /// Per frame: pitch in Hz, and 0.0 where nothing is sung.
    pub f0_hz: Vec<f32>,
    /// Per frame: energy from 0 to 1 — velocity shaped by the envelope and the expression pedal.
    pub energy: Vec<f32>,
}

impl SingerFrames {
    /// How many frames there are.
    pub fn len(&self) -> usize {
        self.phonemes.len()
    }

    /// `true` when the track had nothing to sing.
    pub fn is_empty(&self) -> bool {
        self.phonemes.is_empty()
    }
}

/// One note flattened onto the timeline, with the curves that shape it.
struct TimedNote<'a> {
    /// Seconds where the note begins.
    start: f64,
    /// Seconds where it ends — possibly cut short by the next note.
    end: f64,
    /// MIDI pitch.
    pitch: f32,
    /// Attack strength, 0 to 1.
    velocity: f32,
    /// The phonemes sung, `a` where none were written.
    phonemes: Vec<String>,
    /// Timeline tick the note's clip pass begins at — what the curves are measured from.
    curve_base: Ticks,
    /// The clip's bend, in semitones.
    bend: &'a [CurvePoint],
    /// The clip's expression pedal, 0 to 1, empty meaning "all the way up".
    expression: &'a [CurvePoint],
}

/// Samples a singer track into the frames its voice model is fed.
pub fn render_frames(track: &SingerTrack, tempo_map: &TempoMap) -> SingerFrames {
    let hop = if track.frame_hop.is_finite() && track.frame_hop > 0.0 {
        track.frame_hop
    } else {
        default_frame_hop()
    };

    let notes = timed_notes(track, tempo_map);
    let mut inventory: Vec<String> = vec![SILENCE.to_string()];
    let mut phonemes = Vec::new();
    let mut f0_hz = Vec::new();
    let mut energy = Vec::new();

    let Some(end) = notes.last().map(|note| note.end) else {
        return SingerFrames {
            hop_seconds: hop,
            inventory,
            phonemes,
            f0_hz,
            energy,
        };
    };

    // One frame per hop across the material, plus one trailing frame of silence so the
    // sequence a model reads ends closed rather than mid-note.
    let count = (end / hop).ceil() as usize + 1;
    let mut walker = 0usize;
    for frame in 0..count {
        let t = frame as f64 * hop;
        // Notes are sorted and non-overlapping, so the active one only ever moves forward.
        while walker < notes.len() && notes[walker].end <= t {
            walker += 1;
        }
        let note = notes.get(walker).filter(|note| note.start <= t);
        let Some(note) = note else {
            phonemes.push(0);
            f0_hz.push(0.0);
            energy.push(0.0);
            continue;
        };

        let token = phoneme_at(note, t);
        let id = match inventory.iter().position(|entry| entry == token) {
            Some(id) => id,
            None => {
                inventory.push(token.to_string());
                inventory.len() - 1
            }
        };
        phonemes.push(id as u32);

        let tick = tempo_map.seconds_to_ticks(Seconds(t));
        let bend = curve_at(note.bend, tick - note.curve_base);
        f0_hz.push(pitch_to_hz(note.pitch + bend));

        let expression = match note.expression.is_empty() {
            true => 1.0,
            false => curve_at(note.expression, tick - note.curve_base).clamp(0.0, 1.0),
        };
        energy.push(note.velocity * expression * envelope(note, t));
    }

    SingerFrames {
        hop_seconds: hop,
        inventory,
        phonemes,
        f0_hz,
        energy,
    }
}

/// Every note of every unmuted clip, repeats included, flattened, sorted and made monophonic.
fn timed_notes<'a>(track: &'a SingerTrack, tempo_map: &TempoMap) -> Vec<TimedNote<'a>> {
    let mut placed: Vec<(Ticks, Ticks, TimedNote<'a>)> = Vec::new();
    for clip in &track.clips {
        if clip.muted {
            continue;
        }
        let expression = clip.curve(ClipCurve::Controller(CC_EXPRESSION));
        for (offset, span) in loop_passes(clip.length, clip.loop_end) {
            let base = clip.start + offset;
            for note in clip.playable_notes() {
                if note.start >= span {
                    continue;
                }
                let length = note.length.min(span - note.start);
                let start = base + note.start;
                let phonemes = match note.phonemes.is_empty() {
                    true => vec!["a".to_string()],
                    false => note.phonemes.clone(),
                };
                placed.push((
                    start,
                    start + length,
                    TimedNote {
                        start: 0.0,
                        end: 0.0,
                        pitch: f32::from(note.pitch),
                        velocity: note.velocity.clamp(0.0, 1.0),
                        phonemes,
                        curve_base: base,
                        bend: &clip.bend,
                        expression,
                    },
                ));
            }
        }
    }

    placed.sort_by_key(|(start, end, _)| (start.raw(), end.raw()));
    let ends: Vec<Ticks> = placed
        .iter()
        .enumerate()
        .map(|(at, (_, end, _))| match placed.get(at + 1) {
            // The later note cuts the earlier one off at its own start: one voice, one note.
            Some((next_start, _, _)) => (*end).min(*next_start),
            None => *end,
        })
        .collect();

    placed
        .into_iter()
        .zip(ends)
        .filter(|((start, _, _), end)| *end > *start)
        .map(|((start, _, mut note), end)| {
            note.start = tempo_map.ticks_to_seconds(start).0;
            note.end = tempo_map.ticks_to_seconds(end).0;
            note
        })
        .collect()
}

/// Which phoneme is sounding `t` seconds into the timeline, for a note known to contain `t`.
fn phoneme_at<'a>(note: &'a TimedNote<'a>, t: f64) -> &'a str {
    let segments = segment(note);
    let into = t - note.start;
    segments
        .iter()
        .find(|(from, to, _)| into >= *from && into < *to)
        .or(segments.last())
        .map(|(_, _, token)| token.as_str())
        .unwrap_or(SILENCE)
}

/// The note's phonemes laid across its length: `(from, to, token)` in seconds from its start.
///
/// Fixed-width consonants at the edges, the middle shared equally — the rule the module doc
/// states, in one place, where the tests can measure it.
fn segment<'a>(note: &'a TimedNote<'a>) -> Vec<(f64, f64, &'a String)> {
    let length = (note.end - note.start).max(0.0);
    let phonemes = &note.phonemes;
    let first_syllabic = phonemes.iter().position(|p| is_syllabic(p));
    let last_syllabic = phonemes.iter().rposition(|p| is_syllabic(p));

    let (prefix, middle, suffix) = match (first_syllabic, last_syllabic) {
        (Some(first), Some(last)) => (first, last + 1 - first, phonemes.len() - last - 1),
        // No syllabic at all: share the whole note equally rather than inventing an edge.
        _ => (0, phonemes.len(), 0),
    };

    // Fixed consonant slots, scaled down together so they never claim more than half the note.
    let fixed = (prefix + suffix) as f64 * CONSONANT_SECONDS;
    let scale = match fixed > length / 2.0 {
        true => (length / 2.0) / fixed.max(f64::EPSILON),
        false => 1.0,
    };
    let consonant = CONSONANT_SECONDS * scale;
    let shared = (length - (prefix + suffix) as f64 * consonant).max(0.0) / (middle.max(1)) as f64;

    let mut out = Vec::with_capacity(phonemes.len());
    let mut at = 0.0;
    for (index, token) in phonemes.iter().enumerate() {
        let width = match index < prefix || index >= prefix + middle {
            true => consonant,
            false => shared,
        };
        out.push((at, at + width, token));
        at += width;
    }
    out
}

/// The energy envelope at `t` seconds, for a note known to contain it.
fn envelope(note: &TimedNote<'_>, t: f64) -> f32 {
    let length = note.end - note.start;
    let attack = ATTACK_SECONDS.min(length / 2.0).max(f64::EPSILON);
    let release = RELEASE_SECONDS.min(length / 2.0).max(f64::EPSILON);
    let rise = ((t - note.start) / attack).clamp(0.0, 1.0);
    let fall = ((note.end - t) / release).clamp(0.0, 1.0);
    rise.min(fall) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::project::{ClipId, MidiClip, Note};
    use auris_core::{PluginState, Ticks};

    /// A singer track holding one clip with the given notes, at the default hop.
    fn track(notes: Vec<Note>) -> SingerTrack {
        let mut clip = MidiClip::new(ClipId(1), "Verse", Ticks::ZERO, Ticks::from_beats(8.0));
        clip.notes = notes;
        SingerTrack {
            instrument_id: "auris.synth.voice".into(),
            instrument_state: PluginState::empty(),
            clips: vec![clip],
            frame_hop: default_frame_hop(),
        }
    }

    /// A note singing `phonemes` at `pitch`, from `start` beats for `beats` beats.
    fn sung(pitch: u8, start: f64, beats: f64, phonemes: &[&str]) -> Note {
        let mut note = Note::new(pitch, Ticks::from_beats(start), Ticks::from_beats(beats));
        note.phonemes = phonemes.iter().map(|s| s.to_string()).collect();
        note
    }

    /// 120 BPM: one beat is half a second, so beat arithmetic in tests stays mental.
    fn map() -> TempoMap {
        TempoMap::constant(120.0)
    }

    #[test]
    fn an_empty_track_is_an_empty_answer() {
        let frames = render_frames(&track(Vec::new()), &map());
        assert!(frames.is_empty());
        assert_eq!(frames.inventory, [SILENCE]);
        assert_eq!(frames.hop_seconds, default_frame_hop());
    }

    #[test]
    fn a_held_note_reads_as_its_pitch_its_vowel_and_its_velocity() {
        // A4 for two beats — one second — starting half a second in.
        let frames = render_frames(&track(vec![sung(69, 1.0, 2.0, &["a"])]), &map());

        // 1.5 seconds of material at 10 ms plus the closing frame.
        assert_eq!(frames.len(), 151);
        assert_eq!(frames.inventory, [SILENCE, "a"]);

        // Before the note: silence, no pitch, no energy.
        assert_eq!(frames.phonemes[0], 0);
        assert_eq!(frames.f0_hz[0], 0.0);
        assert_eq!(frames.energy[0], 0.0);

        // In the middle of the note: the vowel at 440 Hz at the default velocity.
        assert_eq!(frames.phonemes[100], 1);
        assert!((frames.f0_hz[100] - 440.0).abs() < 1e-3);
        assert!((frames.energy[100] - 0.8).abs() < 1e-6);

        // The very first frame of the note sits at the foot of the attack ramp.
        assert_eq!(frames.phonemes[50], 1);
        assert_eq!(frames.energy[50], 0.0);
        assert!(frames.energy[51] > 0.0, "the rise is under way by 10 ms");

        // And the last frame is the closing silence.
        assert_eq!(frames.phonemes[150], 0);
    }

    #[test]
    fn a_consonant_takes_its_sixty_milliseconds_and_the_vowel_takes_the_rest() {
        // か — [k a] — for one beat from the start.
        let frames = render_frames(&track(vec![sung(60, 0.0, 1.0, &["k", "a"])]), &map());
        assert_eq!(frames.inventory, [SILENCE, "k", "a"]);
        // 60 ms at a 10 ms hop: frames 0..=5 are the k, frame 6 is the vowel.
        for frame in 0..6 {
            assert_eq!(frames.phonemes[frame], 1, "frame {frame}");
        }
        assert_eq!(frames.phonemes[6], 2);
        // The consonant carries the note's pitch: f0 is a contour, not a voicing flag.
        assert!((frames.f0_hz[0] - pitch_to_hz(60.0)).abs() < 1e-3);
    }

    #[test]
    fn consonants_scale_down_rather_than_swallow_a_short_note() {
        // A 100 ms note singing [k a]: a full 60 ms k would leave the vowel 40 ms; the rule
        // caps the consonant at half the note.
        let frames = render_frames(&track(vec![sung(60, 0.0, 0.2, &["k", "a"])]), &map());
        let k_frames = frames.phonemes.iter().filter(|id| **id == 1).count();
        let a_frames = frames.phonemes.iter().filter(|id| **id == 2).count();
        assert_eq!(k_frames, 5, "50 ms of k — half the note, not sixty");
        assert_eq!(a_frames, 5, "and the vowel keeps its half");
    }

    #[test]
    fn the_bend_curve_moves_the_pitch() {
        let mut singer = track(vec![sung(69, 0.0, 2.0, &["a"])]);
        // Two semitones up, flat across the note.
        singer.clips[0].bend = vec![
            CurvePoint {
                at: Ticks::ZERO,
                value: 2.0,
            },
            CurvePoint {
                at: Ticks::from_beats(2.0),
                value: 2.0,
            },
        ];
        let frames = render_frames(&singer, &map());
        let expected = pitch_to_hz(71.0);
        assert!(
            (frames.f0_hz[50] - expected).abs() < 1e-2,
            "{} should be {expected}",
            frames.f0_hz[50]
        );
    }

    #[test]
    fn overlapping_notes_sing_one_at_a_time() {
        // The second note starts a beat into the first; the first is cut there.
        let frames = render_frames(
            &track(vec![sung(60, 0.0, 2.0, &["a"]), sung(64, 1.0, 1.0, &["i"])]),
            &map(),
        );
        // Frame 45: 0.45 s, still the first note. Frame 55: 0.55 s, the second.
        assert!((frames.f0_hz[45] - pitch_to_hz(60.0)).abs() < 1e-3);
        assert!((frames.f0_hz[55] - pitch_to_hz(64.0)).abs() < 1e-3);
        assert_eq!(frames.inventory, [SILENCE, "a", "i"]);
    }

    #[test]
    fn a_note_with_no_phonemes_sings_an_open_vowel() {
        let frames = render_frames(&track(vec![sung(60, 0.0, 1.0, &[])]), &map());
        assert_eq!(frames.inventory, [SILENCE, "a"]);
        assert_eq!(frames.phonemes[25], 1);
    }

    #[test]
    fn the_expression_pedal_scales_the_energy() {
        let mut singer = track(vec![sung(60, 0.0, 2.0, &["a"])]);
        singer.clips[0].controllers.insert(
            CC_EXPRESSION,
            vec![
                CurvePoint {
                    at: Ticks::ZERO,
                    value: 0.5,
                },
                CurvePoint {
                    at: Ticks::from_beats(2.0),
                    value: 0.5,
                },
            ],
        );
        let frames = render_frames(&singer, &map());
        assert!((frames.energy[50] - 0.8 * 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_looped_clip_sings_its_repeats() {
        let mut singer = track(vec![sung(60, 0.0, 1.0, &["a"])]);
        singer.clips[0].length = Ticks::from_beats(1.0);
        singer.clips[0].length_is_explicit = true;
        singer.clips[0].loop_end = Ticks::from_beats(2.0);
        let frames = render_frames(&singer, &map());
        // The second pass sings the same note over 0.5..1.0 s.
        assert!((frames.f0_hz[75] - pitch_to_hz(60.0)).abs() < 1e-3);
        // 1.0 s of material and the closing frame.
        assert_eq!(frames.len(), 101);
    }

    #[test]
    fn frames_survive_a_round_trip_through_json() {
        let frames = render_frames(&track(vec![sung(69, 0.0, 1.0, &["ɾ", "a"])]), &map());
        let text = serde_json::to_string(&frames).unwrap();
        let back: SingerFrames = serde_json::from_str(&text).unwrap();
        assert_eq!(back, frames);
    }
}
