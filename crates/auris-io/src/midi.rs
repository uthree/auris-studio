//! Reading Standard MIDI Files.
//!
//! A `.mid` file is the one format every other piece of music software can hand over, so this is
//! the door material comes in through. What comes out is not a [`Project`] —
//! that would mean this crate deciding which instrument each track plays, which is the session's
//! business — but everything a project needs: the tempo and meter along the timeline, and a set of
//! named tracks holding notes at absolute positions.
//!
//! # What a tick means
//!
//! An SMF counts time in its own division, declared in the header as ticks per quarter note. Ours
//! is [`TICKS_PER_QUARTER`], so every position is scaled on the way in. The division is usually
//! 480 or 960 and ours is a multiple of neither by luck, so the scaling is done in `i64` with the
//! multiply first: dividing first would quantise every note in the file to the file's own grid
//! before it ever reached ours.
//!
//! A file may instead count in **SMPTE timecode** — frames of real time rather than beats. That is
//! a different kind of thing: it has no beats, so it has no bars, and laying it on a musical
//! timeline would mean choosing a tempo on the file's behalf and writing it down as though the
//! file had said so. Those are refused by name rather than guessed at.
//!
//! # What survives a round trip, and what does not
//!
//! Writing happens at *our* division, because [`TICKS_PER_QUARTER`] is 960 and an SMF header holds
//! up to 32767. So every note position and length written here reads back exactly.
//!
//! A tempo does not, quite. The file stores whole microseconds per quarter note, so 144 bpm is
//! 416 666.67 written as 416 667 and read back as 143.999 88 — a thousandth of a beat per minute,
//! inaudible over any length of piece, and a property of the format rather than of this code. A
//! tempo whose period *is* a whole number of microseconds, 96 or 120, comes back exact.
//!
//! What has nowhere to go in a `.mid` at all: audio tracks, every mixer setting including mute and
//! solo, which instrument a track plays, and the automation. A MIDI file is the notes and the
//! clock.

use std::collections::HashMap;
use std::path::Path;

use auris_core::project::{CURVE_STEP, ClipCurve, CurvePoint, Note, Project};
use auris_core::time::{SignatureMap, TICKS_PER_QUARTER, TempoMap, Ticks, TimeSignature};
use midly::num::{u4, u7, u15, u24, u28};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};

use crate::error::{IoError, Result};

/// File extensions the MIDI importer accepts, for a file-dialog filter.
pub fn midi_extensions() -> &'static [&'static str] {
    &["mid", "midi"]
}

/// One track's worth of what a MIDI file held.
#[derive(Clone, Debug, PartialEq)]
pub struct MidiTrack {
    /// The track's name, from its name meta event or from the channel it played on.
    pub name: String,
    /// Which MIDI channel the notes came from, 0-based. Channel 9 is the drum channel by
    /// convention, which is the only hint a file gives about what a track is *for*.
    pub channel: u8,
    /// Every note, positioned from the start of the song rather than from a clip.
    pub notes: Vec<Note>,
    /// The pitch bend, positioned the same way.
    ///
    /// A file that never bends brings none, which is what keeps this off the overwhelming
    /// majority of imports.
    pub bend: Vec<CurvePoint>,
    /// The modulation, positioned the same way.
    pub modulation: Vec<CurvePoint>,
}

/// Everything a Standard MIDI File said, in this application's units.
#[derive(Clone, Debug, PartialEq)]
pub struct MidiImport {
    /// Tempo along the timeline, from the file's tempo meta events.
    pub tempo_map: TempoMap,
    /// Meter along the timeline, from the file's time signature meta events.
    pub signatures: SignatureMap,
    /// The tracks, in the order they appeared.
    pub tracks: Vec<MidiTrack>,
}

impl MidiImport {
    /// Position just past the last note in the file.
    pub fn end(&self) -> Ticks {
        self.tracks
            .iter()
            .flat_map(|track| track.notes.iter())
            .map(|note| note.end())
            .max()
            .unwrap_or(Ticks::ZERO)
    }

    /// How many notes the file held, across every track.
    pub fn note_count(&self) -> usize {
        self.tracks.iter().map(|track| track.notes.len()).sum()
    }
}

/// Reads a Standard MIDI File.
pub fn read_midi_file(path: &Path) -> Result<MidiImport> {
    let bytes = std::fs::read(path).map_err(|source| match source.kind() {
        std::io::ErrorKind::NotFound => IoError::FileNotFound(path.to_path_buf()),
        _ => IoError::Filesystem {
            path: path.to_path_buf(),
            source,
        },
    })?;
    let smf = Smf::parse(&bytes).map_err(|error| IoError::MidiParse(error.to_string()))?;
    read_smf(&smf)
}

/// The half of [`read_midi_file`] that touches no filesystem, so it can be tested on bytes.
pub fn read_midi_bytes(bytes: &[u8]) -> Result<MidiImport> {
    let smf = Smf::parse(bytes).map_err(|error| IoError::MidiParse(error.to_string()))?;
    read_smf(&smf)
}

fn read_smf(smf: &Smf) -> Result<MidiImport> {
    let per_quarter = match smf.header.timing {
        Timing::Metrical(per_quarter) => u32::from(per_quarter.as_int()).max(1),
        Timing::Timecode(fps, subframe) => {
            return Err(IoError::MidiTimecode {
                fps: fps.as_f32(),
                subframe,
            });
        }
    };

    let mut tempo_map = TempoMap::constant(DEFAULT_BPM);
    let mut signatures = SignatureMap::constant(TimeSignature::default());
    let mut saw_tempo = false;
    let mut saw_signature = false;
    // Notes are gathered per source track *and* channel. A format 0 file is one track carrying
    // every channel, and a format 1 track can still carry more than one — either way two channels
    // are two instruments, and merging them would put a bass line inside the drum part.
    let mut parts: HashMap<(usize, u8), Part> = HashMap::new();
    let mut order: Vec<(usize, u8)> = Vec::new();

    for (index, track) in smf.tracks.iter().enumerate() {
        let mut at: u64 = 0;
        let mut track_name: Option<String> = None;
        // Sounding notes, keyed by channel and pitch. A stack per key rather than one slot: the
        // same pitch struck twice before either release is legal, and the engine already keeps
        // such notes independent.
        let mut sounding: HashMap<(u8, u8), Vec<(u64, u8)>> = HashMap::new();

        for event in track.iter() {
            at += u64::from(event.delta.as_int());
            let tick = scale(at, per_quarter);
            match event.kind {
                TrackEventKind::Meta(MetaMessage::Tempo(micros)) => {
                    let micros = micros.as_int().max(1);
                    let bpm = 60_000_000.0 / f64::from(micros);
                    // A tempo at the very start *is* the song's tempo rather than a change
                    // written on top of a default that was never in the file. One that arrives
                    // later is a change, and the head of the song ran at the default until it —
                    // so the question is where the event sits and not merely whether it is the
                    // first one seen. Everything after that goes through `set_point`, which
                    // replaces the anchor in place when it lands at zero: a later track
                    // restating the opening tempo, which a format 1 file often does, must not
                    // throw away the changes already read.
                    match saw_tempo || tick != Ticks::ZERO {
                        false => tempo_map = TempoMap::constant(bpm),
                        true => tempo_map.set_point(tick, bpm),
                    }
                    saw_tempo = true;
                }
                TrackEventKind::Meta(MetaMessage::TimeSignature(numerator, denominator, ..)) => {
                    // The file stores the denominator as a power of two: 3 means an eighth.
                    let denominator = 1u32 << u32::from(denominator).min(MAX_DENOMINATOR_POWER);
                    let numerator = u32::from(numerator);
                    // The range is checked here rather than left to `TimeSignature::new`, which
                    // answers 4/4 for anything outside it. That is the right answer for a control
                    // with bounds and the wrong one for a file: it would write down a meter the
                    // file never claimed, and the reader could not tell it apart from one it did.
                    if !TimeSignature::NUMERATORS.contains(&numerator)
                        || !TimeSignature::DENOMINATORS.contains(&denominator)
                    {
                        log::warn!(
                            "MIDI file names a {numerator}/{denominator} meter; ignoring it"
                        );
                        continue;
                    }
                    let signature = TimeSignature::new(numerator, denominator);
                    match saw_signature || tick != Ticks::ZERO {
                        false => signatures = SignatureMap::constant(signature),
                        true => signatures.set_point(tick, signature),
                    }
                    saw_signature = true;
                }
                TrackEventKind::Meta(MetaMessage::TrackName(name)) => {
                    track_name = Some(String::from_utf8_lossy(name).trim().to_string())
                        .filter(|name| !name.is_empty());
                }
                TrackEventKind::Midi { channel, message } => {
                    let channel = channel.as_int();
                    let key = (index, channel);
                    // The order is kept separately so tracks come out in the order the file put
                    // them in rather than in a hash map's order, which would shuffle a song's
                    // parts differently on every run.
                    parts.entry(key).or_insert_with(|| {
                        order.push(key);
                        Part::default()
                    });
                    match message {
                        // A note-on at zero velocity is a note-off. Every sequencer that ever
                        // used running status emits them, so this is not an edge case.
                        MidiMessage::NoteOn { key: pitch, vel } if vel.as_int() > 0 => {
                            sounding
                                .entry((channel, pitch.as_int()))
                                .or_default()
                                .push((at, vel.as_int()));
                        }
                        MidiMessage::NoteOn { key: pitch, vel: _ }
                        | MidiMessage::NoteOff { key: pitch, vel: _ } => {
                            let pitch = pitch.as_int();
                            if let Some(started) = sounding
                                .get_mut(&(channel, pitch))
                                .and_then(|stack| stack.pop())
                                && let Some(part) = parts.get_mut(&key)
                            {
                                part.notes.push(note(started, at, pitch, per_quarter));
                            }
                        }
                        MidiMessage::PitchBend { bend } => {
                            if let Some(part) = parts.get_mut(&key) {
                                part.bend.push(CurvePoint {
                                    at: scale(at, per_quarter),
                                    value: bend.as_f32() * BEND_RANGE,
                                });
                            }
                        }
                        MidiMessage::Controller { controller, value }
                            if controller.as_int() == CC_MODULATION =>
                        {
                            if let Some(part) = parts.get_mut(&key) {
                                part.modulation.push(CurvePoint {
                                    at: scale(at, per_quarter),
                                    value: f32::from(value.as_int()) / 127.0,
                                });
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            if let Some(name) = &track_name
                && let Some(part) = parts.get_mut(&(index, channel_of(&event.kind)))
            {
                part.name.get_or_insert_with(|| name.clone());
            }
        }

        // Anything still held when the track runs out is closed there rather than dropped: a file
        // that forgot a note-off still meant the note to sound, and a note with no length would be
        // silence the user cannot see the cause of.
        for ((channel, pitch), stack) in sounding {
            for started in stack {
                if let Some(part) = parts.get_mut(&(index, channel)) {
                    part.notes.push(note(started, at, pitch, per_quarter));
                }
            }
        }
    }

    let tracks = order
        .into_iter()
        .filter_map(|key| {
            let part = parts.remove(&key)?;
            (!part.notes.is_empty()).then(|| {
                let mut notes = part.notes;
                notes.sort_by_key(|note| (note.start, note.pitch));
                let mut bend = part.bend;
                bend.sort_by_key(|point| point.at);
                let mut modulation = part.modulation;
                modulation.sort_by_key(|point| point.at);
                MidiTrack {
                    name: part.name.unwrap_or_else(|| default_name(key.1)),
                    channel: key.1,
                    notes,
                    bend,
                    modulation,
                }
            })
        })
        .collect();

    if smf.header.format == Format::Sequential {
        // Format 2 is a bag of separate songs in one file, not one song in several tracks. There
        // is no right way to lay them on a single timeline, and they are rare enough that guessing
        // is worse than saying so — they come in stacked, and the log is where that is admitted.
        log::warn!(
            "MIDI file is format 2: its {} tracks are separate pieces, imported stacked together",
            smf.tracks.len()
        );
    }
    Ok(MidiImport {
        tempo_map,
        signatures,
        tracks,
    })
}

/// Writes the project's instrument tracks as a Standard MIDI File.
///
/// Format 1, at this application's own division — [`TICKS_PER_QUARTER`] is 960 and an SMF header
/// holds up to 32767, so nothing is scaled on the way out and a file written here reads back as
/// exactly the notes that went into it.
///
/// What does not travel, because a MIDI file has nowhere to put it: audio tracks, which have no
/// notes; every mixer setting, including mute and solo; the instrument each track plays; and the
/// automation. A `.mid` is the notes and the clock, and saying so here is better than a reader
/// discovering it by comparing two files.
pub fn write_midi_file(path: &Path, project: &Project) -> Result<usize> {
    let (smf_tracks, count) = build_tracks(project);
    let smf = Smf {
        header: Header::new(
            Format::Parallel,
            Timing::Metrical(u15::new(TICKS_PER_QUARTER as u16)),
        ),
        tracks: smf_tracks,
    };
    smf.save(path).map_err(|error| IoError::Filesystem {
        path: path.to_path_buf(),
        source: std::io::Error::other(error.to_string()),
    })?;
    Ok(count)
}

/// [`write_midi_file`] into memory, so a round trip can be tested without a filesystem.
pub fn write_midi_bytes(project: &Project) -> Result<Vec<u8>> {
    let (tracks, _) = build_tracks(project);
    let smf = Smf {
        header: Header::new(
            Format::Parallel,
            Timing::Metrical(u15::new(TICKS_PER_QUARTER as u16)),
        ),
        tracks,
    };
    let mut bytes = Vec::new();
    smf.write(&mut bytes)
        .map_err(|error| IoError::MidiParse(error.to_string()))?;
    Ok(bytes)
}

/// The file's tracks, and how many notes went into them.
fn build_tracks(project: &Project) -> (Vec<Vec<TrackEvent<'static>>>, usize) {
    let mut tracks = vec![conductor_track(project)];
    let mut count = 0;
    for (index, track) in project.tracks.iter().enumerate() {
        let Some(instrument) = track.kind.as_instrument() else {
            continue;
        };
        // Channel 16 is not a limit anybody hits here, but wrapping keeps a seventeenth track
        // playing rather than dropping it, and format 1 puts each track in its own chunk anyway.
        let channel = u4::new((index % 16) as u8);
        let mut events: Vec<(Ticks, TrackEventKind<'static>)> = Vec::new();
        for clip in &instrument.clips {
            if clip.muted {
                continue;
            }
            for note in clip.playable_notes() {
                count += 1;
                let start = clip.start + note.start;
                events.push((start, message(channel, note.pitch, velocity(note.velocity))));
                events.push((
                    start + note.length,
                    message(channel, note.pitch, u7::new(0)),
                ));
            }
            // The curves, sampled by the clip's own rule rather than by one of this file's. What
            // the wire carries — fourteen bits of bend, seven of controller — is this file's
            // business and stops here; the document works in semitones and in a fraction.
            for (at, semitones) in clip.curve_events(ClipCurve::Bend, CURVE_STEP) {
                events.push((clip.start + at, bend_message(channel, semitones)));
            }
            for (at, amount) in clip.curve_events(ClipCurve::Modulation, CURVE_STEP) {
                events.push((clip.start + at, modulation_message(channel, amount)));
            }
        }
        // Sorted by position, and at one position the releases go first: a note struck again at
        // the instant the last one ended must not have its release land on the new one.
        events.sort_by_key(|(at, kind)| (*at, !is_release(kind)));
        tracks.push(delta_encode(track.name.clone(), events));
    }
    (tracks, count)
}

/// The first track of a format 1 file: the clock, and nothing that makes a sound.
fn conductor_track(project: &Project) -> Vec<TrackEvent<'static>> {
    let mut events: Vec<(Ticks, TrackEventKind<'static>)> = Vec::new();
    for point in project.tempo_map.points() {
        let micros = (60_000_000.0 / point.bpm).round().clamp(1.0, MAX_MICROS) as u32;
        events.push((
            point.tick,
            TrackEventKind::Meta(MetaMessage::Tempo(u24::new(micros))),
        ));
    }
    for point in project.signatures.points() {
        // Back to the power of two the file wants. Every denominator this application holds is
        // one, so the count of trailing zeros is exact rather than rounded.
        let power = point.signature.denominator.trailing_zeros().min(255) as u8;
        events.push((
            point.tick,
            TrackEventKind::Meta(MetaMessage::TimeSignature(
                point.signature.numerator.min(255) as u8,
                power,
                24,
                8,
            )),
        ));
    }
    events.sort_by_key(|(at, _)| *at);
    delta_encode(project.name.clone(), events)
}

/// Turns absolute positions into the deltas a file stores, with a name and an end marker.
fn delta_encode(
    name: String,
    events: Vec<(Ticks, TrackEventKind<'static>)>,
) -> Vec<TrackEvent<'static>> {
    let mut out = Vec::with_capacity(events.len() + 2);
    out.push(TrackEvent {
        delta: u28::new(0),
        kind: TrackEventKind::Meta(MetaMessage::TrackName(
            // Leaked on purpose and once per track: `midly` borrows the bytes of a meta event for
            // the file's lifetime, and a name is a handful of bytes written once per export.
            Box::leak(name.into_bytes().into_boxed_slice()),
        )),
    });
    let mut previous = Ticks::ZERO;
    for (at, kind) in events {
        let delta = (at - previous).raw().max(0) as u32;
        previous = at;
        out.push(TrackEvent {
            delta: u28::new(delta),
            kind,
        });
    }
    out.push(TrackEvent {
        delta: u28::new(0),
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    });
    out
}

/// A note-on, or a note-off when the velocity is zero.
fn message(channel: u4, pitch: u8, vel: u7) -> TrackEventKind<'static> {
    TrackEventKind::Midi {
        channel,
        message: MidiMessage::NoteOn {
            key: u7::new(pitch.min(127)),
            vel,
        },
    }
}

/// How many semitones a bend at full deflection means, in and out.
///
/// Two, which is MIDI's default and what a receiver assumes when nothing has told it otherwise.
/// The document works in semitones and can hold an octave; a bend past this range is written to
/// the file at its edge, because a file that said otherwise would play as something else
/// everywhere but here.
const BEND_RANGE: f32 = 2.0;

/// Controller 1: the modulation wheel.
const CC_MODULATION: u8 = 1;

/// A pitch bend message carrying `semitones`.
fn bend_message(channel: u4, semitones: f32) -> TrackEventKind<'static> {
    TrackEventKind::Midi {
        channel,
        message: MidiMessage::PitchBend {
            bend: midly::PitchBend::from_f32((semitones / BEND_RANGE).clamp(-1.0, 1.0)),
        },
    }
}

/// A modulation message carrying `amount`, from 0 to 1.
///
/// The seven bits are the wire's business and stop here: the document works in a fraction, the
/// way [`NoteEvent::Modulation`](auris_core::NoteEvent::Modulation) does.
fn modulation_message(channel: u4, amount: f32) -> TrackEventKind<'static> {
    TrackEventKind::Midi {
        channel,
        message: MidiMessage::Controller {
            controller: u7::new(CC_MODULATION),
            value: u7::new((amount.clamp(0.0, 1.0) * 127.0).round() as u8),
        },
    }
}

/// Whether an event releases a note rather than starting one.
fn is_release(kind: &TrackEventKind<'static>) -> bool {
    matches!(
        kind,
        TrackEventKind::Midi {
            message: MidiMessage::NoteOn { vel, .. },
            ..
        } if vel.as_int() == 0
    )
}

/// Our 0.0..=1.0 as MIDI's 1..=127.
///
/// Never zero: a note-on at zero velocity *is* a note-off, so a note played that softly would
/// release itself the instant it started.
fn velocity(value: f32) -> u7 {
    let scaled = (value.clamp(0.0, 1.0) * 127.0).round() as u8;
    u7::new(scaled.clamp(1, 127))
}

/// Largest tempo a file can name, in microseconds per quarter: `u24`'s ceiling.
const MAX_MICROS: f64 = 16_777_215.0;

/// What a file says when it says nothing: the tempo every sequencer assumes.
const DEFAULT_BPM: f64 = 120.0;

/// Largest power of two a denominator byte is allowed to name.
///
/// The byte is a shift, so an absurd one would shift a `u32` off its own end. Sixteenth notes are
/// the finest denominator [`TimeSignature`] holds, and a file naming a 1/1024 meter is a file that
/// has gone wrong rather than one making a point.
const MAX_DENOMINATOR_POWER: u32 = 4;

/// One track-and-channel's notes as they are gathered.
#[derive(Default)]
struct Part {
    name: Option<String>,
    notes: Vec<Note>,
    /// The bend, at absolute ticks. Rebased onto the clip once the clip's start is known.
    bend: Vec<CurvePoint>,
    /// The modulation, gathered and rebased the same way.
    modulation: Vec<CurvePoint>,
}

/// The channel an event belongs to, or 0 for the ones that belong to the track as a whole.
fn channel_of(kind: &TrackEventKind) -> u8 {
    match kind {
        TrackEventKind::Midi { channel, .. } => channel.as_int(),
        _ => 0,
    }
}

/// A track with no name of its own, called after the channel it played on.
fn default_name(channel: u8) -> String {
    match channel {
        // Channel 10, counting from one. Every General MIDI file puts its drums there, and it is
        // the only thing a bare SMF says about what a track is for.
        9 => "Drums".to_string(),
        other => format!("Channel {}", u16::from(other) + 1),
    }
}

/// Turns a position in the file's own division into one of ours.
///
/// The multiply comes first on purpose. Dividing first would round every position onto the file's
/// grid before it reached ours, which for a file written at a coarse division is audible as every
/// note landing a little early or late.
fn scale(at: u64, per_quarter: u32) -> Ticks {
    let scaled = (at as i128 * i128::from(TICKS_PER_QUARTER)) / i128::from(per_quarter);
    Ticks(scaled.clamp(0, i128::from(i64::MAX)) as i64)
}

/// A note from its onset and release, in the file's division.
fn note(started: (u64, u8), ended: u64, pitch: u8, per_quarter: u32) -> Note {
    let start = scale(started.0, per_quarter);
    // At least one tick: a note-on and note-off at the same instant is something a file can
    // contain, and a zero-length note is one nothing would ever play or let you grab hold of.
    let length = Ticks((scale(ended, per_quarter) - start).raw().max(1));
    Note {
        pitch,
        // MIDI velocity is 1..=127 and ours is 0.0..=1.0. Divided by 127 rather than 128 so a
        // full-strength note comes out at exactly 1.0 rather than a hair under it.
        velocity: f32::from(started.1) / 127.0,
        start,
        length,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midly::num::{u4, u7, u15, u24, u28};
    use midly::{Header, Track, TrackEvent};

    /// Ticks per quarter used by the fixtures, deliberately not ours.
    const PPQ: u16 = 480;

    fn event(delta: u32, kind: TrackEventKind<'_>) -> TrackEvent<'_> {
        TrackEvent {
            delta: u28::new(delta),
            kind,
        }
    }

    fn note_on(delta: u32, pitch: u8, velocity: u8) -> TrackEvent<'static> {
        event(
            delta,
            TrackEventKind::Midi {
                channel: u4::new(0),
                message: MidiMessage::NoteOn {
                    key: u7::new(pitch),
                    vel: u7::new(velocity),
                },
            },
        )
    }

    fn note_off(delta: u32, pitch: u8) -> TrackEvent<'static> {
        event(
            delta,
            TrackEventKind::Midi {
                channel: u4::new(0),
                message: MidiMessage::NoteOff {
                    key: u7::new(pitch),
                    vel: u7::new(64),
                },
            },
        )
    }

    fn parse(header: Header, tracks: Vec<Track<'_>>) -> MidiImport {
        let smf = Smf { header, tracks };
        let mut bytes = Vec::new();
        smf.write(&mut bytes).expect("the fixture is writable");
        read_midi_bytes(&bytes).expect("the fixture is readable")
    }

    fn metrical(tracks: Vec<Track<'_>>) -> MidiImport {
        parse(
            Header::new(Format::Parallel, Timing::Metrical(u15::new(PPQ))),
            tracks,
        )
    }

    #[test]
    fn a_quarter_note_in_the_files_division_is_a_quarter_note_in_ours() {
        // The whole point of the scaling: a file counts in its own ticks and we count in ours,
        // and a quarter note has to survive the trip as a quarter note.
        let imported = metrical(vec![vec![
            note_on(0, 60, 100),
            note_off(u32::from(PPQ), 60),
        ]]);
        let note = &imported.tracks[0].notes[0];
        assert_eq!(note.start, Ticks::ZERO);
        assert_eq!(note.length, Ticks::QUARTER);
    }

    #[test]
    fn a_position_is_not_quantised_onto_the_files_own_grid() {
        // Dividing before multiplying would land this on a whole tick of the file's division and
        // move the note. At 480 ppq one file tick is a 1920th note; ours is finer.
        let imported = metrical(vec![vec![note_on(1, 60, 100), note_off(1, 60)]]);
        let note = &imported.tracks[0].notes[0];
        assert_eq!(note.start, Ticks(TICKS_PER_QUARTER / 480));
        assert!(note.start > Ticks::ZERO, "a tick of the file is not zero");
    }

    #[test]
    fn a_note_on_at_zero_velocity_is_a_note_off() {
        // Every sequencer that used running status emits these, so it is not an edge case.
        let imported = metrical(vec![vec![
            note_on(0, 60, 100),
            note_on(u32::from(PPQ), 60, 0),
        ]]);
        assert_eq!(imported.note_count(), 1);
        assert_eq!(imported.tracks[0].notes[0].length, Ticks::QUARTER);
    }

    #[test]
    fn the_same_pitch_struck_twice_before_either_release_stays_two_notes() {
        let imported = metrical(vec![vec![
            note_on(0, 60, 100),
            note_on(240, 60, 90),
            note_off(240, 60),
            note_off(240, 60),
        ]]);
        assert_eq!(imported.note_count(), 2);
    }

    #[test]
    fn a_note_nobody_released_is_closed_where_the_track_ends() {
        // A file that forgot a note-off still meant the note to sound, and a note with no length
        // would be silence with no visible cause.
        let imported = metrical(vec![vec![note_on(0, 60, 100), note_on(960, 62, 100)]]);
        assert_eq!(imported.note_count(), 2);
        assert!(
            imported.tracks[0]
                .notes
                .iter()
                .all(|note| note.length > Ticks::ZERO)
        );
    }

    #[test]
    fn velocity_arrives_as_a_fraction_with_full_strength_reaching_one() {
        let imported = metrical(vec![vec![note_on(0, 60, 127), note_off(480, 60)]]);
        assert_eq!(imported.tracks[0].notes[0].velocity, 1.0);
    }

    #[test]
    fn the_tempo_the_file_opens_with_is_the_songs_tempo() {
        // 500 000 microseconds per quarter is 120 bpm; 400 000 is 150.
        let imported = metrical(vec![vec![
            event(
                0,
                TrackEventKind::Meta(MetaMessage::Tempo(u24::new(400_000))),
            ),
            note_on(0, 60, 100),
            note_off(480, 60),
        ]]);
        assert_eq!(imported.tempo_map.bpm_at(Ticks::ZERO), 150.0);
        assert_eq!(
            imported.tempo_map.points().len(),
            1,
            "not a change on a default"
        );
    }

    #[test]
    fn a_tempo_change_further_in_becomes_a_point() {
        let imported = metrical(vec![vec![
            event(
                0,
                TrackEventKind::Meta(MetaMessage::Tempo(u24::new(500_000))),
            ),
            note_on(0, 60, 100),
            note_off(1_920, 60),
            event(
                0,
                TrackEventKind::Meta(MetaMessage::Tempo(u24::new(250_000))),
            ),
        ]]);
        assert_eq!(imported.tempo_map.bpm_at(Ticks::ZERO), 120.0);
        assert_eq!(
            imported.tempo_map.bpm_at(Ticks(TICKS_PER_QUARTER * 4)),
            240.0
        );
    }

    #[test]
    fn a_later_track_restating_the_opening_tempo_does_not_erase_the_changes() {
        // A format 1 file often repeats the tempo at the head of every track, because a player
        // that started reading at track two would otherwise have nothing to go on. That is the
        // same tempo said twice, not an instruction to forget the rest of the map.
        let imported = metrical(vec![
            vec![
                event(
                    0,
                    TrackEventKind::Meta(MetaMessage::Tempo(u24::new(500_000))),
                ),
                event(
                    1_920,
                    TrackEventKind::Meta(MetaMessage::Tempo(u24::new(250_000))),
                ),
            ],
            vec![
                event(
                    0,
                    TrackEventKind::Meta(MetaMessage::Tempo(u24::new(500_000))),
                ),
                note_on(0, 60, 100),
                note_off(480, 60),
            ],
        ]);
        assert_eq!(imported.tempo_map.bpm_at(Ticks::ZERO), 120.0);
        assert_eq!(
            imported.tempo_map.bpm_at(Ticks(TICKS_PER_QUARTER * 4)),
            240.0,
            "the change in the first track survived the second track's restatement"
        );
        assert_eq!(imported.tempo_map.points().len(), 2);
    }

    #[test]
    fn a_tempo_that_first_appears_partway_in_leaves_the_head_at_the_default() {
        // Nothing said what the opening bars run at, so they run at the tempo every sequencer
        // assumes. Taking the first event as the song's tempo wherever it sat would play the
        // head at a speed the file never named for it.
        let imported = metrical(vec![vec![
            note_on(0, 60, 100),
            note_off(1_920, 60),
            event(
                0,
                TrackEventKind::Meta(MetaMessage::Tempo(u24::new(250_000))),
            ),
        ]]);
        assert_eq!(
            imported.tempo_map.bpm_at(Ticks::ZERO),
            120.0,
            "the head is the default, not the tempo that arrives four beats in"
        );
        assert_eq!(
            imported.tempo_map.bpm_at(Ticks(TICKS_PER_QUARTER * 4)),
            240.0
        );
        assert_eq!(imported.tempo_map.points().len(), 2);
    }

    #[test]
    fn a_time_signature_arrives_with_its_denominator_read_as_a_power_of_two() {
        // The file stores 3 for an eighth. Reading it as a literal 3 would give 6/3.
        let imported = metrical(vec![vec![
            event(
                0,
                TrackEventKind::Meta(MetaMessage::TimeSignature(6, 3, 24, 8)),
            ),
            note_on(0, 60, 100),
            note_off(480, 60),
        ]]);
        assert_eq!(
            imported.signatures.signature_at(Ticks::ZERO),
            TimeSignature::new(6, 8)
        );
    }

    #[test]
    fn a_file_with_no_tempo_or_meter_opens_at_the_one_every_sequencer_assumes() {
        let imported = metrical(vec![vec![note_on(0, 60, 100), note_off(480, 60)]]);
        assert_eq!(imported.tempo_map.bpm_at(Ticks::ZERO), 120.0);
        assert_eq!(
            imported.signatures.signature_at(Ticks::ZERO),
            TimeSignature::default()
        );
    }

    #[test]
    fn two_channels_in_one_track_come_out_as_two_tracks() {
        // A format 0 file is exactly this: one track carrying every channel. Merging them would
        // put a bass line inside the drum part.
        let on = |channel: u8, pitch: u8| {
            event(
                0,
                TrackEventKind::Midi {
                    channel: u4::new(channel),
                    message: MidiMessage::NoteOn {
                        key: u7::new(pitch),
                        vel: u7::new(100),
                    },
                },
            )
        };
        let imported = parse(
            Header::new(Format::SingleTrack, Timing::Metrical(u15::new(PPQ))),
            vec![vec![on(0, 60), on(9, 36), note_off(480, 60)]],
        );
        assert_eq!(imported.tracks.len(), 2);
        assert_eq!(imported.tracks[1].channel, 9);
        assert_eq!(
            imported.tracks[1].name, "Drums",
            "channel 10 is where drums go"
        );
    }

    #[test]
    fn a_track_keeps_the_name_the_file_gave_it() {
        let imported = metrical(vec![vec![
            event(0, TrackEventKind::Meta(MetaMessage::TrackName(b"Bass"))),
            note_on(0, 40, 100),
            note_off(480, 40),
        ]]);
        assert_eq!(imported.tracks[0].name, "Bass");
    }

    #[test]
    fn a_track_with_no_notes_is_not_a_track() {
        // The conductor track of a format 1 file carries the tempo and nothing else, and an empty
        // track in the arrangement is a row that can only be deleted.
        let imported = metrical(vec![
            vec![event(
                0,
                TrackEventKind::Meta(MetaMessage::Tempo(u24::new(500_000))),
            )],
            vec![note_on(0, 60, 100), note_off(480, 60)],
        ]);
        assert_eq!(imported.tracks.len(), 1);
    }

    #[test]
    fn a_file_counted_in_smpte_frames_is_refused_by_name() {
        // It has no beats, so it has no bars. Laying it on a musical timeline would mean choosing
        // a tempo on the file's behalf and writing it down as though the file had said so.
        let smf = Smf {
            header: Header::new(Format::Parallel, Timing::Timecode(midly::Fps::Fps25, 40)),
            tracks: vec![vec![note_on(0, 60, 100), note_off(40, 60)]],
        };
        let mut bytes = Vec::new();
        smf.write(&mut bytes).expect("writable");
        assert!(matches!(
            read_midi_bytes(&bytes),
            Err(IoError::MidiTimecode { .. })
        ));
    }

    #[test]
    fn rubbish_is_refused_rather_than_read_as_an_empty_song() {
        assert!(read_midi_bytes(b"this is not a MIDI file").is_err());
    }

    // ------------------------------------------------------------------- writing

    /// A project with one instrument track holding one clip of `notes`.
    fn project_with(notes: Vec<Note>, clip_length: Ticks) -> Project {
        let mut project = Project::new("Round Trip", 48_000.0);
        let track = project.add_instrument_track("Lead", "auris.synth.chiptune");
        let clip = project
            .add_midi_clip(track, "Riff", Ticks::ZERO, clip_length)
            .expect("an instrument track takes a clip");
        project.midi_clip_mut(clip).expect("the clip").notes = notes;
        project
    }

    fn round_trip(project: &Project) -> MidiImport {
        let bytes = write_midi_bytes(project).expect("writable");
        read_midi_bytes(&bytes).expect("readable")
    }

    #[test]
    fn a_bend_goes_out_and_comes_back_as_the_same_slide() {
        // The one thing here that is *not* exact: a file carries fourteen bits across the range
        // a receiver assumes, so a semitone is quantised on the way through. The tolerance below
        // is a hundredth of one, which is a fiftieth of a cent and inaudible by a wide margin.
        let mut project = project_with(
            vec![Note::new(60, Ticks::ZERO, Ticks(TICKS_PER_QUARTER * 4))],
            Ticks(TICKS_PER_QUARTER * 4),
        );
        let clip = project.tracks[0]
            .kind
            .as_instrument()
            .expect("an instrument track")
            .clips[0]
            .id;
        project.midi_clip_mut(clip).expect("the clip").bend = vec![
            CurvePoint {
                at: Ticks::ZERO,
                value: 0.0,
            },
            CurvePoint {
                at: Ticks(TICKS_PER_QUARTER * 2),
                value: 2.0,
            },
        ];

        let imported = round_trip(&project);
        let bend = &imported.tracks[0].bend;
        assert!(!bend.is_empty(), "the bend did not survive the file");
        for pair in bend.windows(2) {
            assert!(pair[0].at <= pair[1].at, "out of order: {bend:?}");
        }
        let at = |tick: i64| {
            bend.iter()
                .rfind(|point| point.at <= Ticks(tick))
                .map(|point| point.value)
                .unwrap_or(0.0)
        };
        assert!(at(0).abs() < 0.01, "it started bent: {}", at(0));
        assert!(
            (at(TICKS_PER_QUARTER) - 1.0).abs() < 0.01,
            "halfway up is {}",
            at(TICKS_PER_QUARTER)
        );
        assert!(
            (at(TICKS_PER_QUARTER * 2) - 2.0).abs() < 0.01,
            "the top is {}",
            at(TICKS_PER_QUARTER * 2)
        );
        // And it is let go before the clip ends, or everything after it would play sharp.
        assert!(
            at(TICKS_PER_QUARTER * 4).abs() < 0.01,
            "the bend was left hanging at {}",
            at(TICKS_PER_QUARTER * 4)
        );

        // A file that never bends brings none, which keeps this off almost every import.
        assert!(
            round_trip(&project_with(
                vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)],
                Ticks::QUARTER
            ))
            .tracks[0]
                .bend
                .is_empty()
        );
    }

    #[test]
    fn the_wheel_goes_out_as_controller_one_and_comes_back() {
        // Seven bits rather than fourteen, so the tolerance is a hundred and twenty-eighth of the
        // travel — and that *is* the resolution the wire has, so a receiver reading this file
        // hears exactly what a receiver reading any other one would.
        let mut project = project_with(
            vec![Note::new(60, Ticks::ZERO, Ticks(TICKS_PER_QUARTER * 4))],
            Ticks(TICKS_PER_QUARTER * 4),
        );
        let clip = project.tracks[0]
            .kind
            .as_instrument()
            .expect("an instrument track")
            .clips[0]
            .id;
        project.midi_clip_mut(clip).expect("the clip").modulation = vec![
            CurvePoint {
                at: Ticks::ZERO,
                value: 0.0,
            },
            CurvePoint {
                at: Ticks(TICKS_PER_QUARTER * 2),
                value: 1.0,
            },
        ];

        let imported = round_trip(&project);
        let wheel = &imported.tracks[0].modulation;
        assert!(!wheel.is_empty(), "the wheel did not survive the file");
        for pair in wheel.windows(2) {
            assert!(pair[0].at <= pair[1].at, "out of order: {wheel:?}");
        }
        let at = |tick: i64| {
            wheel
                .iter()
                .rfind(|point| point.at <= Ticks(tick))
                .map(|point| point.value)
                .unwrap_or(0.0)
        };
        assert!(at(0).abs() < 0.01, "it started up: {}", at(0));
        assert!(
            (at(TICKS_PER_QUARTER) - 0.5).abs() < 0.01,
            "halfway up is {}",
            at(TICKS_PER_QUARTER)
        );
        assert!(
            (at(TICKS_PER_QUARTER * 2) - 1.0).abs() < 0.01,
            "the top is {}",
            at(TICKS_PER_QUARTER * 2)
        );
        // Let go before the clip ends, for the reason the bend is: a wheel left up is channel
        // state, and everything after it would go on wobbling.
        assert!(
            at(TICKS_PER_QUARTER * 4).abs() < 0.01,
            "the wheel was left up at {}",
            at(TICKS_PER_QUARTER * 4)
        );
        // And a bend written beside it comes back as a bend rather than as a wheel: two curves on
        // one channel, told apart by the message they are carried in.
        assert!(imported.tracks[0].bend.is_empty());
    }

    #[test]
    fn what_goes_out_comes_back_as_the_same_notes() {
        // Written at our own division, so this is exact rather than approximate: nothing is
        // scaled in either direction.
        let notes = vec![
            Note::new(60, Ticks::ZERO, Ticks::QUARTER),
            Note::new(64, Ticks::QUARTER, Ticks::QUARTER),
            Note::new(
                67,
                Ticks(TICKS_PER_QUARTER * 2),
                Ticks(TICKS_PER_QUARTER * 2),
            ),
        ];
        let imported = round_trip(&project_with(notes.clone(), Ticks(TICKS_PER_QUARTER * 4)));
        assert_eq!(imported.tracks.len(), 1);
        let back = &imported.tracks[0].notes;
        assert_eq!(back.len(), notes.len());
        for (was, now) in notes.iter().zip(back) {
            assert_eq!(
                (now.pitch, now.start, now.length),
                (was.pitch, was.start, was.length)
            );
        }
    }

    #[test]
    fn the_tempo_and_the_meter_come_back() {
        let mut project = project_with(
            vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)],
            Ticks(TICKS_PER_QUARTER * 8),
        );
        project.tempo_map.set_point(Ticks::ZERO, 96.0);
        project
            .tempo_map
            .set_point(Ticks(TICKS_PER_QUARTER * 4), 144.0);
        project.signatures = SignatureMap::constant(TimeSignature::new(3, 4));

        let imported = round_trip(&project);
        // 96 bpm is 625 000 microseconds exactly and comes back exact; 144 does not divide, and
        // is why the comparison below has a tolerance at all. See the test after this one.
        assert_eq!(imported.tempo_map.bpm_at(Ticks::ZERO), 96.0);
        assert!((imported.tempo_map.bpm_at(Ticks(TICKS_PER_QUARTER * 4)) - 144.0).abs() < 0.001);
        assert_eq!(
            imported.signatures.signature_at(Ticks::ZERO),
            TimeSignature::new(3, 4)
        );
    }

    #[test]
    fn a_tempo_survives_the_trip_to_within_what_the_format_can_say() {
        // A MIDI file stores tempo as whole microseconds per quarter note, so a tempo whose
        // period is not a whole number of them cannot come back exactly — 144 bpm is 416 666.67,
        // written as 416 667, read back as 143.999 88.
        //
        // Pinned rather than papered over: the error is a thousandth of a beat per minute, which
        // is inaudible over any length of piece, and knowing that is better than a round number
        // that quietly is not one.
        for bpm in [60.0, 96.0, 120.0, 128.0, 140.0, 144.0, 174.0, 200.0] {
            let mut project = project_with(
                vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)],
                Ticks::QUARTER,
            );
            project.tempo_map.set_point(Ticks::ZERO, bpm);
            let back = round_trip(&project).tempo_map.bpm_at(Ticks::ZERO);
            assert!((back - bpm).abs() < 0.001, "{bpm} bpm came back as {back}");
        }
    }

    #[test]
    fn a_track_keeps_its_name_through_the_trip() {
        let imported = round_trip(&project_with(
            vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)],
            Ticks::QUARTER,
        ));
        assert_eq!(imported.tracks[0].name, "Lead");
    }

    #[test]
    fn only_the_notes_a_clip_plays_are_written() {
        // A clip is a window onto its notes, and the exporter asks the same question the renderer
        // does — otherwise the file would be a piece nobody can hear by pressing play.
        let notes = vec![
            Note::new(60, Ticks::ZERO, Ticks::QUARTER),
            // Runs past the clip's end, so it is cut off there.
            Note::new(62, Ticks::QUARTER, Ticks(TICKS_PER_QUARTER * 4)),
            // Starts past the end, so it never sounds at all.
            Note::new(64, Ticks(TICKS_PER_QUARTER * 3), Ticks::QUARTER),
        ];
        let imported = round_trip(&project_with(notes, Ticks(TICKS_PER_QUARTER * 2)));
        let back = &imported.tracks[0].notes;
        assert_eq!(back.len(), 2, "the one past the end is not written");
        assert_eq!(back[1].pitch, 62);
        assert_eq!(
            back[1].end(),
            Ticks(TICKS_PER_QUARTER * 2),
            "cut at the clip's end"
        );
    }

    #[test]
    fn a_muted_clip_is_not_written() {
        let mut project = project_with(
            vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)],
            Ticks::QUARTER,
        );
        let clip = project.tracks[0]
            .kind
            .as_instrument()
            .expect("an instrument track")
            .clips[0]
            .id;
        project.midi_clip_mut(clip).expect("the clip").muted = true;
        assert_eq!(round_trip(&project).note_count(), 0);
    }

    #[test]
    fn a_note_played_as_softly_as_possible_does_not_release_itself() {
        // A note-on at zero velocity *is* a note-off, so the floor is 1 rather than 0 — otherwise
        // the quietest note in a piece would vanish on the way out.
        let mut notes = vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)];
        notes[0].velocity = 0.0;
        let imported = round_trip(&project_with(notes, Ticks::QUARTER));
        assert_eq!(imported.note_count(), 1);
        assert!(imported.tracks[0].notes[0].velocity > 0.0);
    }

    #[test]
    fn a_note_struck_again_the_instant_the_last_one_ended_keeps_both() {
        // At one position the releases have to be written first, or the release of the first note
        // lands on the second and cuts it to nothing.
        let notes = vec![
            Note::new(60, Ticks::ZERO, Ticks::QUARTER),
            Note::new(60, Ticks::QUARTER, Ticks::QUARTER),
        ];
        let imported = round_trip(&project_with(notes, Ticks(TICKS_PER_QUARTER * 2)));
        assert_eq!(imported.note_count(), 2);
        assert!(
            imported.tracks[0]
                .notes
                .iter()
                .all(|note| note.length == Ticks::QUARTER),
            "neither note was cut short by the other's release"
        );
    }

    #[test]
    fn an_audio_track_has_no_notes_to_write_and_makes_no_track() {
        let mut project = project_with(
            vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)],
            Ticks::QUARTER,
        );
        project.add_audio_track("Vocals");
        assert_eq!(round_trip(&project).tracks.len(), 1);
    }
}
