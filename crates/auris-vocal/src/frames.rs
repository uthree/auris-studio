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
//!   phoneme takes its width at the note's start, each one after the last takes its width at
//!   the end, and everything between shares the remainder equally. The width is the voice
//!   model's own measurement where the track's voice carries a table
//!   ([`ConsonantWidths`]) and [`CONSONANT_SECONDS`] where it
//!   does not. Consonants scale down rather than swallow a short note, never past half of it.
//! * **Pitch is the note plus its bend plus its ornaments, everywhere in the note.** The bend
//!   curve is where a slide is written, and a scoop, fall or vibrato sounds only where the
//!   note carries one ([`ornament_offset`] is the shape). Consonant frames carry the same
//!   pitch as the vowel they lead into, because a model treats f0 as a contour and decides
//!   voicing from the phoneme, not from a zero.
//! * **Where two notes touch, the pitch travels between them.** A singer's pitch takes time
//!   to move, and a voice model trained on singers has never seen it jump: a straight line of
//!   [`GLIDE_SECONDS`] centred on the boundary joins the two pitches, its halves capped at a
//!   quarter of each note. It is the measured shape of a note change, not a portamento — a
//!   fall on the earlier note or a scoop on the later one is a slide somebody wrote, and it
//!   replaces the glide; a rest between the notes leaves each on its own pitch.
//! * **Energy is the velocity, shaped.** A linear rise over [`ATTACK_SECONDS`], a linear fall
//!   over the last [`RELEASE_SECONDS`], scaled by the expression pedal (controller 11) where
//!   one is written — and, where the track's voice carries a table
//!   ([`ConsonantLevels`]), scaled down on each consonant by how far under its vowel the
//!   voice's training data sang it: twenty-odd decibels for a voiceless plosive, three for
//!   an approximant. A consonant at the vowel's level is one the model has never heard, and
//!   the plateau alone was measured to cost half the words. The last
//!   [`CONSONANT_RELEASE_SECONDS`] of a consonant are the exception and come back up to the
//!   vowel's level: that is the release — a plosive's burst, a fricative's run into the
//!   vowel — and a /k/ held at its closure's level to the end is a /k/ that never opens.
//!   Outside every note the energy is zero, the pitch is zero, and the phoneme is
//!   [`SILENCE`].
//! * **A note with no phonemes sings `a`.** Losing the melody line over a missing word would
//!   make every export of a half-written song useless; an open vowel keeps the contour intact
//!   and is obviously a placeholder to the ear.

use serde::{Deserialize, Serialize};

use auris_core::plugin::{CC_EXPRESSION, pitch_to_hz};
use auris_core::project::{ClipCurve, CurvePoint, curve_at};
use auris_core::time::{Seconds, TempoMap, Ticks};
use auris_core::{
    ConsonantLevels, ConsonantWidths, Fall, Scoop, SingerTrack, Vibrato, default_frame_hop,
    loop_passes,
};

use crate::ornament::ornament_offset;
use crate::phoneme::{SILENCE, is_syllabic};

/// Seconds a consonant is given before its vowel, when its voice measured nothing better.
///
/// Sixty milliseconds sits inside the range measured for Japanese obstruents in running speech
/// and is short enough that a sixteenth note at 120 BPM (125 ms) keeps most of itself for the
/// vowel. Consonant length actually spans a factor of three by phoneme class, which is why a
/// voice model's export can carry its own per-phoneme table
/// ([`ConsonantWidths`]) and the table wins wherever the track's
/// voice has one; this number is the fallback for the models — and the voiceless tracks — that
/// do not.
pub const CONSONANT_SECONDS: f64 = 0.060;

/// Seconds the pitch takes to travel from one note to the next where they touch.
///
/// The JSUT-song corpus was measured (`training/runs/exp/glide_shape.py`): across 1,568 note
/// changes of a semitone or more, the pitch spends a median 60 ms between a tenth and nine
/// tenths of the way, and the travel straddles the boundary — it begins some 20 ms before the
/// next note's first phoneme and ends 50 ms after it, inside the consonant where there is one.
/// Eighty milliseconds of straight line centred on the boundary puts those two marks at −32
/// and +32 ms, inside the corpus's quartiles either side. The straight line is a choice of
/// plainness: the corpus rises slower than it falls, and neither shape is a curve the ear
/// picks out at this length.
pub const GLIDE_SECONDS: f64 = 0.080;

/// Seconds at the end of a consonant that are sung at the vowel's level: its release.
///
/// Measured on JSUT-song from the labels (`training/runs/exp/plosive_shape.py`), a voiceless
/// plosive sits 25 dB under its vowel for its closure and 8 dB under it over its last 20 ms,
/// and every consonant class rises the same way into the vowel. A level table carries the
/// closure's number, and held to the last frame it is a stop that never bursts: sung through
/// the host, the composed verse's /k/ was heard right 25 times in 40 with the flat level and
/// 35 with these two frames at the vowel's level, /t/ 22 in 30 against 28, and the phoneme
/// error rate went 0.23 → 0.14 over ten takes. Twenty milliseconds, capped at half the
/// consonant, is the measured window; making it thirty changed nothing.
pub const CONSONANT_RELEASE_SECONDS: f64 = 0.020;

/// Seconds the energy takes to rise from silence at a note's start.
pub const ATTACK_SECONDS: f64 = 0.015;

/// Seconds the energy takes to fall back to silence before a note's end.
pub const RELEASE_SECONDS: f64 = 0.040;

/// Smallest supported feature-frame hop, in seconds.
///
/// A smaller value approaches one frame per audio sample and can turn merely inspecting a
/// malformed project into an unbounded allocation.
const MIN_FRAME_HOP: f64 = 0.001;

/// Largest supported feature-frame hop, in seconds.
const MAX_FRAME_HOP: f64 = 0.100;

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
    /// Seconds each phoneme is pinned to, 0 meaning the rule decides. Empty when the note
    /// carries no pins — or no phonemes, since a pin on the placeholder vowel would be a
    /// pin on nothing the person ever saw.
    phoneme_seconds: Vec<f64>,
    /// The scoop into the note, where one is written.
    scoop: Option<Scoop>,
    /// The fall off its end, where one is written.
    fall: Option<Fall>,
    /// The sway across it, where one is written.
    vibrato: Option<Vibrato>,
    /// Timeline tick the note's clip pass begins at — what the curves are measured from.
    curve_base: Ticks,
    /// The clip's bend, in semitones.
    bend: &'a [CurvePoint],
    /// The clip's expression pedal, 0 to 1, empty meaning "all the way up".
    expression: &'a [CurvePoint],
    /// The track's voice's consonant widths, where its export carried a table.
    widths: Option<&'a ConsonantWidths>,
    /// The track's voice's consonant levels, where its export carried a table.
    levels: Option<&'a ConsonantLevels>,
}

/// Samples a singer track into the frames its voice model is fed.
pub fn render_frames(track: &SingerTrack, tempo_map: &TempoMap) -> SingerFrames {
    let hop = match track.frame_hop.is_finite() {
        true => track.frame_hop.clamp(MIN_FRAME_HOP, MAX_FRAME_HOP),
        false => default_frame_hop(),
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

        let (token, release) = phoneme_at(note, t);
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
        let ornament = ornament_offset(
            note.scoop.as_ref(),
            note.fall.as_ref(),
            note.vibrato.as_ref(),
            t - note.start,
            note.end - note.start,
        );
        let glide = glide_offset(
            walker.checked_sub(1).and_then(|at| notes.get(at)),
            note,
            notes.get(walker + 1),
            t,
        );
        f0_hz.push(pitch_to_hz(note.pitch + glide + bend + ornament));

        let expression = match note.expression.is_empty() {
            true => 1.0,
            false => curve_at(note.expression, tick - note.curve_base).clamp(0.0, 1.0),
        };
        // The release of a consonant comes back up to the vowel; the rest keeps its level.
        let gain = if release {
            1.0
        } else {
            level_gain(note.levels, token)
        };
        energy.push(note.velocity * expression * envelope(note, t) * gain);
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
    let widths = track
        .voice
        .as_ref()
        .and_then(|voice| voice.consonants.as_ref());
    let levels = track.voice.as_ref().and_then(|voice| voice.levels.as_ref());
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
                let (phonemes, phoneme_seconds) = match note.phonemes.is_empty() {
                    true => (vec!["a".to_string()], Vec::new()),
                    false => (note.phonemes.clone(), note.phoneme_seconds.clone()),
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
                        phoneme_seconds,
                        scoop: note.scoop,
                        fall: note.fall,
                        vibrato: note.vibrato,
                        curve_base: base,
                        bend: &clip.bend,
                        expression,
                        widths,
                        levels,
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

/// Which phoneme is sounding `t` seconds into the timeline, for a note known to contain `t`,
/// and whether that moment is inside a consonant's release — its last
/// [`CONSONANT_RELEASE_SECONDS`], capped at half of it.
fn phoneme_at<'a>(note: &'a TimedNote<'a>, t: f64) -> (&'a str, bool) {
    let segments = segment(note);
    let into = t - note.start;
    let Some((from, to, token)) = segments
        .iter()
        .find(|(from, to, _)| into >= *from && into < *to)
        .or(segments.last())
    else {
        return (SILENCE, false);
    };
    let release = CONSONANT_RELEASE_SECONDS.min((to - from) / 2.0);
    let remaining = to - into;
    (
        token.as_str(),
        !is_syllabic(token) && remaining >= 0.0 && remaining <= release,
    )
}

/// The note's phonemes laid across its length: `(from, to, token)` in seconds from its start.
fn segment<'a>(note: &'a TimedNote<'a>) -> Vec<(f64, f64, &'a String)> {
    let length = (note.end - note.start).max(0.0);
    phoneme_layout(&note.phonemes, &note.phoneme_seconds, length, note.widths)
        .into_iter()
        .zip(&note.phonemes)
        .map(|((from, to), token)| (from, to, token))
        .collect()
}

/// The seconds each of a note's phonemes occupies, as `(from, to)` pairs from its start.
///
/// The rule the module doc states — measured-width consonants at the edges, the middle shared
/// equally, consonants scaled down together rather than swallowing a short note — bent by
/// any pins in `pinned` (parallel to `phonemes`, 0 meaning the rule decides): a pinned
/// phoneme takes exactly its seconds, the unpinned middle shares whatever remains, and where
/// the widths alone overrun the note everything is squeezed together proportionally so every
/// phoneme still sounds. A total shorter than the note is left alone — the frames extend the
/// final phoneme over the tail, which is what holding a vowel means.
///
/// `widths` is the track's voice's own consonant table, where its export carried one; `None`
/// gives every consonant [`CONSONANT_SECONDS`], the rule as it stood before models measured.
///
/// Public because an editor that lets a boundary be dragged has to lay the phonemes out
/// exactly as the frames will, and two implementations of this rule would drift.
pub fn phoneme_layout(
    phonemes: &[String],
    pinned: &[f64],
    length: f64,
    widths: Option<&ConsonantWidths>,
) -> Vec<(f64, f64)> {
    let length = length.max(0.0);
    let count = phonemes.len();
    if count == 0 {
        return Vec::new();
    }
    let first_syllabic = phonemes.iter().position(|p| is_syllabic(p));
    let last_syllabic = phonemes.iter().rposition(|p| is_syllabic(p));
    let (prefix, middle) = match (first_syllabic, last_syllabic) {
        (Some(first), Some(last)) => (first, last + 1 - first),
        // No syllabic at all: share the whole note equally rather than inventing an edge.
        _ => (0, count),
    };
    let edge = |index: usize| index < prefix || index >= prefix + middle;
    let pin = |index: usize| pinned.get(index).copied().filter(|seconds| *seconds > 0.0);
    // A width that is not a positive number is a table entry nobody can mean; the fallback
    // keeps a broken export from writing zero-length consonants.
    let width_of = |index: usize| {
        widths
            .map(|table| table.width(&phonemes[index]))
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
            .unwrap_or(CONSONANT_SECONDS)
    };

    // Unpinned edge consonants keep their measured slots, scaled down together so a cluster
    // of them still leaves the vowel half the note.
    let fixed: f64 = (0..count)
        .filter(|index| edge(*index) && pin(*index).is_none())
        .map(&width_of)
        .sum();
    let scale = match fixed > length / 2.0 {
        true => (length / 2.0) / fixed.max(f64::EPSILON),
        false => 1.0,
    };

    let mut widths: Vec<f64> = (0..count)
        .map(|index| match pin(index) {
            Some(seconds) => seconds,
            None if edge(index) => width_of(index) * scale,
            // A placeholder the stretchy middle fills in below.
            None => f64::NAN,
        })
        .collect();
    let reserved: f64 = widths.iter().filter(|width| width.is_finite()).sum();
    let stretchy = widths.iter().filter(|width| width.is_nan()).count();
    let shared = (length - reserved).max(0.0) / stretchy.max(1) as f64;
    for width in &mut widths {
        if width.is_nan() {
            *width = shared;
        }
    }

    // Pins can overrun the note; a squeeze keeps every phoneme audible, proportionally.
    let total: f64 = widths.iter().sum();
    if total > length && total > f64::EPSILON {
        let squeeze = length / total;
        for width in &mut widths {
            *width *= squeeze;
        }
    }

    let mut out = Vec::with_capacity(count);
    let mut at = 0.0;
    for width in widths {
        out.push((at, at + width));
        at += width;
    }
    out
}

/// How much of the note's level a phoneme is given: the voice's measured consonant level
/// as a linear gain, the default for a consonant it never measured, and all of it for a
/// syllabic — or for everything, on a voice with no table.
pub fn level_gain(levels: Option<&ConsonantLevels>, phoneme: &str) -> f32 {
    match levels {
        Some(levels) if levels.measured(phoneme) || !is_syllabic(phoneme) => {
            10f32.powf(levels.db(phoneme) as f32 / 20.0)
        }
        _ => 1.0,
    }
}

/// Semitones the glide between touching notes moves the pitch at `t`, for a frame in `note`.
///
/// Where `prev` ends exactly where `note` begins, the last half-glide of `prev` and the first
/// half-glide of `note` carry a straight line from the one pitch to the other, and the same
/// where `note` ends exactly where `next` begins; each half is capped at a quarter of its own
/// note so a short note keeps its pitch for the half in the middle. A fall written on the
/// earlier note or a scoop on the later one is a slide somebody meant, and switches the glide
/// off at that boundary. Anything but touching — a rest, however short — gets nothing: a note
/// after silence starts on its pitch.
fn glide_offset(
    prev: Option<&TimedNote<'_>>,
    note: &TimedNote<'_>,
    next: Option<&TimedNote<'_>>,
    t: f64,
) -> f32 {
    let touching = |earlier: &TimedNote<'_>, later: &TimedNote<'_>| {
        (later.start - earlier.end).abs() < 1e-9 && earlier.fall.is_none() && later.scoop.is_none()
    };
    let half = |note: &TimedNote<'_>| (GLIDE_SECONDS / 2.0).min((note.end - note.start) / 4.0);
    let progress =
        |from: f64, to: f64| ((t - from) / (to - from).max(f64::EPSILON)).clamp(0.0, 1.0);

    let mut offset = 0.0f64;
    if let Some(prev) = prev.filter(|prev| touching(prev, note)) {
        let travelled = progress(note.start - half(prev), note.start + half(note));
        offset += f64::from(prev.pitch - note.pitch) * (1.0 - travelled);
    }
    if let Some(next) = next.filter(|next| touching(note, next)) {
        let travelled = progress(note.end - half(note), note.end + half(next));
        offset += f64::from(next.pitch - note.pitch) * travelled;
    }
    offset as f32
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
            voice: None,
            take: None,
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
    fn a_frame_hop_loaded_from_a_project_is_bounded_before_rendering() {
        let mut singer = track(vec![sung(69, 0.0, 1.0, &["a"])]);
        singer.frame_hop = 1e-9;
        let frames = render_frames(&singer, &map());
        assert_eq!(frames.hop_seconds, MIN_FRAME_HOP);
        assert_eq!(frames.len(), 501);

        singer.frame_hop = 99.0;
        let frames = render_frames(&singer, &map());
        assert_eq!(frames.hop_seconds, MAX_FRAME_HOP);
        assert_eq!(frames.len(), 6);
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
    fn a_pinned_phoneme_takes_exactly_its_seconds() {
        // か for one beat, with the k pinned to 120 ms — twice what the rule would give it.
        let mut note = sung(60, 0.0, 1.0, &["k", "a"]);
        note.phoneme_seconds = vec![0.120, 0.0];
        let frames = render_frames(&track(vec![note]), &map());
        let k_frames = frames.phonemes.iter().filter(|id| **id == 1).count();
        assert_eq!(k_frames, 12, "120 ms of k at a 10 ms hop");
        assert_eq!(
            frames.phonemes[12], 2,
            "the vowel starts where the pin ends"
        );
    }

    #[test]
    fn a_trailing_consonant_held_past_its_pin_is_not_forever_in_release() {
        let mut note = sung(60, 0.0, 4.0, &["a", "s"]);
        note.phoneme_seconds = vec![0.010, 0.010];
        let plain = render_frames(&track(vec![note.clone()]), &map());

        let mut levelled = track(vec![note]);
        levelled.voice = Some(auris_core::SingerVoice {
            path: auris_core::AssetPath::external("/nowhere/voice.onnx"),
            name: "Measured".into(),
            consonants: None,
            levels: Some(ConsonantLevels {
                default: -12.0,
                db: [("s".to_string(), -20.0)].into_iter().collect(),
            }),
            speaker: None,
        });
        let levelled = render_frames(&levelled, &map());
        let late = 100;
        let token = plain.inventory[plain.phonemes[late] as usize].as_str();
        assert_eq!(token, "s", "the final phoneme is held over the unused tail");
        assert!(
            (levelled.energy[late] / plain.energy[late] - 0.1).abs() < 1e-3,
            "past its own release, /s/ returns to its measured closure level"
        );
    }

    #[test]
    fn pins_that_overrun_the_note_squeeze_together() {
        // A 200 ms note whose pins ask for two full seconds: everything scales down in
        // proportion, so both phonemes still sound and the note still ends on time.
        let mut note = sung(60, 0.0, 0.4, &["k", "a"]);
        note.phoneme_seconds = vec![1.0, 1.0];
        let frames = render_frames(&track(vec![note]), &map());
        let k_frames = frames.phonemes.iter().filter(|id| **id == 1).count();
        let a_frames = frames.phonemes.iter().filter(|id| **id == 2).count();
        assert_eq!(k_frames, 10, "equal pins squeeze to equal halves");
        assert_eq!(a_frames, 10);
    }

    #[test]
    fn a_pin_that_ends_early_leaves_the_tail_to_the_last_phoneme() {
        // The vowel pinned to 100 ms of a one-second note: past the pin there is no next
        // segment, and holding the final phoneme is what a held note means.
        let mut note = sung(60, 0.0, 2.0, &["k", "a"]);
        note.phoneme_seconds = vec![0.0, 0.100];
        let frames = render_frames(&track(vec![note]), &map());
        assert_eq!(frames.phonemes[50], 2, "half a second in, the vowel holds");
    }

    #[test]
    fn the_layout_without_pins_is_the_standing_rule() {
        let phonemes: Vec<String> = ["k", "a"].iter().map(|s| s.to_string()).collect();
        let layout = phoneme_layout(&phonemes, &[], 1.0, None);
        assert_eq!(layout.len(), 2);
        assert!((layout[0].1 - CONSONANT_SECONDS).abs() < 1e-9);
        assert!((layout[1].1 - 1.0).abs() < 1e-9, "the vowel fills the rest");
    }

    /// The table the width tests below hand in: an affricate twice a stop's length, roughly
    /// the spread the auris-singer measurement reports.
    fn measured() -> auris_core::ConsonantWidths {
        auris_core::ConsonantWidths {
            default: 0.070,
            seconds: [("ts".to_string(), 0.120), ("k".to_string(), 0.090)]
                .into_iter()
                .collect(),
        }
    }

    #[test]
    fn a_voice_s_own_widths_replace_the_fixed_sixty_milliseconds() {
        let phonemes: Vec<String> = ["ts", "a"].iter().map(|s| s.to_string()).collect();
        let layout = phoneme_layout(&phonemes, &[], 1.0, Some(&measured()));
        assert!(
            (layout[0].1 - 0.120).abs() < 1e-9,
            "the affricate's measured width"
        );

        // A consonant the table never measured takes the table's own default, not the
        // built-in one.
        let phonemes: Vec<String> = ["m", "a"].iter().map(|s| s.to_string()).collect();
        let layout = phoneme_layout(&phonemes, &[], 1.0, Some(&measured()));
        assert!((layout[0].1 - 0.070).abs() < 1e-9);

        // A pin still beats the table: the by-hand correction outranks the measurement.
        let phonemes: Vec<String> = ["ts", "a"].iter().map(|s| s.to_string()).collect();
        let layout = phoneme_layout(&phonemes, &[0.200, 0.0], 1.0, Some(&measured()));
        assert!((layout[0].1 - 0.200).abs() < 1e-9);
    }

    #[test]
    fn measured_widths_scale_down_together_on_a_short_note() {
        // ts + k ask for 210 ms of a 200 ms note; the half-note cap squeezes them in
        // proportion, so the affricate stays longer than the stop.
        let phonemes: Vec<String> = ["ts", "k", "a"].iter().map(|s| s.to_string()).collect();
        let layout = phoneme_layout(&phonemes, &[], 0.2, Some(&measured()));
        let ts = layout[0].1 - layout[0].0;
        let k = layout[1].1 - layout[1].0;
        assert!(
            (ts + k - 0.1).abs() < 1e-9,
            "together they take half the note"
        );
        assert!(
            (ts / k - 0.120 / 0.090).abs() < 1e-9,
            "in their measured ratio"
        );
    }

    #[test]
    fn the_frames_read_the_widths_off_the_track_s_voice() {
        // The same [ts a] note, sung twice: once voiceless, once by a voice whose export
        // measured its consonants. The frames move without any caller passing a table.
        let note = sung(60, 0.0, 1.0, &["ts", "a"]);
        let plain = render_frames(&track(vec![note.clone()]), &map());
        let mut voiced = track(vec![note]);
        voiced.voice = Some(auris_core::SingerVoice {
            levels: None,
            path: auris_core::AssetPath::external("/nowhere/voice.onnx"),
            name: "Measured".into(),
            consonants: Some(measured()),
            speaker: None,
        });
        let voiced = render_frames(&voiced, &map());

        let ts = |frames: &SingerFrames| frames.phonemes.iter().filter(|id| **id == 1).count();
        assert_eq!(ts(&plain), 6, "60 ms at a 10 ms hop, the fallback");
        assert_eq!(ts(&voiced), 12, "120 ms — the voice's own measurement");
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
    fn ornaments_move_the_frames_the_model_is_fed() {
        // A4 for two beats — one second — with a 100 ms scoop of one semitone.
        let mut note = sung(69, 0.0, 2.0, &["a"]);
        note.scoop = Some(Scoop {
            depth: 1.0,
            seconds: 0.100,
        });
        let frames = render_frames(&track(vec![note]), &map());
        // The first frame starts the full semitone under, and by the rise's end the note
        // stands at its own pitch.
        assert!((frames.f0_hz[0] - pitch_to_hz(68.0)).abs() < 1e-2);
        assert!((frames.f0_hz[50] - pitch_to_hz(69.0)).abs() < 1e-2);
        assert!(
            frames.f0_hz[5] > frames.f0_hz[0] && frames.f0_hz[5] < frames.f0_hz[50],
            "the rise is under way at 50 ms"
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
    fn touching_notes_glide_into_each_other() {
        // C4 then E4, two beats each: the boundary is at 1.0 s, frame 100.
        let frames = render_frames(
            &track(vec![sung(60, 0.0, 2.0, &["a"]), sung(64, 2.0, 2.0, &["i"])]),
            &map(),
        );
        // Forty milliseconds either side is the glide; outside it each note holds its pitch.
        assert!((frames.f0_hz[95] - pitch_to_hz(60.0)).abs() < 1e-3);
        assert!((frames.f0_hz[105] - pitch_to_hz(64.0)).abs() < 1e-3);
        // A straight line: a quarter of the way at 0.98 s, halfway on the boundary itself.
        assert!((frames.f0_hz[98] - pitch_to_hz(61.0)).abs() < 1e-2);
        assert!((frames.f0_hz[100] - pitch_to_hz(62.0)).abs() < 1e-2);
        assert!((frames.f0_hz[102] - pitch_to_hz(63.0)).abs() < 1e-2);
        // The phoneme changes on the boundary, not with the pitch.
        assert_eq!(frames.phonemes[99], 1);
        assert_eq!(frames.phonemes[100], 2);
    }

    #[test]
    fn a_rest_or_a_written_slide_leaves_the_step_alone() {
        // A sixteenth of rest between the notes: each starts and ends on its own pitch.
        let frames = render_frames(
            &track(vec![
                sung(60, 0.0, 1.75, &["a"]),
                sung(64, 2.0, 2.0, &["i"]),
            ]),
            &map(),
        );
        assert!((frames.f0_hz[86] - pitch_to_hz(60.0)).abs() < 1e-3);
        assert!((frames.f0_hz[100] - pitch_to_hz(64.0)).abs() < 1e-3);

        // A scoop on the second note is the written way in; the first note holds to its end.
        let mut singer = track(vec![sung(60, 0.0, 2.0, &["a"]), sung(64, 2.0, 2.0, &["i"])]);
        singer.clips[0].notes[1].scoop = Some(Scoop {
            depth: 2.0,
            seconds: 0.1,
        });
        let frames = render_frames(&singer, &map());
        assert!((frames.f0_hz[99] - pitch_to_hz(60.0)).abs() < 1e-3);
        assert!((frames.f0_hz[100] - pitch_to_hz(62.0)).abs() < 1e-2);
    }

    #[test]
    fn a_short_note_keeps_its_pitch_for_its_middle_half() {
        // Sixteenths at 120 BPM: 125 ms each, so each half-glide is capped at 31.25 ms.
        let frames = render_frames(
            &track(vec![
                sung(60, 0.0, 0.25, &["a"]),
                sung(64, 0.25, 0.25, &["i"]),
                sung(60, 0.5, 0.25, &["a"]),
            ]),
            &map(),
        );
        // 0.19 s is 60 ms into the middle note, past its entry and before its exit.
        assert!((frames.f0_hz[19] - pitch_to_hz(64.0)).abs() < 1e-3);
        // Frame 12 is 5 ms short of the first boundary: on the line, and short of halfway.
        assert!(frames.f0_hz[12] > pitch_to_hz(61.0) && frames.f0_hz[12] < pitch_to_hz(62.0));
        // Frame 25 is the second boundary itself: halfway back down.
        assert!((frames.f0_hz[25] - pitch_to_hz(62.0)).abs() < 1e-2);
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

    #[test]
    fn a_voice_s_own_levels_turn_the_consonants_down_and_leave_the_vowel_alone() {
        // か on one long note at velocity 0.8: once on no voice, once on a voice whose export
        // measured its /k/ at −20 dB. The frames move without any caller passing a table.
        let note = sung(60, 0.0, 4.0, &["k", "a"]);
        let plain = render_frames(&track(vec![note.clone()]), &map());
        let mut levelled = track(vec![note]);
        levelled.voice = Some(auris_core::SingerVoice {
            path: auris_core::AssetPath::external("/nowhere/voice.onnx"),
            name: "Measured".into(),
            consonants: None,
            levels: Some(ConsonantLevels {
                default: -12.0,
                db: [("k".to_string(), -20.0)].into_iter().collect(),
            }),
            speaker: None,
        });
        let levelled = render_frames(&levelled, &map());
        let token = |frames: &SingerFrames, at: usize| {
            frames.inventory[frames.phonemes[at] as usize].clone()
        };
        // Well inside the consonant (60 ms at a 10 ms hop), and well inside the vowel.
        let (k, a) = (3usize, 100usize);
        assert_eq!(token(&plain, k), "k");
        assert_eq!(token(&plain, a), "a");
        assert!(
            (levelled.energy[k] / plain.energy[k] - 0.1).abs() < 1e-3,
            "−20 dB is a tenth: {} against {}",
            levelled.energy[k],
            plain.energy[k]
        );
        assert!(
            (levelled.energy[a] - plain.energy[a]).abs() < 1e-6,
            "the vowel keeps its level"
        );
        // The consonant's last 20 ms — frames 4 and 5 of a 60 ms /k/ — are its release, and
        // come back up to the vowel's level; the frame before them is still the closure.
        assert_eq!(token(&plain, 5), "k");
        assert!(
            (levelled.energy[5] - plain.energy[5]).abs() < 1e-6,
            "the burst"
        );
        assert!(
            (levelled.energy[4] - plain.energy[4]).abs() < 1e-6,
            "the burst"
        );
        assert!(
            (levelled.energy[3] / plain.energy[3] - 0.1).abs() < 1e-3,
            "still closed"
        );

        assert_eq!(level_gain(None, "k"), 1.0, "no table, no change");
        let table = ConsonantLevels {
            default: -6.0,
            db: Default::default(),
        };
        assert!(
            (level_gain(Some(&table), "s") - 0.501).abs() < 1e-3,
            "an unmeasured consonant takes the default"
        );
        assert_eq!(
            level_gain(Some(&table), "a"),
            1.0,
            "a vowel never takes the default"
        );
        assert_eq!(
            level_gain(Some(&table), "ɴ"),
            1.0,
            "nor the moraic nasal, a syllabic"
        );
    }
}
