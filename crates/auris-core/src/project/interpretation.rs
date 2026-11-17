//! Score-reading performance: stages that decide how to play a note from the notes around it.
//!
//! The wander stages are random and the phrase arch is a fixed shape; neither listens to the
//! line. These three do, and are deterministic for it — the same text always plays the same
//! way, so freezing them writes nothing a seed would have to name.
//!
//! * `contour` shapes dynamics from the melody: higher notes and longer notes carry more
//!   weight, and each phrase's last attack arrives a little late and a little softer.
//! * `voicing` balances a chord: the top voice sings, inner voices recede, and the top note
//!   sounds a few milliseconds ahead of the rest.
//! * `articulate` reads each written-legato connection: steps overlap, leaps breathe and a
//!   repeated key is lifted before it is struck again.
//!
//! The constants are first guesses in the sizes the performance literature reports (the KTH
//! rule system's high-loud, duration-contrast and final-ritard rules; piano melody lead), and
//! are meant to be re-measured against recorded performances before they move.

use super::performance::{chords, milliseconds};
use super::{Note, PerformanceContext};
use crate::Ticks;

/// Velocity gained per octave above the phrase's mean top pitch, at full contour.
const HIGH_LOUD_PER_OCTAVE: f32 = 0.10;
/// The furthest a note's pitch is read from its phrase's mean, in octaves.
const HIGH_LOUD_MAX_OCTAVES: f32 = 1.5;
/// Velocity gained per doubling of an inter-onset interval against the phrase median.
const DURATION_CONTRAST: f32 = 0.06;
/// How much softer a phrase's last attack is, at full contour.
const FINAL_SOFTEN: f32 = 0.15;
/// How late a phrase's last attack is, as a fraction of the interval before it.
const FINAL_DELAY_FRACTION: f32 = 0.10;
/// The cap on that delay, in milliseconds, so a long final interval does not stall.
const FINAL_DELAY_MAX_MS: f32 = 40.0;

/// How much softer inner chord voices are than the top voice, at full voicing.
const INNER_SOFTEN: f32 = 0.25;
/// How much softer the lowest chord voice is than the top voice, at full voicing.
const BASS_SOFTEN: f32 = 0.10;
/// How far the top voice sounds ahead of its chord, in milliseconds, at full voicing.
const MELODY_LEAD_MS: f32 = 15.0;

/// How close to the next attack a release must be to read as written legato.
const LEGATO_TOLERANCE: Ticks = Ticks(crate::time::TICKS_PER_QUARTER / 8);
/// The largest top-voice interval, in semitones, played as an overlapping step.
const STEP_SEMITONES: u8 = 2;
/// The smallest top-voice interval, in semitones, played with a breath before it.
const LEAP_SEMITONES: u8 = 5;
/// How far a step overlaps into the next note, in milliseconds, at full articulation.
const STEP_OVERLAP_MS: f32 = 25.0;
/// The silence left before a leap, in milliseconds, at full articulation.
const LEAP_GAP_MS: f32 = 30.0;
/// The silence left before the same key is struck again, in milliseconds.
const REPEAT_GAP_MS: f32 = 40.0;

/// Groups of chord indices (into `groups`) that form one phrase: attacks separated by no more
/// than a quarter note of silence, the split the phrase arch uses.
fn phrases(notes: &[Note], groups: &[Vec<usize>]) -> Vec<std::ops::Range<usize>> {
    let group_end = |g: &Vec<usize>| g.iter().map(|&i| notes[i].end()).max();
    let mut out = Vec::new();
    let mut first = 0;
    while first < groups.len() {
        let mut end = group_end(&groups[first]).unwrap_or(notes[groups[first][0]].start);
        let mut last = first + 1;
        while last < groups.len() && notes[groups[last][0]].start <= end + Ticks::QUARTER {
            end = end.max(group_end(&groups[last]).unwrap_or(end));
            last += 1;
        }
        out.push(first..last);
        first = last;
    }
    out
}

fn top(notes: &[Note], group: &[usize]) -> u8 {
    group.iter().map(|&i| notes[i].pitch).max().unwrap_or(0)
}

/// Shapes each phrase's dynamics and its final attack from the written line.
pub(super) fn contour(notes: &mut [Note], amount: f32, context: PerformanceContext<'_>) {
    let amount = amount.clamp(0.0, 1.0);
    if amount <= 0.0 {
        return;
    }
    let groups = chords(notes);
    for phrase in phrases(notes, &groups) {
        let groups = &groups[phrase];
        if groups.len() < 2 {
            continue;
        }
        let starts: Vec<_> = groups.iter().map(|g| notes[g[0]].start).collect();
        let tops: Vec<_> = groups.iter().map(|g| f32::from(top(notes, g))).collect();
        let mean = tops.iter().sum::<f32>() / tops.len() as f32;
        let intervals: Vec<_> = starts.windows(2).map(|w| (w[1] - w[0]).raw()).collect();
        let mut sorted = intervals.clone();
        sorted.sort_unstable();
        let median = sorted[sorted.len() / 2].max(1) as f32;
        let last = groups.len() - 1;
        for (g, group) in groups.iter().enumerate() {
            let octaves =
                ((tops[g] - mean) / 12.0).clamp(-HIGH_LOUD_MAX_OCTAVES, HIGH_LOUD_MAX_OCTAVES);
            let mut weight = HIGH_LOUD_PER_OCTAVE * octaves;
            if g < last {
                let ratio = (intervals[g].max(1) as f32 / median)
                    .log2()
                    .clamp(-1.0, 1.0);
                weight += DURATION_CONTRAST * ratio;
            }
            let mut factor = 1.0 + amount * weight;
            if g == last {
                factor *= 1.0 - amount * FINAL_SOFTEN;
            }
            for &i in group {
                notes[i].velocity = (notes[i].velocity * factor).clamp(0.0, 1.0);
            }
        }
        // The last attack arrives late; its release stays where it was written.
        let before = intervals[last - 1] as f32;
        let wanted = (before * FINAL_DELAY_FRACTION * amount)
            .min(milliseconds(FINAL_DELAY_MAX_MS * amount, context.bpm).raw() as f32);
        let delay = Ticks(wanted.round() as i64);
        if delay > Ticks::ZERO {
            for &i in &groups[last] {
                let shift = delay.min(notes[i].length - Ticks(1)).max_zero();
                notes[i].start += shift;
                notes[i].length -= shift;
            }
        }
    }
}

/// Brings out each chord's top voice by level and by a melody lead.
pub(super) fn voicing(notes: &mut [Note], amount: f32, context: PerformanceContext<'_>) {
    let amount = amount.clamp(0.0, 1.0);
    if amount <= 0.0 {
        return;
    }
    let lead = milliseconds(MELODY_LEAD_MS * amount, context.bpm);
    // Where each note's key was last released before it, so a lead never reaches back into
    // the previous strike of the same key (its NoteOff would silence the early one).
    let mut order: Vec<_> = (0..notes.len()).collect();
    order.sort_by_key(|&i| (notes[i].pitch, notes[i].start));
    let mut released = vec![None; notes.len()];
    for pair in order.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if notes[a].pitch == notes[b].pitch && notes[a].start < notes[b].start {
            released[b] = Some(notes[a].end());
        }
    }
    for group in chords(notes) {
        let high = top(notes, &group);
        let low = group.iter().map(|&i| notes[i].pitch).min().unwrap_or(high);
        if high == low {
            continue;
        }
        for &i in &group {
            let pitch = notes[i].pitch;
            if pitch == high {
                let floor = released[i].unwrap_or(Ticks::ZERO).max(Ticks::ZERO);
                let shift = lead.min(notes[i].start - floor).max_zero();
                notes[i].start -= shift;
                notes[i].length += shift;
            } else {
                let soften = if pitch == low {
                    BASS_SOFTEN
                } else {
                    INNER_SOFTEN
                };
                notes[i].velocity *= 1.0 - amount * soften;
            }
        }
    }
}

/// Plays each written-legato connection by what it connects to.
pub(super) fn articulate(notes: &mut [Note], amount: f32, context: PerformanceContext<'_>) {
    let amount = amount.clamp(0.0, 1.0);
    if amount <= 0.0 {
        return;
    }
    let overlap = milliseconds(STEP_OVERLAP_MS * amount, context.bpm);
    let leap_gap = milliseconds(LEAP_GAP_MS * amount, context.bpm);
    let repeat_gap = milliseconds(REPEAT_GAP_MS * amount, context.bpm);
    // The next strike of each note's key, which an overlap may never reach.
    let mut order: Vec<_> = (0..notes.len()).collect();
    order.sort_by_key(|&i| (notes[i].pitch, notes[i].start));
    let mut next_strike = vec![None; notes.len()];
    for pair in order.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if notes[a].pitch == notes[b].pitch && notes[a].start < notes[b].start {
            next_strike[a] = Some(notes[b].start);
        }
    }
    let groups = chords(notes);
    let mut ends = Vec::new();
    for pair in groups.windows(2) {
        let (here, next) = (&pair[0], &pair[1]);
        let next_start = notes[next[0]].start;
        let interval = top(notes, here).abs_diff(top(notes, next));
        for &i in here {
            let note = &notes[i];
            let end = note.end();
            if end > next_start || end + LEGATO_TOLERANCE < next_start {
                continue;
            }
            let shortest = Ticks((note.length.raw() / 2).max(1));
            let repeated = next.iter().any(|&j| notes[j].pitch == note.pitch);
            let wanted = if repeated {
                (next_start - repeat_gap).max(note.start + shortest)
            } else if interval <= STEP_SEMITONES {
                let mut end = next_start + overlap;
                if let Some(strike) = next_strike[i] {
                    end = end.min(strike);
                }
                end.min(context.length)
            } else if interval >= LEAP_SEMITONES {
                (next_start - leap_gap).max(note.start + shortest)
            } else {
                next_start
            };
            ends.push((i, wanted));
        }
    }
    for (i, end) in ends {
        if end > notes[i].start {
            notes[i].length = end - notes[i].start;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::SignatureMap;

    fn n(pitch: u8, start: i64, length: i64) -> Note {
        Note {
            velocity: 0.5,
            ..Note::new(pitch, Ticks(start), Ticks(length))
        }
    }

    fn with(signatures: &SignatureMap) -> PerformanceContext<'_> {
        PerformanceContext {
            bpm: 120.0,
            pass: 0,
            start: Ticks::ZERO,
            length: Ticks(3840 * 4),
            signatures,
        }
    }

    // At 120 BPM one millisecond is 1.92 ticks.

    #[test]
    fn contour_weights_high_and_long_notes_and_eases_into_the_phrase_end() {
        let signatures = SignatureMap::default();
        // Four eighths rising, a quarter at the top, then the last note.
        let mut notes = vec![
            n(60, 0, 480),
            n(62, 480, 480),
            n(64, 960, 480),
            n(72, 1440, 960),
            n(67, 2400, 960),
        ];
        contour(&mut notes, 1.0, with(&signatures));
        let v: Vec<_> = notes.iter().map(|n| n.velocity).collect();
        // Mean top pitch 65: middle C sits five semitones under it, the top C seven over.
        assert!(
            (v[0] - 0.5 * (1.0 - 0.10 * 5.0 / 12.0)).abs() < 1e-4,
            "{v:?}"
        );
        // The quarter at the top is both high and twice the median interval long.
        assert!(
            (v[3] - 0.5 * (1.0 + 0.10 * 7.0 / 12.0 + 0.06)).abs() < 1e-4,
            "{v:?}"
        );
        // The last note: slightly above the mean, then 15 % softer.
        assert!(
            (v[4] - 0.5 * (1.0 + 0.10 * 2.0 / 12.0) * 0.85).abs() < 1e-4,
            "{v:?}"
        );
        // Arrives late by 10 % of the 960-tick interval before it, capped at 40 ms = 77 ticks,
        // and still releases where it was written.
        assert_eq!(notes[4].start, Ticks(2400 + 77));
        assert_eq!(notes[4].end(), Ticks(3360));
        assert_eq!(notes[0].start, Ticks::ZERO);
    }

    #[test]
    fn contour_reads_each_phrase_on_its_own_and_does_nothing_at_zero() {
        let signatures = SignatureMap::default();
        let written = vec![
            n(60, 0, 480),
            n(72, 480, 480),
            n(48, 3840, 480),
            n(50, 4320, 480),
        ];
        let mut notes = written.clone();
        contour(&mut notes, 0.0, with(&signatures));
        assert_eq!(notes, written);
        contour(&mut notes, 1.0, with(&signatures));
        // The low second phrase is judged against its own mean, not the first phrase's.
        assert!(notes[3].velocity < 0.5 && notes[2].velocity < notes[3].velocity / 0.85);
        assert!(notes[2].velocity > 0.45, "{:?}", notes[2]);
    }

    #[test]
    fn voicing_softens_the_inner_and_bass_voices_and_leads_with_the_top() {
        let signatures = SignatureMap::default();
        let mut notes = vec![
            n(48, 960, 960),
            n(64, 960, 960),
            n(67, 960, 960),
            n(72, 960, 960),
        ];
        voicing(&mut notes, 1.0, with(&signatures));
        assert!((notes[0].velocity - 0.45).abs() < 1e-6);
        assert!((notes[1].velocity - 0.375).abs() < 1e-6);
        assert!((notes[2].velocity - 0.375).abs() < 1e-6);
        assert_eq!(notes[3].velocity, 0.5);
        // 15 ms ahead, released with the chord.
        assert_eq!(notes[3].start, Ticks(960 - 29));
        assert_eq!(notes[3].end(), Ticks(1920));
        assert_eq!(notes[0].start, Ticks(960));
    }

    #[test]
    fn a_melody_lead_never_reaches_back_into_the_same_key() {
        let signatures = SignatureMap::default();
        // The top C is struck again 10 ticks after its previous release.
        let mut notes = vec![
            n(72, 0, 950),
            n(72, 960, 480),
            n(60, 960, 480),
            n(60, 0, 960),
        ];
        voicing(&mut notes, 1.0, with(&signatures));
        assert_eq!(notes[1].start, Ticks(950));
        assert_eq!(notes[0].start, Ticks::ZERO);
    }

    #[test]
    fn single_lines_are_left_to_contour_by_voicing() {
        let signatures = SignatureMap::default();
        let written = vec![n(60, 0, 480), n(62, 480, 480)];
        let mut notes = written.clone();
        voicing(&mut notes, 1.0, with(&signatures));
        assert_eq!(notes, written);
    }

    #[test]
    fn articulation_overlaps_steps_breathes_before_leaps_and_lifts_repeats() {
        let signatures = SignatureMap::default();
        let mut notes = vec![
            n(60, 0, 480),    // step up to D: overlaps 25 ms = 48 ticks
            n(62, 480, 480),  // leap up to A: leaves 30 ms = 58 ticks
            n(69, 960, 480),  // repeated A: lifts 40 ms = 77 ticks
            n(69, 1440, 480), // third down to F#: as written
            n(66, 1920, 240), // detached: left alone
            n(64, 2400, 480), // last note: nothing follows
        ];
        articulate(&mut notes, 1.0, with(&signatures));
        let ends: Vec<_> = notes.iter().map(|n| n.end().raw()).collect();
        assert_eq!(ends, vec![528, 902, 1363, 1920, 2160, 2880]);
        assert!(
            notes
                .iter()
                .zip([0, 480, 960, 1440, 1920, 2400])
                .all(|(n, s)| n.start.raw() == s)
        );
    }

    #[test]
    fn an_overlap_stops_at_the_next_strike_of_its_key_and_at_the_clip_end() {
        let signatures = SignatureMap::default();
        let context = PerformanceContext {
            length: Ticks(500),
            ..with(&signatures)
        };
        let mut notes = vec![n(60, 0, 480), n(62, 480, 480)];
        articulate(&mut notes, 1.0, context);
        assert_eq!(notes[0].end(), Ticks(500));
        // A step whose own key returns 20 ticks after the next attack.
        let mut notes = vec![n(60, 0, 480), n(62, 480, 20), n(60, 500, 480)];
        articulate(&mut notes, 1.0, with(&signatures));
        assert_eq!(notes[0].end(), Ticks(500));
    }

    #[test]
    fn a_repeat_lift_keeps_at_least_half_the_written_note() {
        let signatures = SignatureMap::default();
        let mut notes = vec![n(60, 0, 60), n(60, 60, 60)];
        articulate(&mut notes, 1.0, with(&signatures));
        assert_eq!(notes[0].length, Ticks(30));
    }
}
