//! Reading a melody: what key it is in, and what chords would go under it.
//!
//! The composer's other direction. Everywhere else in this crate a specification goes in and notes
//! come out; here notes go in and the *harmony* comes out — a key and a chart — which is exactly
//! what [`write_phrase`](crate::write_phrase) needs in order to write a bass line, a comp and a
//! pad under a tune somebody played by hand.
//!
//! Nothing here draws a random number. The same melody gives the same key and the same chords every
//! time, which is what makes accompaniment something a person can iterate on: change one note,
//! press it again, hear what that note was doing to the harmony.
//!
//! # How much this can know
//!
//! Not everything, and the shape of what it misses is worth saying out loud. A melody is a thin
//! slice of a piece — one voice, no bass, no inner parts — so a passage that would be
//! unambiguous with a chord under it is genuinely ambiguous without one. The two failures to
//! expect are a relative-key swap (a tune in A minor read as C major, which shares every note) and
//! a bar of passing notes read as the chord they pass through.
//!
//! Both are *nudged* rather than solved, and both are recoverable: what this produces is written
//! into the harmony lane where it can be seen and edited, and the parts written from it carry
//! recipes, so correcting a chord and regenerating is one gesture. That is the design — a first
//! draft a person corrects, not an oracle.

use auris_core::time::{Ticks, TimeSignature};
use auris_core::{ClipPreset, Note};

use crate::gm;

use crate::theory::chart::{Chart, ChartMode, ChartOrigin};
use crate::theory::key::Key;
use crate::theory::numeral::{Numeral, diatonic_quality};
use crate::theory::pitch::PitchClass;
use crate::theory::scale::ScaleId;

/// How much weight the twelve pitch classes carry in a major key, and in a minor one.
///
/// Krumhansl and Kessler's probe-tone profiles, which are the measured answer to "how well does
/// this note fit this key" and have been the standard basis for key-finding since 1982. They are
/// listeners' ratings rather than a theory of the scale, which is why the numbers are not simply
/// *in the key* or *out of it*: the fifth outranks the third, and the leading note outranks the
/// second, because that is what people reported.
const MAJOR_PROFILE: [f64; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];

/// The minor half of [`MAJOR_PROFILE`]'s pair.
const MINOR_PROFILE: [f64; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

/// How much of a note's weight comes from being the first thing in the bar.
///
/// A melody spends most of its length on notes that are not the chord and most of its *arrivals*
/// on notes that are. Weighting only by duration reads a bar of passing quavers as whatever it
/// passes through; weighting only by position throws away the difference between a held note and a
/// grace note. This is the mix, and it is deliberately gentle — the downbeat gets a thumb on the
/// scale, not the vote.
const DOWNBEAT_BONUS: f64 = 0.5;

/// How much a bar's chord is pulled towards the one before it.
///
/// A progression that changes chord on every plausible reading is not a progression, it is a
/// harmonisation exercise. This is small enough that a bar which genuinely wants a new chord gets
/// one, and large enough that a bar whose two best readings are a hair apart stays put.
const INERTIA: f64 = 0.15;

/// How weight is spread over a chord's three tones.
///
/// The root carries most, the fifth least. A bar full of thirds and fifths of a chord and nothing
/// else is more often that chord's relative than that chord, and this is what lets the root break
/// the tie.
const TONE_WEIGHTS: [f64; 3] = [1.0, 0.8, 0.6];

/// The General MIDI sound an accompanying part should be played on.
///
/// Here rather than in the session because it is a musical choice and not a plumbing one: which
/// file answers to a program number is the session's business, and *which program a bass line
/// wants* is this crate's. The composer's own presets make the same choice through
/// [`Role`](crate::Role); a part written straight onto the timeline has no role, only a
/// [`ClipPreset`], and this is that vocabulary's answer.
///
/// The choices are the unremarkable ones on purpose. An accompaniment is a bed for something
/// somebody already wrote, and a first draft played on a slap bass and a church organ is a draft
/// about the timbres rather than about the harmony.
pub fn sound_for(preset: ClipPreset) -> gm::Sound {
    let program = match preset {
        ClipPreset::Lead => 80, // Square lead: a tune wants to be heard over the rest.
        ClipPreset::Chords => 0, // Acoustic grand — the comping instrument that suits everything.
        ClipPreset::Pad => 48,  // String ensemble.
        ClipPreset::Arp => 4,   // Electric piano, which arpeggiates without smearing.
        ClipPreset::Stab => 48, // Strings again: a stab is a section hit.
        ClipPreset::Bass => 33, // Finger bass.
        // Every drum part is the standard kit. Which *pieces* of it play is the preset's business
        // and not the sound's — a kick track and a hat track are the same kit struck differently.
        ClipPreset::Drums | ClipPreset::Kick | ClipPreset::Snare | ClipPreset::Hat => 0,
    };
    gm::Program(program).sound(preset.is_drums())
}

/// What reading a melody produced.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    /// The key the melody is in.
    pub key: Key,
    /// One chord per bar, from the first bar of the melody.
    pub chart: Chart,
    /// How many bars the melody covers.
    pub bars: usize,
}

/// The key a run of notes is in.
///
/// Twenty-four candidates — a major and a minor on each of the twelve tonics — scored by
/// correlating a duration-weighted histogram of what the melody plays against Krumhansl and
/// Kessler's probe-tone profiles, which are the measured answer to "how well does this note fit
/// this key". The highest correlation wins.
///
/// C major for a melody with no notes in it. That is not a guess dressed as an answer: something
/// has to come back, an empty melody is in no key at all, and C major is the key a document already
/// holds before anybody touches it.
pub fn detect_key(notes: &[Note]) -> Key {
    let weights = pitch_weights(notes);
    if weights.iter().all(|weight| *weight <= 0.0) {
        return Key::new(PitchClass::new(0), ScaleId::Major);
    }

    let mut best = (
        f64::NEG_INFINITY,
        Key::new(PitchClass::new(0), ScaleId::Major),
    );
    for tonic in 0..12 {
        for (profile, scale) in [
            (&MAJOR_PROFILE, ScaleId::Major),
            (&MINOR_PROFILE, ScaleId::Minor),
        ] {
            // Rotated so that index 0 of the profile lands on this candidate's tonic.
            let rotated: Vec<f64> = (0..12)
                .map(|step| profile[((step + 12 - tonic) % 12) as usize])
                .collect();
            let score = correlation(&weights, &rotated);
            if score > best.0 {
                best = (score, Key::new(PitchClass::new(tonic), scale));
            }
        }
    }
    best.1
}

/// How much of the melody's length each pitch class holds.
///
/// Length rather than count, because a whole note is a stronger statement about the key than four
/// semiquavers of passing tone, and struck harder counts for more than brushed — both of which are
/// true of the phrase a person actually hears.
fn pitch_weights(notes: &[Note]) -> [f64; 12] {
    let mut weights = [0.0f64; 12];
    for note in notes {
        let length = note.length.raw().max(0) as f64;
        let velocity = f64::from(note.velocity.clamp(0.0, 1.0)).max(0.1);
        weights[(note.pitch % 12) as usize] += length * velocity;
    }
    weights
}

/// Pearson's correlation between two twelve-element vectors.
///
/// Correlation rather than a dot product, and that is the whole of why this works on a melody that
/// plays five notes as well as on one that plays all twelve: it compares the *shape* of what was
/// played against the shape of the profile, so a sparse tune is not simply outscored by the key
/// whose profile has the largest numbers.
fn correlation(a: &[f64; 12], b: &[f64]) -> f64 {
    let mean = |values: &[f64]| values.iter().sum::<f64>() / values.len() as f64;
    let (mean_a, mean_b) = (mean(a), mean(b));
    let mut covariance = 0.0;
    let (mut variance_a, mut variance_b) = (0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        let (dx, dy) = (x - mean_a, y - mean_b);
        covariance += dx * dy;
        variance_a += dx * dx;
        variance_b += dy * dy;
    }
    let spread = (variance_a * variance_b).sqrt();
    if spread <= f64::EPSILON {
        return 0.0;
    }
    covariance / spread
}

/// The line of a melody, written as the scale steps a motif is given in.
///
/// The bridge from "this clip" to `motif = "0 2 4 2"`: the top voice of each onset — chords in
/// a melody clip are voicings, and what a person hums back is the top of them — read as scale
/// degrees of `key` and written relative to the first note, which is the anchor every generated
/// figure starts from. A note outside the scale is pulled onto the nearest degree, exactly as a
/// transposed figure would be when it is played back.
///
/// What comes back obeys the motif field's own bounds — steps clamped to a ninth either way,
/// at most thirty-two of them — so it can be written into a specification verbatim. Fewer than
/// two notes come back as fewer than two, and the caller says "that is not a line" rather than
/// this inventing one.
pub fn motif_of(notes: &[Note], key: Key) -> Vec<i32> {
    let mut ordered: Vec<(Ticks, i32)> = Vec::new();
    let mut sorted: Vec<&Note> = notes.iter().collect();
    sorted.sort_by_key(|note| (note.start, note.pitch));
    for note in sorted {
        match ordered.last_mut() {
            // The top voice of a chord: the last note wins because the sort put it highest.
            Some((start, pitch)) if *start == note.start => *pitch = i32::from(note.pitch),
            _ => ordered.push((note.start, i32::from(note.pitch))),
        }
    }
    let degrees: Vec<i32> = ordered
        .iter()
        .map(|(_, pitch)| key.scale.nearest_degree(pitch - key.tonic.semitones()))
        .collect();
    let Some(first) = degrees.first().copied() else {
        return Vec::new();
    };
    degrees
        .iter()
        .map(|degree| (degree - first).clamp(-9, 9))
        .take(32)
        .collect()
}

/// A chord for every bar of a melody, in `key`.
///
/// One chord per bar. Half-bar chords are perfectly musical and are not what a first draft should
/// produce: twice as many chords is twice as many to disagree with, and a progression a person
/// keeps is one they could read at a glance.
///
/// `bars` says how many to write; `bar_ticks` how long one is. A bar with nothing sounding in it
/// keeps the chord before it — silence is not a chord change — and the first bar of a silent
/// melody gets the tonic, because a progression has to start somewhere and that is where.
pub fn harmonise(notes: &[Note], key: Key, from: Ticks, bar_ticks: Ticks, bars: usize) -> Chart {
    let span = Ticks(bar_ticks.raw().max(1));
    let mut written: Vec<Vec<Numeral>> = Vec::with_capacity(bars);
    let mut previous: Option<u8> = None;

    for bar in 0..bars {
        let start = from + span * bar as i64;
        let weights = bar_weights(notes, start, start + span);
        let degree = match weights.iter().all(|weight| *weight <= 0.0) {
            // Nothing sounding: hold what was already going, or open on the tonic.
            true => previous.unwrap_or(1),
            false => best_degree(&weights, key, previous),
        };
        previous = Some(degree);
        written.push(vec![Numeral::new(
            degree,
            diatonic_quality(key, degree).is_minor(),
        )]);
    }

    // `Given` rather than `Generated`: these chords were read off something a person played, so
    // the composer's colouring and cadence scheduling must not rewrite them into a progression
    // that lands somewhere the melody does not.
    Chart::new(written, ChartOrigin::Given).written_in(ChartMode::of(key))
}

/// How much each pitch class is worth over one bar.
///
/// A note counts for the part of itself inside the bar, so one held across a bar line speaks in
/// both — which is what it does to the ear.
fn bar_weights(notes: &[Note], from: Ticks, to: Ticks) -> [f64; 12] {
    let mut weights = [0.0f64; 12];
    for note in notes {
        let (start, end) = (note.start, note.end());
        let overlap = end.min(to).raw() - start.max(from).raw();
        if overlap <= 0 {
            continue;
        }
        let mut weight = overlap as f64;
        if start >= from && start < to {
            // An arrival inside the bar, not merely a continuation through it.
            weight += DOWNBEAT_BONUS * (to.raw() - from.raw()) as f64 * strength(start, from, to);
        }
        weights[(note.pitch % 12) as usize] += weight;
    }
    weights
}

/// How much a position inside a bar counts as an arrival, from 1 on the bar line to 0 at its end.
///
/// A straight line rather than the metric hierarchy the rhythm module uses. This is deciding
/// between seven chords from one bar of a single voice, and a finer weighting would be a claim
/// about the phrasing that one melodic line does not support.
fn strength(at: Ticks, from: Ticks, to: Ticks) -> f64 {
    let span = (to.raw() - from.raw()).max(1) as f64;
    1.0 - ((at.raw() - from.raw()) as f64 / span).clamp(0.0, 1.0)
}

/// Which of the key's seven degrees fits `weights` best.
///
/// Every degree's triad is scored by how much of the bar its three tones account for, with the
/// root weighted heaviest — see [`TONE_WEIGHTS`] — and the previous bar's chord given a small
/// head start so a progression does not change on every coin toss.
fn best_degree(weights: &[f64; 12], key: Key, previous: Option<u8>) -> u8 {
    let total: f64 = weights.iter().sum();
    let mut best = (f64::NEG_INFINITY, 1u8);
    for degree in 1..=7u8 {
        let chord = Numeral::new(degree, diatonic_quality(key, degree).is_minor()).chord_in(key);
        let mut score = 0.0;
        for (tone, weight) in chord.classes().into_iter().zip(TONE_WEIGHTS) {
            score += weights[tone.semitones().rem_euclid(12) as usize] * weight;
        }
        if previous == Some(degree) {
            score += INERTIA * total;
        }
        if score > best.0 {
            best = (score, degree);
        }
    }
    best.1
}

/// The key and the chords of a melody, over the bars it actually spans.
///
/// The one call an accompaniment needs. `from` is where the melody begins on the timeline and
/// `signature` the meter it is written in; the bar count is worked out from how far the notes
/// reach, rounded up, because half a bar of tune still needs a chord under it.
pub fn read_melody(notes: &[Note], from: Ticks, signature: TimeSignature) -> Reading {
    let key = detect_key(notes);
    let bar_ticks = Ticks(signature.ticks_per_bar().raw().max(1));
    let last = notes
        .iter()
        .map(|note| note.end())
        .max()
        .unwrap_or(from)
        .max(from);
    let bars = (((last - from).raw() + bar_ticks.raw() - 1) / bar_ticks.raw()).max(1) as usize;
    Reading {
        key,
        chart: harmonise(notes, key, from, bar_ticks, bars),
        bars,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAR: Ticks = Ticks(3_840);

    /// A note of `beats` beats at `beat`, in a bar starting at `bar`.
    fn note(pitch: u8, bar: i64, beat: f64, beats: f64) -> Note {
        Note::new(
            pitch,
            BAR * bar + Ticks((beat * 960.0) as i64),
            Ticks((beats * 960.0) as i64),
        )
    }

    /// The degrees a chart writes, one per bar.
    fn degrees(chart: &Chart) -> Vec<u8> {
        chart.bars.iter().map(|bar| bar[0].degree).collect()
    }

    #[test]
    fn a_major_tune_comes_back_in_the_key_it_was_written_in() {
        // A plain C major arpeggio and scale: C E G C, then a descent through the scale.
        let melody: Vec<Note> = [60, 64, 67, 72, 71, 69, 67, 65, 64, 62, 60]
            .into_iter()
            .enumerate()
            .map(|(index, pitch)| note(pitch, index as i64 / 4, (index % 4) as f64, 1.0))
            .collect();
        let key = detect_key(&melody);
        assert_eq!(key.tonic.semitones(), 0, "{}", key.to_text());
        assert!(!key.is_minor(), "{}", key.to_text());
    }

    #[test]
    fn a_transposed_tune_comes_back_transposed() {
        // The same shape moved is the same answer moved. This is the property that says the
        // detector is reading the melody rather than the accident of where C sits.
        let shape = [60, 64, 67, 72, 71, 69, 67, 65, 64, 62, 60];
        for steps in 0..12u8 {
            let melody: Vec<Note> = shape
                .into_iter()
                .enumerate()
                .map(|(index, pitch)| {
                    note(pitch + steps, index as i64 / 4, (index % 4) as f64, 1.0)
                })
                .collect();
            let key = detect_key(&melody);
            assert_eq!(
                key.tonic.semitones(),
                i32::from(steps),
                "moved by {steps} it came back in {}",
                key.to_text()
            );
            assert!(!key.is_minor(), "moved by {steps}: {}", key.to_text());
        }
    }

    #[test]
    fn a_minor_tune_is_not_read_as_its_relative_major() {
        // The hard case, and the one the header warns about: A minor and C major share every
        // note, so nothing but the *weight* on them can tell the two apart. A tune that dwells on
        // A and E and ends on A is in A minor, and the profiles are what say so.
        let melody = vec![
            note(69, 0, 0.0, 2.0), // A, held
            note(72, 0, 2.0, 1.0),
            note(76, 0, 3.0, 1.0),
            note(74, 1, 0.0, 1.0),
            note(72, 1, 1.0, 1.0),
            note(71, 1, 2.0, 1.0),
            note(69, 1, 3.0, 4.0), // and home to A, held
        ];
        let key = detect_key(&melody);
        assert_eq!(key.tonic.semitones(), 9, "{}", key.to_text());
        assert!(key.is_minor(), "{}", key.to_text());
    }

    #[test]
    fn silence_is_not_a_key_and_answers_with_the_one_a_document_already_holds() {
        let key = detect_key(&[]);
        assert_eq!(key.tonic.semitones(), 0);
        assert!(!key.is_minor());
        // And a melody with no length in it is the same case rather than a division by zero.
        assert_eq!(detect_key(&[Note::new(60, Ticks::ZERO, Ticks(0))]), key);
    }

    #[test]
    fn a_melody_reads_back_as_the_motif_that_would_restate_it() {
        let key = Key::new(PitchClass::new(0), ScaleId::Major);
        // C E G E: up a third, up a third, back — the axis of every motif test in this crate.
        let melody = vec![
            note(60, 0, 0.0, 1.0),
            note(64, 0, 1.0, 1.0),
            note(67, 0, 2.0, 1.0),
            note(64, 0, 3.0, 1.0),
        ];
        assert_eq!(motif_of(&melody, key), [0, 2, 4, 2]);

        // A chord under the tune is a voicing, and the line is its top: the same motif with a
        // C struck under the E reads back as the same motif.
        let mut voiced = melody.clone();
        voiced.push(note(48, 0, 1.0, 1.0));
        assert_eq!(motif_of(&voiced, key), [0, 2, 4, 2]);

        // A note off the scale is pulled onto the nearest degree, as playback would pull it.
        let chromatic = vec![note(60, 0, 0.0, 1.0), note(61, 0, 1.0, 1.0)];
        let read = motif_of(&chromatic, key);
        assert_eq!(read.len(), 2);
        assert!((0..=1).contains(&read[1]), "{read:?}");

        // The line is relative to its own first note, so the register does not matter.
        let high: Vec<Note> = melody
            .iter()
            .map(|held| Note::new(held.pitch + 12, held.start, held.length))
            .collect();
        assert_eq!(motif_of(&high, key), [0, 2, 4, 2]);

        // And what comes back fits the field it is written into: at most 32 steps, and fewer
        // than two notes are returned as fewer than two rather than padded into a line.
        let long: Vec<Note> = (0..40i64)
            .map(|index| note(60 + (index % 5) as u8, index / 4, (index % 4) as f64, 1.0))
            .collect();
        assert_eq!(motif_of(&long, key).len(), 32);
        assert!(motif_of(&[], key).is_empty());
        assert_eq!(motif_of(&[note(60, 0, 0.0, 1.0)], key), [0]);
    }

    #[test]
    fn a_bar_gets_the_chord_its_notes_actually_spell() {
        let key = Key::new(PitchClass::new(0), ScaleId::Major);
        // Bar one is a C triad, bar two an F triad, bar three a G triad.
        let melody = vec![
            note(60, 0, 0.0, 1.0),
            note(64, 0, 1.0, 1.0),
            note(67, 0, 2.0, 2.0),
            note(65, 1, 0.0, 1.0),
            note(69, 1, 1.0, 1.0),
            note(72, 1, 2.0, 2.0),
            note(67, 2, 0.0, 1.0),
            note(71, 2, 1.0, 1.0),
            note(74, 2, 2.0, 2.0),
        ];
        let chart = harmonise(&melody, key, Ticks::ZERO, BAR, 3);
        assert_eq!(degrees(&chart), vec![1, 4, 5]);
        // Read off something a person played, so the composer may not rewrite it.
        assert_eq!(chart.origin, ChartOrigin::Given);
    }

    #[test]
    fn a_bar_of_nothing_holds_the_chord_that_was_already_sounding() {
        // Silence is not a chord change. A rest in the middle of a phrase that reset the harmony
        // to the tonic would put a cadence in the middle of every phrase with a breath in it.
        let key = Key::new(PitchClass::new(0), ScaleId::Major);
        let melody = vec![
            note(65, 0, 0.0, 1.0),
            note(69, 0, 1.0, 1.0),
            note(72, 0, 2.0, 2.0),
        ];
        let chart = harmonise(&melody, key, Ticks::ZERO, BAR, 3);
        assert_eq!(degrees(&chart), vec![4, 4, 4]);

        // And a melody of nothing at all opens on the tonic rather than on nothing.
        let empty = harmonise(&[], key, Ticks::ZERO, BAR, 2);
        assert_eq!(degrees(&empty), vec![1, 1]);
    }

    #[test]
    fn the_downbeat_decides_a_bar_its_passing_notes_would_not() {
        // Every note of the bar is in C major, and by sheer length the passing tones outweigh the
        // arrival. What a listener hears is the chord the bar *lands* on, which is what the
        // downbeat weighting is for.
        let key = Key::new(PitchClass::new(0), ScaleId::Major);
        let melody = vec![
            note(67, 0, 0.0, 2.0), // G, on the downbeat and held
            note(69, 0, 2.0, 1.0), // A
            note(71, 0, 3.0, 1.0), // B
        ];
        let chart = harmonise(&melody, key, Ticks::ZERO, BAR, 1);
        assert_eq!(
            degrees(&chart),
            vec![5],
            "the bar arrives on G and the chord should say so"
        );
    }

    #[test]
    fn a_note_held_over_a_bar_line_speaks_in_both_bars() {
        let key = Key::new(PitchClass::new(0), ScaleId::Major);
        // One G, four beats long, starting halfway through bar one.
        let melody = vec![note(67, 0, 2.0, 4.0), note(71, 1, 2.0, 1.0)];
        let chart = harmonise(&melody, key, Ticks::ZERO, BAR, 2);
        assert_eq!(degrees(&chart), vec![5, 5]);
    }

    #[test]
    fn a_melody_is_read_over_exactly_the_bars_it_covers() {
        let melody = vec![note(60, 0, 0.0, 1.0), note(64, 2, 0.0, 1.0)];
        let reading = read_melody(&melody, Ticks::ZERO, TimeSignature::default());
        assert_eq!(reading.bars, 3, "the tune reaches into bar three");
        assert_eq!(reading.chart.bar_count(), 3);

        // Half a bar of tune still needs a chord under it, and a melody of nothing still needs a
        // bar to put one in.
        let scrap = vec![note(60, 0, 0.0, 1.0)];
        assert_eq!(
            read_melody(&scrap, Ticks::ZERO, TimeSignature::default()).bars,
            1
        );
        assert_eq!(
            read_melody(&[], Ticks::ZERO, TimeSignature::default()).bars,
            1
        );
    }

    #[test]
    fn a_melody_that_does_not_start_at_the_beginning_is_read_from_where_it_does() {
        // The bars are counted from the clip, not from the timeline: a tune sitting at bar nine
        // is four bars long, not twelve.
        let melody = vec![note(60, 8, 0.0, 4.0), note(67, 9, 0.0, 4.0)];
        let reading = read_melody(&melody, BAR * 8, TimeSignature::default());
        assert_eq!(reading.bars, 2);
    }

    #[test]
    fn the_chart_is_written_in_the_mode_of_the_key_it_was_read_in() {
        // A chart that did not say declares no mode, and `spelled_in` would then read its degrees
        // literally wherever it landed — so a minor reading stamped into a minor stretch would
        // come out right by luck and a major one by nothing at all.
        let minor = Key::new(PitchClass::new(9), ScaleId::Minor);
        let chart = harmonise(&[], minor, Ticks::ZERO, BAR, 1);
        assert_eq!(chart.mode, Some(ChartMode::Minor));
        // And its tonic is written in lower case, as a minor tonic is.
        assert!(chart.bars[0][0].minor_case);
    }
}
