//! Writing the parts.
//!
//! Every part is a pure function of the frozen [`Frame`] and its own name,
//! so no part can depend on another's notes. What makes them sound like a band anyway is that
//! they all read the same harmony, and the rhythm section all reads the same groove.
//!
//! # Where things are
//!
//! Here: what a part is — [`Draft`], [`PartDraft`] and [`ScoreSettings`] — the roster loop in
//! [`write_parts`], and the three passes that pick the notes up again once a writer has put them
//! down. `shorten` and `humanise` are post-passes rather than each writer's business so that one
//! setting means the same thing in every part, and for the same reason the tests that hold for
//! the whole band are here rather than in any one writer's file. `untangle` is a post-pass because
//! it has to run *after* the other two: it clears up what moving a start without moving a length
//! leaves behind, so no writer could do it for itself.
//!
//! One file per role — `melody`, `comp`, `arp`, `bass` and `drums` — because not one of them
//! calls another. They are spokes, and somebody reading how a bass line is built should not have
//! to read the drummer to do it. What they do all read is `writer`, the seven helpers they share
//! and nothing else, and `fixture`, which is the same arrangement for what their tests are
//! written against.

mod arp;
mod bass;
mod comp;
mod drums;
mod joins;
mod melody;
mod writer;

#[cfg(test)]
mod fixture;

use auris_core::time::{TICKS_PER_QUARTER, Ticks};

use crate::frame::Frame;
use crate::rhythm::swing_offset;
use crate::rng::{Key as RngKey, Rng};
use crate::spec::{Mood, PartSpec, Role, SongSpec};

use arp::arp;
use bass::bass;
use comp::comp;
use drums::drums;
use joins::joins;
use melody::melody;
use writer::part_grid;

/// A note as the composer writes it, before it becomes a clip.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Draft {
    /// Which section it belongs to.
    pub section: usize,
    /// MIDI pitch.
    pub pitch: u8,
    /// How hard it is struck, from 0 to 1.
    pub velocity: f32,
    /// Where it starts, from the beginning of the song.
    pub start: Ticks,
    /// How long it sounds.
    pub length: Ticks,
}

/// Everything one part plays.
#[derive(Clone, Debug)]
pub struct PartDraft {
    /// The part's name, which becomes its track name.
    pub name: String,
    /// The plugin that plays it, when no [`Self::sound`] names a SoundFont one.
    pub instrument: String,
    /// The General MIDI sound it asked for, if it asked for one.
    pub sound: Option<crate::gm::Sound>,
    /// Level trim.
    pub gain_db: f32,
    /// Stereo position.
    pub pan: f32,
    /// The notes, in time order.
    pub notes: Vec<Draft>,
}

/// How a part is played, as opposed to what it plays.
///
/// The five dials the writers read that are neither the harmony, the form, nor the part itself.
/// They arrive separately from a [`SongSpec`] so that a caller who has no specification — one
/// regenerating a single clip against the harmony already in a document — can still ask for a
/// part without inventing a whole song around it.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreSettings {
    /// How the music should feel, which sets density and syncopation.
    pub mood: Mood,
    /// How far the offbeats are delayed, as a percentage where 50 is straight.
    pub swing: u8,
    /// How far timing and velocity wander, from 0 for a machine to 1 for a sloppy band.
    ///
    /// Straight through to nothing at 0, and the timing half of it is a duration rather than a
    /// count of ticks, so a low setting is a slight looseness and the same setting means the same
    /// thing at every tempo. The kit does not wander at all; see `humanise` for why it still
    /// leans.
    pub humanize: f32,
    /// How far apart the hardest and softest notes are struck, from 0 to 1.
    ///
    /// Distinct from how hard the part is played, which is the section's intensity: this is how
    /// much the playing varies *around* that. It is the one dial the metric hierarchy answers to,
    /// so it reaches every accent and every phrase shape rather than one writer's idea of them.
    pub dynamics: f32,
    /// How much of a section's last bar the snare runs as a fill, from 0 to 1.
    pub fill: f32,
    /// How much a repeat departs from what the section played the first time.
    pub variation: f32,
    /// Which drum groove the rhythm section plays.
    pub groove: String,
}

impl From<&SongSpec> for ScoreSettings {
    fn from(spec: &SongSpec) -> Self {
        Self {
            mood: spec.mood,
            swing: spec.swing,
            humanize: spec.humanize,
            dynamics: spec.dynamics,
            fill: spec.fill,
            variation: spec.variation,
            groove: spec.groove.clone(),
        }
    }
}

/// Writes every part of a roster against a frame.
pub fn write_parts(settings: &ScoreSettings, roster: &[PartSpec], frame: &Frame) -> Vec<PartDraft> {
    roster
        .iter()
        .map(|part| {
            let mut draft = PartDraft {
                name: part.name.clone(),
                instrument: part.instrument.clone(),
                sound: part.sound(),
                gain_db: part.gain_db,
                pan: part.pan,
                notes: Vec::new(),
            };
            // The part as each section plays it, resolved once for the whole part. A section may
            // patch how it plays — busier, an octave up, on sixteenths — and *every* pass below
            // has to read the same answer: a writer taking the chorus's subdivision while the
            // gate and the swing afterwards took the roster's would be one part played two ways
            // at once, and the seam would show as notes that do not line up with themselves.
            let played: Vec<PartSpec> = frame
                .sections
                .iter()
                .map(|plan| plan.played(part))
                .collect();

            for (index, section) in frame.sections.iter().enumerate() {
                if !section.parts.is_empty() && !section.parts.contains(&part.name) {
                    continue;
                }
                let part = &played[index];
                let notes = match part.role {
                    Role::Melody => melody(settings, frame, section, index, part),
                    Role::Chords | Role::Pad | Role::Stab => {
                        comp(settings, frame, section, index, part)
                    }
                    Role::Arp => arp(settings, frame, section, index, part),
                    Role::Bass => bass(settings, frame, section, index, part),
                    // Written against the joins of the form rather than against a groove: it is
                    // handed a section and asks whether arriving there is worth striking
                    // something for, which is a question no bar-long pattern can answer.
                    Role::Crash => joins(settings, frame, section, index, part),
                    Role::Kick | Role::Snare | Role::Hat => {
                        drums(settings, frame, section, index, part)
                    }
                };
                draft.notes.extend(notes);
            }
            shorten(&played, &mut draft.notes);
            humanise(settings, frame, &played, &mut draft.notes);
            draft
                .notes
                .sort_by_key(|note| (note.start.raw(), note.pitch));
            untangle(&mut draft.notes);
            draft
        })
        .collect()
}

/// The shortest a gate is allowed to cut a note to. Below this it is a click rather than a pitch.
const MIN_NOTE_TICKS: i64 = 30;

/// The lowest the gate goes: a twentieth of the gap, which is already a staccatissimo.
const MIN_GATE: f32 = 0.05;

/// Cuts every note back to its share of the gap to the one after it.
///
/// Applied here rather than inside each writer so that one setting means the same thing in every
/// part. Each writer has already decided a note's length by where the next note starts; the gate
/// says how much of that the note actually sounds for, which is the difference between a chord
/// struck sixteen times a bar and a chord held for one.
///
/// A drum is left alone. A one-shot ignores its note-off, so shortening one would change nothing
/// anybody can hear and only make the piano roll harder to read.
///
/// `played` is the part as each section plays it, so a gate a section patches reaches the notes of
/// that section and no others.
fn shorten(played: &[PartSpec], notes: &mut [Draft]) {
    for note in notes.iter_mut() {
        let Some(part) = played.get(note.section) else {
            continue;
        };
        if part.role.is_drum() {
            continue;
        }
        let gate = part.gate.clamp(MIN_GATE, 1.0);
        if gate >= 1.0 {
            continue;
        }
        // The floor never lengthens a note: a chord shorter than the floor to begin with is a
        // chord the harmony asked for, and the gate is not the place to argue with it.
        let floor = MIN_NOTE_TICKS.min(note.length.raw()).max(1);
        let shortened = (note.length.raw() as f32 * gate).round() as i64;
        note.length = Ticks(shortened.max(floor));
    }
}

/// How far a pitched part's timing wanders at `humanize` 1, as a standard deviation in
/// milliseconds.
///
/// Fifteen because the default humanisation is 0.35 and 15 × 0.35 is 5.25 ms, which is about where
/// a band that is playing well sits: tight enough to be together, loose enough not to be a
/// sequencer. The other end follows from it rather than being chosen — 15 ms with the three-sigma
/// bound the jitter already has means the wander never reaches 45 ms, which is as far as "sloppy"
/// can go before it stops being one band.
///
/// A round number and not a fitted one. The target was "about 5 ms at the default", and a constant
/// carried to two decimal places would be claiming a precision that the ear, which cannot hear the
/// difference between 5 and 5.25 ms of spread, does not have.
const WANDER_MS: f32 = 15.0;

/// Swings, nudges and softens the timing so the part does not sound quantised.
///
/// `humanize: 0` is exactly the identity apart from swing, which is what lets every timing test
/// assert on an exact tick rather than on a tolerance. So is any humanisation at all for a drum,
/// which is a stronger promise and the reason for the guard below.
fn humanise(settings: &ScoreSettings, frame: &Frame, played: &[PartSpec], notes: &mut [Draft]) {
    let Some(part) = played.first() else {
        return;
    };
    // Where a player sits against the beat: a hat pushes, a bass drags. Off the roster's own copy
    // and not a section's, because the role is not something a section patches — a part is what it
    // is for the whole song, and only *how* it plays can change.
    let push = match part.role {
        Role::Hat => -8.0,
        Role::Melody | Role::Arp => -4.0,
        Role::Bass => 6.0,
        Role::Snare => 10.0,
        _ => 0.0,
    } * settings.humanize;

    // How far the wander reaches at this tempo, as a standard deviation in ticks.
    //
    // The dial is read straight, with no floor under it, because a floor makes the first step off
    // zero a jump rather than a step: the old wander was `6 + 19 × humanize` ticks, so it was
    // already six ticks wide — 3.75 ms at 120 BPM, 4.9 at 76 — the instant the dial left zero. At
    // the default of 0.35 that floor was still 47 per cent of the whole wander, and at 0.1 it was
    // 76, so most of the dial's travel was a number nobody had chosen.
    //
    // A quarter note is `TICKS_PER_QUARTER` ticks and lasts `60000 / tempo` milliseconds, so this
    // is how many ticks go by in one of them. Doing the conversion is the whole point: a fixed
    // number of ticks is a fixed *fraction of a beat*, and the same fraction is nearly twice as
    // long a wait at 64 BPM as at 120 — which is how one dial came to read "a bit loose" for the
    // rock preset and "nobody is together" for the ambient one.
    //
    // Per section, because the tempo is: a piece that lifts into its chorus would otherwise have
    // the wander of the section it left, and the whole promise here is that one dial is one feel
    // at whatever speed the music happens to be going. A section the note does not belong to
    // cannot happen — a draft is written *by* section — and answers no wander rather than a
    // guessed tempo, since there is nothing true to convert against.
    let sigma = |section: usize| {
        frame.sections.get(section).map_or(0.0, |plan| {
            let ticks_per_ms = TICKS_PER_QUARTER as f64 * plan.tempo.max(0.0) / 60_000.0;
            WANDER_MS * settings.humanize * ticks_per_ms as f32
        })
    };

    for note in notes.iter_mut() {
        // The grid this note was written on, which is the section's and not the roster's: a part
        // put onto triplets for one section would otherwise have its swing looked up on a
        // sixteenth grid, and the offbeat it delayed would be the wrong step of the bar.
        let grid = part_grid(frame, played.get(note.section).unwrap_or(part));
        let bar_position = note.start.raw().rem_euclid(grid.bar_ticks().raw().max(1));
        let step = grid.step_of(Ticks(bar_position));
        let mut start = note.start + swing_offset(grid, step, settings.swing);
        if settings.humanize > 0.0 {
            // Named by *where the note is* rather than by how many notes came before it, so
            // adding a note to bar one does not re-time the whole song.
            let mut rng = Rng::stream(
                frame.seed,
                &[
                    RngKey::Word("part"),
                    RngKey::Word(&part.name),
                    RngKey::Word("humanize"),
                    RngKey::Index(note.start.raw().max(0) as u64),
                    RngKey::Index(u64::from(note.pitch)),
                ],
            );
            // Drawn whether this note can use it or not, which is the roll-anyway rule: the
            // velocity below is the next number out of the same stream, and silencing the kit's
            // timing should not also have restruck it.
            let wander = rng.jitter(sigma(note.section));
            // A drum does not wander. Everything else in the band is loose *against* somebody
            // keeping the time, and if the kit moves too there is nothing to be loose against.
            //
            // `push` is not part of that and survives the guard: it is a constant lean rather
            // than a wobble — the hat a little early, the snare a little late, by the same amount
            // in every bar of the piece — which is a thing a drummer does on purpose and reads as
            // a feel. What reads as sloppy is the note-to-note scatter, and that is the only
            // thing being taken away here.
            let offset = if part.role.is_drum() {
                push
            } else {
                wander + push
            };
            start += Ticks(offset.round() as i64);
            let scale = 1.0 + rng.jitter(0.06 * settings.humanize);
            note.velocity = (note.velocity * scale).clamp(0.05, 1.0);
        }
        note.start = start.max_zero();
    }
}

/// Cuts every note back to where the next note of the same pitch begins.
///
/// A writer sets a note's length from where the *next* note starts, so nothing it writes overlaps
/// itself. Then `humanise` moves the starts and leaves the lengths alone — the swing delays an
/// offbeat note past the end of its own bar-mate, the wander nudges a repeated note a few ticks
/// early — and a note now ends after the next note of its own pitch has already begun. Measured
/// over the eight presets at four seeds each, 41,184 notes: 13 per cent of them at the default
/// humanisation, and 0.8 per cent from the swing alone with the wander switched off entirely.
///
/// What that costs depends on the instrument, which is exactly why the composer must not write
/// one. A note-off names a pitch and not a note, so an instrument meeting two of them has to
/// choose which it ends, and the workspace's own answer is written down for it: release the one
/// that started first — `auris_session::guide`, and `auris_synth::VoiceAllocator::note_off` for
/// the implementation the built-in voices share.
///
/// That is the answer here and not everywhere. Seven of the eight presets play through a
/// SoundFont, where a note-off reaches the font's own synthesiser on one channel and the library
/// decides; a hosted CLAP plugin may do whatever it does. Neither can be asked, so the question
/// is better not put.
///
/// The cut lands *exactly* on the next note's start rather than a tick before it, so a repeated
/// note stays legato. That is safe because both places that read these notes put releases first
/// where they tie — `graph::schedule::event_rank` for playback, and the MIDI writer's own sort for
/// export — and both say so for this reason.
///
/// `notes` must be sorted by start, which is what makes the reverse walk find the *nearest*
/// following note of a pitch. Two notes of one pitch struck at the same tick are left alone: there
/// is nothing to cut back to, and shortening one to a tick would turn a doubled note into a click.
fn untangle(notes: &mut [Draft]) {
    let mut next_start = [None; 256];
    for note in notes.iter_mut().rev() {
        let slot = &mut next_start[usize::from(note.pitch)];
        if let Some(next) = *slot
            && next > note.start
            && note.start + note.length > next
        {
            note.length = next - note.start;
        }
        *slot = Some(note.start);
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{BASE, draft, part, roster, section_body, section_notes};
    use super::*;
    use crate::theory::pitch::PitchClass;

    /// How many seeds a timing measurement is taken over.
    ///
    /// Seeds of one fixture and not the eight presets. They each write a seed of their own now,
    /// so they would be eight independent draws rather than one draw eight times — but they also
    /// each choose a tempo, a meter, a groove and a roster, and every number below is a
    /// displacement in *milliseconds* measured against the same piece written at `humanize` 0.
    /// Twenty-four seeds of one specification put tens of thousands of paired notes behind each
    /// of them with nothing moved but the dial.
    const SEEDS: u64 = 24;

    /// A piece written to be measured: enough bars to draw from, and no swing to confuse the
    /// comparison with a displacement the dial did not cause.
    fn timing_spec(seed: u64, tempo: f64, humanize: f32) -> String {
        format!(
            r#"
            form = "verse chorus"
            chords = "@axis"
            seed = {seed}
            tempo = {tempo:.1}
            humanize = {humanize:.4}
            swing = 50
            [section.verse]
            bars = 8
            [section.chorus]
            bars = 8
            "#
        )
    }

    /// One part's notes ordered so that two performances of it line up note for note.
    ///
    /// By pitch first and then by time, rather than the other way round: humanisation moves a note
    /// but cannot change its pitch, so this order is one the displacement cannot disturb, while
    /// the order the drafts arrive in — by time, then by pitch — is exactly the one two notes
    /// struck together can be swapped in.
    fn by_pitch(draft: &PartDraft) -> Vec<Draft> {
        let mut notes = draft.notes.clone();
        notes.sort_by_key(|note| (note.pitch, note.start.raw()));
        notes
    }

    /// How far every note of `name` moved from where a machine would have put it, in
    /// milliseconds, pooled over [`SEEDS`] pieces.
    ///
    /// Measured against the same specification written at `humanize` 0, which is the identity
    /// apart from swing: which notes are played is drawn from streams this dial does not reach, so
    /// the two performances hold the same notes and the pairing above pairs each note with itself.
    fn displacements(tempo: f64, humanize: f32, name: &str) -> Vec<f32> {
        let ms_per_tick = 62.5 / tempo as f32;
        let mut out = Vec::new();
        for seed in 0..SEEDS {
            let (_, _, loose) = draft(&timing_spec(seed, tempo, humanize));
            let (_, _, machine) = draft(&timing_spec(seed, tempo, 0.0));
            let played = by_pitch(part(&loose, name));
            let written = by_pitch(part(&machine, name));
            assert_eq!(
                played.len(),
                written.len(),
                "seed {seed}: humanising `{name}` changed how many notes it plays"
            );
            for (played, written) in played.iter().zip(&written) {
                assert_eq!(
                    played.pitch, written.pitch,
                    "seed {seed}: `{name}` mispaired"
                );
                out.push((played.start - written.start).raw() as f32 * ms_per_tick);
            }
        }
        out
    }

    /// The standard deviation of a set of displacements, about their own mean.
    ///
    /// About the mean rather than about zero, because a part's constant lean is not wander: it is
    /// the same number of ticks in every bar of the piece, and what a listener hears as loose is
    /// the scatter around it rather than where the middle of it sits.
    fn spread(values: &[f32]) -> f32 {
        assert!(!values.is_empty(), "nothing was measured");
        let mean = values.iter().sum::<f32>() / values.len() as f32;
        let variance =
            values.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / values.len() as f32;
        variance.sqrt()
    }

    #[test]
    fn the_kit_does_not_wander_however_loose_the_dial_is() {
        // At the top of the dial, where the old formula scattered the kit by twenty-five ticks.
        // Exactly, and not nearly: a drummer who is *almost* on the beat is the complaint.
        for seed in 0..SEEDS {
            let (_, _, loose) = draft(&timing_spec(seed, 120.0, 1.0));
            let (_, _, machine) = draft(&timing_spec(seed, 120.0, 0.0));
            for (name, role) in [
                ("kick", Role::Kick),
                ("snare", Role::Snare),
                ("hat", Role::Hat),
            ] {
                let played = by_pitch(part(&loose, name));
                let written = by_pitch(part(&machine, name));
                assert!(!played.is_empty(), "`{name}` played nothing to measure");
                assert_eq!(played.len(), written.len(), "seed {seed}: `{name}`");
                // The lean the guard keeps on purpose: `humanise`'s own table, at a dial of 1.
                let lean = Ticks(match role {
                    Role::Hat => -8,
                    Role::Snare => 10,
                    _ => 0,
                });
                for (played, written) in played.iter().zip(&written) {
                    assert_eq!(
                        played.start,
                        // A note leaning back off the front of the piece is held at zero, which
                        // is the one place a lean does not survive intact.
                        (written.start + lean).max_zero(),
                        "seed {seed}: `{name}` moved off {} by more than its lean",
                        written.start.raw()
                    );
                }
            }
        }
    }

    #[test]
    fn the_dial_scales_the_wander_all_the_way_down_to_nothing() {
        // The old wander was `6 + 19 × humanize` ticks, so it began at six ticks the instant the
        // dial left zero: 3.75 ms at 120 BPM whatever was asked for. There was nothing between a
        // machine and a player already that far out, and these are the settings that had no
        // meaning. Each is now within a fifth of `WANDER_MS × humanize`, and the smallest of them
        // is a quarter of what the floor alone used to be.
        for humanize in [0.05, 0.1, 0.2, 0.4] {
            let measured = spread(&displacements(120.0, humanize, "lead"));
            let wanted = WANDER_MS * humanize;
            assert!(
                (measured - wanted).abs() < wanted * 0.2,
                "humanize {humanize}: {measured:.2} ms against the {wanted:.2} ms asked for"
            );
        }
        // Named separately so that the number the old floor would have failed on is written down
        // rather than left to be recomputed from the loop above.
        let floor = spread(&displacements(120.0, 0.05, "lead"));
        assert!(
            floor < 1.5,
            "the smallest audible setting still wanders by {floor:.2} ms"
        );
    }

    #[test]
    fn the_same_dial_is_the_same_feel_at_any_tempo() {
        // Three times apart, which covers everything the presets ask for and then some.
        let slow = spread(&displacements(60.0, 0.5, "lead"));
        let fast = spread(&displacements(180.0, 0.5, "lead"));

        // Five per cent, and the reasoning for it: the two pieces hold the same notes and draw the
        // same numbers, so all that is left between them is rounding a displacement onto a whole
        // tick. A tick is 1.04 ms at 60 BPM against 0.35 at 180, and rounding onto a grid of width
        // w adds w²/12 to the variance — 0.09 ms² against a variance of 56, under a fifth of one
        // per cent. Five leaves room for the one note per piece that leans off the front and is
        // held at zero.
        assert!(
            (slow - fast).abs() < 0.05 * slow.max(fast),
            "the same dial gave {slow:.2} ms at 60 BPM and {fast:.2} ms at 180"
        );
        // And both of them are the wander that was asked for, rather than merely equal to each
        // other: 15 ms × 0.5.
        for (tempo, measured) in [(60.0, slow), (180.0, fast)] {
            let wanted = WANDER_MS * 0.5;
            assert!(
                (measured - wanted).abs() < wanted * 0.15,
                "{tempo} BPM: {measured:.2} ms against the {wanted:.2} ms asked for"
            );
        }
    }

    #[test]
    fn a_section_that_lifts_the_tempo_lifts_the_wander_with_it() {
        // The dial asks for a wander in milliseconds and the conversion into ticks needs a tempo.
        // Once a section can name its own, taking the song's would scatter a chorus by the number
        // of ticks its *verse* wanted — at 60 against 180 that is three times the time asked for,
        // which is the whole failure the millisecond conversion was written to stop.
        //
        // Measured in ticks and not in milliseconds on purpose: in milliseconds a right answer
        // and a wrong one both come out as one number twice over, and only the ratio of the ticks
        // says which conversion was used.
        let text = |seed: u64, humanize: f32| {
            format!(
                r#"
                form     = "verse chorus"
                chords   = "@axis"
                seed     = {seed}
                tempo    = 60
                humanize = {humanize:.4}
                swing    = 50
                [section.verse]
                bars = 8
                [section.chorus]
                bars  = 8
                tempo = 180
                "#
            )
        };
        let mut moved: [Vec<f32>; 2] = [Vec::new(), Vec::new()];
        for seed in 0..SEEDS {
            let (_, _, loose) = draft(&text(seed, 0.5));
            let (_, _, machine) = draft(&text(seed, 0.0));
            let played = by_pitch(part(&loose, "lead"));
            let written = by_pitch(part(&machine, "lead"));
            assert_eq!(played.len(), written.len(), "seed {seed} changed the notes");
            for (played, written) in played.iter().zip(&written) {
                moved[played.section.min(1)].push((played.start - written.start).raw() as f32);
            }
        }
        let (verse, chorus) = (spread(&moved[0]), spread(&moved[1]));
        let ratio = chorus / verse;
        assert!(
            (ratio - 3.0).abs() < 0.3,
            "the chorus runs three times the verse's tempo and wandered {chorus:.1} ticks against \
             {verse:.1}, a ratio of {ratio:.2}"
        );
    }

    /// The default roster over a two-section form, with `lead` patched in the chorus.
    fn tweaked(lines: &str) -> (Frame, Vec<PartDraft>) {
        let (_, frame, parts) = draft(&format!(
            r#"
            form     = "verse chorus"
            chords   = "@axis"
            humanize = 0
            seed     = 5

            [section.verse]
            bars = 4
            [section.chorus]
            bars = 4

            [section.chorus.part.lead]
            {lines}
            "#
        ));
        (frame, parts)
    }

    /// One part's notes in one section.
    fn in_section(draft: &PartDraft, section: usize) -> Vec<Draft> {
        draft
            .notes
            .iter()
            .filter(|note| note.section == section)
            .copied()
            .collect()
    }

    #[test]
    fn a_section_can_send_a_part_an_octave_up_without_moving_it_anywhere_else() {
        let (_, parts) = tweaked("octave = 6");
        let lead = part(&parts, "lead");
        let low = in_section(lead, 0).iter().map(|n| n.pitch).min().unwrap();
        let high = in_section(lead, 1).iter().map(|n| n.pitch).min().unwrap();
        assert!(
            high > low,
            "the chorus sits at {high} against the verse's {low}"
        );
        // The verse is what it was without the tweak, which is the half that says this is a patch
        // on one section rather than a change to the part.
        let (_, plain) = tweaked("density = 0.5");
        assert_eq!(
            in_section(lead, 0),
            in_section(part(&plain, "lead"), 0),
            "the tweak reached back into the verse"
        );
    }

    #[test]
    fn a_gate_a_section_patches_reaches_that_sections_notes_and_no_others() {
        // The trap the whole resolution exists for. `shorten` runs once over the finished part,
        // after every section has been written, so a gate read off the roster there would apply
        // the verse's value to the chorus's notes — the one place a per-section field silently
        // does nothing.
        let (_, parts) = tweaked("gate = 0.25");
        let lead = part(&parts, "lead");
        let mean = |notes: &[Draft]| {
            notes.iter().map(|n| n.length.raw()).sum::<i64>() / notes.len().max(1) as i64
        };
        let verse = mean(&in_section(lead, 0));
        let chorus = mean(&in_section(lead, 1));
        assert!(
            chorus * 2 < verse,
            "a quarter of the gap should be well under half: {chorus} against {verse}"
        );
    }

    #[test]
    fn a_subdivision_a_section_patches_is_swung_on_its_own_grid() {
        // The other half of the same trap, and the subtler one: `humanise` looks a note's step up
        // on a grid to decide whether it is an offbeat worth delaying. Read off the roster, a
        // section put onto triplets would have its swing measured against sixteenths and the
        // wrong steps of the bar would move.
        // `humanize = 0` is the identity apart from swing, which is what lets this assert on an
        // exact tick instead of on a tolerance the wander would have to fit inside.
        let (_, _, parts) = draft(
            r#"
            form     = "verse chorus"
            chords   = "@axis"
            swing    = 66
            humanize = 0
            seed     = 5

            [section.verse]
            bars = 4
            [section.chorus]
            bars = 4

            [section.chorus.part.lead]
            subdivision = "8t"
            density     = 0.9
            "#,
        );
        let lead = part(&parts, "lead");
        // A triplet grid has nothing for swing to do — the offbeat is already where the dial
        // would push it — so every note of that section lands exactly on a third of a beat.
        for note in in_section(lead, 1) {
            assert_eq!(
                note.start.raw() % 320,
                0,
                "a chorus note at {} is not on a triplet",
                note.start.raw()
            );
        }
    }

    #[test]
    fn every_default_part_writes_notes() {
        let (_, _, parts) = draft(BASE);
        assert_eq!(parts.len(), 6);
        for part in &parts {
            assert!(!part.notes.is_empty(), "`{}` wrote nothing", part.name);
        }
    }

    #[test]
    fn notes_stay_inside_their_parts_range() {
        let (spec, _, parts) = draft(BASE);
        for (draft, declared) in parts.iter().zip(&spec.parts) {
            if declared.role.is_drum() {
                continue;
            }
            let (low, high) = declared.range();
            for note in &draft.notes {
                assert!(
                    (low..=high).contains(&i32::from(note.pitch)),
                    "`{}` played {} outside {low}..{high}",
                    draft.name,
                    note.pitch
                );
            }
        }
    }

    #[test]
    fn every_pitched_note_belongs_to_the_key() {
        // Not every note has to be a chord tone, but a note outside the scale is a wrong note.
        let (spec, frame, parts) = draft(BASE);
        let section = &frame.sections[0];
        for (draft, declared) in parts.iter().zip(&spec.parts) {
            if declared.role.is_drum() {
                continue;
            }
            for note in &draft.notes {
                let class = PitchClass::new(i32::from(note.pitch));
                let in_scale = section.key.scale.contains(section.key.tonic, class);
                let in_chord = section
                    .chord_at(note.start - section.start)
                    .is_some_and(|event| event.chord.contains(class));
                assert!(
                    in_scale || in_chord,
                    "`{}` played {class} which is in neither the scale nor the chord",
                    draft.name
                );
            }
        }
    }

    #[test]
    fn no_note_starts_before_the_song_or_runs_past_it() {
        let (_, frame, parts) = draft(BASE);
        for draft in &parts {
            for note in &draft.notes {
                assert!(note.start >= Ticks::ZERO, "`{}` started early", draft.name);
                assert!(
                    note.start < frame.length,
                    "`{}` started past the end",
                    draft.name
                );
                assert!(
                    note.length > Ticks::ZERO,
                    "`{}` wrote a zero-length note",
                    draft.name
                );
            }
        }
    }

    #[test]
    fn without_humanising_every_note_lands_exactly_on_the_grid() {
        let (_, frame, parts) = draft(BASE);
        let step = frame.grid.step_ticks().raw();
        for draft in &parts {
            for note in &draft.notes {
                assert_eq!(
                    note.start.raw() % step,
                    0,
                    "`{}` placed a note off the grid at {}",
                    draft.name,
                    note.start.raw()
                );
            }
        }
    }

    #[test]
    fn humanising_moves_notes_and_the_seed_decides_where() {
        let straight = draft(BASE).2;
        let loose = draft(&BASE.replace("humanize = 0", "humanize = 0.8")).2;
        let moved = straight
            .iter()
            .zip(&loose)
            .flat_map(|(a, b)| a.notes.iter().zip(&b.notes))
            .filter(|(a, b)| a.start != b.start)
            .count();
        assert!(moved > 0, "humanising did nothing");

        // And it is reproducible.
        let again = draft(&BASE.replace("humanize = 0", "humanize = 0.8")).2;
        for (a, b) in loose.iter().zip(&again) {
            assert_eq!(a.notes, b.notes, "`{}` was not reproducible", a.name);
        }
    }

    #[test]
    fn swing_delays_the_offbeats_of_a_busy_part() {
        let straight = draft(BASE).2;
        let swung = draft(&BASE.replace("swing = 50", "swing = 66")).2;
        let hat_straight = part(&straight, "hat");
        let hat_swung = part(&swung, "hat");
        let delayed = hat_straight
            .notes
            .iter()
            .zip(&hat_swung.notes)
            .filter(|(a, b)| b.start > a.start)
            .count();
        assert!(delayed > 0, "swing moved nothing");
        assert!(
            hat_straight
                .notes
                .iter()
                .zip(&hat_swung.notes)
                .all(|(a, b)| b.start >= a.start),
            "swing must never rush a note"
        );
    }

    #[test]
    fn nothing_answers_a_borrowed_note_with_the_degree_it_replaced() {
        // A secondary dominant raises a degree; a part still drawing on the key's own scale goes
        // on playing the unraised one, and both versions sound at once. That is not colour, it is
        // the one dissonance an ear calls a mistake — the melody answered the G7 of a minor-key
        // 丸サ進行 with a B flat, a semitone under the chord's own third.
        //
        // Measured as a rate rather than by ear: over these four charts it was twenty-one notes
        // in eight hundred, all of them in the melody, and it is none.
        for (key_text, chart) in [
            ("C minor", "@marusa"),
            ("C major", "@marusa"),
            ("A minor", "@royal-road"),
            ("Eb major", "@naki"),
            ("F# minor", "@junjo"),
        ] {
            let text = format!(
                r#"
                key    = "{key_text}"
                chords = "{chart}"
                form   = "verse chorus"
                seed   = 3
                # Straight and unhumanised, so a note struck on a chord change is not nudged a
                # few ticks back into the chord that is leaving. This is about which note a
                # part chooses, not about when it arrives.
                humanize = 0
                swing    = 50

                [section.verse]
                bars = 8

                [section.chorus]
                bars = 8
                "#
            );
            let (spec, frame, parts) = draft(&text);
            for (part_draft, declared) in parts.iter().zip(&spec.parts) {
                if declared.role.is_drum() {
                    continue;
                }
                for note in &part_draft.notes {
                    let section = &frame.sections[note.section];
                    let Some(event) = section.chord_at(note.start - section.start) else {
                        continue;
                    };
                    let class = PitchClass::new(i32::from(note.pitch));
                    if event.chord.contains(class) {
                        continue;
                    }
                    for tone in event.chord.classes() {
                        // Only a chord tone the key does not have: a semitone between two notes
                        // the key itself offers is ordinary tension and stays.
                        if event.key.scale.contains(event.key.tonic, tone) {
                            continue;
                        }
                        let apart = class.distance_up_to(tone).min(tone.distance_up_to(class));
                        assert_ne!(
                            apart, 1,
                            "`{}` played {class} a semitone from the {tone} of {} in {key_text} \
                             {chart}",
                            part_draft.name, event.chord
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn no_part_plays_a_note_outside_the_scale_or_the_chord() {
        // The fixture deliberately contains a diminished triad and a slash chord, which is where
        // a bass line that assumed a perfect fifth above the sounding bass went wrong.
        for chart in [
            "| I | vii | I | V |",
            "@koakuma",
            "@marusa",
            "@junjo",
            "@blues",
        ] {
            let text = format!(
                r#"
                    key = "C major"
                    form = "verse"
                    chords = "{chart}"
                    humanize = 0
                    [section.verse]
                    bars = 4
                    "#
            );
            let (spec, frame, parts) = draft(&text);
            let section = &frame.sections[0];
            for (part_draft, declared) in parts.iter().zip(&spec.parts) {
                if declared.role.is_drum() {
                    continue;
                }
                for note in &part_draft.notes {
                    let class = PitchClass::new(i32::from(note.pitch));
                    let chord = section.chord_at(note.start - section.start);
                    let in_chord = chord.is_some_and(|event| event.chord.contains(class));
                    assert!(
                        section.key.scale.contains(section.key.tonic, class) || in_chord,
                        "`{}` played {class} over {} in `{chart}`",
                        part_draft.name,
                        chord.map(|e| e.chord.to_string()).unwrap_or_default()
                    );
                }
            }
        }
    }

    #[test]
    fn adding_a_part_leaves_the_other_parts_alone() {
        // Every part hangs off the same skeleton, so taking that skeleton from whichever melody
        // part happened to be in the roster meant adding a part rewrote the whole arrangement.
        let base = r#"
            form = "verse"
            chords = "@axis"
            humanize = 0
            [section.verse]
            bars = 4
            [[part]]
            name = "bass"
            [[part]]
            name = "kick"
            "#;
        let before = draft(base).2;
        let after = draft(&format!(
            r#"
            {base}
            [[part]]
            name = "extra"
            role = "pad"
            "#
        ))
        .2;
        for name in ["bass", "kick"] {
            assert_eq!(
                part(&before, name).notes,
                part(&after, name).notes,
                "adding a part rewrote `{name}`"
            );
        }
    }

    #[test]
    fn editing_one_section_leaves_the_others_alone() {
        // The humanise stream used to be one sequential draw per part, so a note added anywhere
        // re-timed every note after it.
        let base = r#"
            form     = "verse chorus"
            chords   = "@axis"
            humanize = 0.6
            seed     = 3

            [section.verse]
            bars = 2

            [section.chorus]
            bars      = 2
            intensity = {}
        "#;
        let quiet = draft(&base.replace("{}", "0.9")).2;
        let loud = draft(&base.replace("{}", "0.4")).2;
        for (a, b) in quiet.iter().zip(&loud) {
            let verse_a: Vec<&Draft> = a.notes.iter().filter(|n| n.section == 0).collect();
            let verse_b: Vec<&Draft> = b.notes.iter().filter(|n| n.section == 0).collect();
            assert_eq!(
                verse_a, verse_b,
                "changing the chorus rewrote the verse of `{}`",
                a.name
            );
        }
    }

    #[test]
    fn a_section_can_leave_a_part_out() {
        let (_, _, parts) = draft(
            r#"
            form = "intro chorus"
            humanize = 0

            [section.intro]
            parts = "bass"
            "#,
        );
        let hat = part(&parts, "hat");
        // Nothing in the intro, which is section zero.
        assert!(
            hat.notes.iter().all(|note| note.section == 1),
            "the hat played in a section it was left out of"
        );
        assert!(!part(&parts, "bass").notes.is_empty());
    }

    #[test]
    fn a_repeated_section_plays_the_same_music() {
        // The section instance used to be part of every stream name, so a second chorus shared
        // nothing with the first and the piece had no chorus, only two sections with one name.
        let (_, frame, parts) = draft(
            r#"
                form = "verse verse"
                chords = "@axis"
                humanize = 0
                variation = 0
                [section.verse]
                bars = 4
                "#,
        );
        assert_eq!(frame.sections.len(), 2);
        for draft in &parts {
            assert_eq!(
                section_body(&frame, draft, 0),
                section_body(&frame, draft, 1),
                "`{}` played a different second verse",
                draft.name
            );
        }
    }

    #[test]
    fn a_written_rhythm_reaches_every_pitched_part() {
        // The field's contract is that it overrides the generated rhythm, and the format
        // promises that nothing it accepts is quietly ignored. The melody and the drums kept
        // that promise; the chords, the pad, the stab, the arp and the bass read the field,
        // round-tripped it through the document, and played their own rhythm anyway.
        for role in ["chords", "pad", "stab", "arp", "bass"] {
            let text = format!(
                r#"
                form     = "verse"
                chords   = "@axis"
                humanize = 0
                swing    = 50

                [section.verse]
                bars = 2

                [[part]]
                name   = "{role}"
                rhythm = "x ~ ~ ~ x ~ ~ ~ x ~ ~ ~ x ~ ~ ~"
                "#
            );
            let (_, frame, parts) = draft(&text);
            let played = part(&parts, role);
            let notes = section_notes(&frame, played, 0);
            assert!(!notes.is_empty(), "{role} wrote nothing at all");
            let beat = frame.grid.bar_ticks().raw() / 4;
            for (start, ..) in &notes {
                assert!(
                    start % beat == 0,
                    "{role} struck tick {start}, off the written rhythm"
                );
            }
        }
    }

    #[test]
    fn a_subdivision_and_a_gate_reach_only_the_part_that_asked_for_them() {
        // Both live on the part, so turning them up must leave every other part where it was.
        // This is also what makes the fixture in `render` readable: when it moves, the part that
        // moved it is the part that was changed.
        let before = draft(&roster(5, "")).2;
        let after = draft(&roster(5, "subdivision = \"16t\"\n            gate = 0.25")).2;
        for name in ["lead", "bass", "kick"] {
            assert_eq!(
                part(&before, name).notes,
                part(&after, name).notes,
                "changing the chords rewrote `{name}`"
            );
        }
        assert_ne!(
            part(&before, "chords").notes,
            part(&after, "chords").notes,
            "the settings reached nothing"
        );
    }

    #[test]
    fn the_gate_shortens_a_note_without_moving_it() {
        // Articulation, not rhythm. A gate that shifted a note would be a second timing control
        // fighting the swing and the humanising for the same tick.
        let long = draft(&roster(5, "")).2;
        let short = draft(&roster(5, "gate = 0.25")).2;
        let (long, short) = (part(&long, "chords"), part(&short, "chords"));
        assert_eq!(long.notes.len(), short.notes.len());

        let mut shortened = 0;
        for (a, b) in long.notes.iter().zip(&short.notes) {
            assert_eq!(a.start, b.start, "the gate moved a note");
            assert_eq!(a.pitch, b.pitch);
            assert!(b.length <= a.length, "the gate lengthened a note");
            assert!(b.length > Ticks::ZERO, "the gate silenced a note");
            if b.length < a.length {
                shortened += 1;
            }
        }
        assert!(shortened > 0, "the gate shortened nothing");
    }

    #[test]
    fn the_same_spec_writes_the_same_notes_every_time() {
        let first = draft(BASE).2;
        let second = draft(BASE).2;
        for (a, b) in first.iter().zip(&second) {
            assert_eq!(a.notes, b.notes, "`{}` is not deterministic", a.name);
        }
    }

    #[test]
    fn a_different_seed_writes_a_different_piece() {
        let a = draft(&format!(
            r#"
            seed = 1
            {BASE}
            "#
        ))
        .2;
        let b = draft(&format!(
            r#"
            seed = 2
            {BASE}
            "#
        ))
        .2;
        let melody_a = &part(&a, "lead").notes;
        let melody_b = &part(&b, "lead").notes;
        assert_ne!(melody_a, melody_b, "the seed did not reach the melody");
    }

    #[test]
    fn a_note_is_cut_back_to_where_its_pitch_is_struck_again_and_no_further() {
        let note = |pitch, start, length| Draft {
            section: 0,
            pitch,
            velocity: 0.5,
            start: Ticks(start),
            length: Ticks(length),
        };
        // In the order `write_parts` hands them over: by start, then by pitch.
        let mut notes = vec![
            note(60, 0, 500),
            note(64, 0, 500),
            note(60, 480, 480),
            note(64, 960, 480),
        ];
        untangle(&mut notes);
        assert_eq!(
            notes[0].length,
            Ticks(480),
            "cut to the restrike, and to it exactly"
        );
        assert_eq!(
            notes[1].length,
            Ticks(500),
            "another pitch is another voice"
        );
        assert_eq!(
            notes[2].length,
            Ticks(480),
            "the last of a pitch keeps its length"
        );
        assert_eq!(notes[3].length, Ticks(480));

        // Two struck on the same tick are a doubled note and not an overlap: there is nothing to
        // cut back to, and cutting one to a tick would turn it into a click.
        let mut doubled = vec![note(60, 0, 480), note(60, 0, 480)];
        untangle(&mut doubled);
        assert_eq!(doubled[0].length, Ticks(480));
        assert_eq!(doubled[1].length, Ticks(480));
    }

    #[test]
    fn nothing_is_left_sounding_when_its_own_pitch_is_struck_again() {
        // A note-off names a pitch and not a note, so two notes of one pitch overlapping is a
        // question the composer is asking the instrument rather than answering — and most of the
        // instruments a composed piece reaches are a SoundFont library or somebody else's plugin,
        // neither of which can be asked. Both of the composer's own timing passes used to write
        // them, the swing on its own and the wander on top of it, and at the default humanisation
        // it was thirteen notes in every hundred.
        //
        // Over the presets rather than a fixture, because two of them are the ones that swing.
        for preset in crate::preset::PRESETS {
            for seed in 0..4u64 {
                let mut spec = preset.spec();
                spec.seed = seed;
                let piece = crate::render::compose(&spec);
                for track in &piece.tracks {
                    for clip in &track.clips {
                        let mut sounding: std::collections::BTreeMap<u8, Ticks> =
                            std::collections::BTreeMap::new();
                        for note in &clip.notes {
                            let Some(ends) = sounding.insert(note.pitch, note.end()) else {
                                continue;
                            };
                            assert!(
                                ends <= note.start,
                                "{} seed {seed}: in `{}` a {} sounding to {} is struck again at {}",
                                preset.name,
                                clip.name,
                                note.pitch,
                                ends.raw(),
                                note.start.raw(),
                            );
                        }
                    }
                }
            }
        }
    }
}
