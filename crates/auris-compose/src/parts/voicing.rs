//! Which notes a chord sounds, and how its voices are led from the chord before.
//!
//! A file of its own for the reason `writer` is: the sharing is real. The chords and the pad
//! strike what [`voiced`] chooses and the arpeggio breaks it, and an arpeggio that worked its
//! notes out for itself — the chord stacked from its root, every time — leapt nine semitones at
//! a chord change the keyboard beside it crossed in one or two. Nothing here knows about rhythm
//! or about a part: a chord, the voicing before it, a window and a centre in; pitches out.

use crate::PerformanceStyle;
use crate::rng::Rng;
use crate::theory::chord::Chord;
use crate::theory::pitch::{OCTAVE, PitchClass, fold_into};

/// Motion of ordered voices, allowing a voice to enter or leave without remapping every other
/// voice to it. The dynamic program finds a non-crossing assignment between successive chords.
fn voice_motion(previous: &[i32], next: &[i32]) -> i32 {
    let mut costs: Vec<i32> = (0..=next.len()).map(|n| n as i32 * 7).collect();
    for (index, before) in previous.iter().enumerate() {
        let mut row = vec![(index as i32 + 1) * 7; next.len() + 1];
        for (at, after) in next.iter().enumerate() {
            let distance = (after - before).abs();
            let moving = distance + (distance - 7).max(0) * 2;
            row[at + 1] = (costs[at] + moving).min(costs[at + 1] + 7).min(row[at] + 7);
        }
        costs = row;
    }
    costs[next.len()]
}

/// How telling a chord tone is, from its interval above the root: lower is kept longer.
///
/// The third (or the suspended tone standing in for it) says major or minor, and the seventh or
/// sixth says which kind of chord it is, so those go last; an altered fifth is as much the chord's
/// name as either. The root is next — with no bass part it is the only thing saying which chord
/// this is — then the colour tones, and the perfect fifth first of all, because the root's own
/// overtones already sound it.
fn tone_priority(interval: i32, intervals: &[i32]) -> u8 {
    let has_third = intervals.iter().any(|tone| matches!(tone, 3 | 4));
    match interval {
        3 | 4 => 0,
        2 | 5 if !has_third => 0,
        6 | 8 | 9 | 10 | 11 => 1,
        0 => 2,
        7 => 4,
        _ => 3,
    }
}

/// The tones a chord sounds as `voices` notes, doublings counted: its own tones when `voices` is
/// zero, the most telling of them when it asks for fewer, and the root, the fifth and the third
/// doubled in turn when it asks for more.
fn chord_tones(chord: Chord, voices: usize) -> Vec<PitchClass> {
    let intervals = chord.quality.intervals();
    let wanted = if voices == 0 { intervals.len() } else { voices };
    let mut kept: Vec<i32> = intervals.to_vec();
    kept.sort_by_key(|interval| (tone_priority(*interval, intervals), *interval));
    kept.truncate(wanted.max(1));
    let mut doubling: Vec<i32> = intervals.to_vec();
    doubling.sort_by_key(|interval| match interval {
        0 => (0, 0),
        7 => (1, 0),
        3 | 4 => (2, 0),
        other => (3, *other),
    });
    let mut next = 0;
    while kept.len() < wanted {
        kept.push(doubling[next % doubling.len()]);
        next += 1;
    }
    kept.into_iter()
        .map(|interval| chord.root.transposed(interval))
        .collect()
}

/// Every voicing of `tones` inside `range`: each tone in the bass with the rest stacked closed
/// above it, then opened by drop-two, by drop-two-and-four and by lifting the second voice.
///
/// A voicing that puts two voices on one pitch is not a voicing of this many notes, so it is
/// skipped rather than deduplicated into a thinner chord — which is how the count asked for is
/// the count that sounds.
fn voicings(tones: &[PitchClass], range: (i32, i32)) -> Vec<Vec<i32>> {
    let (low, high) = range;
    let mut out = Vec::new();
    let mut basses: Vec<PitchClass> = tones.to_vec();
    basses.sort_by_key(|class| class.semitones());
    basses.dedup();
    for bass in basses {
        for bottom in (low..=high).filter(|pitch| PitchClass::new(*pitch) == bass) {
            let mut remaining = tones.to_vec();
            let at = remaining
                .iter()
                .position(|class| *class == bass)
                .expect("the bass is one of the tones");
            remaining.remove(at);
            let mut closed = vec![bottom];
            while !remaining.is_empty() {
                let below = *closed.last().expect("the bottom voice exists");
                // The nearest remaining tone above, which is what "closed" means; a doubled tone
                // lands a full octave over its twin rather than on top of it.
                let (index, step) = remaining
                    .iter()
                    .map(
                        |class| match PitchClass::new(below).distance_up_to(*class) {
                            0 => OCTAVE,
                            step => step,
                        },
                    )
                    .enumerate()
                    .min_by_key(|(_, step)| *step)
                    .expect("a tone remains");
                remaining.remove(index);
                closed.push(below + step);
            }
            let count = closed.len();
            let mut spreads = vec![closed.clone()];
            if count >= 3 {
                let mut drop_two = closed.clone();
                drop_two[count - 2] -= OCTAVE;
                spreads.push(drop_two.clone());
                let mut lifted = closed.clone();
                lifted[1] += OCTAVE;
                spreads.push(lifted);
                if count >= 5 {
                    drop_two[count - 4] -= OCTAVE;
                    spreads.push(drop_two);
                }
            }
            for mut candidate in spreads {
                candidate.sort_unstable();
                if candidate.windows(2).any(|pair| pair[0] == pair[1])
                    || candidate.iter().any(|pitch| !(low..=high).contains(pitch))
                {
                    continue;
                }
                out.push(candidate);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// What a voicing costs: how far its middle sits from `centre`, how far its voices travel from
/// `previous`, and how badly it is spaced. Low seconds are expensive; upper seconds and open
/// voicings remain available. The previous upper voice gets its own continuity cost instead of
/// disappearing into the chord's mean.
fn voicing_cost(candidate: &[i32], previous: &[i32], centre: i32, open: bool) -> i32 {
    let count = candidate.len() as i32;
    let sum: i32 = candidate.iter().sum();
    // In quarter-semitones, so a middle half a semitone off still costs something. The pull holds
    // on every chord and not only the first: voice leading left free to drift does drift, and a
    // part that began on the note asked for and had wandered a fifth from it by the chorus is not
    // sitting where it was asked to.
    let off = (sum - centre * count).abs() * 4 / count;
    let mut cost = off * if previous.is_empty() { 3 } else { 2 };
    if let (Some(before), Some(after)) = (previous.last(), candidate.last()) {
        cost += voice_motion(previous, candidate) * 4;
        cost += (before - after).abs() * 2;
    }
    for pair in candidate.windows(2) {
        let gap = pair[1] - pair[0];
        if pair[0] < 55 {
            cost += (5 - gap).max(0) * 8;
        }
        cost += (gap - OCTAVE).max(0) * 3;
    }
    let span = candidate.last().unwrap() - candidate.first().unwrap();
    cost += if open {
        (OCTAVE - span).max(0) * 2
    } else {
        // Five or six voices need more room than a triad before the spread is a choice.
        (span - 19 - (count - 4).max(0) * 3).max(0)
    };
    cost
}

/// How far above the cheapest voicing a part's first chord may land and still be drawn.
///
/// The first chord sets where every later one is led from, so it is where a seed gets to say
/// something about a part with no rhythm to vary. Small enough that the middle stays within about
/// half a semitone of the note asked for.
const FIRST_CHORD_SLACK: i32 = 6;

/// An inversion and a spacing for `chord` as `voices` notes, its middle at `centre`.
///
/// The first chord of a part, with no `previous` to be led from, is drawn by `rng` from the
/// voicings nearly as good as the best, so two seeds sit the part differently; every later chord
/// is the cheapest, which is what leading the voices means.
pub(super) fn voiced(
    chord: Chord,
    previous: &[i32],
    range: (i32, i32),
    centre: i32,
    voices: usize,
    style: Option<PerformanceStyle>,
    rng: Option<&mut Rng>,
) -> Vec<i32> {
    let open = matches!(
        style,
        Some(PerformanceStyle::Orchestral | PerformanceStyle::Ambient)
    );
    let tones = chord_tones(chord, voices);
    chosen(
        &tones,
        voicings(&tones, range),
        previous,
        range,
        centre,
        open,
        rng,
    )
}

/// [`voiced`] for a chord that is going to be broken rather than struck: the chord's own tones
/// in closed position, an inversion of it and nothing wider.
///
/// A struck chord may open its spacing — drop a voice an octave, lift one — and sounds the
/// fuller for it. Broken, the same spacing is a figure that leaps a sixth or a seventh between
/// every two of its notes, which is a different thing from an arpeggio. What is led from chord
/// to chord is the same either way.
pub(super) fn voiced_closed(
    chord: Chord,
    previous: &[i32],
    range: (i32, i32),
    centre: i32,
    rng: Option<&mut Rng>,
) -> Vec<i32> {
    let tones = chord_tones(chord, 0);
    let closed = voicings(&tones, range)
        .into_iter()
        .filter(|candidate| match (candidate.first(), candidate.last()) {
            (Some(bottom), Some(top)) => top - bottom < OCTAVE,
            _ => false,
        })
        .collect();
    chosen(&tones, closed, previous, range, centre, false, rng)
}

/// The cheapest of `candidates`, or for a part's first chord one of the nearly cheapest.
fn chosen(
    tones: &[PitchClass],
    candidates: Vec<Vec<i32>>,
    previous: &[i32],
    (low, high): (i32, i32),
    centre: i32,
    open: bool,
    rng: Option<&mut Rng>,
) -> Vec<i32> {
    let mut candidates: Vec<(i32, Vec<i32>)> = candidates
        .into_iter()
        .map(|candidate| (voicing_cost(&candidate, previous, centre, open), candidate))
        .collect();
    candidates.sort();
    let Some(best) = candidates.first().map(|(cost, _)| *cost) else {
        let mut pitches: Vec<_> = tones
            .iter()
            .map(|class| fold_into(class.midi(4), low, high))
            .collect();
        pitches.sort_unstable();
        pitches.dedup();
        return pitches;
    };
    let pick = match rng {
        Some(rng) if previous.is_empty() => {
            let near = candidates
                .iter()
                .take_while(|(cost, _)| *cost <= best + FIRST_CHORD_SLACK)
                .count();
            rng.below(near)
        }
        _ => 0,
    };
    candidates.swap_remove(pick).1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chord_to_be_broken_is_voiced_inside_an_octave_and_still_led() {
        let mut previous: Vec<i32> = Vec::new();
        let mut moved = 0;
        for name in ["C", "G", "Am", "F", "Dm7", "G7", "Cmaj7"] {
            let chord = Chord::parse(name).unwrap();
            let next = voiced_closed(chord, &previous, (52, 79), 65, None);
            assert_eq!(next.len(), chord.classes().len(), "{name}: {next:?}");
            assert!(next.windows(2).all(|pair| pair[0] < pair[1]));
            assert!(next.iter().all(|pitch| chord.contains_midi(*pitch)));
            assert!(
                next.last().unwrap() - next.first().unwrap() < OCTAVE,
                "{name} was opened past an octave: {next:?}"
            );
            if let (Some(before), Some(after)) = (previous.first(), next.first()) {
                moved += (after - before).abs();
            }
            previous = next;
        }
        assert!(
            moved <= 15,
            "the lowest voice travelled {moved} semitones in six changes"
        );
    }

    #[test]
    fn inversions_reduce_voice_motion_and_preserve_common_tones() {
        let mut previous = vec![60, 64, 67];
        let mut root_previous = previous.clone();
        let (mut led_motion, mut root_motion, mut held) = (0, 0, 0);
        for name in ["F", "G", "C", "Am", "F", "G", "C"] {
            let chord = Chord::parse(name).unwrap();
            let next = voiced(chord, &previous, (48, 84), 64, 0, None, None);
            assert_eq!(next.len(), chord.classes().len());
            assert!(
                next.iter()
                    .all(|pitch| (48..=84).contains(pitch) && chord.contains_midi(*pitch))
            );
            assert!(next.windows(2).all(|pair| pair[0] < pair[1]));
            led_motion += voice_motion(&previous, &next);
            held += next.iter().filter(|pitch| previous.contains(pitch)).count();
            let root_next = chord.voiced_from(60);
            root_motion += voice_motion(&root_previous, &root_next);
            root_previous = root_next;
            previous = next;
        }
        assert!(
            led_motion < root_motion,
            "inversions cost {led_motion}, root positions cost {root_motion}"
        );
        assert!(held >= 3, "only {held} common tones remained in place");
    }

    #[test]
    fn an_extended_low_chord_can_open_its_spacing() {
        let chord = Chord::parse("Cmaj9").unwrap();
        let pitches = voiced(
            chord,
            &[],
            (36, 84),
            54,
            0,
            Some(PerformanceStyle::Ambient),
            None,
        );
        assert_eq!(pitches.len(), 5);
        assert!(
            pitches
                .windows(2)
                .all(|pair| pair[0] >= 55 || pair[1] - pair[0] >= 3),
            "cramped low voices: {pitches:?}"
        );
        assert!(
            pitches.last().unwrap() - pitches.first().unwrap() >= OCTAVE,
            "no room was opened: {pitches:?}"
        );
    }

    #[test]
    fn two_notes_of_a_seventh_chord_are_its_third_and_seventh() {
        let names = |chord: &str, voices: usize| {
            let mut names: Vec<String> = chord_tones(Chord::parse(chord).unwrap(), voices)
                .into_iter()
                .map(|class| class.to_string())
                .collect();
            names.sort();
            names
        };
        assert_eq!(names("G7", 2), ["B", "F"]);
        assert_eq!(names("C", 2), ["C", "E"]);
        assert_eq!(names("Cmaj9", 3), ["B", "C", "E"]);
        assert_eq!(names("C", 5), ["C", "C", "E", "G", "G"]);
        assert_eq!(names("Bdim", 2), ["D", "F"]);
    }
}
